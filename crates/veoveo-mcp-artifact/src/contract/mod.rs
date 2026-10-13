//! Artifact's public types. Consumers select `default-features = false,
//! features = ["contract"]`, which builds without a database, runtime or object-storage
//! dependency.

mod ids;
mod records;
mod resource;
mod tools;
mod transfer;
mod values;
mod vocabulary;

pub use ids::{AccessRequestId, ArtifactId, ArtifactIdentifierError, ShareLinkId, UploadId};
pub use records::{
    AccessRequestRecord, ActiveUploadLimit, AllowedMediaTypes, ArtifactGrant, ArtifactProvenance,
    ArtifactRecord, ByteCount, DerivedFrom, MAX_DERIVED_FROM, MAX_NOTE_BYTES, Note, PartNumber,
    PartSize, ShareLinkRecord, StorageUsage, UploadLimits, UploadRecord,
};
pub use resource::{
    AccessRequestRole, ArtifactResource, ArtifactResourceError, ListCursor, MAX_CURSOR_BYTES,
};
pub use tools::{
    AbortUploadResult, AccessRequestRef, AccessRequestResult, ArtifactRequest, ArtifactResult,
    CompleteUploadResult, CreateDownloadResult, CreateShareLinkRequest, CreateShareLinkResult,
    CreateUploadRequest, CreateUploadResult, DecideAccessRequestRequest, DeleteArtifactResult,
    GrantArtifactRequest, GrantArtifactResult, MAX_PARTS_PER_SIGNING, MAX_SHARE_LINK_DAYS,
    PartRequest, RequestArtifactAccessRequest, RevokeArtifactGrantRequest,
    RevokeArtifactGrantResult, RevokeShareLinkRequest, SetReleaseStateRequest,
    SetUploadLimitsRequest, ShareLinkDays, ShareLinkResult, SignUploadPartsRequest,
    SignUploadPartsResult, UploadLimitsResult, UploadRequest,
};
pub use transfer::{
    MAX_SIGNED_URL_BYTES, SignedPart, SignedTransfer, SignedUrl, TransferMethod, UploadTransfer,
};
pub use values::{
    ArtifactLabels, ArtifactMetadataMap, ArtifactName, IdempotencyKey, MAX_IDEMPOTENCY_KEY_BYTES,
    MAX_LABELS, MAX_METADATA_BYTES, MAX_NAME_BYTES, MAX_OBJECT_BYTES, MAX_RETENTION_DAYS,
    MediaType, NamePrefix, ObjectSize, RetentionDays, ValueError,
};
pub use vocabulary::{
    AccessRequestDecision, AccessRequestState, ArtifactPermission, ArtifactState, ReleaseState,
    UploadForm, UploadState,
};
