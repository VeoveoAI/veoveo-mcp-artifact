//! Artifact's audit vocabulary: one class per decision, `artifact.<name>`, each with a
//! closed detail type whose JSON Schema the server declares. A record's target is the
//! artifact or upload resource it names, and a decision another server asked for on the
//! caller's behalf names that server as `actingServer`.
use std::num::NonZeroU32;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use veoveo_audit_contract::{AuditDetailError, AuditDetailType, AuditRegistryBuilder};
use veoveo_types::{AccessLevel, AccessSubject, PrincipalId, ServerSlug, Sha256Digest, TenantId};

use super::{
    AccessRequestDecision, AccessRequestId, ArtifactId, ArtifactState, ObjectSize, ReleaseState,
    ShareLinkId, UploadForm, UploadId, UploadLimits,
};

/// An upload reserved an artifact and its bytes: class `artifact.upload_created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadCreated {
    pub artifact_id: ArtifactId,
    pub upload_id: UploadId,
    pub form: UploadForm,
    pub reserved: ObjectSize,
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
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
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
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
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
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
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
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
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
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
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
}
impl AuditDetailType for ArtifactVerified {
    const CLASS: &'static str = "artifact.verified";
}

/// An artifact was deleted: class `artifact.deleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactDeleted {
    pub artifact_id: ArtifactId,
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
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
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
}
impl AuditDetailType for UploadLimitsSet {
    const CLASS: &'static str = "artifact.upload_limits_set";
}

/// An administrator moved an artifact between private and released: class
/// `artifact.release_changed`. Returning it to private ends its live share links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseChanged {
    pub artifact_id: ArtifactId,
    pub release_state: ReleaseState,
    /// How many live share links the change ended.
    pub ended_share_links: u32,
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
}
impl AuditDetailType for ReleaseChanged {
    const CLASS: &'static str = "artifact.release_changed";
}

/// An administrator created a share link: class `artifact.share_link_created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareLinkCreated {
    pub artifact_id: ArtifactId,
    pub share_link_id: ShareLinkId,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redemption_limit: Option<NonZeroU32>,
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
}
impl AuditDetailType for ShareLinkCreated {
    const CLASS: &'static str = "artifact.share_link_created";
}

/// An administrator revoked a share link: class `artifact.share_link_revoked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareLinkRevoked {
    pub artifact_id: ArtifactId,
    pub share_link_id: ShareLinkId,
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
}
impl AuditDetailType for ShareLinkRevoked {
    const CLASS: &'static str = "artifact.share_link_revoked";
}

/// Someone holding a share link received a download URL, valid until `expires_at`:
/// class `artifact.share_link_redeemed`. The holder has no account, so the record names
/// no actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareLinkRedeemed {
    pub artifact_id: ArtifactId,
    pub share_link_id: ShareLinkId,
    /// The link's redemptions, this one included.
    pub redemptions: u32,
    pub expires_at: DateTime<Utc>,
}
impl AuditDetailType for ShareLinkRedeemed {
    const CLASS: &'static str = "artifact.share_link_redeemed";
}

/// A caller asked an artifact's administrators for a level: class
/// `artifact.access_requested`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequested {
    pub artifact_id: ArtifactId,
    pub access_request_id: AccessRequestId,
    pub level: AccessLevel,
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
}
impl AuditDetailType for AccessRequested {
    const CLASS: &'static str = "artifact.access_requested";
}

/// A requester withdrew a pending request: class `artifact.access_request_cancelled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequestCancelled {
    pub artifact_id: ArtifactId,
    pub access_request_id: AccessRequestId,
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
}
impl AuditDetailType for AccessRequestCancelled {
    const CLASS: &'static str = "artifact.access_request_cancelled";
}

/// An administrator approved or denied a request: class
/// `artifact.access_request_decided`. An approval also records the grant it wrote as
/// `artifact.grant_changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequestDecided {
    pub artifact_id: ArtifactId,
    pub access_request_id: AccessRequestId,
    pub requester: PrincipalId,
    pub level: AccessLevel,
    pub decision: AccessRequestDecision,
    /// The server that made the call for the caller, when another server called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
}
impl AuditDetailType for AccessRequestDecided {
    const CLASS: &'static str = "artifact.access_request_decided";
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
    builder.declare::<ReleaseChanged>()?;
    builder.declare::<ShareLinkCreated>()?;
    builder.declare::<ShareLinkRevoked>()?;
    builder.declare::<ShareLinkRedeemed>()?;
    builder.declare::<AccessRequested>()?;
    builder.declare::<AccessRequestCancelled>()?;
    builder.declare::<AccessRequestDecided>()?;
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
        assert_eq!(classes.len(), 15, "{classes:?}");
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
