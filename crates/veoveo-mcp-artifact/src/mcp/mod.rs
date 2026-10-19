//! Typed routes bind Artifact requests, outputs and current policies. Handlers build
//! the caller from the admitted identity; SQL decides its level on each artifact.
mod declarations;
mod errors;
pub mod identity;
mod prompts;
mod requirements;
mod resources;
mod setup;
mod subscriptions;

use std::sync::Arc;

use chrono::Utc;
use rmcp::{ErrorData as McpError, RoleServer, service::RequestContext};
use veoveo_hub_sdk::{
    definition::Product,
    hosting::{DomainServer, hub_identity},
    server_contract::McpServerSetup,
};
use veoveo_types::Checked;

use crate::{
    contract::{
        AbortUploadRequest, AbortUploadResult, AccessRequestResult, ArtifactResult,
        CancelAccessRequestRequest, CompleteUploadRequest, CompleteUploadResult,
        CreateDownloadRequest, CreateDownloadResult, CreateShareLinkRequest, CreateShareLinkResult,
        CreateUploadRequest, CreateUploadResult, DecideAccessRequestRequest, DeleteArtifactRequest,
        DeleteArtifactResult, GrantArtifactRequest, GrantArtifactResult,
        RequestArtifactAccessRequest, RevokeArtifactGrantRequest, RevokeArtifactGrantResult,
        RevokeShareLinkRequest, SetReleaseStateRequest, SetUploadLimitsRequest, ShareLinkResult,
        SignUploadPartsRequest, SignUploadPartsResult, UploadLimitsResult,
    },
    service::ArtifactService,
};

pub use declarations::{REGISTRATION, registration};
pub use requirements::AUTHORIZATION;
pub use setup::ArtifactContract;
pub use subscriptions::{ArtifactSubscriptions, TenantHubs, watch_changes};

use errors::tool_error;
use setup::SERVER_SETUP;

/// Build and check the MCP setup, so a bad declaration stops startup.
pub fn force_setup() {
    std::sync::LazyLock::force(&SERVER_SETUP);
}

/// What every handler shares: the service and the tenants' subscription hubs.
pub struct ArtifactApp {
    pub service: ArtifactService,
    pub hubs: Arc<TenantHubs>,
}

#[derive(Clone)]
pub struct ArtifactMcp {
    app: Arc<ArtifactApp>,
}

impl ArtifactMcp {
    pub fn new(app: Arc<ArtifactApp>) -> Self {
        Self { app }
    }

    async fn create_upload(
        &self,
        request: Checked<CreateUploadRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<Product<CreateUploadResult>, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .create_upload(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        Product::new(
            &crate::contract::ArtifactResource::Upload(result.upload.upload_id),
            result,
        )
        .map_err(|_| McpError::internal_error("upload product construction failed", None))
    }

    async fn sign_upload_parts(
        &self,
        request: Checked<SignUploadPartsRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<SignUploadPartsResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .sign_upload_parts(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(result)
    }

    async fn complete_upload(
        &self,
        request: CompleteUploadRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<Product<CompleteUploadResult>, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let owner = identity::task_owner(&identity)?;
        let artifact = self
            .app
            .service
            .complete_upload(&caller, &owner, request.upload_id, Utc::now())
            .await
            .map_err(tool_error)?;
        Product::new(
            &crate::contract::ArtifactResource::Artifact(artifact.artifact_id),
            CompleteUploadResult { artifact },
        )
        .map_err(|_| McpError::internal_error("artifact product construction failed", None))
    }

    async fn abort_upload(
        &self,
        request: AbortUploadRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<AbortUploadResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .abort_upload(&caller, request.upload_id, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(result)
    }

    async fn create_download(
        &self,
        request: CreateDownloadRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CreateDownloadResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .create_download(&caller, request.artifact_id, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(result)
    }

    async fn grant_artifact(
        &self,
        request: GrantArtifactRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<GrantArtifactResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let grant = self
            .app
            .service
            .grant_artifact(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(GrantArtifactResult { grant })
    }

    async fn revoke_artifact_grant(
        &self,
        request: RevokeArtifactGrantRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<RevokeArtifactGrantResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let revoked = self
            .app
            .service
            .revoke_artifact_grant(&caller, request.artifact_id, &request.subject, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(RevokeArtifactGrantResult { revoked })
    }

    async fn set_release_state(
        &self,
        request: SetReleaseStateRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<Product<ArtifactResult>, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let artifact = self
            .app
            .service
            .set_release_state(
                &caller,
                request.artifact_id,
                request.release_state,
                Utc::now(),
            )
            .await
            .map_err(tool_error)?;
        Product::new(
            &crate::contract::ArtifactResource::Artifact(artifact.artifact_id),
            ArtifactResult { artifact },
        )
        .map_err(|_| McpError::internal_error("artifact product construction failed", None))
    }

    async fn create_share_link(
        &self,
        request: CreateShareLinkRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CreateShareLinkResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .create_share_link(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(result)
    }

    async fn revoke_share_link(
        &self,
        request: RevokeShareLinkRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ShareLinkResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let share_link = self
            .app
            .service
            .revoke_share_link(
                &caller,
                request.artifact_id,
                request.share_link_id,
                Utc::now(),
            )
            .await
            .map_err(tool_error)?;
        Ok(ShareLinkResult { share_link })
    }

    async fn request_artifact_access(
        &self,
        request: RequestArtifactAccessRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<AccessRequestResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let access_request = self
            .app
            .service
            .request_access(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(AccessRequestResult {
            access_request,
            grant: None,
        })
    }

    async fn cancel_access_request(
        &self,
        request: CancelAccessRequestRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<AccessRequestResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let access_request = self
            .app
            .service
            .cancel_access_request(&caller, request.access_request_id, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(AccessRequestResult {
            access_request,
            grant: None,
        })
    }

    async fn decide_access_request(
        &self,
        request: DecideAccessRequestRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<AccessRequestResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .decide_access_request(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(result)
    }

    async fn delete_artifact(
        &self,
        request: DeleteArtifactRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<DeleteArtifactResult, McpError> {
        let identity = hub_identity(&context)?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .delete_artifact(&caller, request.artifact_id, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(result)
    }

    async fn set_upload_limits(
        &self,
        request: SetUploadLimitsRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<UploadLimitsResult, McpError> {
        let identity = hub_identity(&context)?;
        let administrator = identity::caller(&identity)?;
        let limits = self
            .app
            .service
            .set_upload_limits(&administrator, &request.tenant, &request.limits, Utc::now())
            .await
            .map_err(tool_error)?;
        Ok(UploadLimitsResult {
            tenant: request.tenant,
            limits,
        })
    }
}

static ROUTES: std::sync::LazyLock<veoveo_hub_sdk::hosting::Routes<ArtifactMcp>> =
    std::sync::LazyLock::new(|| {
        use crate::contract::ArtifactPermission::*;
        use veoveo_hub_sdk::hosting::{RoutePolicy, Routes};
        let routes = Routes::<ArtifactMcp>::builder(
            RoutePolicy::permission(ArtifactsRead).expect("Artifact permission"),
        );
        let routes = routes.tool::<Checked<CreateUploadRequest>, _, _>(
            "Reserve an artifact in your selected Work Context and receive where to send its \
             bytes: one signed PUT, or a part size and signed part URLs. Give the size and \
             SHA-256 when you know them; object storage then refuses different bytes. Send \
             each signed request with exactly its method, URL and headers, then call \
             `complete_upload`. A repeated `idempotencyKey` returns the same upload.",
            RoutePolicy::permission(ArtifactsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .create_upload(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<Checked<SignUploadPartsRequest>, _, _>(
            "Sign 1 to 100 parts of your open multipart upload, or sign expired parts again. \
             Give each part's size and SHA-256 when you know them.",
            RoutePolicy::permission(ArtifactsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .sign_upload_parts(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<CompleteUploadRequest, _, _>(
            "Complete your upload after its bytes arrived. The artifact becomes available, or \
             reads `verifying` while the server computes the SHA-256 of a large object. \
             Completing it again returns the artifact.",
            RoutePolicy::permission(ArtifactsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .complete_upload(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<AbortUploadRequest, _, _>(
            "Abort your open upload and release its reserved bytes.",
            RoutePolicy::permission(ArtifactsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .abort_upload(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<CreateDownloadRequest, _, _>(
            "Receive a signed GET URL in `get.url`, valid for 5 minutes, for an available \
             artifact you may read. Fetch it with `get.method` and the unchanged \
             `get.headers`. The HTTP download names the artifact as an attachment with its \
             media type; ranges and resumed transfers use the same URL.",
            RoutePolicy::permission(ArtifactsRead).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .create_download(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<GrantArtifactRequest, _, _>(
            "Give a principal or group read, write or admin access to an artifact you \
             administer, until an optional expiry. A later grant to the same subject replaces \
             it.",
            RoutePolicy::permission(GrantsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .grant_artifact(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<RevokeArtifactGrantRequest, _, _>(
            "Remove a principal's or group's grant on an artifact you administer.",
            RoutePolicy::permission(GrantsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .revoke_artifact_grant(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<SetReleaseStateRequest, _, _>(
            "Move an available artifact you administer between `private` and `released`. A \
             released artifact may be shared through share links; returning it to private \
             ends its live links at once.",
            RoutePolicy::permission(ReleasesWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .set_release_state(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<CreateShareLinkRequest, _, _>(
            "Issue an anonymous read link for a released, available artifact without data \
             labels that you administer. The link lives 1 to 30 days and may carry a \
             redemption limit. The URL holds the link's secret and is returned only now; \
             anyone holding it receives a fresh download URL each time they open it.",
            RoutePolicy::permission(ShareLinksWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .create_share_link(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<RevokeShareLinkRequest, _, _>(
            "Revoke one of the share links of an artifact you administer. It stops working at \
             once; a download URL it already issued expires within 5 minutes.",
            RoutePolicy::permission(ShareLinksWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .revoke_share_link(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<RequestArtifactAccessRequest, _, _>(
            "Ask the administrators of an artifact you can read for a higher level: write or \
             admin, with a justification. You hold at most one pending request per artifact.",
            RoutePolicy::permission(AccessRequestsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .request_artifact_access(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<CancelAccessRequestRequest, _, _>(
            "Withdraw your own pending access request.",
            RoutePolicy::permission(AccessRequestsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .cancel_access_request(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<DecideAccessRequestRequest, _, _>(
            "Approve or deny a pending request on an artifact you administer, with an \
             optional note. Approval gives the requester the level it asked for.",
            RoutePolicy::permission(GrantsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .decide_access_request(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<DeleteArtifactRequest, _, _>(
            "Delete an artifact you administer, with its grants, share links and access \
             requests. Its bytes are removed shortly after.",
            RoutePolicy::permission(ArtifactsDelete).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .delete_artifact(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<SetUploadLimitsRequest, _, _>(
            "Set a tenant's quota, largest object, allowed media types and open-upload limit. \
             For installation administrators.",
            RoutePolicy::permission(UploadLimitsAdminister).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .set_upload_limits(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let mut routes = resources::register(routes);
        for prompt in prompts::ArtifactPrompt::ALL {
            routes = routes.prompt(
                prompt.definition(),
                RoutePolicy::permission(ArtifactsRead).expect("Artifact prompt permission"),
                move |_, request, _| async move { prompt.render(request.arguments) },
            );
        }
        routes.build()
    });

impl DomainServer for ArtifactMcp {
    type Contract = ArtifactContract;

    fn setup() -> &'static McpServerSetup<ArtifactContract> {
        &SERVER_SETUP
    }

    fn routes() -> &'static veoveo_hub_sdk::hosting::Routes<Self> {
        &ROUTES
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_setup_and_documents_admit() {
        std::sync::LazyLock::force(&super::SERVER_SETUP);
        assert!(super::setup::SERVER_DOCS.agent_manual().is_some());
    }
}
