//! Artifact's audit vocabulary: one class per decision, `artifact.<name>`, each with a
//! closed detail type whose JSON Schema the server declares and the OCSF API Activity it
//! maps to, and the reason codes of decisions that did not succeed. A record's target is
//! the Artifact resource it concerns. The envelope names the caller, and a decision another
//! server asked for on the caller's behalf names that server and its Task as `via`.
use std::num::NonZeroU32;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use veoveo_audit_contract::{
    AuditDetailError, AuditDetailType, AuditOcsfClass, AuditReason, AuditRegistryBuilder,
};
use veoveo_types::{AccessLevel, AccessSubject, PrincipalId, TenantId, Vocabulary as _};

use super::{
    AccessRequestDecision, AccessRequestId, ArtifactId, ArtifactState, ObjectSize, ReleaseState,
    ShareLinkId, UploadForm, UploadId, UploadLimits,
};

/// Why an Artifact decision did not succeed, as the reason code of its audit record.
/// Artifact records the decisions it makes; a refused request changes nothing and
/// records nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, veoveo_types::Vocabulary)]
pub enum ArtifactAuditReason {
    /// The bytes that arrived differ in size from the size the upload declared or
    /// exceed the bytes it reserved, so the artifact failed.
    SizeMismatch,
}

impl ArtifactAuditReason {
    /// The record's reason code.
    pub fn code(self) -> AuditReason {
        AuditReason::parse(self.as_str()).expect("an Artifact reason is a reason code")
    }
}

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
    /// OCSF API Activity, Create.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(1);
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
    /// OCSF API Activity, Update.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(3);
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
    /// OCSF API Activity, Delete.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(4);
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
    /// OCSF API Activity, Read.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(2);
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
    /// OCSF API Activity, Update.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(3);
}

/// An artifact was deleted: class `artifact.deleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactDeleted {
    pub artifact_id: ArtifactId,
}
impl AuditDetailType for ArtifactDeleted {
    const CLASS: &'static str = "artifact.deleted";
    /// OCSF API Activity, Delete.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(4);
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
    /// OCSF API Activity, Update.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(3);
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
}
impl AuditDetailType for ReleaseChanged {
    const CLASS: &'static str = "artifact.release_changed";
    /// OCSF API Activity, Update.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(3);
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
}
impl AuditDetailType for ShareLinkCreated {
    const CLASS: &'static str = "artifact.share_link_created";
    /// OCSF API Activity, Create.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(1);
}

/// An administrator revoked a share link: class `artifact.share_link_revoked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareLinkRevoked {
    pub artifact_id: ArtifactId,
    pub share_link_id: ShareLinkId,
}
impl AuditDetailType for ShareLinkRevoked {
    const CLASS: &'static str = "artifact.share_link_revoked";
    /// OCSF API Activity, Delete.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(4);
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
    /// OCSF API Activity, Read.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(2);
}

/// A caller asked an artifact's administrators for a level: class
/// `artifact.access_requested`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequested {
    pub artifact_id: ArtifactId,
    pub access_request_id: AccessRequestId,
    pub level: AccessLevel,
}
impl AuditDetailType for AccessRequested {
    const CLASS: &'static str = "artifact.access_requested";
    /// OCSF API Activity, Create.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(1);
}

/// A requester withdrew a pending request: class `artifact.access_request_cancelled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequestCancelled {
    pub artifact_id: ArtifactId,
    pub access_request_id: AccessRequestId,
}
impl AuditDetailType for AccessRequestCancelled {
    const CLASS: &'static str = "artifact.access_request_cancelled";
    /// OCSF API Activity, Update.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(3);
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
}
impl AuditDetailType for AccessRequestDecided {
    const CLASS: &'static str = "artifact.access_request_decided";
    /// OCSF API Activity, Update.
    const OCSF: AuditOcsfClass = AuditOcsfClass::api_activity(3);
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
        assert_eq!(classes.len(), 14, "{classes:?}");
        for declaration in registry
            .declarations()
            .filter(|declaration| declaration.class().to_string().starts_with("artifact."))
        {
            let schema = declaration
                .schema()
                .expect("Artifact documents every detail");
            assert_eq!(
                schema["additionalProperties"],
                serde_json::Value::Bool(false),
                "{}",
                declaration.class()
            );
            let ocsf = declaration.ocsf();
            assert_eq!(ocsf.class_uid, 6003, "{}", declaration.class());
            assert!(
                (1..=4).contains(&ocsf.activity_id),
                "{} maps to no API Activity",
                declaration.class()
            );
        }
    }

    #[test]
    fn reasons_are_audit_reason_codes() {
        let codes: Vec<String> = ArtifactAuditReason::ALL
            .iter()
            .map(|reason| reason.code().to_string())
            .collect();
        assert_eq!(codes, ["size_mismatch"]);
    }
}
