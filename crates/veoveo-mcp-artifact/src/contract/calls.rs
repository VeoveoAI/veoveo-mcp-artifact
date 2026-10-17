//! Artifact as a provider of calls between servers. Each served tool's request names its
//! tool and its typed result, and `ReadArtifact` names one artifact's resource and its
//! content, so a server that declares uses of Artifact's permissions calls and reads
//! Artifact through the Hub with the SDK's `HubClient`. The `client` feature's
//! `ArtifactClient` builds uploads and downloads on these calls.
use veoveo_hub_sdk::calls::{ProviderContract, ResourceRead, ToolCall};
use veoveo_types::ServerSlug;

use super::{
    AbortUploadRequest, AbortUploadResult, AccessRequestResult, ArtifactEntry, ArtifactId,
    ArtifactResource, ArtifactResult, CancelAccessRequestRequest, CompleteUploadRequest,
    CompleteUploadResult, CreateDownloadRequest, CreateDownloadResult, CreateShareLinkRequest,
    CreateShareLinkResult, CreateUploadRequest, CreateUploadResult, DecideAccessRequestRequest,
    DeleteArtifactRequest, DeleteArtifactResult, GrantArtifactRequest, GrantArtifactResult,
    RequestArtifactAccessRequest, RevokeArtifactGrantRequest, RevokeArtifactGrantResult,
    RevokeShareLinkRequest, SetReleaseStateRequest, SetUploadLimitsRequest, ShareLinkResult,
    SignUploadPartsRequest, SignUploadPartsResult, UploadLimitsResult,
};

/// The Artifact server, the provider these calls reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactProvider;

impl ProviderContract for ArtifactProvider {
    fn slug() -> ServerSlug {
        crate::server()
    }
}

impl ToolCall for CreateUploadRequest {
    type Provider = ArtifactProvider;
    type Output = CreateUploadResult;
    const TOOL: &'static str = "create_upload";
}

impl ToolCall for SignUploadPartsRequest {
    type Provider = ArtifactProvider;
    type Output = SignUploadPartsResult;
    const TOOL: &'static str = "sign_upload_parts";
}

impl ToolCall for CompleteUploadRequest {
    type Provider = ArtifactProvider;
    type Output = CompleteUploadResult;
    const TOOL: &'static str = "complete_upload";
}

impl ToolCall for AbortUploadRequest {
    type Provider = ArtifactProvider;
    type Output = AbortUploadResult;
    const TOOL: &'static str = "abort_upload";
}

impl ToolCall for CreateDownloadRequest {
    type Provider = ArtifactProvider;
    type Output = CreateDownloadResult;
    const TOOL: &'static str = "create_download";
}

impl ToolCall for GrantArtifactRequest {
    type Provider = ArtifactProvider;
    type Output = GrantArtifactResult;
    const TOOL: &'static str = "grant_artifact";
}

impl ToolCall for RevokeArtifactGrantRequest {
    type Provider = ArtifactProvider;
    type Output = RevokeArtifactGrantResult;
    const TOOL: &'static str = "revoke_artifact_grant";
}

impl ToolCall for SetReleaseStateRequest {
    type Provider = ArtifactProvider;
    type Output = ArtifactResult;
    const TOOL: &'static str = "set_release_state";
}

impl ToolCall for CreateShareLinkRequest {
    type Provider = ArtifactProvider;
    type Output = CreateShareLinkResult;
    const TOOL: &'static str = "create_share_link";
}

impl ToolCall for RevokeShareLinkRequest {
    type Provider = ArtifactProvider;
    type Output = ShareLinkResult;
    const TOOL: &'static str = "revoke_share_link";
}

impl ToolCall for RequestArtifactAccessRequest {
    type Provider = ArtifactProvider;
    type Output = AccessRequestResult;
    const TOOL: &'static str = "request_artifact_access";
}

impl ToolCall for CancelAccessRequestRequest {
    type Provider = ArtifactProvider;
    type Output = AccessRequestResult;
    const TOOL: &'static str = "cancel_access_request";
}

impl ToolCall for DecideAccessRequestRequest {
    type Provider = ArtifactProvider;
    type Output = AccessRequestResult;
    const TOOL: &'static str = "decide_access_request";
}

impl ToolCall for DeleteArtifactRequest {
    type Provider = ArtifactProvider;
    type Output = DeleteArtifactResult;
    const TOOL: &'static str = "delete_artifact";
}

impl ToolCall for SetUploadLimitsRequest {
    type Provider = ArtifactProvider;
    type Output = UploadLimitsResult;
    const TOOL: &'static str = "set_upload_limits";
}

/// A read of one artifact as the caller sees it, `artifact://artifacts/{artifact_id}`:
/// its record and the caller's level on it. A wait on its updates sees the artifact
/// leave `verifying`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadArtifact {
    pub artifact: ArtifactId,
}

impl ResourceRead for ReadArtifact {
    type Provider = ArtifactProvider;
    type Address = ArtifactResource;
    type Content = ArtifactEntry;

    fn address(&self) -> ArtifactResource {
        ArtifactResource::Artifact(self.artifact)
    }
}

/// The tool of every request type above, in the order the server serves them. The
/// server's tests hold this list to the tools it lists.
pub fn served_tools() -> [&'static str; 15] {
    [
        CreateUploadRequest::TOOL,
        SignUploadPartsRequest::TOOL,
        CompleteUploadRequest::TOOL,
        AbortUploadRequest::TOOL,
        CreateDownloadRequest::TOOL,
        GrantArtifactRequest::TOOL,
        RevokeArtifactGrantRequest::TOOL,
        SetReleaseStateRequest::TOOL,
        CreateShareLinkRequest::TOOL,
        RevokeShareLinkRequest::TOOL,
        RequestArtifactAccessRequest::TOOL,
        CancelAccessRequestRequest::TOOL,
        DecideAccessRequestRequest::TOOL,
        DeleteArtifactRequest::TOOL,
        SetUploadLimitsRequest::TOOL,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_request_names_its_tool_through_the_hub() {
        assert_eq!(
            CreateUploadRequest::hub_tool_name().unwrap().to_string(),
            "artifact__create_upload"
        );
        for tool in served_tools() {
            veoveo_hub_sdk::LocalToolName::parse(tool).unwrap();
        }
    }
}
