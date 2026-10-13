//! A tenant's upload limits. A tenant without its own row has the defaults.
use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use veoveo_platform_store::TenantId as StoredTenant;
use veoveo_types::TenantId;

use crate::contract::{
    ActiveUploadLimit, AllowedMediaTypes, ArtifactId, DownloadIssued, MediaType, UploadLimits,
    UploadLimitsSet,
};

use super::{
    ArtifactStore, ArtifactStoreError, Caller,
    audit::{Decided, artifact_target, server_target},
    rows::object_size,
    rows::size_column,
};

pub(crate) struct LimitsRow {
    pub quota_bytes: Option<i64>,
    pub largest_object_bytes: i64,
    pub allowed_media_types: Option<Vec<String>>,
    pub active_uploads: i32,
}

impl LimitsRow {
    pub(crate) fn limits(&self) -> Result<UploadLimits, ArtifactStoreError> {
        let allowed_media_types = match &self.allowed_media_types {
            None => AllowedMediaTypes::Any,
            Some(types) => AllowedMediaTypes::Listed(
                types
                    .iter()
                    .map(|text| MediaType::parse(text.clone()))
                    .collect::<Result<BTreeSet<_>, _>>()
                    .map_err(|_| ArtifactStoreError::Corrupt("allowed media type"))?,
            ),
        };
        Ok(UploadLimits {
            quota_bytes: self
                .quota_bytes
                .map(|bytes| u64::try_from(bytes).map_err(|_| ArtifactStoreError::Corrupt("quota")))
                .transpose()?,
            largest_object: object_size(self.largest_object_bytes)?,
            allowed_media_types,
            active_uploads: u16::try_from(self.active_uploads)
                .ok()
                .and_then(|count| ActiveUploadLimit::new(count).ok())
                .ok_or(ArtifactStoreError::Corrupt("active upload limit"))?,
        })
    }
}

impl ArtifactStore {
    /// A tenant's upload limits, or the defaults when it has none of its own.
    pub async fn upload_limits(
        &self,
        tenant: StoredTenant,
    ) -> Result<UploadLimits, ArtifactStoreError> {
        sqlx::query_file_as!(
            LimitsRow,
            "src/store/limits/upload_limits.sql",
            tenant as StoredTenant
        )
        .fetch_optional(self.pool())
        .await?
        .map_or_else(|| Ok(UploadLimits::default()), |row| row.limits())
    }

    /// Replace a tenant's upload limits on an installation administrator's behalf.
    pub async fn set_upload_limits(
        &self,
        administrator: &Caller,
        tenant: &TenantId,
        limits: &UploadLimits,
        now: DateTime<Utc>,
    ) -> Result<UploadLimits, ArtifactStoreError> {
        let stored = veoveo_platform_store::deterministic_tenant_id(tenant.as_str())
            .map_err(|_| ArtifactStoreError::InvalidCaller)?;
        let allowed: Option<Vec<String>> = match &limits.allowed_media_types {
            AllowedMediaTypes::Any => None,
            AllowedMediaTypes::Listed(types) => {
                Some(types.iter().map(ToString::to_string).collect())
            }
        };
        let quota = limits
            .quota_bytes
            .map(|bytes| {
                i64::try_from(bytes)
                    .map_err(|_| ArtifactStoreError::InvalidLimits("a quota fits in 63 bits"))
            })
            .transpose()?;
        let mut tx = self.pool().begin().await?;
        let set = sqlx::query_file_as!(
            LimitsRow,
            "src/store/limits/set_upload_limits.sql",
            stored as StoredTenant,
            quota,
            size_column(limits.largest_object),
            allowed.as_deref(),
            i32::from(limits.active_uploads.get()),
            administrator.principal_key().as_str(),
            now,
        )
        .fetch_one(&mut *tx)
        .await?
        .limits()?;
        self.record(
            &mut tx,
            administrator,
            server_target(),
            &UploadLimitsSet {
                tenant: tenant.clone(),
                limits: set.clone(),
                acting_server: administrator.acting_server().cloned(),
                acting_task: administrator.acting_task().cloned(),
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok(set)
    }

    /// Record that `caller` received a download URL for an artifact, valid until
    /// `expires_at`. The service records it before it returns the URL, so a URL whose
    /// record failed never reaches the caller.
    pub async fn record_download(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        expires_at: DateTime<Utc>,
    ) -> Result<(), ArtifactStoreError> {
        let mut tx = self.pool().begin().await?;
        self.record(
            &mut tx,
            caller,
            artifact_target(artifact_id),
            &DownloadIssued {
                artifact_id,
                expires_at,
                acting_server: caller.acting_server().cloned(),
                acting_task: caller.acting_task().cloned(),
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
