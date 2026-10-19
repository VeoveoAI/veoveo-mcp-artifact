//! Why a store operation refused or failed. Refusals name the rule; none repeats a
//! caller's value or a stored secret.
use veoveo_types::AccessLevel;
use veoveo_types::PagingError;

use crate::contract::{AccessRequestState, ArtifactState, MediaType, UploadState};

use super::caller::level_text;

#[derive(Debug, thiserror::Error)]
pub enum ArtifactStoreError {
    #[error("the caller's tenant or principal key cannot name a stored identity")]
    InvalidCaller,
    /// The artifact does not exist for the caller: another tenant's, deleted, past its
    /// retention, outside the caller's labels, or unknown.
    #[error("no such artifact")]
    ArtifactNotFound,
    #[error("the caller needs {} access to the artifact", level_text(*needed))]
    Forbidden { needed: AccessLevel },
    #[error("no such upload")]
    UploadNotFound,
    #[error("the upload is {state}, not open")]
    UploadNotOpen { state: UploadState },
    #[error("the artifact is {state}")]
    ArtifactState { state: ArtifactState },
    #[error("creating an artifact needs contributor membership in the selected Work Context")]
    WorkContextReadOnly,
    #[error("a label of the new artifact lies outside the caller's labels")]
    LabelOutsideClearance,
    #[error("an artifact it derives from does not exist for the caller")]
    UnknownSource,
    #[error("the tenant does not accept {0} uploads")]
    MediaTypeNotAllowed(MediaType),
    #[error("the object exceeds the tenant's largest object of {largest} bytes")]
    TooLarge { largest: u64 },
    #[error("the tenant holds its limit of {limit} open uploads")]
    TooManyOpenUploads { limit: u16 },
    #[error("the upload needs {needed} bytes and the tenant's quota leaves {available}")]
    QuotaExceeded { needed: u64, available: u64 },
    #[error("the received object holds {received} bytes, not what the upload declared")]
    SizeMismatch { received: u64 },
    #[error("the artifact holds its limit of {0} grants")]
    TooManyGrants(usize),
    #[error("a share link reaches an artifact only while it is released")]
    NotReleased,
    #[error(
        "a share link reaches people without data labels, so a labelled artifact cannot be shared by link"
    )]
    LabelledArtifact,
    #[error("the artifact holds its limit of {0} live share links")]
    TooManyShareLinks(usize),
    #[error("invalid share link: {0}")]
    InvalidShareLink(&'static str),
    #[error("no such share link")]
    ShareLinkNotFound,
    #[error("the caller already holds {} access to the artifact", level_text(*level))]
    AlreadyHolds { level: AccessLevel },
    #[error("the caller already has a pending request for the artifact")]
    AccessRequestPending,
    #[error("no such access request")]
    AccessRequestNotFound,
    #[error("the access request is {state}, not pending")]
    AccessRequestSettled { state: AccessRequestState },
    #[error("invalid upload limits: {0}")]
    InvalidLimits(&'static str),
    #[error(transparent)]
    Paging(#[from] PagingError),
    #[error("a stored {0} failed admission")]
    Corrupt(&'static str),
    #[error("the audit record failed admission: {0}")]
    AuditDetail(#[from] veoveo_audit_contract::AuditDetailError),
    #[error("the audit record failed validation: {0}")]
    AuditDraft(#[from] veoveo_audit_contract::AuditValidationError),
    #[error("the audit record could not be appended: {0}")]
    AuditAppend(#[from] veoveo_audit_contract::AuditAppendError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
