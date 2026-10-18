//! Artifact's MCP surface. Each tool checks its permission against the requirement
//! table, builds the caller from the forwarded identity and calls the service; the store
//! decides the caller's level on each artifact.
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
use rmcp::{
    ErrorData as McpError, RoleServer,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, GetPromptRequestParams, GetPromptResult, Prompt, ReadResourceRequestParams,
    },
    service::RequestContext,
    tool, tool_router,
};
use veoveo_hub_sdk::{
    hosting::{DomainAddress, DomainRead, DomainServer, structured_result, unknown_prompt},
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
use requirements::{Target, require};
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
    tool_router: ToolRouter<ArtifactMcp>,
}

#[tool_router]
impl ArtifactMcp {
    pub fn new(app: Arc<ArtifactApp>) -> Self {
        Self {
            app,
            tool_router: Self::tools(),
        }
    }

    /// Every tool Artifact serves.
    pub(crate) fn tools() -> ToolRouter<Self> {
        Self::tool_router()
    }

    #[tool(
        title = "Create an upload",
        description = "Reserve an artifact in your selected Work Context and receive where to send its bytes: one signed PUT, or a part size and signed part URLs. Give the size and SHA-256 when you know them; object storage then refuses different bytes. Send each signed request with exactly its method, URL and headers, then call `complete_upload`. A repeated `idempotencyKey` returns the same upload.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<CreateUploadResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn create_upload(
        &self,
        Parameters(request): Parameters<Checked<CreateUploadRequest>>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("create_upload"))?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .create_upload(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            format!(
                "upload {} is {}",
                result.upload.upload_id, result.upload.state
            ),
            &result,
        )
    }

    #[tool(
        title = "Sign upload parts",
        description = "Sign 1 to 100 parts of your open multipart upload, or sign expired parts again. Give each part's size and SHA-256 when you know them.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<SignUploadPartsResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn sign_upload_parts(
        &self,
        Parameters(request): Parameters<Checked<SignUploadPartsRequest>>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("sign_upload_parts"))?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .sign_upload_parts(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(format!("signed {} part(s)", result.parts.len()), &result)
    }

    #[tool(
        title = "Complete an upload",
        description = "Complete your upload after its bytes arrived. The artifact becomes available, or reads `verifying` while the server computes the SHA-256 of a large object. Completing it again returns the artifact.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<CompleteUploadResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn complete_upload(
        &self,
        Parameters(request): Parameters<CompleteUploadRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("complete_upload"))?;
        let caller = identity::caller(&identity)?;
        let owner = identity::task_owner(&identity)?;
        let artifact = self
            .app
            .service
            .complete_upload(&caller, &owner, request.upload_id, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            format!("artifact {} is {}", artifact.artifact_id, artifact.state),
            &CompleteUploadResult { artifact },
        )
    }

    #[tool(
        title = "Abort an upload",
        description = "Abort your open upload and release its reserved bytes.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<AbortUploadResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn abort_upload(
        &self,
        Parameters(request): Parameters<AbortUploadRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("abort_upload"))?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .abort_upload(&caller, request.upload_id, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            format!("upload {} aborted", result.upload.upload_id),
            &result,
        )
    }

    #[tool(
        title = "Create a download",
        description = "Receive a signed GET URL in `get.url`, valid for 5 minutes, for an available artifact you may read. Fetch it with `get.method` and the unchanged `get.headers`. The HTTP download names the artifact as an attachment with its media type; ranges and resumed transfers use the same URL.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<CreateDownloadResult>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn create_download(
        &self,
        Parameters(request): Parameters<CreateDownloadRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("create_download"))?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .create_download(&caller, request.artifact_id, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            format!("download URL valid until {}", result.get.expires_at),
            &result,
        )
    }

    #[tool(
        title = "Grant access to an artifact",
        description = "Give a principal or group read, write or admin access to an artifact you administer, until an optional expiry. A later grant to the same subject replaces it.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<GrantArtifactResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn grant_artifact(
        &self,
        Parameters(request): Parameters<GrantArtifactRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("grant_artifact"))?;
        let caller = identity::caller(&identity)?;
        let grant = self
            .app
            .service
            .grant_artifact(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result("granted".to_owned(), &GrantArtifactResult { grant })
    }

    #[tool(
        title = "Revoke an artifact grant",
        description = "Remove a principal's or group's grant on an artifact you administer.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<RevokeArtifactGrantResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn revoke_artifact_grant(
        &self,
        Parameters(request): Parameters<RevokeArtifactGrantRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("revoke_artifact_grant"))?;
        let caller = identity::caller(&identity)?;
        let revoked = self
            .app
            .service
            .revoke_artifact_grant(&caller, request.artifact_id, &request.subject, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            if revoked {
                "revoked"
            } else {
                "no grant to revoke"
            }
            .to_owned(),
            &RevokeArtifactGrantResult { revoked },
        )
    }

    #[tool(
        title = "Set an artifact's release state",
        description = "Move an available artifact you administer between `private` and `released`. A released artifact may be shared through share links; returning it to private ends its live links at once.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<ArtifactResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn set_release_state(
        &self,
        Parameters(request): Parameters<SetReleaseStateRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("set_release_state"))?;
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
        structured_result(
            format!(
                "artifact {} is {}",
                artifact.artifact_id, artifact.release_state
            ),
            &ArtifactResult { artifact },
        )
    }

    #[tool(
        title = "Create a share link",
        description = "Issue an anonymous read link for a released, available artifact without data labels that you administer. The link lives 1 to 30 days and may carry a redemption limit. The URL holds the link's secret and is returned only now; anyone holding it receives a fresh download URL each time they open it.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<CreateShareLinkResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn create_share_link(
        &self,
        Parameters(request): Parameters<CreateShareLinkRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("create_share_link"))?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .create_share_link(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            format!(
                "share link {} valid until {}",
                result.share_link.share_link_id, result.share_link.expires_at
            ),
            &result,
        )
    }

    #[tool(
        title = "Revoke a share link",
        description = "Revoke one of the share links of an artifact you administer. It stops working at once; a download URL it already issued expires within 5 minutes.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<ShareLinkResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn revoke_share_link(
        &self,
        Parameters(request): Parameters<RevokeShareLinkRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("revoke_share_link"))?;
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
        structured_result(
            format!("share link {} revoked", share_link.share_link_id),
            &ShareLinkResult { share_link },
        )
    }

    #[tool(
        title = "Request access to an artifact",
        description = "Ask the administrators of an artifact you can read for a higher level: write or admin, with a justification. You hold at most one pending request per artifact.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<AccessRequestResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn request_artifact_access(
        &self,
        Parameters(request): Parameters<RequestArtifactAccessRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("request_artifact_access"))?;
        let caller = identity::caller(&identity)?;
        let access_request = self
            .app
            .service
            .request_access(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            format!(
                "access request {} is pending",
                access_request.access_request_id
            ),
            &AccessRequestResult {
                access_request,
                grant: None,
            },
        )
    }

    #[tool(
        title = "Cancel an access request",
        description = "Withdraw your own pending access request.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<AccessRequestResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn cancel_access_request(
        &self,
        Parameters(request): Parameters<CancelAccessRequestRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("cancel_access_request"))?;
        let caller = identity::caller(&identity)?;
        let access_request = self
            .app
            .service
            .cancel_access_request(&caller, request.access_request_id, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            format!(
                "access request {} cancelled",
                access_request.access_request_id
            ),
            &AccessRequestResult {
                access_request,
                grant: None,
            },
        )
    }

    #[tool(
        title = "Decide an access request",
        description = "Approve or deny a pending request on an artifact you administer, with an optional note. Approval gives the requester the level it asked for.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<AccessRequestResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn decide_access_request(
        &self,
        Parameters(request): Parameters<DecideAccessRequestRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("decide_access_request"))?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .decide_access_request(&caller, &request, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            format!(
                "access request {} is {}",
                result.access_request.access_request_id, result.access_request.state
            ),
            &result,
        )
    }

    #[tool(
        title = "Delete an artifact",
        description = "Delete an artifact you administer, with its grants, share links and access requests. Its bytes are removed shortly after.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<DeleteArtifactResult>(),
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn delete_artifact(
        &self,
        Parameters(request): Parameters<DeleteArtifactRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("delete_artifact"))?;
        let caller = identity::caller(&identity)?;
        let result = self
            .app
            .service
            .delete_artifact(&caller, request.artifact_id, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(format!("artifact {} deleted", result.artifact_id), &result)
    }

    #[tool(
        title = "Set a tenant's upload limits",
        description = "Set a tenant's quota, largest object, allowed media types and open-upload limit. For installation administrators.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<UploadLimitsResult>(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn set_upload_limits(
        &self,
        Parameters(request): Parameters<SetUploadLimitsRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let identity = require(&context, Target::Tool("set_upload_limits"))?;
        let administrator = identity::caller(&identity)?;
        let limits = self
            .app
            .service
            .set_upload_limits(&administrator, &request.tenant, &request.limits, Utc::now())
            .await
            .map_err(tool_error)?;
        structured_result(
            format!("upload limits set for {}", request.tenant),
            &UploadLimitsResult {
                tenant: request.tenant,
                limits,
            },
        )
    }
}

impl DomainServer for ArtifactMcp {
    type Contract = ArtifactContract;

    fn setup() -> &'static McpServerSetup<ArtifactContract> {
        &SERVER_SETUP
    }

    fn tool_router(&self) -> &ToolRouter<Self> {
        &self.tool_router
    }

    async fn read(
        &self,
        address: DomainAddress<ArtifactContract>,
        request: &ReadResourceRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<DomainRead, McpError> {
        resources::read(&self.app, address, request, context).await
    }

    fn prompts(&self) -> Vec<Prompt> {
        prompts::ArtifactPrompt::ALL
            .into_iter()
            .map(prompts::ArtifactPrompt::definition)
            .collect()
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        require(&context, Target::Prompt(&request.name))?;
        prompts::ArtifactPrompt::by_name(&request.name)
            .ok_or_else(|| unknown_prompt(&request.name))?
            .render(request.arguments)
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
