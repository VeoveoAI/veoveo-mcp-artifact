//! The server's operations: each one combines the store's decision with the object
//! storage call it needs. MCP handlers translate requests into these calls; the tests
//! drive them directly.
mod sharing;
mod transfers;

use chrono::{DateTime, Utc};
use veoveo_types::{AccessLevel, AccessSubject, TenantId};

use crate::{
    contract::{
        ArtifactGrant, ArtifactId, ArtifactRecord, ArtifactState, CreateDownloadResult,
        DeleteArtifactResult, GrantArtifactRequest, UploadLimits,
    },
    share::ShareBase,
    storage::{ObjectKey, ObjectStorage, StorageError},
    store::{ArtifactStore, ArtifactStoreError, Caller},
};

/// Why an operation failed: the store refused or failed, or object storage failed.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error(transparent)]
    Store(#[from] ArtifactStoreError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("no bytes arrived for the upload")]
    NoBytes,
    #[error("a part declares {declared} bytes, more than the upload's part size of {part_size}")]
    PartTooLarge { declared: u64, part_size: u64 },
    #[error("the upload takes one signed PutObject, not parts")]
    NotMultipart,
    #[error("the operating system's random source failed: {0}")]
    Random(getrandom::Error),
}

/// The server's state: its store, its bucket and the origin its share
/// links name.
#[derive(Clone)]
pub struct ArtifactService {
    store: ArtifactStore,
    storage: ObjectStorage,
    share_base: ShareBase,
}

impl ArtifactService {
    pub fn new(store: ArtifactStore, storage: ObjectStorage, share_base: ShareBase) -> Self {
        Self {
            store,
            storage,
            share_base,
        }
    }

    pub fn store(&self) -> &ArtifactStore {
        &self.store
    }

    pub fn storage(&self) -> &ObjectStorage {
        &self.storage
    }

    /// A signed GET for an available artifact the caller may read.
    pub async fn create_download(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        now: DateTime<Utc>,
    ) -> Result<CreateDownloadResult, ArtifactError> {
        let visible = self
            .store
            .artifact_requiring(caller, artifact_id, AccessLevel::Read, now)
            .await?;
        if visible.record.state != ArtifactState::Available {
            return Err(ArtifactStoreError::ArtifactState {
                state: visible.record.state,
            }
            .into());
        }
        let key = ObjectKey::for_artifact(caller.tenant(), artifact_id);
        let get = self
            .storage
            .sign_get(&key, &visible.record.name, &visible.record.media_type, now)
            .await?;
        self.store
            .record_download(caller, artifact_id, get.expires_at)
            .await?;
        Ok(CreateDownloadResult {
            artifact: visible.record,
            get,
        })
    }

    /// Delete an artifact the caller administers. The sweep removes its object.
    pub async fn delete_artifact(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        now: DateTime<Utc>,
    ) -> Result<DeleteArtifactResult, ArtifactError> {
        let state = self.store.delete_artifact(caller, artifact_id, now).await?;
        Ok(DeleteArtifactResult { artifact_id, state })
    }

    pub async fn grant_artifact(
        &self,
        caller: &Caller,
        request: &GrantArtifactRequest,
        now: DateTime<Utc>,
    ) -> Result<ArtifactGrant, ArtifactError> {
        Ok(self.store.grant_artifact(caller, request, now).await?)
    }

    pub async fn revoke_artifact_grant(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        subject: &AccessSubject,
        now: DateTime<Utc>,
    ) -> Result<bool, ArtifactError> {
        Ok(self
            .store
            .revoke_artifact_grant(caller, artifact_id, subject, now)
            .await?)
    }

    pub async fn set_upload_limits(
        &self,
        administrator: &Caller,
        tenant: &TenantId,
        limits: &UploadLimits,
        now: DateTime<Utc>,
    ) -> Result<UploadLimits, ArtifactError> {
        Ok(self
            .store
            .set_upload_limits(administrator, tenant, limits, now)
            .await?)
    }

    /// The artifact as the caller now sees it.
    pub async fn artifact(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        now: DateTime<Utc>,
    ) -> Result<ArtifactRecord, ArtifactError> {
        Ok(self.store.artifact(caller, artifact_id, now).await?.record)
    }

    /// Remove the object of an artifact whose bytes failed or never completed. A failure
    /// here leaves the object to the sweep, which removes failed artifacts' objects
    /// before their rows.
    pub(crate) async fn discard_object(&self, caller: &Caller, artifact_id: ArtifactId) {
        let key = ObjectKey::for_artifact(caller.tenant(), artifact_id);
        if let Err(error) = self.storage.delete(&key).await {
            tracing::warn!(%error, %artifact_id, "could not remove a failed artifact's object");
        }
    }
}
