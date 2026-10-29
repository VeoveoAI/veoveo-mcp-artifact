//! Artifact's OAuth scope and the access kind and risks of each permission, as the
//! `ai.veoveo/authorization` declaration carries them.
use veoveo_hub_sdk::registration::{Access, PermissionDefinition, Risk};
use veoveo_types::{PermissionName, Vocabulary};

use super::ArtifactPermission;

/// The OAuth scope that names Artifact's namespace. Scopes are coarse; Artifact's
/// permissions carry fine-grained access.
/// ```compile_fail
/// use veoveo_mcp_artifact::contract::ArtifactScope;
/// fn requires_artifact(_: ArtifactScope) {}
/// requires_artifact("artifact");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[vocabulary(scope)]
pub enum ArtifactScope {
    #[vocabulary(rename = "artifact")]
    Artifact,
}

impl PermissionDefinition for ArtifactPermission {
    fn resource(self) -> &'static str {
        self.resource_type()
    }

    fn access(self) -> Access {
        match self {
            Self::ArtifactsRead => Access::Read,
            Self::ArtifactsWrite
            | Self::ArtifactsDelete
            | Self::GrantsWrite
            | Self::ReleasesWrite
            | Self::ShareLinksWrite
            | Self::AccessRequestsWrite
            | Self::UploadLimitsAdminister => Access::Write,
        }
    }

    fn risks(self) -> &'static [Risk] {
        match self {
            Self::ArtifactsDelete => &[Risk::Destructive],
            Self::ShareLinksWrite => &[Risk::Sensitive],
            Self::UploadLimitsAdminister => &[Risk::Administer],
            Self::ArtifactsRead
            | Self::ArtifactsWrite
            | Self::GrantsWrite
            | Self::ReleasesWrite
            | Self::AccessRequestsWrite => &[],
        }
    }
}

impl ArtifactPermission {
    /// The permission's wire name, as declared and as granted.
    pub fn name(self) -> PermissionName {
        PermissionName::parse(self.as_str()).expect("declared Artifact permission spelling")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_reading_is_read_access_and_risks_follow_the_design() {
        for permission in ArtifactPermission::ALL {
            assert_eq!(
                permission.access() == Access::Read,
                *permission == ArtifactPermission::ArtifactsRead
            );
        }
        assert_eq!(
            ArtifactPermission::ArtifactsDelete.risks(),
            &[Risk::Destructive]
        );
        assert_eq!(ArtifactPermission::GrantsWrite.risks(), &[]);
        assert_eq!(
            ArtifactPermission::ShareLinksWrite.risks(),
            &[Risk::Sensitive]
        );
        assert_eq!(
            ArtifactPermission::UploadLimitsAdminister.risks(),
            &[Risk::Administer]
        );
        assert_eq!(ArtifactScope::Artifact.as_str(), "artifact");
    }
}
