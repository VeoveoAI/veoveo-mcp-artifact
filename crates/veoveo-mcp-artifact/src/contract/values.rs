//! Checked values that describe an artifact: its name, media type, size, labels,
//! retention, caller metadata and the key that makes a create idempotent. Each value
//! is admitted once, on entry, and stays typed until the database driver.
use std::{borrow::Cow, collections::BTreeSet, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use veoveo_types::DataLabelId;

/// The largest object S3 stores: 5 TiB.
pub const MAX_OBJECT_BYTES: u64 = 5 * 1024 * 1024 * 1024 * 1024;
/// The most bytes an artifact name holds.
pub const MAX_NAME_BYTES: usize = 255;
/// The most labels one artifact carries.
pub const MAX_LABELS: usize = 64;
/// The most bytes an artifact's metadata object holds, serialized as JSON.
pub const MAX_METADATA_BYTES: usize = 16 * 1024;
/// The longest retention an artifact may request, in days.
pub const MAX_RETENTION_DAYS: u16 = 36_500;
/// The most bytes an idempotency key holds.
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

/// A value that failed admission. The message names the rule and never repeats the
/// rejected value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValueError(Cow<'static, str>);

impl ValueError {
    fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self(message.into())
    }

    pub(crate) fn from_static(message: &'static str) -> Self {
        Self(Cow::Borrowed(message))
    }
}

fn printable(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

/// An artifact's file name: 1 to 255 bytes without control characters, `/` or `\`,
/// and neither `.` nor `..`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ArtifactName(String);

impl ArtifactName {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        admit_name_text(&value)?;
        if value == "." || value == ".." {
            return Err(ValueError::new("an artifact name is not `.` or `..`"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn admit_name_text(value: &str) -> Result<(), ValueError> {
    if value.is_empty() || value.len() > MAX_NAME_BYTES {
        return Err(ValueError::new("an artifact name holds 1 to 255 bytes"));
    }
    if !printable(value) || value.contains(['/', '\\']) {
        return Err(ValueError::new(
            "an artifact name holds no control characters, `/` or `\\`",
        ));
    }
    Ok(())
}

/// The start of artifact names a listing matches: 1 to 255 bytes under the name rules.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NamePrefix(String);

impl NamePrefix {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        admit_name_text(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A bare RFC 6838 media type, `type/subtype` in lowercase without parameters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MediaType(String);

impl MediaType {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        let Some((kind, subtype)) = value.split_once('/') else {
            return Err(ValueError::new("a media type is `type/subtype`"));
        };
        if !restricted_name(kind) || !restricted_name(subtype) {
            return Err(ValueError::new(
                "a media type is a lowercase RFC 6838 `type/subtype` without parameters",
            ));
        }
        Ok(Self(value))
    }

    /// The media type of an HTTP `Content-Type` value: parameters dropped and the type
    /// lowercased, as a provider's response header reaches an upload.
    pub fn from_content_type(header: &str) -> Result<Self, ValueError> {
        let bare = header.split(';').next().unwrap_or_default().trim();
        Self::parse(bare.to_ascii_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// RFC 6838 `restricted-name` in lowercase: a letter or digit, then up to 126 of the
/// listed characters.
fn restricted_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 127
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'-' | b'^' | b'_' | b'.' | b'+'
                )
        })
}

/// A caller's key that makes `create_upload` idempotent: 1 to 256 bytes without
/// control characters. It is unique per tenant and acting identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_IDEMPOTENCY_KEY_BYTES || !printable(&value) {
            return Err(ValueError::new(
                "an idempotency key holds 1 to 256 bytes without control characters",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An object's size in bytes, from zero to [`MAX_OBJECT_BYTES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectSize(u64);

impl ObjectSize {
    pub fn new(bytes: u64) -> Result<Self, ValueError> {
        if bytes > MAX_OBJECT_BYTES {
            return Err(ValueError::new("an object holds at most 5 TiB"));
        }
        Ok(Self(bytes))
    }

    pub fn bytes(self) -> u64 {
        self.0
    }
}

/// How long an artifact stays after it becomes available: 1 to 36,500 days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RetentionDays(u16);

impl RetentionDays {
    pub fn new(days: u16) -> Result<Self, ValueError> {
        if days == 0 || days > MAX_RETENTION_DAYS {
            return Err(ValueError::new("retention lasts 1 to 36500 days"));
        }
        Ok(Self(days))
    }

    pub fn days(self) -> u16 {
        self.0
    }
}

/// The data labels an artifact carries, at most [`MAX_LABELS`]. A caller reads the
/// artifact only when it holds every label.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ArtifactLabels(BTreeSet<DataLabelId>);

impl ArtifactLabels {
    pub fn new(labels: BTreeSet<DataLabelId>) -> Result<Self, ValueError> {
        if labels.len() > MAX_LABELS {
            return Err(ValueError::new("an artifact carries at most 64 labels"));
        }
        Ok(Self(labels))
    }

    pub fn iter(&self) -> impl Iterator<Item = &DataLabelId> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// True when `clearance` holds every label.
    pub fn within(&self, clearance: &BTreeSet<DataLabelId>) -> bool {
        self.0.is_subset(clearance)
    }

    /// These labels together with `more`, refused when the union exceeds the limit.
    pub fn union(&self, more: impl IntoIterator<Item = DataLabelId>) -> Result<Self, ValueError> {
        let mut labels = self.0.clone();
        labels.extend(more);
        Self::new(labels)
    }

    pub fn as_set(&self) -> &BTreeSet<DataLabelId> {
        &self.0
    }
}

/// Caller-supplied metadata: a JSON object of at most [`MAX_METADATA_BYTES`] bytes.
/// Artifact stores it and never interprets it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactMetadataMap(serde_json::Map<String, serde_json::Value>);

impl ArtifactMetadataMap {
    pub fn new(object: serde_json::Map<String, serde_json::Value>) -> Result<Self, ValueError> {
        let bytes = serde_json::to_vec(&object).expect("a JSON object serializes");
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(ValueError::new(
                "artifact metadata holds at most 16384 bytes of JSON",
            ));
        }
        Ok(Self(object))
    }

    pub fn as_object(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.0
    }
}

impl TryFrom<String> for ArtifactName {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}
impl From<ArtifactName> for String {
    fn from(value: ArtifactName) -> Self {
        value.0
    }
}
impl fmt::Display for ArtifactName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl JsonSchema for ArtifactName {
    fn schema_name() -> Cow<'static, str> {
        "ArtifactName".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "minLength": 1, "maxLength": 255, "description": "A file name without control characters, `/` or `\\`, and neither `.` nor `..`." })
    }
}

impl TryFrom<String> for NamePrefix {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}
impl From<NamePrefix> for String {
    fn from(value: NamePrefix) -> Self {
        value.0
    }
}
impl fmt::Display for NamePrefix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl JsonSchema for NamePrefix {
    fn schema_name() -> Cow<'static, str> {
        "NamePrefix".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "minLength": 1, "maxLength": 255, "description": "The start of artifact names, under the artifact name rules." })
    }
}

impl TryFrom<String> for MediaType {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}
impl From<MediaType> for String {
    fn from(value: MediaType) -> Self {
        value.0
    }
}
impl fmt::Display for MediaType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl JsonSchema for MediaType {
    fn schema_name() -> Cow<'static, str> {
        "MediaType".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "pattern": "^[a-z0-9][a-z0-9!#$&^_.+-]{0,126}/[a-z0-9][a-z0-9!#$&^_.+-]{0,126}$", "description": "A bare RFC 6838 media type in lowercase, without parameters." })
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}
impl From<IdempotencyKey> for String {
    fn from(value: IdempotencyKey) -> Self {
        value.0
    }
}
impl fmt::Display for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl JsonSchema for IdempotencyKey {
    fn schema_name() -> Cow<'static, str> {
        "IdempotencyKey".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "minLength": 1, "maxLength": 256, "description": "A key that makes a create idempotent for its tenant and caller." })
    }
}

impl Serialize for ObjectSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}
impl<'de> Deserialize<'de> for ObjectSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for ObjectSize {
    fn schema_name() -> Cow<'static, str> {
        "ObjectSize".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": 0,
            "maximum": MAX_OBJECT_BYTES,
            "description": "An object's size in bytes."
        })
    }
}

impl Serialize for RetentionDays {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(self.0)
    }
}
impl<'de> Deserialize<'de> for RetentionDays {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u16::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for RetentionDays {
    fn schema_name() -> Cow<'static, str> {
        "RetentionDays".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": 1,
            "maximum": MAX_RETENTION_DAYS,
            "description": "Days an artifact stays after it becomes available."
        })
    }
}

impl Serialize for ArtifactLabels {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ArtifactLabels {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(BTreeSet::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for ArtifactLabels {
    fn schema_name() -> Cow<'static, str> {
        "ArtifactLabels".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let item = generator.subschema_for::<DataLabelId>();
        json_schema!({
            "type": "array",
            "items": item,
            "uniqueItems": true,
            "maxItems": MAX_LABELS,
            "description": "Data labels; a caller reads the artifact only when it holds every one."
        })
    }
}

impl Serialize for ArtifactMetadataMap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ArtifactMetadataMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(serde_json::Map::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for ArtifactMetadataMap {
    fn schema_name() -> Cow<'static, str> {
        "ArtifactMetadataMap".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "description": "Caller metadata, at most 16384 bytes of JSON, stored without interpretation."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_refuse_paths_controls_and_dot_names() {
        assert!(ArtifactName::parse("render 01.mp4").is_ok());
        for bad in ["", ".", "..", "a/b", "a\\b", "a\nb", &"x".repeat(256)] {
            assert!(ArtifactName::parse(bad).is_err(), "{bad:?}");
        }
        assert!(NamePrefix::parse(".").is_ok());
    }

    #[test]
    fn media_types_are_bare_and_lowercase() {
        assert_eq!(MediaType::parse("video/mp4").unwrap().as_str(), "video/mp4");
        for bad in [
            "video",
            "Video/MP4",
            "text/plain; charset=utf-8",
            "/mp4",
            "a/",
        ] {
            assert!(MediaType::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            MediaType::from_content_type("Text/Plain; charset=UTF-8")
                .unwrap()
                .as_str(),
            "text/plain"
        );
    }

    #[test]
    fn quantities_hold_their_bounds() {
        assert!(ObjectSize::new(0).is_ok());
        assert!(ObjectSize::new(MAX_OBJECT_BYTES).is_ok());
        assert!(ObjectSize::new(MAX_OBJECT_BYTES + 1).is_err());
        assert!(RetentionDays::new(0).is_err());
        assert!(RetentionDays::new(MAX_RETENTION_DAYS + 1).is_err());
        assert!(IdempotencyKey::parse("media:task:output:0").is_ok());
        assert!(IdempotencyKey::parse("").is_err());
    }

    #[test]
    fn labels_and_metadata_hold_their_limits() {
        let many: BTreeSet<_> = (0..65)
            .map(|index| DataLabelId::parse(format!("label-{index}")).unwrap())
            .collect();
        assert!(ArtifactLabels::new(many).is_err());
        let mut object = serde_json::Map::new();
        object.insert("blob".into(), "x".repeat(MAX_METADATA_BYTES).into());
        assert!(ArtifactMetadataMap::new(object).is_err());
        let decoded: Result<ArtifactMetadataMap, _> = serde_json::from_str("[1]");
        assert!(decoded.is_err());
    }
}
