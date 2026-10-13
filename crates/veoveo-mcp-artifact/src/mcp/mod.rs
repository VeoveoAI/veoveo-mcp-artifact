//! Artifact's MCP surface. Each tool checks its permission against the requirement
//! table, builds the caller from the forwarded identity and calls the service; the store
//! decides the caller's level on each artifact.
mod declarations;
mod errors;
pub mod identity;
mod requirements;
mod resources;
mod setup;
mod subscriptions;

use std::sync::Arc;

use chrono::Utc;
use rmcp::{
    ErrorData as McpError, RoleServer,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ReadResourceRequestParams},
    service::RequestContext,
    tool, tool_router,
};
use veoveo_sdk_rust::{
    hosting::{DomainAddress, DomainRead, DomainServer, structured_result},
    server_contract::McpServerSetup,
};
use veoveo_types::Checked;

use crate::{
    contract::{
        AbortUploadResult, ArtifactRequest, CompleteUploadResult, CreateDownloadResult,
        CreateUploadRequest, CreateUploadResult, DeleteArtifactResult, GrantArtifactRequest,
        GrantArtifactResult, RevokeArtifactGrantRequest, RevokeArtifactGrantResult,
        SetUploadLimitsRequest, SignUploadPartsRequest, SignUploadPartsResult, UploadLimitsResult,
        UploadRequest,
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
        Parameters(request): Parameters<UploadRequest>,
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
        Parameters(request): Parameters<UploadRequest>,
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
        description = "Receive a signed GET URL, valid for 5 minutes, for an available artifact you may read. The response names the artifact as an attachment with its media type; ranges and resumed transfers use the same URL.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<CreateDownloadResult>(),
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false)
    )]
    async fn create_download(
        &self,
        Parameters(request): Parameters<ArtifactRequest>,
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
        title = "Delete an artifact",
        description = "Delete an artifact you administer, with its grants. Its bytes are removed shortly after.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<DeleteArtifactResult>(),
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn delete_artifact(
        &self,
        Parameters(request): Parameters<ArtifactRequest>,
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
        let limits = self
            .app
            .service
            .set_upload_limits(
                &request.tenant,
                &request.limits,
                &identity.actor.id,
                Utc::now(),
            )
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
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_setup_and_documents_admit() {
        std::sync::LazyLock::force(&super::SERVER_SETUP);
        assert!(super::setup::SERVER_DOCS.agent_manual().is_some());
    }
}
