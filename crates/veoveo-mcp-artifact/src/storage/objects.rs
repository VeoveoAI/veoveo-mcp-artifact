//! The server's own calls: multipart control, object metadata, deletion and the
//! SHA-256 of a stored object.
use aws_sdk_s3::{
    error::ProvideErrorMetadata,
    types::{ChecksumMode, CompletedMultipartUpload, CompletedPart},
};
use sha2::{Digest as _, Sha256};
use veoveo_types::Sha256Digest;

use crate::contract::{MediaType, ObjectSize};

use super::{ObjectKey, ObjectStorage, StorageError, digest_from_checksum};

/// What object storage holds at a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub size: ObjectSize,
    /// The SHA-256 the store checked when a single PutObject declared one.
    pub sha256: Option<Sha256Digest>,
}

/// The store's codes for an upload or object that does not exist.
fn absent(code: Option<&str>) -> bool {
    matches!(code, Some("NoSuchUpload" | "NoSuchKey" | "NotFound"))
}

impl ObjectStorage {
    /// Start a multipart upload and return the store's identifier for it.
    pub async fn start_multipart(
        &self,
        key: &ObjectKey,
        media_type: &MediaType,
    ) -> Result<String, StorageError> {
        let started = self
            .internal
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(key.as_str())
            .content_type(media_type.as_str())
            .send()
            .await
            .map_err(|error| StorageError::call("CreateMultipartUpload", error))?;
        started
            .upload_id()
            .map(ToOwned::to_owned)
            .ok_or(StorageError::Missing {
                operation: "CreateMultipartUpload",
                field: "upload ID",
            })
    }

    /// Complete a multipart upload from the parts the store received, create-only. A
    /// repeated completion finds the upload gone and leaves the object as it is.
    pub async fn complete_multipart(
        &self,
        key: &ObjectKey,
        object_upload: &str,
    ) -> Result<(), StorageError> {
        let mut parts = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let page = match self
                .internal
                .list_parts()
                .bucket(&self.bucket)
                .key(key.as_str())
                .upload_id(object_upload)
                .set_part_number_marker(marker.clone())
                .send()
                .await
            {
                Ok(page) => page,
                Err(error) if absent(error.code()) => return Ok(()),
                Err(error) => return Err(StorageError::call("ListParts", error)),
            };
            for part in page.parts() {
                parts.push(
                    CompletedPart::builder()
                        .set_part_number(part.part_number())
                        .set_e_tag(part.e_tag().map(ToOwned::to_owned))
                        .set_checksum_sha256(part.checksum_sha256().map(ToOwned::to_owned))
                        .build(),
                );
            }
            if page.is_truncated() != Some(true) {
                break;
            }
            marker = page.next_part_number_marker().map(ToOwned::to_owned);
            if marker.is_none() {
                break;
            }
        }
        if parts.is_empty() {
            return Err(StorageError::NoParts);
        }
        match self
            .internal
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(key.as_str())
            .upload_id(object_upload)
            .if_none_match("*")
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if absent(error.code()) => Ok(()),
            Err(error) if error.code() == Some("PreconditionFailed") => {
                Err(StorageError::ObjectExists)
            }
            Err(error) => Err(StorageError::call("CompleteMultipartUpload", error)),
        }
    }

    /// Abort a multipart upload. An upload the store no longer holds is already gone.
    pub async fn abort_multipart(
        &self,
        key: &ObjectKey,
        object_upload: &str,
    ) -> Result<(), StorageError> {
        match self
            .internal
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(key.as_str())
            .upload_id(object_upload)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if absent(error.code()) => Ok(()),
            Err(error) => Err(StorageError::call("AbortMultipartUpload", error)),
        }
    }

    /// The object at `key`, or none when the store holds nothing there.
    pub async fn head(&self, key: &ObjectKey) -> Result<Option<StoredObject>, StorageError> {
        let head = match self
            .internal
            .head_object()
            .bucket(&self.bucket)
            .key(key.as_str())
            .checksum_mode(ChecksumMode::Enabled)
            .send()
            .await
        {
            Ok(head) => head,
            Err(error) if absent(error.code()) || not_found(&error) => return Ok(None),
            Err(error) => return Err(StorageError::call("HeadObject", error)),
        };
        let size = head
            .content_length()
            .and_then(|length| u64::try_from(length).ok())
            .ok_or(StorageError::Missing {
                operation: "HeadObject",
                field: "content length",
            })?;
        Ok(Some(StoredObject {
            size: ObjectSize::new(size).map_err(|_| StorageError::Size)?,
            sha256: head.checksum_sha256().and_then(digest_from_checksum),
        }))
    }

    /// Delete the object at `key`. Deleting an absent object succeeds.
    pub async fn delete(&self, key: &ObjectKey) -> Result<(), StorageError> {
        self.internal
            .delete_object()
            .bucket(&self.bucket)
            .key(key.as_str())
            .send()
            .await
            .map_err(|error| StorageError::call("DeleteObject", error))?;
        Ok(())
    }

    /// Stream the object at `key` once and return its SHA-256 and size.
    pub async fn sha256(&self, key: &ObjectKey) -> Result<(Sha256Digest, u64), StorageError> {
        let object = self
            .internal
            .get_object()
            .bucket(&self.bucket)
            .key(key.as_str())
            .send()
            .await
            .map_err(|error| StorageError::call("GetObject", error))?;
        let mut body = object.body;
        let mut hasher = Sha256::new();
        let mut size = 0_u64;
        while let Some(chunk) = body.try_next().await.map_err(|_| StorageError::Call {
            operation: "GetObject",
            code: "interrupted body".to_owned(),
        })? {
            size += chunk.len() as u64;
            hasher.update(&chunk);
        }
        let digest = Sha256Digest::parse(format!("sha256:{}", hex::encode(hasher.finalize())))
            .expect("a SHA-256 has the digest spelling");
        Ok((digest, size))
    }
}

/// HeadObject carries no body, so a missing object arrives as a bare 404.
fn not_found<R>(
    error: &aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::head_object::HeadObjectError, R>,
) -> bool {
    matches!(
        error.as_service_error(),
        Some(aws_sdk_s3::operation::head_object::HeadObjectError::NotFound(_))
    )
}
