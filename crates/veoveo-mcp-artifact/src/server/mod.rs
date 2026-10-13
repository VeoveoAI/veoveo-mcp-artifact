//! The hosted process: prepare the bucket, register with the Gateway, connect with the
//! registration's database login, migrate, then serve MCP and the share route while the
//! sweep, the verification worker and the change watcher run. Stopping deletes the
//! registration.
mod config;
mod registration;

use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use tokio_util::sync::CancellationToken;
use veoveo_sdk_rust::{
    PublicDeployment, TelemetryGuard,
    hosting::{Hosted, HostedServer, ListenOnly},
    init_server_telemetry,
    registration::client::Registration,
};
use veoveo_task_runtime::{TaskRuntime, TaskRuntimeConfig};

use veoveo_audit::AuditStore;

use crate::{
    contract::audit_registry,
    mcp::{self, ArtifactApp, ArtifactMcp, ArtifactSubscriptions, TenantHubs, watch_changes},
    service::{ArtifactService, run_verifications},
    share,
    storage::ObjectStorage,
    store::ArtifactStore,
    sweep::run_sweep,
};

use config::Args;

/// How long startup waits for object storage to accept the bucket's preparation.
const STORAGE_READINESS: std::time::Duration = std::time::Duration::from_secs(120);

pub async fn run() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _ = dotenvy::dotenv();
    let _telemetry: TelemetryGuard =
        init_server_telemetry("veoveo-mcp-artifact", "info,veoveo_mcp_artifact=debug")?;
    let args = Args::parse();
    let deployment = args.public_deployment()?;
    let declaration = mcp::registration(&deployment)?;
    mcp::REGISTRATION
        .set(declaration.clone())
        .map_err(|_| anyhow::anyhow!("Artifact's registration was declared twice"))?;
    mcp::force_setup();
    let storage = ObjectStorage::new(&args.storage()).context("configuring object storage")?;
    storage
        .prepare_within(STORAGE_READINESS)
        .await
        .context("preparing Artifact's bucket")?;
    let (registration_client, registration, http) =
        registration::register(&args, declaration).await?;
    // The registration lasts while Artifact's login holds a live process instance. Once
    // serving ends, on SIGTERM or Ctrl-C or on a failure, the client deletes it, which
    // also drops the login.
    let issued = registration.clone();
    let mut served = None;
    let maintained = registration_client
        .maintain(registration, async {
            served = Some(serve(&args, &deployment, &issued, http, storage).await);
        })
        .await;
    if let Err(error) = maintained {
        tracing::warn!(%error, "Artifact could not remove its registration");
    }
    served.context("Artifact's registration ended before serving finished")?
}

/// Connect with the registration's database login, migrate Artifact's schema and serve
/// until SIGTERM or Ctrl-C, or until the process instance loses its lock.
async fn serve(
    args: &Args,
    deployment: &PublicDeployment,
    registration: &Registration,
    http: reqwest::Client,
    storage: ObjectStorage,
) -> Result<()> {
    let grant = registration
        .database()
        .context("the Gateway issued no database login for Artifact")?;
    let tasks = TaskRuntime::connect(
        TaskRuntimeConfig::new(
            grant.url.as_str(),
            grant.username.as_str(),
            grant.password.clone(),
        )?,
        crate::server(),
    )
    .await
    .context("connecting to the installation database")?;
    let store = tasks.platform_store().clone();
    store
        .migrate_server(&crate::server(), crate::migrator())
        .await
        .context("applying Artifact's migrations")?;
    let instance = tasks.instance().clone();
    let registry = Arc::new(audit_registry().context("building Artifact's audit registry")?);
    AuditStore::new(store.clone(), registry.clone())
        .declare_as_server(&crate::server())
        .await
        .context("declaring Artifact's audit classes")?;
    let share_base = args.share_base().context("configuring share links")?;
    let service = Arc::new(ArtifactService::new(
        ArtifactStore::new(store.clone(), registry),
        storage.clone(),
        tasks,
        share_base.clone(),
    ));
    let hubs = Arc::new(TenantHubs::default());
    let app = Arc::new(ArtifactApp {
        service: (*service).clone(),
        hubs: hubs.clone(),
    });

    let stop = CancellationToken::new();
    let watching = tokio::spawn(watch_changes(store.clone(), hubs));
    let sweeping = tokio::spawn(run_sweep(service.clone(), stop.clone()));
    let mut verifying = tokio::spawn(run_verifications(service.clone(), stop.clone()));

    let ready = storage.clone();
    let hosted = HostedServer::for_domain::<ArtifactMcp>()
        .deployment(deployment, args.allow_loopback_hosts)?
        // A browser following a share link names the share origin's host.
        .allowed_hosts(
            args.allowed_hosts
                .iter()
                .cloned()
                .chain([share_base.authority()]),
        )
        .internal_trust(registration.access_tokens(), http)?
        .handler(move || {
            Hosted::new(ArtifactMcp::new(app.clone()))
                .with_tasks(ListenOnly::new(ArtifactSubscriptions::new(app.clone())))
        })
        // Share links work for people without an account, so their route needs no
        // Gateway authentication; it verifies the link's secret itself.
        .public_routes(share::routes(service.clone()))
        // Artifact serves only while its bucket answers.
        .readiness(move || {
            let storage = ready.clone();
            async move { storage.ready().await }
        })
        .build();
    // Serving ends on SIGTERM or Ctrl-C, or when Artifact can no longer claim its
    // verification Tasks, which includes the loss of its instance lock.
    let served = tokio::select! {
        served = hosted.serve(SocketAddr::from(([0, 0, 0, 0], args.port))) => served,
        worked = &mut verifying => match worked {
            Ok(Ok(())) => Err(anyhow::anyhow!("Artifact's verification worker stopped")),
            Ok(Err(error)) => Err(error.context("Artifact stopped claiming verifications")),
            Err(error) => Err(anyhow::Error::from(error).context("Artifact's verification worker failed")),
        },
    };
    stop.cancel();
    watching.abort();
    let _ = sweeping.await;
    if !verifying.is_finished() {
        let _ = verifying.await;
    }
    drop(service);
    // Deleting the instance's row releases the Tasks it still owns at once.
    if let Err(error) = instance.shutdown().await {
        tracing::warn!(%error, "Artifact's process instance did not stop cleanly");
    }
    store.close().await;
    served
}
