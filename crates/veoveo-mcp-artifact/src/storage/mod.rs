//! Object storage: the server's one bucket on an S3-compatible store. The server signs
//! the requests that move bytes and makes its own calls for multipart control, object
//! metadata, deletion and checksum verification. Its credential never leaves it.
//!
//! SigV4 binds the Host header, so the server keeps two clients built from one
//! configuration: one signs with the address clients use, and one calls the store on the
//! installation network.
mod bucket;
mod objects;
mod sign;

use std::{fmt, time::Duration};

use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use base64::Engine as _;
use url::Url;
use veoveo_platform_store::TenantId as StoredTenant;
use veoveo_types::Sha256Digest;

use crate::contract::ArtifactId;

pub use objects::StoredObject;

/// How long a signed upload or part URL lives: 15 minutes.
pub const UPLOAD_URL_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// How long a signed download URL lives: 5 minutes.
pub const DOWNLOAD_URL_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// How long the store keeps a multipart upload nobody completed: 1 day.
pub const ABANDONED_UPLOAD_DAYS: i32 = 1;

/// Where the store is and how the server reaches it.
#[derive(Clone)]
pub struct StorageConfig {
    /// The address the server calls on the installation network.
    pub internal_endpoint: Url,
    /// The address clients send signed requests to. SigV4 binds its host.
    pub signing_endpoint: Url,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    /// Browser origins allowed to send signed requests.
    pub cors_origins: Vec<String>,
}

impl fmt::Debug for StorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StorageConfig")
            .field("internal_endpoint", &self.internal_endpoint.as_str())
            .field("signing_endpoint", &self.signing_endpoint.as_str())
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("cors_origins", &self.cors_origins)
            .finish()
    }
}

/// Why an object-storage call failed. Messages name the operation and the store's error
/// code, never a URL, key or credential.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("invalid object storage configuration: {0}")]
    Config(&'static str),
    #[error("object storage {operation} failed: {code}")]
    Call {
        operation: &'static str,
        code: String,
    },
    #[error("object storage {operation} returned no {field}")]
    Missing {
        operation: &'static str,
        field: &'static str,
    },
    #[error("the object already exists")]
    ObjectExists,
    #[error("the multipart upload holds no parts")]
    NoParts,
    #[error("object storage reported a size outside the supported range")]
    Size,
}

impl StorageError {
    pub(crate) fn call<E>(
        operation: &'static str,
        error: aws_sdk_s3::error::SdkError<E, aws_sdk_s3::config::http::HttpResponse>,
    ) -> Self
    where
        E: aws_sdk_s3::error::ProvideErrorMetadata,
    {
        let code = match &error {
            aws_sdk_s3::error::SdkError::ServiceError(service) => match service.err().code() {
                Some(code) => code.to_owned(),
                None => format!("HTTP {}", service.raw().status().as_u16()),
            },
            aws_sdk_s3::error::SdkError::TimeoutError(_) => "timeout".to_owned(),
            aws_sdk_s3::error::SdkError::DispatchFailure(_) => "dispatch failure".to_owned(),
            aws_sdk_s3::error::SdkError::ResponseError(_) => "unreadable response".to_owned(),
            aws_sdk_s3::error::SdkError::ConstructionFailure(_) => "invalid request".to_owned(),
            _ => "unknown error".to_owned(),
        };
        Self::Call { operation, code }
    }
}

/// The key of an artifact's object: `tenants/{tenant}/artifacts/{artifact}`, with the
/// tenant's stored identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    pub fn for_artifact(tenant: StoredTenant, artifact: ArtifactId) -> Self {
        Self(format!("tenants/{}/artifacts/{artifact}", tenant.as_uuid()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The server's bucket and its two clients.
#[derive(Clone)]
pub struct ObjectStorage {
    internal: aws_sdk_s3::Client,
    signing: aws_sdk_s3::Client,
    bucket: String,
    cors_origins: Vec<String>,
}

impl fmt::Debug for ObjectStorage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObjectStorage")
            .field("bucket", &self.bucket)
            .finish_non_exhaustive()
    }
}

impl ObjectStorage {
    pub fn new(config: &StorageConfig) -> Result<Self, StorageError> {
        if config.bucket.is_empty() || config.region.is_empty() {
            return Err(StorageError::Config("the bucket and region are required"));
        }
        if config.access_key.is_empty() || config.secret_key.is_empty() {
            return Err(StorageError::Config("the storage credential is required"));
        }
        let http = aws_smithy_http_client::Builder::new()
            .tls_provider(aws_smithy_http_client::tls::Provider::Rustls(
                aws_smithy_http_client::tls::rustls_provider::CryptoMode::Ring,
            ))
            .build_https();
        let credentials = Credentials::new(
            config.access_key.clone(),
            config.secret_key.clone(),
            None,
            None,
            "veoveo-mcp-artifact",
        );
        let client = |endpoint: &Url| {
            aws_sdk_s3::Client::from_conf(
                aws_sdk_s3::Config::builder()
                    .behavior_version(BehaviorVersion::latest())
                    .region(Region::new(config.region.clone()))
                    .endpoint_url(endpoint.as_str().trim_end_matches('/'))
                    .credentials_provider(credentials.clone())
                    .force_path_style(true)
                    .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
                    .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
                    .http_client(http.clone())
                    .build(),
            )
        };
        Ok(Self {
            internal: client(&config.internal_endpoint),
            signing: client(&config.signing_endpoint),
            bucket: config.bucket.clone(),
            cors_origins: config.cors_origins.clone(),
        })
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }
}

/// The base64 of a digest's 32 bytes, as `x-amz-checksum-sha256` carries it.
pub(crate) fn checksum_header(digest: &Sha256Digest) -> String {
    let hex = digest
        .as_str()
        .strip_prefix("sha256:")
        .expect("an admitted digest carries its algorithm");
    let bytes = hex::decode(hex).expect("an admitted digest is hexadecimal");
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The digest an `x-amz-checksum-sha256` value names, when it is one.
pub(crate) fn digest_from_checksum(header: &str) -> Option<Sha256Digest> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(header)
        .ok()
        .filter(|bytes| bytes.len() == 32)?;
    Sha256Digest::parse(format!("sha256:{}", hex::encode(bytes))).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_headers_round_trip_digests() {
        let digest = Sha256Digest::parse(
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        )
        .unwrap();
        let header = checksum_header(&digest);
        assert_eq!(header, "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=");
        assert_eq!(digest_from_checksum(&header), Some(digest));
        assert_eq!(digest_from_checksum("not base64"), None);
    }

    #[test]
    fn keys_name_the_tenant_and_artifact() {
        let tenant = veoveo_platform_store::deterministic_tenant_id("acme").unwrap();
        let artifact = ArtifactId::new();
        let key = ObjectKey::for_artifact(tenant, artifact);
        assert_eq!(
            key.as_str(),
            format!("tenants/{}/artifacts/{artifact}", tenant.as_uuid())
        );
    }

    #[test]
    fn configuration_never_prints_its_credential() {
        let config = StorageConfig {
            internal_endpoint: Url::parse("http://127.0.0.1:9000").unwrap(),
            signing_endpoint: Url::parse("http://127.0.0.1:9000").unwrap(),
            region: "us-east-1".into(),
            bucket: "artifacts".into(),
            access_key: "access-visible".into(),
            secret_key: "secret-visible".into(),
            cors_origins: Vec::new(),
        };
        let printed = format!("{config:?}");
        assert!(!printed.contains("access-visible"));
        assert!(!printed.contains("secret-visible"));
    }
}
