//! Uploads: reserving an artifact, signing where its bytes go, completing it from what
//! object storage holds, and aborting it.
use chrono::{DateTime, Utc};
use veoveo_task_runtime::TaskOwner;
use veoveo_types::Checked;

use crate::{
    contract::{
        AbortUploadResult, ArtifactRecord, ArtifactState, CreateUploadRequest, CreateUploadResult,
        MAX_PARTS_PER_SIGNING, ObjectSize, PartNumber, PartRequest, PartSize,
        SignUploadPartsRequest, SignUploadPartsResult, SignedPart, UploadForm, UploadId,
        UploadRecord, UploadState, UploadTransfer,
    },
    storage::ObjectKey,
    store::{ArtifactStoreError, Caller, CompletedObject},
};

use super::{ArtifactError, ArtifactService, INLINE_DIGEST_BYTES};

impl ArtifactService {
    /// Reserve an artifact and sign where its bytes go. A repeated idempotency key signs
    /// the open upload again, or returns the ended one without a transfer.
    pub async fn create_upload(
        &self,
        caller: &Caller,
        request: &Checked<CreateUploadRequest>,
        now: DateTime<Utc>,
    ) -> Result<CreateUploadResult, ArtifactError> {
        let created = self.store.create_upload(caller, request, now).await?;
        if created.upload.state != UploadState::Open {
            return Ok(CreateUploadResult {
                upload: created.upload,
                artifact: created.artifact,
                transfer: None,
            });
        }
        let key = ObjectKey::for_artifact(caller.tenant(), created.artifact.artifact_id);
        let transfer = match created.upload.form {
            UploadForm::Single => {
                let size = created
                    .upload
                    .declared_size
                    .ok_or(ArtifactStoreError::Corrupt("single upload without a size"))?;
                let put = self
                    .storage
                    .sign_put(
                        &key,
                        size,
                        &created.artifact.media_type,
                        created.upload.declared_sha256.as_ref(),
                        now,
                    )
                    .await?;
                UploadTransfer::Single { put }
            }
            UploadForm::Multipart => {
                let object_upload = match created.object_upload {
                    Some(object_upload) => object_upload,
                    None => {
                        self.start_object_upload(caller, &created.upload, &key, &created.artifact)
                            .await?
                    }
                };
                let part_size = created.upload.part_size.ok_or(ArtifactStoreError::Corrupt(
                    "multipart upload without a part size",
                ))?;
                let parts = initial_parts(&created.upload, part_size)?;
                let mut signed = Vec::with_capacity(parts.len());
                for (number, size) in parts {
                    let transfer = self
                        .storage
                        .sign_part(&key, &object_upload, number, size, None, now)
                        .await?;
                    signed.push(SignedPart {
                        part_number: number,
                        transfer,
                    });
                }
                UploadTransfer::Multipart {
                    part_size,
                    parts: signed,
                }
            }
        };
        Ok(CreateUploadResult {
            upload: created.upload,
            artifact: created.artifact,
            transfer: Some(transfer),
        })
    }

    /// Start the multipart upload and record it. When a concurrent call recorded one
    /// first, abort this one and use the recorded upload.
    async fn start_object_upload(
        &self,
        caller: &Caller,
        upload: &UploadRecord,
        key: &ObjectKey,
        artifact: &ArtifactRecord,
    ) -> Result<String, ArtifactError> {
        let started = self
            .storage
            .start_multipart(key, &artifact.media_type)
            .await?;
        match self
            .store
            .record_object_upload(caller, upload.upload_id, &started)
            .await
        {
            Ok(()) => Ok(started),
            Err(ArtifactStoreError::UploadNotFound) => {
                self.storage.abort_multipart(key, &started).await?;
                let (_, recorded) = self.store.upload(caller, upload.upload_id).await?;
                recorded.ok_or_else(|| ArtifactStoreError::UploadNotFound.into())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Sign the requested parts of the caller's open multipart upload.
    pub async fn sign_upload_parts(
        &self,
        caller: &Caller,
        request: &Checked<SignUploadPartsRequest>,
        now: DateTime<Utc>,
    ) -> Result<SignUploadPartsResult, ArtifactError> {
        let (upload, object_upload) = self.store.upload(caller, request.upload_id).await?;
        if upload.state != UploadState::Open {
            return Err(ArtifactStoreError::UploadNotOpen {
                state: upload.state,
            }
            .into());
        }
        let (Some(object_upload), Some(part_size)) = (object_upload, upload.part_size) else {
            return Err(ArtifactError::NotMultipart);
        };
        let key = ObjectKey::for_artifact(caller.tenant(), upload.artifact_id);
        let mut parts = Vec::with_capacity(request.parts.len());
        for PartRequest {
            part_number,
            size,
            sha256,
        } in &request.parts
        {
            if let Some(size) = size
                && size.bytes() > part_size.bytes()
            {
                return Err(ArtifactError::PartTooLarge {
                    declared: size.bytes(),
                    part_size: part_size.bytes(),
                });
            }
            let transfer = self
                .storage
                .sign_part(
                    &key,
                    &object_upload,
                    *part_number,
                    *size,
                    sha256.as_ref(),
                    now,
                )
                .await?;
            parts.push(SignedPart {
                part_number: *part_number,
                transfer,
            });
        }
        Ok(SignUploadPartsResult { parts })
    }

    /// Complete the caller's upload from what object storage holds. A small object whose
    /// SHA-256 the store did not check is hashed here; a larger one verifies in a Task
    /// that `owner` owns. Completing an upload that already completed returns its
    /// artifact and makes sure its verification runs.
    pub async fn complete_upload(
        &self,
        caller: &Caller,
        owner: &TaskOwner,
        upload_id: UploadId,
        now: DateTime<Utc>,
    ) -> Result<ArtifactRecord, ArtifactError> {
        let (upload, object_upload) = self.store.upload(caller, upload_id).await?;
        let artifact_id = upload.artifact_id;
        let key = ObjectKey::for_artifact(caller.tenant(), artifact_id);
        let artifact = match upload.state {
            UploadState::Open => {
                if let Some(object_upload) = &object_upload {
                    self.storage.complete_multipart(&key, object_upload).await?;
                }
                let stored = self
                    .storage
                    .head(&key)
                    .await?
                    .ok_or(ArtifactError::NoBytes)?;
                let checked_sha256 = match (upload.form, &upload.declared_sha256, &stored.sha256) {
                    (UploadForm::Single, Some(declared), Some(checked)) if declared == checked => {
                        Some(declared.clone())
                    }
                    _ => None,
                };
                let object = CompletedObject {
                    size: stored.size,
                    checked_sha256,
                };
                match self
                    .store
                    .complete_upload(caller, upload_id, &object, now)
                    .await
                {
                    Ok(artifact) => artifact,
                    Err(error @ ArtifactStoreError::SizeMismatch { .. }) => {
                        self.discard_object(caller, artifact_id).await;
                        return Err(error.into());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            UploadState::Completed => self.artifact(caller, artifact_id, now).await?,
            state @ (UploadState::Aborted | UploadState::Expired) => {
                return Err(ArtifactStoreError::UploadNotOpen { state }.into());
            }
        };
        if artifact.state != ArtifactState::Verifying {
            return Ok(artifact);
        }
        let size = artifact.size.map_or(0, ObjectSize::bytes);
        if size <= INLINE_DIGEST_BYTES {
            let (computed, _) = self.storage.sha256(&key).await?;
            let state = self
                .store
                .record_digest(
                    caller,
                    artifact_id,
                    upload.declared_sha256.as_ref(),
                    &computed,
                    now,
                )
                .await?;
            if state == ArtifactState::Failed {
                self.discard_object(caller, artifact_id).await;
                return Err(ArtifactError::DigestMismatch);
            }
            return self.artifact(caller, artifact_id, now).await;
        }
        self.start_verification(owner, artifact_id, upload.declared_sha256)
            .await?;
        Ok(artifact)
    }

    /// Abort the caller's open upload, its multipart upload and any object that arrived.
    pub async fn abort_upload(
        &self,
        caller: &Caller,
        upload_id: UploadId,
        now: DateTime<Utc>,
    ) -> Result<AbortUploadResult, ArtifactError> {
        let (upload, object_upload) = self.store.abort_upload(caller, upload_id, now).await?;
        let key = ObjectKey::for_artifact(caller.tenant(), upload.artifact_id);
        if let Some(object_upload) = object_upload {
            self.storage.abort_multipart(&key, &object_upload).await?;
        }
        self.discard_object(caller, upload.artifact_id).await;
        Ok(AbortUploadResult { upload })
    }
}

/// The parts `create_upload` signs at once: every part of a known size, each bound to
/// its length, or the first parts of an unknown size, unbound. At most 100.
fn initial_parts(
    upload: &UploadRecord,
    part_size: PartSize,
) -> Result<Vec<(PartNumber, Option<ObjectSize>)>, ArtifactError> {
    let bound = upload.declared_size.unwrap_or(upload.reserved).bytes();
    let count = bound
        .div_ceil(part_size.bytes())
        .clamp(1, MAX_PARTS_PER_SIGNING as u64);
    (1..=count)
        .map(|index| {
            let number = PartNumber::new(u16::try_from(index).expect("at most 100 parts"))
                .map_err(|_| ArtifactStoreError::Corrupt("part number"))?;
            let size = upload
                .declared_size
                .map(|declared| {
                    let start = (index - 1) * part_size.bytes();
                    ObjectSize::new(part_size.bytes().min(declared.bytes() - start))
                        .map_err(|_| ArtifactStoreError::Corrupt("part length"))
                })
                .transpose()?;
            Ok((number, size))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::contract::{ArtifactId, UploadState};

    fn upload(declared: Option<u64>, reserved: u64, part: u64) -> (UploadRecord, PartSize) {
        let part_size = PartSize::new(part).unwrap();
        (
            UploadRecord {
                upload_id: UploadId::new(),
                artifact_id: ArtifactId::new(),
                form: UploadForm::Multipart,
                state: UploadState::Open,
                declared_size: declared.map(|bytes| ObjectSize::new(bytes).unwrap()),
                declared_sha256: None,
                reserved: ObjectSize::new(reserved).unwrap(),
                part_size: Some(part_size),
                created_at: Utc::now(),
                expires_at: Utc::now(),
                ended_at: None,
            },
            part_size,
        )
    }

    #[test]
    fn known_sizes_bind_every_part_and_a_shorter_last_part() {
        let mib = 1024 * 1024;
        let (record, part) = upload(Some(40 * mib + 5), 40 * mib + 5, 16 * mib);
        let parts = initial_parts(&record, part).unwrap();
        let sizes: Vec<u64> = parts
            .iter()
            .map(|(_, size)| size.unwrap().bytes())
            .collect();
        assert_eq!(sizes, vec![16 * mib, 16 * mib, 8 * mib + 5]);
        assert_eq!(parts[2].0.get(), 3);
    }

    #[test]
    fn unknown_sizes_sign_at_most_one_hundred_unbound_parts() {
        let mib = 1024 * 1024;
        let (record, part) = upload(None, 5 * 1024 * 1024 * mib, 525 * mib);
        let parts = initial_parts(&record, part).unwrap();
        assert_eq!(parts.len(), 100);
        assert!(parts.iter().all(|(_, size)| size.is_none()));
    }
}
