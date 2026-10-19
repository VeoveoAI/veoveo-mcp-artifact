//! Resource subscriptions. Row triggers notify the `artifact` channel with each changed
//! row's key and tenant; every replica's watcher hands the change to that tenant's hub,
//! and each `subscriptions/listen` request tells its caller which of its accepted
//! resources the change may have altered. A subscriber hears only its own tenant.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

use futures::StreamExt as _;
use rmcp::{
    ErrorData as McpError,
    model::SubscriptionFilter,
    service::{SubscriptionContext, SubscriptionSendError},
};
use tokio::sync::broadcast;
use veoveo_hub_sdk::hosting::{SubscriptionListener, requested_addresses};
use veoveo_store::{
    ChangeSignal, ChangeStream, Channel, PlatformStore, ServerSchema, TenantId as StoredTenant,
};

use crate::contract::{ArtifactId, ArtifactResource, UploadId};

use super::{ArtifactApp, identity, requirements::authorize};

/// Changes a hub holds for a slow listener before it falls back to a full refresh.
const HUB_BUFFER: usize = 256;

/// What changed in one tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactChange {
    /// An artifact, its grants or its share links.
    Artifact(ArtifactId),
    Upload(UploadId),
    /// The tenant's stored or reserved bytes, or its limits.
    Usage,
    AccessRequests,
    /// Anything may have changed: the listener reconnected or fell behind.
    Everything,
}

/// The tenants' change hubs. A hub lives while a listener holds it.
#[derive(Default)]
pub struct TenantHubs {
    hubs: Mutex<BTreeMap<StoredTenant, Weak<broadcast::Sender<ArtifactChange>>>>,
}

impl TenantHubs {
    fn hub(&self, tenant: StoredTenant) -> Arc<broadcast::Sender<ArtifactChange>> {
        let mut hubs = self.hubs.lock().expect("subscription hubs lock");
        hubs.retain(|_, hub| hub.strong_count() > 0);
        if let Some(hub) = hubs.get(&tenant).and_then(Weak::upgrade) {
            return hub;
        }
        let (sender, _) = broadcast::channel(HUB_BUFFER);
        let hub = Arc::new(sender);
        hubs.insert(tenant, Arc::downgrade(&hub));
        hub
    }

    fn publish(&self, tenant: StoredTenant, change: ArtifactChange) {
        let hub = self
            .hubs
            .lock()
            .expect("subscription hubs lock")
            .get(&tenant)
            .and_then(Weak::upgrade);
        if let Some(hub) = hub {
            let _ = hub.send(change);
        }
    }

    fn publish_everywhere(&self) {
        let hubs: Vec<_> = self
            .hubs
            .lock()
            .expect("subscription hubs lock")
            .values()
            .filter_map(Weak::upgrade)
            .collect();
        for hub in hubs {
            let _ = hub.send(ArtifactChange::Everything);
        }
    }
}

/// The channel Artifact's tables notify: its schema's name.
pub fn channel() -> Channel {
    ServerSchema::new(&crate::server())
        .expect("Artifact's slug names a server schema")
        .channel()
}

/// Hand every change on the `artifact` channel to its tenant's hub until the stream
/// ends.
pub async fn watch_changes(store: PlatformStore, hubs: Arc<TenantHubs>) {
    let mut changes: ChangeStream = store.listen(vec![channel()]);
    while let Some(signal) = changes.next().await {
        match signal {
            Ok(ChangeSignal::Reconcile) => hubs.publish_everywhere(),
            Ok(ChangeSignal::Changed(change)) => {
                let Some(tenant) = change.tenant else {
                    continue;
                };
                let key = change.key.as_deref();
                let parsed = match change.table.as_str() {
                    "artifact" | "grant" | "share_link" => key
                        .and_then(|key| key.parse::<ArtifactId>().ok())
                        .map(ArtifactChange::Artifact),
                    "upload" => key
                        .and_then(|key| key.parse::<UploadId>().ok())
                        .map(ArtifactChange::Upload),
                    "storage_usage" | "upload_limits" => Some(ArtifactChange::Usage),
                    "access_request" => Some(ArtifactChange::AccessRequests),
                    _ => None,
                };
                hubs.publish(tenant, parsed.unwrap_or(ArtifactChange::Everything));
            }
            Err(error) => tracing::warn!(%error, "the Artifact change listener reconnects"),
        }
    }
}

/// Whether `change` may alter the resource at `address`.
fn affects(change: &ArtifactChange, address: &ArtifactResource) -> bool {
    match (change, address) {
        (ArtifactChange::Everything, _) => true,
        (ArtifactChange::Artifact(changed), ArtifactResource::Artifact(id))
        | (ArtifactChange::Artifact(changed), ArtifactResource::Grants(id)) => changed == id,
        (ArtifactChange::Artifact(changed), ArtifactResource::ShareLinks { artifact_id, .. }) => {
            changed == artifact_id
        }
        (ArtifactChange::Artifact(_), ArtifactResource::Artifacts { .. }) => true,
        (ArtifactChange::Upload(changed), ArtifactResource::Upload(id)) => changed == id,
        (ArtifactChange::Upload(_), ArtifactResource::Uploads { .. }) => true,
        (ArtifactChange::Usage, ArtifactResource::StorageUsage) => true,
        (ArtifactChange::AccessRequests, ArtifactResource::AccessRequests { .. }) => true,
        _ => false,
    }
}

/// Artifact's `subscriptions/listen`: it admits each requested resource for the caller,
/// then delivers the caller's tenant's changes until the request ends.
pub struct ArtifactSubscriptions {
    app: Arc<ArtifactApp>,
}

impl ArtifactSubscriptions {
    pub fn new(app: Arc<ArtifactApp>) -> Self {
        Self { app }
    }
}

impl SubscriptionListener for ArtifactSubscriptions {
    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        let resources = requested
            .resource_subscriptions
            .as_ref()
            .filter(|uris| !uris.is_empty())?;
        Some(
            SubscriptionFilter::builder()
                .resource_subscriptions(resources.clone())
                .build(),
        )
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let uris: Vec<String> = context
            .accepted()
            .resource_subscriptions
            .clone()
            .unwrap_or_default();
        let addresses = requested_addresses::<ArtifactResource>(Some(&uris))?;
        if let Some(fixed) = addresses.iter().find(|address| !address.is_subscribable()) {
            tracing::debug!(?fixed, "refused an Artifact subscription");
            return Err(McpError::invalid_params(
                "the resource accepts no subscription",
                None,
            ));
        }
        let identity = veoveo_hub_sdk::hosting::hub_identity(context.request_context())?;
        for uri in &uris {
            authorize(&identity, uri)?;
        }
        let caller = identity::caller(&identity)?;
        let store = self.app.service.store();
        let now = chrono::Utc::now();
        for address in &addresses {
            match address {
                ArtifactResource::Artifact(artifact_id) | ArtifactResource::Grants(artifact_id) => {
                    store
                        .artifact(&caller, *artifact_id, now)
                        .await
                        .map_err(super::errors::store_error)?;
                }
                // Share links are for the artifact's administrators.
                ArtifactResource::ShareLinks { artifact_id, .. } => {
                    store
                        .artifact_requiring(
                            &caller,
                            *artifact_id,
                            veoveo_types::AccessLevel::Admin,
                            now,
                        )
                        .await
                        .map_err(super::errors::store_error)?;
                }
                _ => {}
            }
        }
        let hub = self.app.hubs.hub(caller.tenant());
        let mut changes = hub.subscribe();
        notify(&context, &uris, &addresses, &ArtifactChange::Everything).await?;
        loop {
            let change = tokio::select! {
                () = context.cancelled() => return Ok(()),
                change = changes.recv() => match change {
                    Ok(change) => change,
                    Err(broadcast::error::RecvError::Lagged(_)) => ArtifactChange::Everything,
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                },
            };
            notify(&context, &uris, &addresses, &change).await?;
        }
    }
}

async fn notify(
    context: &SubscriptionContext,
    uris: &[String],
    addresses: &[ArtifactResource],
    change: &ArtifactChange,
) -> Result<(), McpError> {
    for (uri, address) in uris.iter().zip(addresses) {
        if affects(change, address) {
            context
                .sink()
                .notify_resource_updated(uri.clone())
                .await
                .map_err(send_error)?;
        }
    }
    Ok(())
}

fn send_error(error: SubscriptionSendError) -> McpError {
    McpError::internal_error(format!("subscription delivery failed: {error}"), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_reaches_its_own_artifact_and_every_listing() {
        let changed = ArtifactId::new();
        let other = ArtifactId::new();
        let change = ArtifactChange::Artifact(changed);
        assert!(affects(&change, &ArtifactResource::Artifact(changed)));
        assert!(affects(&change, &ArtifactResource::Grants(changed)));
        assert!(!affects(&change, &ArtifactResource::Artifact(other)));
        assert!(affects(
            &change,
            &ArtifactResource::Artifacts {
                work_context: None,
                owner: None,
                media_type: None,
                name: None,
                release_state: None,
            }
        ));
        assert!(affects(
            &change,
            &ArtifactResource::ShareLinks {
                artifact_id: changed,
            }
        ));
        assert!(!affects(&change, &ArtifactResource::StorageUsage));
        assert!(affects(
            &ArtifactChange::AccessRequests,
            &ArtifactResource::AccessRequests {
                role: None,
                state: None,
            }
        ));
        assert!(affects(
            &ArtifactChange::Everything,
            &ArtifactResource::StorageUsage
        ));
    }
}
