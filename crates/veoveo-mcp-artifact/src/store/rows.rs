//! Rows as sqlx decodes them, and their admission into contract records. Text columns
//! pass the same admission as a request, so a row the contract would reject fails to load.
use chrono::{DateTime, Utc};
use sqlx::types::Json;
use veoveo_store::PrincipalId as StoredPrincipal;
use veoveo_types::{AccessLevel, DataLabelId, PrincipalId, Sha256Digest, WorkContextId};

use crate::contract::{
    ArtifactId, ArtifactLabels, ArtifactMetadataMap, ArtifactName, ArtifactProvenance,
    ArtifactRecord, ArtifactState, MediaType, ObjectSize, PartSize, ReleaseState, UploadForm,
    UploadId, UploadRecord, UploadState,
};

use super::{ArtifactStoreError, caller::level_from_rank};

/// An artifact row with the caller's level on it as a rank.
pub(crate) struct ArtifactRow {
    pub artifact_id: ArtifactId,
    pub owner_key: String,
    pub work_context: String,
    pub name: String,
    pub media_type: String,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
    pub labels: Vec<String>,
    pub release_state: ReleaseState,
    pub state: ArtifactState,
    pub provenance: Json<ArtifactProvenance>,
    pub metadata: Json<ArtifactMetadataMap>,
    pub created_at: DateTime<Utc>,
    pub available_at: Option<DateTime<Utc>>,
    pub retain_until: Option<DateTime<Utc>>,
    pub level: i16,
}

/// An artifact the caller may at least read, and its level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleArtifact {
    pub record: ArtifactRecord,
    pub level: AccessLevel,
}

impl ArtifactRow {
    /// The record and the caller's level, or `None` when the artifact does not exist for
    /// the caller.
    pub(crate) fn visible(self) -> Result<Option<VisibleArtifact>, ArtifactStoreError> {
        let Some(level) = level_from_rank(self.level) else {
            return Ok(None);
        };
        Ok(Some(VisibleArtifact {
            record: self.record()?,
            level,
        }))
    }

    pub(crate) fn record(self) -> Result<ArtifactRecord, ArtifactStoreError> {
        let labels = self
            .labels
            .into_iter()
            .map(DataLabelId::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| ArtifactStoreError::Corrupt("artifact label"))?;
        Ok(ArtifactRecord {
            artifact_id: self.artifact_id,
            name: ArtifactName::parse(self.name)
                .map_err(|_| ArtifactStoreError::Corrupt("artifact name"))?,
            media_type: MediaType::parse(self.media_type)
                .map_err(|_| ArtifactStoreError::Corrupt("artifact media type"))?,
            size: self.size_bytes.map(object_size).transpose()?,
            sha256: self.sha256.map(digest).transpose()?,
            owner: PrincipalId::parse(self.owner_key)
                .map_err(|_| ArtifactStoreError::Corrupt("artifact owner"))?,
            work_context: WorkContextId::parse(self.work_context)
                .map_err(|_| ArtifactStoreError::Corrupt("artifact Work Context"))?,
            labels: ArtifactLabels::new(labels)
                .map_err(|_| ArtifactStoreError::Corrupt("artifact labels"))?,
            release_state: self.release_state,
            state: self.state,
            provenance: self.provenance.0,
            metadata: self.metadata.0,
            created_at: self.created_at,
            available_at: self.available_at,
            retain_until: self.retain_until,
        })
    }
}

/// An upload row.
pub(crate) struct UploadRow {
    pub upload_id: UploadId,
    pub artifact_id: ArtifactId,
    pub creator: StoredPrincipal,
    pub form: UploadForm,
    pub state: UploadState,
    pub declared_size: Option<i64>,
    pub declared_sha256: Option<String>,
    pub max_size: Option<i64>,
    pub reserved_bytes: i64,
    pub object_upload: Option<String>,
    pub part_size: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

impl UploadRow {
    pub(crate) fn record(&self) -> Result<UploadRecord, ArtifactStoreError> {
        Ok(UploadRecord {
            upload_id: self.upload_id,
            artifact_id: self.artifact_id,
            form: self.form,
            state: self.state,
            declared_size: self.declared_size.map(object_size).transpose()?,
            declared_sha256: self.declared_sha256.clone().map(digest).transpose()?,
            reserved: object_size(self.reserved_bytes)?,
            part_size: self
                .part_size
                .map(|bytes| {
                    u64::try_from(bytes)
                        .ok()
                        .and_then(|bytes| PartSize::new(bytes).ok())
                        .ok_or(ArtifactStoreError::Corrupt("upload part size"))
                })
                .transpose()?,
            created_at: self.created_at,
            expires_at: self.expires_at,
            ended_at: self.ended_at,
        })
    }

    pub(crate) fn declared_size(&self) -> Result<Option<ObjectSize>, ArtifactStoreError> {
        self.declared_size.map(object_size).transpose()
    }

    pub(crate) fn declared_sha256(&self) -> Result<Option<Sha256Digest>, ArtifactStoreError> {
        self.declared_sha256.clone().map(digest).transpose()
    }

    pub(crate) fn max_size(&self) -> Result<Option<ObjectSize>, ArtifactStoreError> {
        self.max_size.map(object_size).transpose()
    }
}

pub(crate) fn object_size(bytes: i64) -> Result<ObjectSize, ArtifactStoreError> {
    u64::try_from(bytes)
        .ok()
        .and_then(|bytes| ObjectSize::new(bytes).ok())
        .ok_or(ArtifactStoreError::Corrupt("object size"))
}

pub(crate) fn digest(text: String) -> Result<Sha256Digest, ArtifactStoreError> {
    Sha256Digest::parse(text).map_err(|_| ArtifactStoreError::Corrupt("SHA-256"))
}

/// A size for a `bigint` column; every admitted size fits.
pub(crate) fn size_column(size: ObjectSize) -> i64 {
    i64::try_from(size.bytes()).expect("an object size fits a bigint")
}
