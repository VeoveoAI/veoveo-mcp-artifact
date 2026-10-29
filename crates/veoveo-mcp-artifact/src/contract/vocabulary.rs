//! Artifact's closed vocabularies. Each spelling is declared once; the server binds
//! them to PostgreSQL `text` columns whose `CHECK` constraints list the same spellings.
use veoveo_types::Vocabulary;

/// Whether an artifact may be shared through a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[cfg_attr(feature = "server", vocabulary(postgres))]
pub enum ReleaseState {
    /// Only its owner, grantees and Work Context members reach it.
    Private,
    /// It may also be shared with anyone through a share link.
    Released,
}

/// Where an artifact is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[cfg_attr(feature = "server", vocabulary(postgres))]
pub enum ArtifactState {
    /// Its upload is open and its bytes have not all arrived.
    Uploading,
    /// Its bytes arrived, passed size validation and can be read.
    Available,
    /// Its bytes did not match what the upload declared.
    Failed,
    /// It is deleted and its object is being removed.
    Deleting,
}

/// How an upload receives its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[cfg_attr(feature = "server", vocabulary(postgres))]
pub enum UploadForm {
    /// One signed PutObject.
    Single,
    /// An S3 multipart upload with signed part URLs.
    Multipart,
}

/// Where an upload is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[cfg_attr(feature = "server", vocabulary(postgres))]
pub enum UploadState {
    /// It accepts bytes.
    Open,
    /// Its artifact received its bytes.
    Completed,
    /// Its creator aborted it.
    Aborted,
    /// It passed its expiry before completion.
    Expired,
}

/// Where an access request is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[cfg_attr(feature = "server", vocabulary(postgres))]
pub enum AccessRequestState {
    Pending,
    Approved,
    Denied,
    Cancelled,
}

/// An administrator's answer to an access request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
pub enum AccessRequestDecision {
    /// Writes the requested grant.
    Approve,
    Deny,
}

/// Artifact's permissions. Each names one action on one resource type; installation
/// roles grant them and the Hub forwards the granted ones with every request. The
/// access kind and risks join each permission through the SDK's permission declaration
/// when the server is hosted.
/// ```compile_fail
/// use veoveo_mcp_artifact::contract::ArtifactPermission;
/// fn requires(_: ArtifactPermission) {}
/// requires("artifact.artifacts.read");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
pub enum ArtifactPermission {
    /// Read artifacts and issue download URLs.
    #[vocabulary(rename = "artifact.artifacts.read")]
    ArtifactsRead,
    /// Create, complete and abort uploads.
    #[vocabulary(rename = "artifact.artifacts.write")]
    ArtifactsWrite,
    /// Delete artifacts. Destructive.
    #[vocabulary(rename = "artifact.artifacts.delete")]
    ArtifactsDelete,
    /// Grant and revoke access, and decide access requests, inside the tenant.
    #[vocabulary(rename = "artifact.grants.write")]
    GrantsWrite,
    /// Move an artifact between private and released.
    #[vocabulary(rename = "artifact.releases.write")]
    ReleasesWrite,
    /// Create and revoke share links. Sensitive.
    #[vocabulary(rename = "artifact.share-links.write")]
    ShareLinksWrite,
    /// Request access to an artifact and cancel the request.
    #[vocabulary(rename = "artifact.access-requests.write")]
    AccessRequestsWrite,
    /// Set a tenant's upload limits. Administer.
    #[vocabulary(rename = "artifact.upload-limits.administer")]
    UploadLimitsAdminister,
}

impl ArtifactPermission {
    /// The resource type the permission acts on.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::ArtifactsRead | Self::ArtifactsWrite | Self::ArtifactsDelete => {
                "artifact.artifact"
            }
            Self::GrantsWrite => "artifact.grant",
            Self::ReleasesWrite => "artifact.release",
            Self::ShareLinksWrite => "artifact.share-link",
            Self::AccessRequestsWrite => "artifact.access-request",
            Self::UploadLimitsAdminister => "artifact.upload-limits",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_follow_the_naming_form() {
        for permission in ArtifactPermission::ALL {
            let name = permission.as_str();
            let mut parts = name.split('.');
            assert_eq!(parts.next(), Some("artifact"), "{name}");
            assert!(parts.next().is_some_and(|plural| !plural.is_empty()));
            assert!(parts.next().is_some_and(|verb| !verb.is_empty()));
            assert_eq!(parts.next(), None, "{name}");
            assert!(permission.resource_type().starts_with("artifact."));
        }
        assert_eq!(
            "artifact.share-links.write".parse::<ArtifactPermission>(),
            Ok(ArtifactPermission::ShareLinksWrite)
        );
    }

    #[test]
    fn states_use_snake_case_spellings() {
        assert_eq!(UploadForm::Multipart.as_str(), "multipart");
        assert_eq!(ReleaseState::Released.as_str(), "released");
    }
}
