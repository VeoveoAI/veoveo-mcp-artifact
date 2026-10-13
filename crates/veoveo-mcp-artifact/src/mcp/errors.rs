//! How a refusal or failure reaches the caller. A refusal names its rule; a failure of
//! the database, object storage or Task runtime is logged and reported without detail.
use rmcp::ErrorData as McpError;

use crate::{service::ArtifactError, store::ArtifactStoreError};

pub(crate) fn tool_error(error: ArtifactError) -> McpError {
    match error {
        ArtifactError::Store(error) => store_error(error),
        refusal @ (ArtifactError::NoBytes
        | ArtifactError::DigestMismatch
        | ArtifactError::PartTooLarge { .. }
        | ArtifactError::NotMultipart) => McpError::invalid_params(refusal.to_string(), None),
        ArtifactError::Storage(crate::storage::StorageError::ObjectExists) => {
            McpError::invalid_request("the artifact's object already exists", None)
        }
        ArtifactError::Storage(crate::storage::StorageError::NoParts) => {
            McpError::invalid_params("the multipart upload holds no parts yet", None)
        }
        failure @ (ArtifactError::Storage(_) | ArtifactError::Tasks(_)) => internal(&failure),
    }
}

pub(crate) fn store_error(error: ArtifactStoreError) -> McpError {
    match error {
        ArtifactStoreError::ArtifactNotFound
        | ArtifactStoreError::UploadNotFound
        | ArtifactStoreError::UnknownSource
        | ArtifactStoreError::MultipleFilters
        | ArtifactStoreError::Cursor(_)
        | ArtifactStoreError::TooLarge { .. }
        | ArtifactStoreError::SizeMismatch { .. }
        | ArtifactStoreError::MediaTypeNotAllowed(_)
        | ArtifactStoreError::InvalidLimits(_) => McpError::invalid_params(error.to_string(), None),
        ArtifactStoreError::Forbidden { .. }
        | ArtifactStoreError::UploadNotOpen { .. }
        | ArtifactStoreError::ArtifactState { .. }
        | ArtifactStoreError::WorkContextReadOnly
        | ArtifactStoreError::LabelOutsideClearance
        | ArtifactStoreError::TooManyOpenUploads { .. }
        | ArtifactStoreError::QuotaExceeded { .. }
        | ArtifactStoreError::TooManyGrants(_)
        | ArtifactStoreError::InvalidCaller => McpError::invalid_request(error.to_string(), None),
        ArtifactStoreError::Corrupt(_) | ArtifactStoreError::Database(_) => internal(&error),
    }
}

fn internal(error: &dyn std::fmt::Display) -> McpError {
    tracing::warn!(%error, "an Artifact request failed");
    McpError::internal_error("the Artifact server could not complete the request", None)
}
