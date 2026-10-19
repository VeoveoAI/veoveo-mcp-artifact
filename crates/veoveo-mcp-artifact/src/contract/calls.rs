//! Artifact as a provider of calls between servers. Each served tool's request names its
//! tool and its typed result, and `ReadArtifact` names one artifact's resource and its
//! content, so a server that declares uses of Artifact's permissions calls and reads
//! Artifact through the Hub with the SDK's `HubClient`. The `client` feature's
//! `ArtifactClient` builds uploads and downloads on these calls.
use veoveo_hub_sdk::{
    calls::{ProviderContract, ResourceRead, ToolCall},
    definition::{CollectionResource, Product, TemplateRead},
};
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
    type Output = Product<CreateUploadResult>;
    const TOOL: &'static str = "create_upload";
}

impl ToolCall for SignUploadPartsRequest {
    type Provider = ArtifactProvider;
    type Output = SignUploadPartsResult;
    const TOOL: &'static str = "sign_upload_parts";
}

impl ToolCall for CompleteUploadRequest {
    type Provider = ArtifactProvider;
    type Output = Product<CompleteUploadResult>;
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
    type Output = Product<ArtifactResult>;
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

impl TemplateRead for ReadArtifact {
    const TEMPLATE: &'static str = "artifact://artifacts/{artifact_id}";
    fn from_address(address: &ArtifactResource) -> Option<Self> {
        if let ArtifactResource::Artifact(artifact) = address {
            Some(Self {
                artifact: *artifact,
            })
        } else {
            None
        }
    }
}
#[derive(Debug, Clone)]
pub struct ReadArtifacts {
    pub filter: Option<super::ArtifactFilter>,
}
impl CollectionResource for ReadArtifacts {
    type Provider = ArtifactProvider;
    type Address = ArtifactResource;
    type Collection = super::Artifacts;
    const TEMPLATE: &'static str =
        "artifact://artifacts{?work_context,owner,media_type,name,release_state}";
    fn address(&self) -> ArtifactResource {
        use super::ArtifactFilter;
        let mut address = ArtifactResource::Artifacts {
            work_context: None,
            owner: None,
            media_type: None,
            name: None,
            release_state: None,
        };
        if let ArtifactResource::Artifacts {
            work_context,
            owner,
            media_type,
            name,
            release_state,
        } = &mut address
        {
            match &self.filter {
                None => {}
                Some(ArtifactFilter::WorkContext(value)) => *work_context = Some(value.clone()),
                Some(ArtifactFilter::Owner(value)) => *owner = Some(value.clone()),
                Some(ArtifactFilter::MediaType(value)) => *media_type = Some(value.clone()),
                Some(ArtifactFilter::NamePrefix(value)) => *name = Some(value.clone()),
                Some(ArtifactFilter::ReleaseState(value)) => *release_state = Some(*value),
            }
        }
        address
    }
    fn filters(&self) -> Option<super::ArtifactFilter> {
        self.filter.clone()
    }
    fn from_address(address: &ArtifactResource) -> Option<Self> {
        if let ArtifactResource::Artifacts {
            work_context,
            owner,
            media_type,
            name,
            release_state,
        } = address
        {
            Some(Self {
                filter: super::ArtifactFilter::from_parts(
                    work_context.clone(),
                    owner.clone(),
                    media_type.clone(),
                    name.clone(),
                    *release_state,
                )
                .ok()?,
            })
        } else {
            None
        }
    }
    fn default_limit() -> veoveo_types::PageSize<100> {
        veoveo_types::PageSize::new(50).unwrap()
    }
}
#[derive(Debug, Clone)]
pub struct ReadUploads {
    pub state: Option<super::UploadState>,
}
impl CollectionResource for ReadUploads {
    type Provider = ArtifactProvider;
    type Address = ArtifactResource;
    type Collection = super::Uploads;
    const TEMPLATE: &'static str = "artifact://uploads{?state}";
    fn address(&self) -> ArtifactResource {
        ArtifactResource::Uploads { state: self.state }
    }
    fn filters(&self) -> super::UploadFilters {
        super::UploadFilters { state: self.state }
    }
    fn from_address(address: &ArtifactResource) -> Option<Self> {
        if let ArtifactResource::Uploads { state } = address {
            Some(Self { state: *state })
        } else {
            None
        }
    }
    fn default_limit() -> veoveo_types::PageSize<100> {
        veoveo_types::PageSize::new(50).unwrap()
    }
}
#[derive(Debug, Clone)]
pub struct ReadShareLinks {
    pub artifact: ArtifactId,
}
impl CollectionResource for ReadShareLinks {
    type Provider = ArtifactProvider;
    type Address = ArtifactResource;
    type Collection = super::ShareLinks;
    const TEMPLATE: &'static str = "artifact://artifacts/{artifact_id}/share-links";
    fn address(&self) -> ArtifactResource {
        ArtifactResource::ShareLinks {
            artifact_id: self.artifact,
        }
    }
    fn filters(&self) -> ArtifactId {
        self.artifact
    }
    fn from_address(address: &ArtifactResource) -> Option<Self> {
        if let ArtifactResource::ShareLinks { artifact_id } = address {
            Some(Self {
                artifact: *artifact_id,
            })
        } else {
            None
        }
    }
    fn default_limit() -> veoveo_types::PageSize<100> {
        veoveo_types::PageSize::new(50).unwrap()
    }
}
#[derive(Debug, Clone)]
pub struct ReadGrants {
    pub artifact: ArtifactId,
}
impl CollectionResource for ReadGrants {
    type Provider = ArtifactProvider;
    type Address = ArtifactResource;
    type Collection = super::Grants;
    const TEMPLATE: &'static str = "artifact://artifacts/{artifact_id}/grants";
    fn address(&self) -> ArtifactResource {
        ArtifactResource::Grants(self.artifact)
    }
    fn filters(&self) -> ArtifactId {
        self.artifact
    }
    fn from_address(address: &ArtifactResource) -> Option<Self> {
        if let ArtifactResource::Grants(artifact) = address {
            Some(Self {
                artifact: *artifact,
            })
        } else {
            None
        }
    }
    fn default_limit() -> veoveo_types::PageSize<256> {
        veoveo_types::PageSize::new(50).unwrap()
    }
}
#[derive(Debug, Clone)]
pub struct ReadAccessRequests {
    pub role: Option<super::AccessRequestRole>,
    pub state: Option<super::AccessRequestState>,
}
impl CollectionResource for ReadAccessRequests {
    type Provider = ArtifactProvider;
    type Address = ArtifactResource;
    type Collection = super::AccessRequests;
    const TEMPLATE: &'static str = "artifact://access-requests{?role,state}";
    fn address(&self) -> ArtifactResource {
        ArtifactResource::AccessRequests {
            role: self.role,
            state: self.state,
        }
    }
    fn filters(&self) -> super::AccessRequestFilters {
        super::AccessRequestFilters {
            role: self.role,
            state: self.state,
        }
    }
    fn from_address(address: &ArtifactResource) -> Option<Self> {
        if let ArtifactResource::AccessRequests { role, state } = address {
            Some(Self {
                role: *role,
                state: *state,
            })
        } else {
            None
        }
    }
    fn default_limit() -> veoveo_types::PageSize<100> {
        veoveo_types::PageSize::new(50).unwrap()
    }
}
#[derive(Debug, Clone)]
pub struct ReadUpload {
    pub upload: super::UploadId,
}
impl ResourceRead for ReadUpload {
    type Provider = ArtifactProvider;
    type Address = ArtifactResource;
    type Content = super::UploadRecord;
    fn address(&self) -> ArtifactResource {
        ArtifactResource::Upload(self.upload)
    }
}
impl TemplateRead for ReadUpload {
    const TEMPLATE: &'static str = "artifact://uploads/{upload_id}";
    fn from_address(address: &ArtifactResource) -> Option<Self> {
        if let ArtifactResource::Upload(upload) = address {
            Some(Self { upload: *upload })
        } else {
            None
        }
    }
}
#[derive(Debug, Clone)]
pub struct ReadStorageUsage;
impl ResourceRead for ReadStorageUsage {
    type Provider = ArtifactProvider;
    type Address = ArtifactResource;
    type Content = super::StorageUsage;
    fn address(&self) -> ArtifactResource {
        ArtifactResource::StorageUsage
    }
}
