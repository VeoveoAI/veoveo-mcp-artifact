//! The sweep's statements. Every replica runs them; `FOR UPDATE SKIP LOCKED` hands each
//! row to one replica, and each batch commits on its own.
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};
use veoveo_platform_store::{MaintenanceBatch, Pruned, TenantId as StoredTenant};

use crate::contract::{ArtifactId, UploadId};

use super::{ArtifactStore, ArtifactStoreError};

/// How long an ended upload stays: 7 days.
pub const UPLOAD_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long a failed artifact stays: 7 days.
pub const FAILED_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long an expired or revoked share link stays: 30 days.
pub const SHARE_LINK_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How long a decided access request stays: 90 days.
pub const ACCESS_REQUEST_RETENTION: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// An upload the sweep ended because it expired. The caller aborts its multipart upload
/// in object storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiredUpload {
    pub tenant: StoredTenant,
    pub upload_id: UploadId,
    pub artifact_id: ArtifactId,
    pub object_upload: Option<String>,
}

/// A deleted artifact whose object the caller removes before it removes the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimedDeletion {
    pub tenant: StoredTenant,
    pub artifact_id: ArtifactId,
}

/// A batch of deleted artifacts held for one transaction. The caller deletes each
/// object, removes each row, then commits; a crash before the commit hands the batch to
/// the next sweep, and deleting an absent object succeeds.
pub struct DeletionBatch {
    tx: Transaction<'static, Postgres>,
    pub claimed: Vec<ClaimedDeletion>,
}

impl DeletionBatch {
    pub async fn remove(&mut self, deletion: ClaimedDeletion) -> Result<(), ArtifactStoreError> {
        sqlx::query_file!(
            "src/store/sweep/remove_deleted.sql",
            deletion.tenant as StoredTenant,
            deletion.artifact_id as ArtifactId,
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    pub async fn commit(self) -> Result<(), ArtifactStoreError> {
        self.tx.commit().await?;
        Ok(())
    }
}

impl ArtifactStore {
    /// End one batch of expired uploads and fail their artifacts.
    pub async fn expire_uploads(
        &self,
        now: DateTime<Utc>,
        batch: MaintenanceBatch,
    ) -> Result<Vec<ExpiredUpload>, ArtifactStoreError> {
        Ok(sqlx::query_file_as!(
            ExpiredUpload,
            "src/store/sweep/expire_uploads.sql",
            now,
            batch.limit(),
        )
        .fetch_all(self.pool())
        .await?)
    }

    /// Mark artifacts past their retention as deleting, batch by batch.
    pub async fn delete_expired_artifacts(
        &self,
        now: DateTime<Utc>,
        batch: MaintenanceBatch,
    ) -> Result<Pruned, ArtifactStoreError> {
        Pruned::in_batches(batch, |limit| async move {
            Ok(
                sqlx::query_file!("src/store/sweep/retention_due.sql", now, limit)
                    .execute(self.pool())
                    .await?
                    .rows_affected(),
            )
        })
        .await
    }

    /// Claim a batch of deleted artifacts for object removal.
    pub async fn claim_deletions(
        &self,
        now: DateTime<Utc>,
        limit: MaintenanceBatch,
    ) -> Result<DeletionBatch, ArtifactStoreError> {
        let mut tx = self.pool().begin().await?;
        let claimed = sqlx::query_file_as!(
            ClaimedDeletion,
            "src/store/sweep/deleting_batch.sql",
            limit.limit(),
            now,
        )
        .fetch_all(&mut *tx)
        .await?;
        Ok(DeletionBatch { tx, claimed })
    }

    /// Remove failed artifacts, ended uploads, expired grants, ended share links and
    /// decided access requests past their retention, batch by batch.
    pub async fn prune(
        &self,
        now: DateTime<Utc>,
        batch: MaintenanceBatch,
    ) -> Result<Pruned, ArtifactStoreError> {
        let failed_before = now - FAILED_RETENTION;
        let uploads_before = now - UPLOAD_RETENTION;
        let links_before = now - SHARE_LINK_RETENTION;
        let requests_before = now - ACCESS_REQUEST_RETENTION;
        let failed = Pruned::in_batches(batch, |limit| async move {
            Ok::<_, ArtifactStoreError>(
                sqlx::query_file!("src/store/sweep/remove_failed.sql", failed_before, limit)
                    .execute(self.pool())
                    .await?
                    .rows_affected(),
            )
        })
        .await?;
        let uploads = Pruned::in_batches(batch, |limit| async move {
            Ok::<_, ArtifactStoreError>(
                sqlx::query_file!("src/store/sweep/prune_uploads.sql", uploads_before, limit)
                    .execute(self.pool())
                    .await?
                    .rows_affected(),
            )
        })
        .await?;
        let grants = Pruned::in_batches(batch, |limit| async move {
            Ok::<_, ArtifactStoreError>(
                sqlx::query_file!("src/store/sweep/prune_grants.sql", now, limit)
                    .execute(self.pool())
                    .await?
                    .rows_affected(),
            )
        })
        .await?;
        let links = Pruned::in_batches(batch, |limit| async move {
            Ok::<_, ArtifactStoreError>(
                sqlx::query_file!("src/store/sweep/prune_share_links.sql", links_before, limit)
                    .execute(self.pool())
                    .await?
                    .rows_affected(),
            )
        })
        .await?;
        let requests = Pruned::in_batches(batch, |limit| async move {
            Ok::<_, ArtifactStoreError>(
                sqlx::query_file!(
                    "src/store/sweep/prune_access_requests.sql",
                    requests_before,
                    limit
                )
                .execute(self.pool())
                .await?
                .rows_affected(),
            )
        })
        .await?;
        Ok(failed + uploads + grants + links + requests)
    }
}
