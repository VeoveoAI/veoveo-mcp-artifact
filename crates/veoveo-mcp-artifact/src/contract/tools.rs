//! Every tool's request and result. Owner, tenant, Work Context and provenance never
//! appear in a request: the server takes them from the forwarded identity.
use std::{borrow::Cow, num::NonZeroU32};

use chrono::{DateTime, Utc};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use veoveo_types::{AccessLevel, AccessSubject, Check, Sha256Digest, TenantId};

use super::{
    AccessRequestDecision, AccessRequestId, AccessRequestRecord, ArtifactGrant, ArtifactId,
    ArtifactLabels, ArtifactMetadataMap, ArtifactName, ArtifactRecord, ArtifactState, DerivedFrom,
    IdempotencyKey, MediaType, Note, ObjectSize, PartNumber, ReleaseState, RetentionDays,
    ShareLinkId, ShareLinkRecord, ShareLinkUrl, SignedPart, SignedTransfer, UploadId, UploadLimits,
    UploadRecord, UploadTransfer, ValueError,
};

/// The most parts one `sign_upload_parts` call signs.
pub const MAX_PARTS_PER_SIGNING: usize = 100;
/// The longest a share link lives, in days.
pub const MAX_SHARE_LINK_DAYS: u8 = 30;

/// `create_upload`: reserve an artifact and receive the URLs for its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateUploadRequest {
    pub name: ArtifactName,
    pub media_type: MediaType,
    /// The exact size, when the caller knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ObjectSize>,
    /// The SHA-256 of the whole object, when the caller knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256Digest>,
    /// The most bytes the caller will send when it does not know the size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<ObjectSize>,
    #[serde(default, skip_serializing_if = "ArtifactLabels::is_empty")]
    pub labels: ArtifactLabels,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<RetentionDays>,
    #[serde(default)]
    pub metadata: ArtifactMetadataMap,
    #[serde(default, skip_serializing_if = "DerivedFrom::is_empty")]
    pub derived_from: DerivedFrom,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,
}

impl Check for CreateUploadRequest {
    type Error = ValueError;
    fn check(&self) -> Result<(), ValueError> {
        if let (Some(size), Some(max)) = (self.size, self.max_size)
            && size > max
        {
            return Err(ValueError::from_static(
                "a declared size cannot exceed the declared maximum size",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateUploadResult {
    pub upload: UploadRecord,
    pub artifact: ArtifactRecord,
    /// Where the bytes go. Absent when a repeated idempotency key finds the upload
    /// already ended; the artifact then holds its outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<UploadTransfer>,
}

/// One part a client asks to have signed, with its length and SHA-256 when it knows
/// them. A signed length and checksum make object storage refuse different bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartRequest {
    pub part_number: PartNumber,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ObjectSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256Digest>,
}

/// `sign_upload_parts`: sign 1 to 100 parts of an open multipart upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignUploadPartsRequest {
    pub upload_id: UploadId,
    pub parts: Vec<PartRequest>,
}

impl Check for SignUploadPartsRequest {
    type Error = ValueError;
    fn check(&self) -> Result<(), ValueError> {
        if self.parts.is_empty() || self.parts.len() > MAX_PARTS_PER_SIGNING {
            return Err(ValueError::from_static(
                "a signing request names 1 to 100 parts",
            ));
        }
        let mut numbers: Vec<_> = self.parts.iter().map(|part| part.part_number).collect();
        numbers.sort_unstable();
        numbers.dedup();
        if numbers.len() != self.parts.len() {
            return Err(ValueError::from_static(
                "a signing request names each part once",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignUploadPartsResult {
    pub parts: Vec<SignedPart>,
}

/// `complete_upload`: complete an upload whose bytes arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteUploadRequest {
    pub upload_id: UploadId,
}

/// `abort_upload`: abort an open upload and release its reserved bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AbortUploadRequest {
    pub upload_id: UploadId,
}

/// The artifact after `complete_upload`: available, or verifying while the server
/// computes the SHA-256 of a large object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteUploadResult {
    pub artifact: ArtifactRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AbortUploadResult {
    pub upload: UploadRecord,
}

/// `create_download`: a signed GET of an artifact's bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateDownloadRequest {
    pub artifact_id: ArtifactId,
}

/// `delete_artifact`: delete an artifact and release its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteArtifactRequest {
    pub artifact_id: ArtifactId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateDownloadResult {
    pub artifact: ArtifactRecord,
    pub get: SignedTransfer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteArtifactResult {
    pub artifact_id: ArtifactId,
    pub state: ArtifactState,
}

/// `grant_artifact`: give a principal or group a level, until an optional expiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantArtifactRequest {
    pub artifact_id: ArtifactId,
    pub subject: AccessSubject,
    pub level: AccessLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantArtifactResult {
    pub grant: ArtifactGrant,
}

/// `revoke_artifact_grant`: remove a subject's grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeArtifactGrantRequest {
    pub artifact_id: ArtifactId,
    pub subject: AccessSubject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeArtifactGrantResult {
    /// False when the subject held no grant.
    pub revoked: bool,
}

/// `set_release_state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetReleaseStateRequest {
    pub artifact_id: ArtifactId,
    pub release_state: ReleaseState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactResult {
    pub artifact: ArtifactRecord,
}

/// How long a share link lives: 1 to 30 days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShareLinkDays(u8);

impl ShareLinkDays {
    pub fn new(days: u8) -> Result<Self, ValueError> {
        if days == 0 || days > MAX_SHARE_LINK_DAYS {
            return Err(ValueError::from_static("a share link lives 1 to 30 days"));
        }
        Ok(Self(days))
    }

    pub fn days(self) -> u8 {
        self.0
    }
}

impl Serialize for ShareLinkDays {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.0)
    }
}

impl<'de> Deserialize<'de> for ShareLinkDays {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u8::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ShareLinkDays {
    fn schema_name() -> Cow<'static, str> {
        "ShareLinkDays".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "integer", "minimum": 1, "maximum": MAX_SHARE_LINK_DAYS })
    }
}

/// `create_share_link`: an anonymous read link for a released artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateShareLinkRequest {
    pub artifact_id: ArtifactId,
    pub expires_in_days: ShareLinkDays,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redemption_limit: Option<NonZeroU32>,
}

/// The link's URL carries its secret, which the server returns once and never stores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateShareLinkResult {
    pub share_link: ShareLinkRecord,
    pub url: ShareLinkUrl,
}

/// `revoke_share_link`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeShareLinkRequest {
    pub artifact_id: ArtifactId,
    pub share_link_id: ShareLinkId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareLinkResult {
    pub share_link: ShareLinkRecord,
}

/// `request_artifact_access`: ask an artifact's administrators for a level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestArtifactAccessRequest {
    pub artifact_id: ArtifactId,
    pub level: AccessLevel,
    pub justification: Note,
}

/// `cancel_access_request`: withdraw the caller's pending request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelAccessRequestRequest {
    pub access_request_id: AccessRequestId,
}

/// `decide_access_request`: approve, which writes the grant, or deny.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecideAccessRequestRequest {
    pub access_request_id: AccessRequestId,
    pub decision: AccessRequestDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<Note>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequestResult {
    pub access_request: AccessRequestRecord,
    /// The grant an approval wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<ArtifactGrant>,
}

/// `set_upload_limits`: an installation administrator sets a tenant's limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetUploadLimitsRequest {
    pub tenant: TenantId,
    pub limits: UploadLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadLimitsResult {
    pub tenant: TenantId,
    pub limits: UploadLimits,
}

#[cfg(test)]
mod tests {
    use veoveo_types::Checked;

    use super::*;

    #[test]
    fn create_upload_admits_known_and_unknown_sizes() {
        let unknown: Checked<CreateUploadRequest> =
            serde_json::from_str(r#"{"name":"render.mp4","mediaType":"video/mp4"}"#).unwrap();
        assert!(unknown.size.is_none());
        assert!(unknown.metadata.as_object().is_empty());
        let too_small = serde_json::from_str::<Checked<CreateUploadRequest>>(
            r#"{"name":"a.bin","mediaType":"application/octet-stream","size":10,"maxSize":9}"#,
        );
        assert!(too_small.is_err());
        let owner = serde_json::from_str::<Checked<CreateUploadRequest>>(
            r#"{"name":"a.bin","mediaType":"application/octet-stream","owner":"mallory"}"#,
        );
        assert!(owner.is_err(), "the owner never comes from arguments");
    }

    #[test]
    fn part_signing_names_each_part_once() {
        let upload = UploadId::new();
        let request = |parts: &str| {
            serde_json::from_str::<Checked<SignUploadPartsRequest>>(&format!(
                r#"{{"uploadId":"{upload}","parts":{parts}}}"#
            ))
        };
        assert!(request(r#"[{"partNumber":1},{"partNumber":2}]"#).is_ok());
        assert!(request("[]").is_err());
        assert!(request(r#"[{"partNumber":1},{"partNumber":1}]"#).is_err());
        assert!(request(r#"[{"partNumber":0}]"#).is_err());
    }

    #[test]
    fn share_links_live_at_most_thirty_days() {
        assert!(ShareLinkDays::new(30).is_ok());
        assert!(ShareLinkDays::new(31).is_err());
        assert!(ShareLinkDays::new(0).is_err());
    }
}
