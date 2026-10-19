//! Artifact's public types. Consumers select `default-features = false,
//! features = ["contract"]`, which builds without a database, runtime or object-storage
//! dependency.

mod audit;
mod calls;
mod collections;
mod ids;
mod permissions;
mod records;
mod resource;
mod tools;
mod transfer;
mod values;
mod vocabulary;

pub use audit::*;
pub use calls::{
    ArtifactProvider, ReadAccessRequests, ReadArtifact, ReadArtifacts, ReadGrants, ReadShareLinks,
    ReadStorageUsage, ReadUpload, ReadUploads,
};
pub use collections::*;
pub use ids::{AccessRequestId, ArtifactId, ArtifactIdentifierError, ShareLinkId, UploadId};
pub use permissions::ArtifactScope;
pub use records::{
    AccessRequestRecord, ActiveUploadLimit, AllowedMediaTypes, ArtifactEntry, ArtifactGrant,
    ArtifactProvenance, ArtifactRecord, ByteCount, DerivedFrom, MAX_DERIVED_FROM, MAX_NOTE_BYTES,
    MAX_SHARE_LINK_URL_BYTES, Note, PartNumber, PartSize, ShareLinkRecord, ShareLinkUrl,
    StorageUsage, UploadLimits, UploadRecord,
};
pub use resource::{AccessRequestRole, ArtifactDocument, ArtifactResource, ArtifactResourceError};
pub use tools::{
    AbortUploadRequest, AbortUploadResult, AccessRequestResult, ArtifactResult,
    CancelAccessRequestRequest, CompleteUploadRequest, CompleteUploadResult, CreateDownloadRequest,
    CreateDownloadResult, CreateShareLinkRequest, CreateShareLinkResult, CreateUploadRequest,
    CreateUploadResult, DecideAccessRequestRequest, DeleteArtifactRequest, DeleteArtifactResult,
    GrantArtifactRequest, GrantArtifactResult, MAX_PARTS_PER_SIGNING, MAX_SHARE_LINK_DAYS,
    PartRequest, RequestArtifactAccessRequest, RevokeArtifactGrantRequest,
    RevokeArtifactGrantResult, RevokeShareLinkRequest, SetReleaseStateRequest,
    SetUploadLimitsRequest, ShareLinkDays, ShareLinkResult, SignUploadPartsRequest,
    SignUploadPartsResult, UploadLimitsResult,
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
