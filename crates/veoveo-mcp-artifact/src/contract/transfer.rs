//! Signed object-storage requests that move bytes. A client sends exactly the method,
//! URL and headers it receives; the signature binds each of them.
use std::{borrow::Cow, collections::BTreeMap, fmt};

use chrono::{DateTime, Utc};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use veoveo_types::Vocabulary;

use super::{PartNumber, PartSize, ValueError};

/// The most bytes a signed URL holds.
pub const MAX_SIGNED_URL_BYTES: usize = 8192;

/// The HTTP method of a signed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
pub enum TransferMethod {
    #[vocabulary(rename = "PUT")]
    Put,
    #[vocabulary(rename = "GET")]
    Get,
}

/// An absolute `http` or `https` URL that carries a SigV4 query signature.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SignedUrl(String);

impl SignedUrl {
    pub fn parse(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        let absolute = value.starts_with("https://") || value.starts_with("http://");
        if !absolute || value.len() > MAX_SIGNED_URL_BYTES || value.chars().any(char::is_control) {
            return Err(ValueError::from_static(
                "a signed URL is an absolute http or https URL of at most 8192 bytes",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SignedUrl {
    type Error = ValueError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<SignedUrl> for String {
    fn from(value: SignedUrl) -> Self {
        value.0
    }
}

impl fmt::Display for SignedUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl JsonSchema for SignedUrl {
    fn schema_name() -> Cow<'static, str> {
        "SignedUrl".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "uri",
            "maxLength": MAX_SIGNED_URL_BYTES,
            "description": "An absolute URL carrying a SigV4 query signature."
        })
    }
}

/// One signed request. The headers are the ones the signature binds, and the client
/// sends each of them unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedTransfer {
    pub method: TransferMethod,
    pub url: SignedUrl,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub expires_at: DateTime<Utc>,
}

/// One signed part of a multipart upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedPart {
    pub part_number: PartNumber,
    pub transfer: SignedTransfer,
}

/// Where an upload's bytes go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "form", deny_unknown_fields)]
pub enum UploadTransfer {
    /// One signed PutObject.
    Single { put: SignedTransfer },
    /// A multipart upload. Every part but the last holds `part_size` bytes, and
    /// `sign_upload_parts` signs more parts or signs expired ones again.
    Multipart {
        #[serde(rename = "partSize")]
        part_size: PartSize,
        parts: Vec<SignedPart>,
    },
}
