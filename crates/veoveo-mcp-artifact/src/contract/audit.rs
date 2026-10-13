//! Artifact's audit vocabulary: one class per decision, `artifact.<name>`, each with a
//! closed detail type whose JSON Schema the server declares. A record's target is the
//! artifact or upload resource it names.
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use veoveo_audit_contract::{AuditDetailError, AuditDetailType, AuditRegistryBuilder};
use veoveo_types::{AccessLevel, AccessSubject, Sha256Digest, TenantId};

use super::{ArtifactId, ArtifactState, ObjectSize, UploadForm, UploadId, UploadLimits};

/// An upload reserved an artifact and its bytes: class `artifact.upload_created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadCreated {
    pub artifact_id: ArtifactId,
    pub upload_id: UploadId,
    pub form: UploadForm,
    pub reserved: ObjectSize,
}
impl AuditDetailType for UploadCreated {
    const CLASS: &'static str = "artifact.upload_created";
}

/// An upload's bytes arrived: class `artifact.upload_completed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadCompleted {
    pub artifact_id: ArtifactId,
    pub upload_id: UploadId,
    pub size: ObjectSize,
    pub state: ArtifactState,
}
impl AuditDetailType for UploadCompleted {
    const CLASS: &'static str = "artifact.upload_completed";
}

/// An upload's creator aborted it: class `artifact.upload_aborted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadAborted {
    pub artifact_id: ArtifactId,
    pub upload_id: UploadId,
}
impl AuditDetailType for UploadAborted {
    const CLASS: &'static str = "artifact.upload_aborted";
}

/// A caller received a download URL, valid until `expires_at`: class
/// `artifact.download_issued`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadIssued {
    pub artifact_id: ArtifactId,
    pub expires_at: DateTime<Utc>,
}
impl AuditDetailType for DownloadIssued {
    const CLASS: &'static str = "artifact.download_issued";
}

/// A grant was written, or revoked when `level` is absent: class
/// `artifact.grant_changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantChanged {
    pub artifact_id: ArtifactId,
    pub subject: AccessSubject,
    pub level: Option<AccessLevel>,
    pub expires_at: Option<DateTime<Utc>>,
}
impl AuditDetailType for GrantChanged {
    const CLASS: &'static str = "artifact.grant_changed";
}

/// The server computed an artifact's SHA-256: class `artifact.verified`. The artifact is
/// available, or failed when the digest differs from the one its upload declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactVerified {
    pub artifact_id: ArtifactId,
    pub sha256: Sha256Digest,
    pub state: ArtifactState,
}
impl AuditDetailType for ArtifactVerified {
    const CLASS: &'static str = "artifact.verified";
}

/// An artifact was deleted: class `artifact.deleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactDeleted {
    pub artifact_id: ArtifactId,
}
impl AuditDetailType for ArtifactDeleted {
    const CLASS: &'static str = "artifact.deleted";
}

/// An installation administrator set a tenant's upload limits: class
/// `artifact.upload_limits_set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadLimitsSet {
    pub tenant: TenantId,
    pub limits: UploadLimits,
}
impl AuditDetailType for UploadLimitsSet {
    const CLASS: &'static str = "artifact.upload_limits_set";
}

/// A registry that holds the platform's declarations and Artifact's.
pub fn audit_registry() -> Result<veoveo_audit_contract::AuditRegistry, AuditDetailError> {
    let mut builder = veoveo_audit_contract::AuditRegistry::builder();
    declare_audit_details(&mut builder)?;
    Ok(builder.build())
}

/// Declare every Artifact audit class in `builder`.
pub fn declare_audit_details(builder: &mut AuditRegistryBuilder) -> Result<(), AuditDetailError> {
    builder.declare::<UploadCreated>()?;
    builder.declare::<UploadCompleted>()?;
    builder.declare::<UploadAborted>()?;
    builder.declare::<DownloadIssued>()?;
    builder.declare::<ArtifactVerified>()?;
    builder.declare::<GrantChanged>()?;
    builder.declare::<ArtifactDeleted>()?;
    builder.declare::<UploadLimitsSet>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use veoveo_audit_contract::AuditRegistry;

    use super::*;

    #[test]
    fn every_class_declares_a_closed_schema_under_the_artifact_owner() {
        let mut builder = AuditRegistry::builder();
        declare_audit_details(&mut builder).unwrap();
        let registry = builder.build();
        let classes: Vec<String> = registry
            .declarations()
            .map(|declaration| declaration.class().to_string())
            .filter(|class| class.starts_with("artifact."))
            .collect();
        assert_eq!(classes.len(), 8, "{classes:?}");
        for declaration in registry
            .declarations()
            .filter(|declaration| declaration.class().to_string().starts_with("artifact."))
        {
            assert_eq!(
                declaration.schema()["additionalProperties"],
                serde_json::Value::Bool(false),
                "{}",
                declaration.class()
            );
        }
    }
}
