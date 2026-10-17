//! What Artifact returns about its records: artifacts, grants, uploads, share links,
//! access requests, storage usage and upload limits.
use std::{borrow::Cow, collections::BTreeSet, fmt, num::NonZeroU32};

use chrono::{DateTime, Utc};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use veoveo_hub_sdk::OpaqueTaskId;
use veoveo_types::{
    AccessLevel, AccessSubject, PrincipalId, ServerSlug, Sha256Digest, WorkContextId,
};

use super::{
    AccessRequestId, AccessRequestState, ArtifactId, ArtifactLabels, ArtifactMetadataMap,
    ArtifactName, ArtifactState, MediaType, ObjectSize, ReleaseState, ShareLinkId, UploadForm,
    UploadId, UploadState, ValueError,
};

/// The most artifacts one artifact names as its sources.
pub const MAX_DERIVED_FROM: usize = 64;
/// The most bytes a justification or decision note holds.
pub const MAX_NOTE_BYTES: usize = 1000;

/// The artifacts an artifact derives from, at most [`MAX_DERIVED_FROM`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "BTreeSet<ArtifactId>", into = "BTreeSet<ArtifactId>")]
pub struct DerivedFrom(BTreeSet<ArtifactId>);

impl DerivedFrom {
    pub fn new(sources: BTreeSet<ArtifactId>) -> Result<Self, ValueError> {
        if sources.len() > MAX_DERIVED_FROM {
            return Err(ValueError::from_static(
                "an artifact derives from at most 64 artifacts",
            ));
        }
        Ok(Self(sources))
    }

    pub fn iter(&self) -> impl Iterator<Item = &ArtifactId> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<BTreeSet<ArtifactId>> for DerivedFrom {
    type Error = ValueError;
    fn try_from(sources: BTreeSet<ArtifactId>) -> Result<Self, Self::Error> {
        Self::new(sources)
    }
}

impl From<DerivedFrom> for BTreeSet<ArtifactId> {
    fn from(value: DerivedFrom) -> Self {
        value.0
    }
}

/// Where an artifact came from. The server fills it from the forwarded identity: the
/// caller, the server acting for the caller when another server called, and that
/// server's Task when the call came from one, as the acting server identifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactProvenance {
    pub caller: PrincipalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_server: Option<ServerSlug>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<OpaqueTaskId>,
    #[serde(default, skip_serializing_if = "DerivedFrom::is_empty")]
    pub derived_from: DerivedFrom,
}

/// One artifact as a caller who may read it sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactRecord {
    pub artifact_id: ArtifactId,
    pub name: ArtifactName,
    pub media_type: MediaType,
    /// Known once the bytes arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ObjectSize>,
    /// Known once the artifact is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256Digest>,
    pub owner: PrincipalId,
    pub work_context: WorkContextId,
    pub labels: ArtifactLabels,
    pub release_state: ReleaseState,
    pub state: ArtifactState,
    pub provenance: ArtifactProvenance,
    pub metadata: ArtifactMetadataMap,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retain_until: Option<DateTime<Utc>>,
}

/// One grant on an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactGrant {
    pub artifact_id: ArtifactId,
    pub subject: AccessSubject,
    pub level: AccessLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub granted_by: PrincipalId,
    pub granted_at: DateTime<Utc>,
}

/// The part size of a multipart upload: 5 MiB to 5 GiB, the S3 bounds for every part
/// but the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartSize(u64);

impl PartSize {
    pub const MIN: u64 = 5 * 1024 * 1024;
    pub const MAX: u64 = 5 * 1024 * 1024 * 1024;
    /// The part size of an upload whose bound fits 10,000 parts of it: 16 MiB.
    pub const DEFAULT: u64 = 16 * 1024 * 1024;

    pub fn new(bytes: u64) -> Result<Self, ValueError> {
        if !(Self::MIN..=Self::MAX).contains(&bytes) {
            return Err(ValueError::from_static("a part holds 5 MiB to 5 GiB"));
        }
        Ok(Self(bytes))
    }

    pub fn bytes(self) -> u64 {
        self.0
    }
}

/// A multipart upload's part number: 1 to 10,000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartNumber(u16);

impl PartNumber {
    pub const MAX: u16 = 10_000;

    pub fn new(number: u16) -> Result<Self, ValueError> {
        if number == 0 || number > Self::MAX {
            return Err(ValueError::from_static("a part number is 1 to 10000"));
        }
        Ok(Self(number))
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

/// One upload as its creator sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadRecord {
    pub upload_id: UploadId,
    pub artifact_id: ArtifactId,
    pub form: UploadForm,
    pub state: UploadState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_size: Option<ObjectSize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_sha256: Option<Sha256Digest>,
    /// The bytes the upload holds against the tenant's quota while it is open.
    pub reserved: ObjectSize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_size: Option<PartSize>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// When the upload completed, was aborted or expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

/// One share link without its secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareLinkRecord {
    pub share_link_id: ShareLinkId,
    pub artifact_id: ArtifactId,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redemption_limit: Option<NonZeroU32>,
    pub redemptions: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_by: PrincipalId,
    pub created_at: DateTime<Utc>,
}

/// The most bytes a share link's URL holds.
pub const MAX_SHARE_LINK_URL_BYTES: usize = 2048;

/// The URL of a share link: an absolute `http` or `https` URL whose last path segment is
/// the link's secret. Anyone holding it reads the artifact while the link lives, so the
/// server returns it once and stores only the secret's SHA-256.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ShareLinkUrl(String);

impl ShareLinkUrl {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        let absolute = value.starts_with("https://") || value.starts_with("http://");
        if !absolute
            || value.len() > MAX_SHARE_LINK_URL_BYTES
            || value
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(ValueError::from_static(
                "a share link URL is an absolute http or https URL of at most 2048 bytes",
            ));
        }
        Ok(Self(value))
    }

    /// The URL, secret included. Hand it only to the people the link is for.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// The URL carries the link's secret, so its debug form names only its kind.
impl fmt::Debug for ShareLinkUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ShareLinkUrl(..)")
    }
}

impl TryFrom<String> for ShareLinkUrl {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ShareLinkUrl> for String {
    fn from(value: ShareLinkUrl) -> Self {
        value.0
    }
}

impl JsonSchema for ShareLinkUrl {
    fn schema_name() -> Cow<'static, str> {
        "ShareLinkUrl".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "uri",
            "maxLength": MAX_SHARE_LINK_URL_BYTES,
            "description": "The share link's URL, which carries its secret."
        })
    }
}

/// A justification or a decision note: 1 to 1000 bytes. Line breaks are allowed; other
/// control characters are not.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Note(String);

impl Note {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        let printable = value
            .chars()
            .all(|character| !character.is_control() || character == '\n');
        if value.is_empty() || value.len() > MAX_NOTE_BYTES || !printable {
            return Err(ValueError::from_static(
                "a note holds 1 to 1000 bytes without control characters besides line breaks",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Note {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<Note> for String {
    fn from(value: Note) -> Self {
        value.0
    }
}

impl fmt::Display for Note {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl JsonSchema for Note {
    fn schema_name() -> Cow<'static, str> {
        "Note".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "minLength": 1, "maxLength": MAX_NOTE_BYTES })
    }
}

/// One access request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequestRecord {
    pub access_request_id: AccessRequestId,
    pub artifact_id: ArtifactId,
    pub requester: PrincipalId,
    pub level: AccessLevel,
    pub justification: Note,
    pub state: AccessRequestState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<PrincipalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<Note>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
}

/// A count of bytes held against a tenant's quota.
pub type ByteCount = u64;

/// A tenant's stored and reserved bytes against its quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageUsage {
    pub stored_bytes: ByteCount,
    pub reserved_bytes: ByteCount,
    /// The tenant's quota; absent when the tenant has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_bytes: Option<ByteCount>,
}

/// How many uploads a tenant may hold open at once: 1 to 10,000.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActiveUploadLimit(u16);

impl ActiveUploadLimit {
    pub fn new(count: u16) -> Result<Self, ValueError> {
        if count == 0 || count > 10_000 {
            return Err(ValueError::from_static(
                "a tenant holds 1 to 10000 open uploads",
            ));
        }
        Ok(Self(count))
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

/// The media types a tenant may upload: any, or a listed set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    rename_all = "snake_case",
    tag = "kind",
    content = "types",
    deny_unknown_fields
)]
pub enum AllowedMediaTypes {
    Any,
    Listed(BTreeSet<MediaType>),
}

impl AllowedMediaTypes {
    pub fn allows(&self, media_type: &MediaType) -> bool {
        match self {
            Self::Any => true,
            Self::Listed(types) => types.contains(media_type),
        }
    }
}

/// A tenant's upload limits. A tenant without its own limits has
/// [`UploadLimits::default`]: no quota, objects up to 5 TiB, any media type and 64 open
/// uploads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_bytes: Option<ByteCount>,
    pub largest_object: ObjectSize,
    pub allowed_media_types: AllowedMediaTypes,
    pub active_uploads: ActiveUploadLimit,
}

impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            quota_bytes: None,
            largest_object: ObjectSize::new(super::MAX_OBJECT_BYTES)
                .expect("the largest object fits its own bound"),
            allowed_media_types: AllowedMediaTypes::Any,
            active_uploads: ActiveUploadLimit::new(64).expect("64 open uploads fits the bound"),
        }
    }
}

fn integer_schema(minimum: u64, maximum: u64, description: &str) -> Schema {
    json_schema!({
        "type": "integer",
        "minimum": minimum,
        "maximum": maximum,
        "description": description
    })
}

impl Serialize for PartSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}
impl<'de> Deserialize<'de> for PartSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for PartSize {
    fn schema_name() -> Cow<'static, str> {
        "PartSize".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        integer_schema(Self::MIN, Self::MAX, "The size of every part but the last.")
    }
}

impl Serialize for PartNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(self.0)
    }
}
impl<'de> Deserialize<'de> for PartNumber {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u16::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for PartNumber {
    fn schema_name() -> Cow<'static, str> {
        "PartNumber".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        integer_schema(1, u64::from(Self::MAX), "A multipart upload's part number.")
    }
}

impl Serialize for ActiveUploadLimit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(self.0)
    }
}
impl<'de> Deserialize<'de> for ActiveUploadLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u16::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for ActiveUploadLimit {
    fn schema_name() -> Cow<'static, str> {
        "ActiveUploadLimit".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        integer_schema(
            1,
            10_000,
            "How many uploads a tenant may hold open at once.",
        )
    }
}

/// One artifact and the caller's level on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactEntry {
    pub artifact: ArtifactRecord,
    pub level: AccessLevel,
}

/// A page of `artifact://artifacts`, newest first, and the cursor of the next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactListing {
    pub artifacts: Vec<ArtifactEntry>,
    pub next_cursor: Option<super::ListCursor>,
}

/// A page of `artifact://uploads`, newest first, and the cursor of the next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadListing {
    pub uploads: Vec<UploadRecord>,
    pub next_cursor: Option<super::ListCursor>,
}

/// The grants on one artifact, at most 256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantListing {
    pub artifact_id: ArtifactId,
    pub grants: Vec<ArtifactGrant>,
}

/// A page of one artifact's share links, newest first, without their secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareLinkListing {
    pub artifact_id: ArtifactId,
    pub share_links: Vec<ShareLinkRecord>,
    pub next_cursor: Option<super::ListCursor>,
}

/// A page of access requests, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequestListing {
    pub access_requests: Vec<AccessRequestRecord>,
    pub next_cursor: Option<super::ListCursor>,
}
