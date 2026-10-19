//! Uploads: reserving an artifact and its bytes, completing, aborting and listing.
use chrono::{DateTime, Utc};
use sqlx::types::Json;
use veoveo_store::{PrincipalId as StoredPrincipal, TenantId as StoredTenant};
use veoveo_types::{
    Checked, CursorToken, Page, PageSize, Pager, Sha256Digest, WorkContextMembershipLevel,
};

use crate::contract::{
    ArtifactId, ArtifactMetadataMap, ArtifactProvenance, ArtifactRecord, ArtifactState,
    CreateUploadRequest, ObjectSize, PrincipalKeyset, ReleaseState, UploadFilters, UploadForm,
    UploadId, UploadLimits, UploadRecord, UploadState, Uploads,
};

use crate::contract::{
    ArtifactAuditReason, ArtifactVerified, UploadAborted, UploadCompleted, UploadCreated,
};

use super::{
    ArtifactStore, ArtifactStoreError, Caller, UPLOAD_LIFETIME, UploadPlan,
    audit::{Decided, artifact_target, upload_target},
    limits::LimitsRow,
    rows::{ArtifactRow, UploadRow, size_column},
};

/// A reserved upload and its artifact. `repeated` is true when an earlier call with the
/// same idempotency key created them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedUpload {
    pub upload: UploadRecord,
    pub artifact: ArtifactRecord,
    /// The object store's identifier of a multipart upload, once recorded.
    pub object_upload: Option<String>,
    pub repeated: bool,
}

/// What object storage reports about a completed object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedObject {
    pub size: ObjectSize,
    /// The SHA-256 object storage checked while it wrote, when the upload declared one
    /// and took a single PutObject.
    pub checked_sha256: Option<Sha256Digest>,
}

impl ArtifactStore {
    /// Reserve an artifact and its bytes for the caller in its selected Work Context. A
    /// repeated idempotency key returns the first call's upload. The tenant's usage row
    /// stays locked from the limit checks to the commit.
    pub async fn create_upload(
        &self,
        caller: &Caller,
        request: &Checked<CreateUploadRequest>,
        now: DateTime<Utc>,
    ) -> Result<CreatedUpload, ArtifactStoreError> {
        if !caller
            .membership()
            .allows(WorkContextMembershipLevel::Contributor)
        {
            return Err(ArtifactStoreError::WorkContextReadOnly);
        }
        let labels = request
            .labels
            .union(caller.output_labels().iter().cloned())
            .map_err(|_| ArtifactStoreError::LabelOutsideClearance)?;
        if !labels.within(caller.labels()) {
            return Err(ArtifactStoreError::LabelOutsideClearance);
        }
        let tenant = caller.tenant();
        let acting = caller.acting_identity();
        let mut tx = self.pool().begin().await?;
        sqlx::query_file!("src/store/uploads/ensure_usage.sql", tenant as StoredTenant)
            .execute(&mut *tx)
            .await?;
        let usage = sqlx::query_file_as!(
            UsageRow,
            "src/store/uploads/lock_usage.sql",
            tenant as StoredTenant
        )
        .fetch_one(&mut *tx)
        .await?;

        if let Some(key) = &request.idempotency_key {
            let existing = sqlx::query_file_as!(
                UploadRow,
                "src/store/uploads/idempotent_upload.sql",
                tenant as StoredTenant,
                acting,
                key.as_str(),
            )
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(upload) = existing {
                let artifact = self
                    .read_artifact(&mut tx, caller, upload.artifact_id, now)
                    .await?
                    .ok_or(ArtifactStoreError::ArtifactNotFound)?;
                tx.commit().await?;
                return Ok(CreatedUpload {
                    upload: upload.record()?,
                    artifact: artifact.record,
                    object_upload: upload.object_upload,
                    repeated: true,
                });
            }
        }

        let limits = usage.limits()?;
        if !limits.allowed_media_types.allows(&request.media_type) {
            return Err(ArtifactStoreError::MediaTypeNotAllowed(
                request.media_type.clone(),
            ));
        }
        if usage.open_uploads >= i32::from(limits.active_uploads.get()) {
            return Err(ArtifactStoreError::TooManyOpenUploads {
                limit: limits.active_uploads.get(),
            });
        }
        let plan = UploadPlan::new(request, &limits)?;
        if let Some(quota) = limits.quota_bytes {
            let held = u64::try_from(usage.stored_bytes + usage.reserved_bytes).unwrap_or(0);
            let available = quota.saturating_sub(held);
            if plan.reserved.bytes() > available {
                return Err(ArtifactStoreError::QuotaExceeded {
                    needed: plan.reserved.bytes(),
                    available,
                });
            }
        }
        if !request.derived_from.is_empty() {
            let sources: Vec<uuid::Uuid> = request
                .derived_from
                .iter()
                .map(|source| source.as_uuid())
                .collect();
            let facts = caller.facts();
            let visible = sqlx::query_file_scalar!(
                "src/store/artifacts/visible_sources.sql",
                facts.tenant as StoredTenant,
                facts.principal as StoredPrincipal,
                facts.principal_key,
                &facts.groups,
                &facts.group_levels,
                facts.work_context,
                facts.membership,
                &facts.labels,
                now,
                &sources,
            )
            .fetch_one(&mut *tx)
            .await?;
            if usize::try_from(visible).ok() != Some(sources.len()) {
                return Err(ArtifactStoreError::UnknownSource);
            }
        }

        let artifact_id = ArtifactId::new();
        let provenance = ArtifactProvenance {
            caller: caller.principal_key().clone(),
            acting_server: caller.acting_server().cloned(),
            task: caller.acting_task().cloned(),
            derived_from: request.derived_from.clone(),
        };
        let label_text: Vec<String> = labels.iter().map(ToString::to_string).collect();
        let artifact = sqlx::query_file_as!(
            ArtifactRow,
            "src/store/uploads/insert_artifact.sql",
            tenant as StoredTenant,
            artifact_id as ArtifactId,
            caller.principal() as StoredPrincipal,
            caller.principal_key().as_str(),
            caller.work_context().as_str(),
            request.name.as_str(),
            request.media_type.as_str(),
            &label_text,
            Json(&provenance) as Json<&ArtifactProvenance>,
            Json(&request.metadata) as Json<&ArtifactMetadataMap>,
            request.retention_days.map(|days| i32::from(days.days())),
            now,
        )
        .fetch_one(&mut *tx)
        .await?;
        let upload = sqlx::query_file_as!(
            UploadRow,
            "src/store/uploads/insert_upload.sql",
            tenant as StoredTenant,
            UploadId::new() as UploadId,
            artifact_id as ArtifactId,
            caller.principal() as StoredPrincipal,
            acting,
            request.idempotency_key.as_ref().map(|key| key.as_str()),
            plan.form as UploadForm,
            request.size.map(size_column),
            request.sha256.as_ref().map(|digest| digest.as_str()),
            request.max_size.map(size_column),
            size_column(plan.reserved),
            plan.part_size
                .map(|size| i64::try_from(size.bytes()).expect("a part size fits a bigint")),
            now,
            now + UPLOAD_LIFETIME,
        )
        .fetch_one(&mut *tx)
        .await?;
        let upload = upload.record()?;
        self.record(
            &mut tx,
            caller,
            upload_target(upload.upload_id),
            &UploadCreated {
                artifact_id,
                upload_id: upload.upload_id,
                form: upload.form,
                reserved: upload.reserved,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok(CreatedUpload {
            upload,
            artifact: artifact.record()?,
            object_upload: None,
            repeated: false,
        })
    }

    /// Record the object store's identifier for a multipart upload the server started.
    pub async fn record_object_upload(
        &self,
        caller: &Caller,
        upload_id: UploadId,
        object_upload: &str,
    ) -> Result<(), ArtifactStoreError> {
        sqlx::query_file_scalar!(
            "src/store/uploads/record_object_upload.sql",
            caller.tenant() as StoredTenant,
            upload_id as UploadId,
            object_upload,
        )
        .fetch_optional(self.pool())
        .await?
        .ok_or(ArtifactStoreError::UploadNotFound)?;
        Ok(())
    }

    /// One of the caller's uploads, with the object store's identifier when one exists.
    pub async fn upload(
        &self,
        caller: &Caller,
        upload_id: UploadId,
    ) -> Result<(UploadRecord, Option<String>), ArtifactStoreError> {
        let row = sqlx::query_file_as!(
            UploadRow,
            "src/store/uploads/upload.sql",
            caller.tenant() as StoredTenant,
            upload_id as UploadId,
        )
        .fetch_optional(self.pool())
        .await?
        .filter(|row| row.creator == caller.principal())
        .ok_or(ArtifactStoreError::UploadNotFound)?;
        Ok((row.record()?, row.object_upload))
    }

    /// Complete the caller's open upload with what object storage reports. Bytes whose
    /// size differs from the declared size, or exceeds the declared maximum, fail the
    /// artifact. An object whose SHA-256 object storage checked becomes available;
    /// otherwise the artifact verifies until [`Self::record_digest`].
    pub async fn complete_upload(
        &self,
        caller: &Caller,
        upload_id: UploadId,
        object: &CompletedObject,
        now: DateTime<Utc>,
    ) -> Result<ArtifactRecord, ArtifactStoreError> {
        let tenant = caller.tenant();
        let mut tx = self.pool().begin().await?;
        let upload = self.lock_open_upload(&mut tx, caller, upload_id).await?;
        let fits = upload
            .declared_size()?
            .is_none_or(|declared| declared == object.size)
            && upload.max_size()?.is_none_or(|max| object.size <= max)
            && object.size.bytes() <= u64::try_from(upload.reserved_bytes).unwrap_or(0);
        let checked = match (&object.checked_sha256, upload.declared_sha256()?) {
            (Some(checked), Some(declared)) if *checked == declared => Some(checked.clone()),
            (Some(_), _) => {
                return Err(ArtifactStoreError::Corrupt(
                    "checked SHA-256 without a matching declaration",
                ));
            }
            (None, _) => None,
        };
        sqlx::query_file_as!(
            UploadRow,
            "src/store/uploads/end_upload.sql",
            tenant as StoredTenant,
            upload_id as UploadId,
            UploadState::Completed as UploadState,
            now,
        )
        .fetch_one(&mut *tx)
        .await?;
        if !fits {
            sqlx::query_file_scalar!(
                "src/store/uploads/fail_artifact.sql",
                tenant as StoredTenant,
                upload.artifact_id as ArtifactId,
                now,
            )
            .fetch_one(&mut *tx)
            .await?;
            self.record(
                &mut tx,
                caller,
                upload_target(upload_id),
                &UploadCompleted {
                    artifact_id: upload.artifact_id,
                    upload_id,
                    size: object.size,
                    state: ArtifactState::Failed,
                },
                Decided::Failed(ArtifactAuditReason::SizeMismatch),
            )
            .await?;
            tx.commit().await?;
            return Err(ArtifactStoreError::SizeMismatch {
                received: object.size.bytes(),
            });
        }
        let artifact = sqlx::query_file_as!(
            ArtifactRow,
            "src/store/uploads/receive_bytes.sql",
            tenant as StoredTenant,
            upload.artifact_id as ArtifactId,
            size_column(object.size),
            checked.as_ref().map(|digest| digest.as_str()),
            now,
        )
        .fetch_one(&mut *tx)
        .await?;
        let artifact = artifact.record()?;
        self.record(
            &mut tx,
            caller,
            upload_target(upload_id),
            &UploadCompleted {
                artifact_id: artifact.artifact_id,
                upload_id,
                size: object.size,
                state: artifact.state,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok(artifact)
    }

    /// Record the SHA-256 the server computed for a verifying artifact. A digest that
    /// differs from the one the upload declared fails the artifact instead.
    pub async fn record_digest(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        declared: Option<&Sha256Digest>,
        computed: &Sha256Digest,
        now: DateTime<Utc>,
    ) -> Result<ArtifactState, ArtifactStoreError> {
        let tenant = caller.tenant();
        let mut tx = self.pool().begin().await?;
        let (state, decided) = if declared.is_some_and(|declared| declared != computed) {
            sqlx::query_file_scalar!(
                "src/store/uploads/fail_artifact.sql",
                tenant as StoredTenant,
                artifact_id as ArtifactId,
                now,
            )
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ArtifactStoreError::ArtifactNotFound)?;
            (
                ArtifactState::Failed,
                Decided::Failed(ArtifactAuditReason::DigestMismatch),
            )
        } else {
            sqlx::query_file_scalar!(
                "src/store/uploads/record_digest.sql",
                tenant as StoredTenant,
                artifact_id as ArtifactId,
                computed.as_str(),
                now,
            )
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ArtifactStoreError::ArtifactNotFound)?;
            (ArtifactState::Available, Decided::Succeeded)
        };
        self.record(
            &mut tx,
            caller,
            artifact_target(artifact_id),
            &ArtifactVerified {
                artifact_id,
                sha256: computed.clone(),
                state,
            },
            decided,
        )
        .await?;
        tx.commit().await?;
        Ok(state)
    }

    /// Abort the caller's open upload and fail its artifact. Triggers release the
    /// reservation. The caller aborts the multipart upload in object storage.
    pub async fn abort_upload(
        &self,
        caller: &Caller,
        upload_id: UploadId,
        now: DateTime<Utc>,
    ) -> Result<(UploadRecord, Option<String>), ArtifactStoreError> {
        let tenant = caller.tenant();
        let mut tx = self.pool().begin().await?;
        let upload = self.lock_open_upload(&mut tx, caller, upload_id).await?;
        let ended = sqlx::query_file_as!(
            UploadRow,
            "src/store/uploads/end_upload.sql",
            tenant as StoredTenant,
            upload_id as UploadId,
            UploadState::Aborted as UploadState,
            now,
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query_file_scalar!(
            "src/store/uploads/fail_artifact.sql",
            tenant as StoredTenant,
            upload.artifact_id as ArtifactId,
            now,
        )
        .fetch_optional(&mut *tx)
        .await?;
        self.record(
            &mut tx,
            caller,
            upload_target(upload_id),
            &UploadAborted {
                artifact_id: upload.artifact_id,
                upload_id,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok((ended.record()?, ended.object_upload))
    }

    /// One page of the caller's uploads, newest first, optionally in one state.
    pub async fn uploads_page(
        &self,
        caller: &Caller,
        state: Option<UploadState>,
        cursor: Option<&CursorToken<Uploads>>,
        size: PageSize<100>,
    ) -> Result<Page<Uploads>, ArtifactStoreError> {
        let tenant = caller.tenant();
        let creator = caller.principal();
        let pager = Pager::<Uploads>::new(
            tenant,
            UploadFilters { state },
            size,
            PrincipalKeyset::new(caller.principal_key().clone()),
            cursor,
        )?;
        let after = pager.after().copied();
        let limit = pager.fetch_limit();
        let rows = match (state, after) {
            (None, None) => {
                sqlx::query_file_as!(
                    UploadRow,
                    "src/store/uploads/uploads_page_first.sql",
                    tenant as StoredTenant,
                    creator as StoredPrincipal,
                    limit,
                )
                .fetch_all(self.pool())
                .await?
            }
            (None, Some(after)) => {
                sqlx::query_file_as!(
                    UploadRow,
                    "src/store/uploads/uploads_page_next.sql",
                    tenant as StoredTenant,
                    creator as StoredPrincipal,
                    after as UploadId,
                    limit,
                )
                .fetch_all(self.pool())
                .await?
            }
            (Some(state), None) => {
                sqlx::query_file_as!(
                    UploadRow,
                    "src/store/uploads/uploads_in_state_page_first.sql",
                    tenant as StoredTenant,
                    creator as StoredPrincipal,
                    state as UploadState,
                    limit,
                )
                .fetch_all(self.pool())
                .await?
            }
            (Some(state), Some(after)) => {
                sqlx::query_file_as!(
                    UploadRow,
                    "src/store/uploads/uploads_in_state_page_next.sql",
                    tenant as StoredTenant,
                    creator as StoredPrincipal,
                    state as UploadState,
                    after as UploadId,
                    limit,
                )
                .fetch_all(self.pool())
                .await?
            }
        };
        let items = rows
            .iter()
            .map(UploadRow::record)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(pager.finish(items)?)
    }

    async fn lock_open_upload(
        &self,
        tx: &mut sqlx::PgConnection,
        caller: &Caller,
        upload_id: UploadId,
    ) -> Result<UploadRow, ArtifactStoreError> {
        let upload = sqlx::query_file_as!(
            UploadRow,
            "src/store/uploads/upload_for_update.sql",
            caller.tenant() as StoredTenant,
            upload_id as UploadId,
        )
        .fetch_optional(&mut *tx)
        .await?
        .filter(|row| row.creator == caller.principal())
        .ok_or(ArtifactStoreError::UploadNotFound)?;
        if upload.state != UploadState::Open {
            return Err(ArtifactStoreError::UploadNotOpen {
                state: upload.state,
            });
        }
        Ok(upload)
    }
}

/// The locked usage row and the tenant's own limits, when it has them.
struct UsageRow {
    stored_bytes: i64,
    reserved_bytes: i64,
    open_uploads: i32,
    quota_bytes: Option<i64>,
    largest_object_bytes: Option<i64>,
    allowed_media_types: Option<Vec<String>>,
    active_uploads: Option<i32>,
}

impl UsageRow {
    fn limits(&self) -> Result<UploadLimits, ArtifactStoreError> {
        match (self.largest_object_bytes, self.active_uploads) {
            (Some(largest_object_bytes), Some(active_uploads)) => LimitsRow {
                quota_bytes: self.quota_bytes,
                largest_object_bytes,
                allowed_media_types: self.allowed_media_types.clone(),
                active_uploads,
            }
            .limits(),
            _ => Ok(UploadLimits::default()),
        }
    }
}
