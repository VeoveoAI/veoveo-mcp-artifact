//! Artifact's public types. Consumers select `default-features = false,
//! features = ["contract"]`, which builds without a database, runtime or object-storage
//! dependency.

mod ids;
mod values;
mod vocabulary;

pub use ids::{AccessRequestId, ArtifactId, ArtifactIdentifierError, ShareLinkId, UploadId};
pub use values::{
    ArtifactLabels, ArtifactMetadataMap, ArtifactName, IdempotencyKey, MAX_IDEMPOTENCY_KEY_BYTES,
    MAX_LABELS, MAX_METADATA_BYTES, MAX_NAME_BYTES, MAX_OBJECT_BYTES, MAX_RETENTION_DAYS,
    MediaType, NamePrefix, ObjectSize, RetentionDays, ValueError,
};
pub use vocabulary::{
    AccessRequestDecision, AccessRequestState, ArtifactPermission, ArtifactState, ReleaseState,
    UploadForm, UploadState,
};
