//! Typed routes bind Artifact requests, outputs and current policies. Handlers build
//! the caller from the admitted identity; SQL decides its level on each artifact.
pub mod app;
mod declarations;
mod errors;
pub mod identity;
mod knowledge;
mod prompts;
mod requirements;
mod resources;
mod setup;
mod subscriptions;

use std::sync::Arc;

use chrono::Utc;
use rmcp::{ErrorData as McpError, RoleServer, service::RequestContext};
use veoveo_hub_sdk::{
    apps::{AppHtml, UiVisibility},
    calls::ToolCall,
    definition::Product,
    durable_tasks::DurableTasks,
    hosting::{ClientContext, DomainServer, Hosted, hub_identity},
    knowledge::{Indexed, WriteIndexing},
    server_contract::McpServerSetup,
};
use veoveo_mcp_thumb::contract::{
    ArtifactRef, FileSource, ReadyThumbnailsRequest, ThumbPermission,
    ThumbnailRequest as ThumbThumbnail, ThumbnailSize as ThumbThumbnailSize,
};
use veoveo_task::TaskRuntime;
use veoveo_types::Checked;

use crate::{
    contract::{
        AbortUploadRequest, AbortUploadResult, AccessRequestResult, ArtifactId, ArtifactIndexing,
        ArtifactResult, CancelAccessRequestRequest, CompleteUploadRequest, CompleteUploadResult,
        CreateDownloadRequest, CreateDownloadResult, CreateShareLinkRequest, CreateShareLinkResult,
        CreateUploadRequest, CreateUploadResult, DecideAccessRequestRequest, DeleteArtifactRequest,
        DeleteArtifactResult, GrantArtifactRequest, GrantArtifactResult, ReadyThumbnail,
        RequestArtifactAccessRequest, RevokeArtifactGrantRequest, RevokeArtifactGrantResult,
        RevokeShareLinkRequest, SetReleaseStateRequest, SetUploadLimitsRequest, ShareLinkResult,
        SignUploadPartsRequest, SignUploadPartsResult, ThumbnailRequest, ThumbnailResult,
        ThumbnailSize, ThumbnailsRequest, ThumbnailsResult, UploadLimitsResult,
    },
    service::ArtifactService,
};

pub use declarations::{REGISTRATION, registration};
pub use knowledge::{ArtifactTasks, Artifacts, ContentError, KNOWLEDGE, collection, words, work};
pub use requirements::AUTHORIZATION;
pub use setup::ArtifactContract;
pub use subscriptions::{ArtifactSubscriptions, TenantHubs, watch_changes};

use errors::tool_error;
use setup::SERVER_SETUP;

/// Build and check the MCP setup, so a bad declaration stops startup.
pub fn force_setup() {
    std::sync::LazyLock::force(&SERVER_SETUP);
}

/// What every handler shares: the service, the tenants' subscription hubs, the Task runtime that
/// holds the index Tasks of people's uploads, and the App's document.
pub struct ArtifactApp {
    pub service: ArtifactService,
    pub hubs: Arc<TenantHubs>,
    pub tasks: TaskRuntime,
    /// The document of the App, `ui://artifact/library`.
    pub library: AppHtml,
}

/// The hosted MCP handler: Artifact's routes, its Tasks and its subscriptions.
pub fn hosted(
    app: Arc<ArtifactApp>,
) -> Hosted<ArtifactMcp, DurableTasks<ArtifactTasks, ArtifactSubscriptions>> {
    Hosted::new(ArtifactMcp::new(app.clone())).with_tasks(DurableTasks::with_listener(
        ArtifactTasks::new(app.tasks.clone()),
        ArtifactSubscriptions::new(app.clone()),
    ))
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
        let completion = self
            .app
            .service
            .complete(&caller, request.upload_id, Utc::now())
            .await
            .map_err(tool_error)?;
        let artifact = completion.artifact;
        // The call that makes an artifact available indexes it, as its last step.
        let indexed = if completion.made_available {
            self.index(&context, artifact.artifact_id).await
        } else {
            None
        };
        Product::new(
            &crate::contract::ArtifactResource::Artifact(artifact.artifact_id),
            CompleteUploadResult { artifact, indexed },
        )
        .map_err(|_| McpError::internal_error("artifact product construction failed", None))
    }

    /// Index an artifact the call made available: in a Task named in the result for a
    /// person's own call, or before returning for another server's call, with the outcome.
    async fn index(
        &self,
        context: &RequestContext<RoleServer>,
        artifact_id: ArtifactId,
    ) -> Option<ArtifactIndexing> {
        let indexing = KNOWLEDGE
            .index_written(
                &self.app.tasks,
                context,
                self,
                &Artifacts,
                &[artifact_id.as_uuid()],
            )
            .await;
        match indexing {
            Ok(WriteIndexing::Task(_)) => None,
            Ok(WriteIndexing::Indexed(outcomes)) => {
                outcomes
                    .into_iter()
                    .next()
                    .map(|(_, outcome)| match outcome {
                        Indexed::Embedded => ArtifactIndexing {
                            embedded: true,
                            reason: None,
                        },
                        Indexed::NotEmbedded(reason) => ArtifactIndexing {
                            embedded: false,
                            reason: Some(reason.to_string()),
                        },
                    })
            }
            Err(error) => {
                tracing::warn!(%artifact_id, error = %error.message, "an artifact could not be indexed");
                Some(ArtifactIndexing {
                    embedded: false,
                    reason: Some(format!(
                        "the artifact could not be indexed: {}",
                        error.message
                    )),
                })
            }
        }
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

    /// The artifact's picture, from the Thumb server, which asks this server as the same person
    /// whether they may read it. No Thumb server leaves no picture.
    async fn thumbnail(
        &self,
        request: ThumbnailRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ThumbnailResult, McpError> {
        let artifact = ArtifactRef::parse(request.artifact_id.to_string())
            .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        let asked = ThumbThumbnail {
            source: FileSource::Artifact { artifact },
            size: thumb_size(request.size),
        };
        match context.client()?.call(asked).await {
            Ok(drawn) => Ok(ThumbnailResult {
                media_type: drawn.media_type,
                data: drawn.data,
            }),
            Err(error) => {
                tracing::debug!(%error, "the Thumb server drew no picture");
                Ok(ThumbnailResult::default())
            }
        }
    }

    /// What is ready of many artifacts by one request, from the Thumb server, which asks this one
    /// as the same person and draws nothing here. Without an answer, each is left for `thumbnail`.
    async fn thumbnails(
        &self,
        request: ThumbnailsRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ThumbnailsResult, McpError> {
        let asked = request.artifact_ids.len();
        let sources = request
            .artifact_ids
            .iter()
            .map(|id| {
                ArtifactRef::parse(id.to_string()).map(|artifact| FileSource::Artifact { artifact })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| McpError::invalid_params(error.to_string(), None))?;
        let ready = context
            .client()?
            .call(ReadyThumbnailsRequest {
                sources,
                size: thumb_size(request.size),
            })
            .await;
        Ok(ThumbnailsResult {
            thumbnails: match ready {
                Ok(ready) => ready
                    .thumbnails
                    .into_iter()
                    .map(|one| ReadyThumbnail {
                        media_type: one.media_type,
                        data: one.data,
                        pending: one.pending,
                    })
                    .collect(),
                Err(error) => {
                    tracing::debug!(%error, "the Thumb server said nothing of what is ready");
                    vec![
                        ReadyThumbnail {
                            pending: true,
                            ..ReadyThumbnail::default()
                        };
                        asked
                    ]
                }
            },
        })
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

fn thumb_size(size: ThumbnailSize) -> ThumbThumbnailSize {
    match size {
        ThumbnailSize::Small => ThumbThumbnailSize::Small,
        ThumbnailSize::Large => ThumbThumbnailSize::Large,
    }
}

static ROUTES: std::sync::LazyLock<veoveo_hub_sdk::hosting::Routes<ArtifactMcp>> =
    std::sync::LazyLock::new(|| {
        use crate::contract::ArtifactPermission::*;
        use veoveo_hub_sdk::hosting::{RoutePolicy, Routes};
        // A link works for anyone holding it: the share route, and the reads of its preview,
        // which the Hub serves only on the link's own origin.
        let routes = Routes::<ArtifactMcp>::builder(
            RoutePolicy::permission(ArtifactsRead).expect("Artifact permission"),
        )
        .public_route(
            crate::share::PATH,
            crate::share::MAX_BODY_BYTES,
            crate::share::REQUESTS_PER_MINUTE,
        )
        .public_route(
            crate::content::LINKS_PATH,
            crate::content::MAX_BODY_BYTES,
            crate::content::LINKS_REQUESTS_PER_MINUTE,
        )
        .content(crate::content::PATH, Some(crate::content::LINKS_PATH));
        let routes = app::register(routes);
        let routes = routes.tool::<Checked<CreateUploadRequest>, _, _>(
            "Reserve an artifact that you own and receive where to send its \
             bytes: one signed PUT, or a part size and signed part URLs. Give the size when \
             you know it. Send \
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
             Give each part's size when you know it.",
            RoutePolicy::permission(ArtifactsWrite).expect("Artifact permission"),
            |domain, request, context| async move {
                domain
                    .sign_upload_parts(request, context)
                    .await
                    .map_err(Into::into)
            },
        );
        let routes = routes.tool::<CompleteUploadRequest, _, _>(
            "Complete your upload after its bytes arrived. Stored bytes that pass size validation \
             become available. Completing it again returns the artifact.",
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
        let routes = routes.tool::<ThumbnailRequest, _, _>(
            "A picture of an artifact you may read, in base64, or none.",
            RoutePolicy::permission(ArtifactsRead).expect("Artifact permission"),
            |domain, request, context| async move {
                domain.thumbnail(request, context).await.map_err(Into::into)
            },
        );
        let routes = routes.tool::<ThumbnailsRequest, _, _>(
            "What is ready of many artifacts you may read by one request: each one's picture in base64, or none, or `pending` when `thumbnail` can still make one.",
            RoutePolicy::permission(ArtifactsRead).expect("Artifact permission"),
            |domain, request, context| async move {
                domain.thumbnails(request, context).await.map_err(Into::into)
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
        // Search: the artifacts the caller may read, by words or media, embedded through
        // Embedding for the person who made them, in Tasks a person's uploads start.
        let routes = routes
            .knowledge(
                RoutePolicy::permission(ArtifactsRead).expect("Artifact permission"),
                KNOWLEDGE.clone(),
            )
            .uses(
                veoveo_types::PermissionName::parse(
                    veoveo_mcp_embedding::contract::EmbeddingPermission::EmbeddingsCreate.as_str(),
                )
                .expect("Embedding's permission"),
            )
            .uses(
                veoveo_types::PermissionName::parse(ThumbPermission::ThumbnailsRead.as_str())
                    .expect("Thumb's permission"),
            )
            .starts_tasks();
        let mut routes = resources::register(routes);
        for prompt in prompts::ArtifactPrompt::ALL {
            routes = routes.prompt(
                prompt.definition(),
                RoutePolicy::permission(ArtifactsRead).expect("Artifact prompt permission"),
                move |_, request, _| async move { prompt.render(request.arguments) },
            );
        }
        routes
            .app_tools(
                &app::uri(),
                &[
                    <ThumbnailRequest as ToolCall>::TOOL,
                    <ThumbnailsRequest as ToolCall>::TOOL,
                ],
                &[UiVisibility::App],
            )
            .build()
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
