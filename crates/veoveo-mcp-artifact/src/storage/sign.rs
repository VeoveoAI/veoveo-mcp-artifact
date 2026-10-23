//! Signed requests that move bytes. Each binds its method, object key and every header
//! the client sends, so object storage refuses a request that changes any of them.
use std::{collections::BTreeMap, time::Duration};

use aws_sdk_s3::presigning::{PresignedRequest, PresigningConfig};
use chrono::{DateTime, Utc};

use crate::contract::{
    ArtifactName, MediaType, ObjectSize, PartNumber, SignedTransfer, SignedUrl, TransferMethod,
};

use super::{ObjectKey, ObjectStorage, StorageError};

impl ObjectStorage {
    /// A create-only PutObject that binds the object's size and media type.
    pub async fn sign_put(
        &self,
        key: &ObjectKey,
        size: ObjectSize,
        media_type: &MediaType,
        now: DateTime<Utc>,
    ) -> Result<SignedTransfer, StorageError> {
        let size = signed_length(size)?;
        let request = self
            .signing
            .put_object()
            .bucket(&self.bucket)
            .key(key.as_str())
            .content_length(size)
            .content_type(media_type.as_str())
            .if_none_match("*")
            .presigned(presigning(super::UPLOAD_URL_LIFETIME)?)
            .await
            .map_err(|error| StorageError::call("presign PutObject", error))?;
        transfer(
            TransferMethod::Put,
            &request,
            now,
            super::UPLOAD_URL_LIFETIME,
        )
    }

    /// One part of a multipart upload, binding its declared length.
    pub async fn sign_part(
        &self,
        key: &ObjectKey,
        object_upload: &str,
        part: PartNumber,
        size: Option<ObjectSize>,
        now: DateTime<Utc>,
    ) -> Result<SignedTransfer, StorageError> {
        let request = self
            .signing
            .upload_part()
            .bucket(&self.bucket)
            .key(key.as_str())
            .upload_id(object_upload)
            .part_number(i32::from(part.get()))
            .set_content_length(size.map(signed_length).transpose()?)
            .presigned(presigning(super::UPLOAD_URL_LIFETIME)?)
            .await
            .map_err(|error| StorageError::call("presign UploadPart", error))?;
        transfer(
            TransferMethod::Put,
            &request,
            now,
            super::UPLOAD_URL_LIFETIME,
        )
    }

    /// A GetObject whose response names the artifact as an attachment with its media
    /// type, so a browser saves it instead of rendering it.
    pub async fn sign_get(
        &self,
        key: &ObjectKey,
        name: &ArtifactName,
        media_type: &MediaType,
        now: DateTime<Utc>,
    ) -> Result<SignedTransfer, StorageError> {
        let request = self
            .signing
            .get_object()
            .bucket(&self.bucket)
            .key(key.as_str())
            .response_content_disposition(attachment(name))
            .response_content_type(media_type.as_str())
            .presigned(presigning(super::DOWNLOAD_URL_LIFETIME)?)
            .await
            .map_err(|error| StorageError::call("presign GetObject", error))?;
        transfer(
            TransferMethod::Get,
            &request,
            now,
            super::DOWNLOAD_URL_LIFETIME,
        )
    }
}

fn presigning(lifetime: Duration) -> Result<PresigningConfig, StorageError> {
    PresigningConfig::expires_in(lifetime)
        .map_err(|_| StorageError::Config("a signed URL lives at most 7 days"))
}

fn signed_length(size: ObjectSize) -> Result<i64, StorageError> {
    i64::try_from(size.bytes()).map_err(|_| StorageError::Size)
}

fn transfer(
    method: TransferMethod,
    request: &PresignedRequest,
    now: DateTime<Utc>,
    lifetime: Duration,
) -> Result<SignedTransfer, StorageError> {
    let url = SignedUrl::parse(request.uri()).map_err(|_| StorageError::Missing {
        operation: "presign",
        field: "absolute URL",
    })?;
    let headers: BTreeMap<String, String> = request
        .headers()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
        .collect();
    Ok(SignedTransfer {
        method,
        url,
        headers,
        expires_at: now
            + chrono::Duration::from_std(lifetime).expect("a URL lifetime fits a duration"),
    })
}

/// An RFC 6266 attachment disposition: an ASCII `filename` for old clients and the exact
/// name as RFC 8187 `filename*`.
pub(crate) fn attachment(name: &ArtifactName) -> String {
    let fallback: String = name
        .as_str()
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() && character != '"' && character != '\\'
                || character == ' '
            {
                character
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for byte in name.as_str().bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ArtifactId;
    use crate::storage::StorageConfig;
    use url::Url;

    /// The installation serves objects under `/artifacts/` on its public host:
    /// path-style addressing puts the bucket first in the path, and SigV4 binds that
    /// host and path. Presigning is local, so no store is reached.
    #[tokio::test]
    async fn signed_requests_name_the_public_host_and_the_bucket_path() {
        let storage = ObjectStorage::new(&StorageConfig {
            internal_endpoint: Url::parse(
                "https://seaweedfs-s3.veoveo-system.svc.cluster.local:9000",
            )
            .unwrap(),
            signing_endpoint: Url::parse("https://veoveo.bioma.ai").unwrap(),
            region: "us-east-1".into(),
            bucket: "artifacts".into(),
            access_key: "access".into(),
            secret_key: "secret".into(),
            cors_origins: Vec::new(),
        })
        .unwrap();
        let tenant = veoveo_store::deterministic_tenant_id("acme").unwrap();
        let key = ObjectKey::for_artifact(tenant, ArtifactId::new());
        let media_type = MediaType::parse("application/pdf").unwrap();
        let now = Utc::now();
        let signed = [
            storage
                .sign_put(&key, ObjectSize::new(11).unwrap(), &media_type, now)
                .await
                .unwrap(),
            storage
                .sign_part(&key, "upload", PartNumber::new(1).unwrap(), None, now)
                .await
                .unwrap(),
            storage
                .sign_get(
                    &key,
                    &ArtifactName::parse("informe.pdf").unwrap(),
                    &media_type,
                    now,
                )
                .await
                .unwrap(),
        ];
        for transfer in &signed {
            let url = Url::parse(transfer.url.as_str()).unwrap();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("veoveo.bioma.ai"));
            assert_eq!(url.port(), None);
            assert_eq!(url.path(), format!("/artifacts/{}", key.as_str()));
            let signed_headers = url
                .query_pairs()
                .find(|(name, _)| name == "X-Amz-SignedHeaders")
                .map(|(_, value)| value.into_owned())
                .unwrap();
            assert!(
                signed_headers.split(';').any(|header| header == "host"),
                "{signed_headers}"
            );
        }
    }

    #[test]
    fn dispositions_carry_an_ascii_fallback_and_the_exact_name() {
        let name = ArtifactName::parse("informe \"final\" ñ.pdf").unwrap();
        assert_eq!(
            attachment(&name),
            "attachment; filename=\"informe _final_ _.pdf\"; \
             filename*=UTF-8''informe%20%22final%22%20%C3%B1.pdf"
        );
    }
}
