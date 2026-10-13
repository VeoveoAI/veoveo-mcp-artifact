//! Persistence in the server's `artifact` schema. Every statement is a `.sql` file beside
//! the module that runs it, checked by sqlx against the migrated schema. Per-artifact
//! access is decided in SQL by `artifact.effective_level` from the caller's facts.

mod artifacts;
mod caller;
mod error;
mod grants;
mod limits;
mod plan;
mod rows;
mod sweep;
mod uploads;

pub use artifacts::{ArtifactFilter, ArtifactPage, ArtifactPageSize, MAX_ARTIFACTS_PER_PAGE};
pub use caller::{Caller, CallerContext, GroupAccess};
pub use error::ArtifactStoreError;
pub use grants::MAX_GRANTS_PER_ARTIFACT;
pub use plan::{DEFAULT_PART_BYTES, SINGLE_PUT_MAX_BYTES, UPLOAD_LIFETIME, UploadPlan};
pub use rows::VisibleArtifact;
pub use sweep::{
    ACCESS_REQUEST_RETENTION, ClaimedDeletion, DeletionBatch, ExpiredUpload, FAILED_RETENTION,
    SHARE_LINK_RETENTION, StaleVerification, UPLOAD_RETENTION,
};
pub use uploads::{
    CompletedObject, CreatedUpload, MAX_UPLOADS_PER_PAGE, UploadPage, UploadPageSize,
};

use veoveo_platform_store::PlatformStore;

/// The Artifact server's persistence over the pool its registration login opened.
#[derive(Clone)]
pub struct ArtifactStore {
    store: PlatformStore,
}

impl ArtifactStore {
    pub fn new(store: PlatformStore) -> Self {
        Self { store }
    }

    pub fn platform(&self) -> &PlatformStore {
        &self.store
    }

    fn pool(&self) -> &sqlx::PgPool {
        self.store.pool()
    }
}
