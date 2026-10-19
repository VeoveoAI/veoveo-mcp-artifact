//! Reading, listing and deleting artifacts, and a tenant's storage usage.
use chrono::{DateTime, Utc};
use sqlx::types::Json;
use veoveo_store::{PrincipalId as StoredPrincipal, TenantId as StoredTenant};
use veoveo_types::{AccessLevel, CursorToken, Page, PageSize, Pager};

use crate::contract::{
    ArtifactDeleted, ArtifactEntry, ArtifactFilter, ArtifactId, ArtifactMetadataMap,
    ArtifactPosition, ArtifactProvenance, ArtifactState, Artifacts, NamePrefix, PrincipalKeyset,
    ReleaseState, StorageUsage,
};

use super::{
    ArtifactStore, ArtifactStoreError, Caller,
    audit::{Decided, artifact_target},
    rows::{ArtifactRow, VisibleArtifact},
};

impl ArtifactStore {
    /// One artifact the caller may read, and its level. An artifact that does not exist
    /// for the caller is refused as unknown.
    pub async fn artifact(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        now: DateTime<Utc>,
    ) -> Result<VisibleArtifact, ArtifactStoreError> {
        let mut connection = self.pool().acquire().await?;
        self.read_artifact(&mut connection, caller, artifact_id, now)
            .await?
            .ok_or(ArtifactStoreError::ArtifactNotFound)
    }

    /// One artifact the caller holds `needed` on. An artifact the caller cannot see is
    /// unknown; one it sees without the level is forbidden.
    pub async fn artifact_requiring(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        needed: AccessLevel,
        now: DateTime<Utc>,
    ) -> Result<VisibleArtifact, ArtifactStoreError> {
        let visible = self.artifact(caller, artifact_id, now).await?;
        require(&visible, needed)?;
        Ok(visible)
    }

    pub(crate) async fn read_artifact(
        &self,
        connection: &mut sqlx::PgConnection,
        caller: &Caller,
        artifact_id: ArtifactId,
        now: DateTime<Utc>,
    ) -> Result<Option<VisibleArtifact>, ArtifactStoreError> {
        let facts = caller.facts();
        let row = sqlx::query_file_as!(
            ArtifactRow,
            "src/store/artifacts/artifact.sql",
            facts.tenant as StoredTenant,
            facts.principal as StoredPrincipal,
            facts.principal_key,
            &facts.groups,
            &facts.group_levels,
            facts.work_context,
            facts.membership,
            &facts.labels,
            now,
            artifact_id as ArtifactId,
        )
        .fetch_optional(&mut *connection)
        .await?;
        match row {
            Some(row) => row.visible(),
            None => Ok(None),
        }
    }

    /// Lock an artifact the caller holds `needed` on until the transaction ends.
    pub(crate) async fn lock_artifact(
        &self,
        connection: &mut sqlx::PgConnection,
        caller: &Caller,
        artifact_id: ArtifactId,
        needed: AccessLevel,
        now: DateTime<Utc>,
    ) -> Result<VisibleArtifact, ArtifactStoreError> {
        let facts = caller.facts();
        let visible = sqlx::query_file_as!(
            ArtifactRow,
            "src/store/artifacts/artifact_for_update.sql",
            facts.tenant as StoredTenant,
            facts.principal as StoredPrincipal,
            facts.principal_key,
            &facts.groups,
            &facts.group_levels,
            facts.work_context,
            facts.membership,
            &facts.labels,
            now,
            artifact_id as ArtifactId,
        )
        .fetch_optional(&mut *connection)
        .await?
        .map(ArtifactRow::visible)
        .transpose()?
        .flatten()
        .ok_or(ArtifactStoreError::ArtifactNotFound)?;
        require(&visible, needed)?;
        Ok(visible)
    }

    /// Delete an artifact the caller administers. It stops existing for every caller at
    /// once; the sweep removes its object and row.
    pub async fn delete_artifact(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        now: DateTime<Utc>,
    ) -> Result<ArtifactState, ArtifactStoreError> {
        let mut tx = self.pool().begin().await?;
        self.lock_artifact(&mut tx, caller, artifact_id, AccessLevel::Admin, now)
            .await?;
        let deleted = sqlx::query_file!(
            "src/store/artifacts/mark_deleting.sql",
            caller.tenant() as StoredTenant,
            artifact_id as ArtifactId,
            now,
        )
        .fetch_one(&mut *tx)
        .await?;
        self.record(
            &mut tx,
            caller,
            artifact_target(artifact_id),
            &ArtifactDeleted { artifact_id },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok(deleted.state)
    }

    /// The caller's tenant's stored and reserved bytes against its quota.
    pub async fn storage_usage(&self, caller: &Caller) -> Result<StorageUsage, ArtifactStoreError> {
        let tenant = caller.tenant();
        let usage = sqlx::query_file!(
            "src/store/artifacts/storage_usage.sql",
            tenant as StoredTenant
        )
        .fetch_optional(self.pool())
        .await?;
        let limits = self.upload_limits(tenant).await?;
        let bytes = |value: i64| {
            u64::try_from(value).map_err(|_| ArtifactStoreError::Corrupt("storage usage"))
        };
        Ok(StorageUsage {
            stored_bytes: usage
                .as_ref()
                .map(|row| bytes(row.stored_bytes))
                .transpose()?
                .unwrap_or(0),
            reserved_bytes: usage
                .as_ref()
                .map(|row| bytes(row.reserved_bytes))
                .transpose()?
                .unwrap_or(0),
            quota_bytes: limits.quota_bytes,
        })
    }

    /// One page of the artifacts the caller may read, newest first, or by name for a name
    /// filter. Visibility is part of each statement, so a page holds only readable rows.
    pub async fn artifacts_page(
        &self,
        caller: &Caller,
        filter: Option<&ArtifactFilter>,
        cursor: Option<&CursorToken<Artifacts>>,
        size: PageSize<100>,
        now: DateTime<Utc>,
    ) -> Result<Page<Artifacts>, ArtifactStoreError> {
        let pager = Pager::<Artifacts>::new(
            caller.tenant(),
            filter.cloned(),
            size,
            PrincipalKeyset::new(caller.principal_key().clone()),
            cursor,
        )?;
        let rows = if let Some(ArtifactFilter::NamePrefix(prefix)) = filter {
            self.artifacts_by_name(caller, prefix, pager.after(), size, now)
                .await?
        } else {
            self.artifact_rows(
                caller,
                filter,
                pager.after().map(|position| position.artifact_id),
                size,
                now,
            )
            .await?
        };
        let items = rows
            .into_iter()
            .map(|row| {
                let visible = row.visible()?.ok_or(ArtifactStoreError::Corrupt(
                    "listed artifact without a level",
                ))?;
                Ok(ArtifactEntry {
                    artifact: visible.record,
                    level: visible.level,
                })
            })
            .collect::<Result<Vec<_>, ArtifactStoreError>>()?;
        Ok(pager.finish(items)?)
    }

    async fn artifact_rows(
        &self,
        caller: &Caller,
        filter: Option<&ArtifactFilter>,
        after: Option<ArtifactId>,
        size: PageSize<100>,
        now: DateTime<Utc>,
    ) -> Result<Vec<ArtifactRow>, ArtifactStoreError> {
        let f = caller.facts();
        let limit = size.fetch_limit();
        let pool = self.pool();
        Ok(match filter {
            None => match after {
                None => {
                    sqlx::query_file_as!(
                        ArtifactRow,
                        "src/store/artifacts/artifacts_first.sql",
                        f.tenant as StoredTenant,
                        f.principal as StoredPrincipal,
                        f.principal_key,
                        &f.groups,
                        &f.group_levels,
                        f.work_context,
                        f.membership,
                        &f.labels,
                        now,
                        limit,
                    )
                    .fetch_all(pool)
                    .await?
                }
                Some(after) => {
                    sqlx::query_file_as!(
                        ArtifactRow,
                        "src/store/artifacts/artifacts_next.sql",
                        f.tenant as StoredTenant,
                        f.principal as StoredPrincipal,
                        f.principal_key,
                        &f.groups,
                        &f.group_levels,
                        f.work_context,
                        f.membership,
                        &f.labels,
                        now,
                        after as ArtifactId,
                        limit,
                    )
                    .fetch_all(pool)
                    .await?
                }
            },
            Some(ArtifactFilter::WorkContext(work_context)) => match after {
                None => {
                    sqlx::query_file_as!(
                        ArtifactRow,
                        "src/store/artifacts/artifacts_by_work_context_first.sql",
                        f.tenant as StoredTenant,
                        f.principal as StoredPrincipal,
                        f.principal_key,
                        &f.groups,
                        &f.group_levels,
                        f.work_context,
                        f.membership,
                        &f.labels,
                        now,
                        work_context.as_str(),
                        limit,
                    )
                    .fetch_all(pool)
                    .await?
                }
                Some(after) => {
                    sqlx::query_file_as!(
                        ArtifactRow,
                        "src/store/artifacts/artifacts_by_work_context_next.sql",
                        f.tenant as StoredTenant,
                        f.principal as StoredPrincipal,
                        f.principal_key,
                        &f.groups,
                        &f.group_levels,
                        f.work_context,
                        f.membership,
                        &f.labels,
                        now,
                        work_context.as_str(),
                        after as ArtifactId,
                        limit,
                    )
                    .fetch_all(pool)
                    .await?
                }
            },
            Some(ArtifactFilter::Owner(owner)) => {
                let owner = veoveo_store::deterministic_principal_id(
                    caller.tenant_key().as_str(),
                    owner.as_str(),
                )
                .map_err(|_| ArtifactStoreError::InvalidCaller)?;
                match after {
                    None => {
                        sqlx::query_file_as!(
                            ArtifactRow,
                            "src/store/artifacts/artifacts_by_owner_first.sql",
                            f.tenant as StoredTenant,
                            f.principal as StoredPrincipal,
                            f.principal_key,
                            &f.groups,
                            &f.group_levels,
                            f.work_context,
                            f.membership,
                            &f.labels,
                            now,
                            owner as StoredPrincipal,
                            limit,
                        )
                        .fetch_all(pool)
                        .await?
                    }
                    Some(after) => {
                        sqlx::query_file_as!(
                            ArtifactRow,
                            "src/store/artifacts/artifacts_by_owner_next.sql",
                            f.tenant as StoredTenant,
                            f.principal as StoredPrincipal,
                            f.principal_key,
                            &f.groups,
                            &f.group_levels,
                            f.work_context,
                            f.membership,
                            &f.labels,
                            now,
                            owner as StoredPrincipal,
                            after as ArtifactId,
                            limit,
                        )
                        .fetch_all(pool)
                        .await?
                    }
                }
            }
            Some(ArtifactFilter::MediaType(media_type)) => match after {
                None => {
                    sqlx::query_file_as!(
                        ArtifactRow,
                        "src/store/artifacts/artifacts_by_media_type_first.sql",
                        f.tenant as StoredTenant,
                        f.principal as StoredPrincipal,
                        f.principal_key,
                        &f.groups,
                        &f.group_levels,
                        f.work_context,
                        f.membership,
                        &f.labels,
                        now,
                        media_type.as_str(),
                        limit,
                    )
                    .fetch_all(pool)
                    .await?
                }
                Some(after) => {
                    sqlx::query_file_as!(
                        ArtifactRow,
                        "src/store/artifacts/artifacts_by_media_type_next.sql",
                        f.tenant as StoredTenant,
                        f.principal as StoredPrincipal,
                        f.principal_key,
                        &f.groups,
                        &f.group_levels,
                        f.work_context,
                        f.membership,
                        &f.labels,
                        now,
                        media_type.as_str(),
                        after as ArtifactId,
                        limit,
                    )
                    .fetch_all(pool)
                    .await?
                }
            },
            Some(ArtifactFilter::ReleaseState(release_state)) => match after {
                None => {
                    sqlx::query_file_as!(
                        ArtifactRow,
                        "src/store/artifacts/artifacts_by_release_state_first.sql",
                        f.tenant as StoredTenant,
                        f.principal as StoredPrincipal,
                        f.principal_key,
                        &f.groups,
                        &f.group_levels,
                        f.work_context,
                        f.membership,
                        &f.labels,
                        now,
                        *release_state as ReleaseState,
                        limit,
                    )
                    .fetch_all(pool)
                    .await?
                }
                Some(after) => {
                    sqlx::query_file_as!(
                        ArtifactRow,
                        "src/store/artifacts/artifacts_by_release_state_next.sql",
                        f.tenant as StoredTenant,
                        f.principal as StoredPrincipal,
                        f.principal_key,
                        &f.groups,
                        &f.group_levels,
                        f.work_context,
                        f.membership,
                        &f.labels,
                        now,
                        *release_state as ReleaseState,
                        after as ArtifactId,
                        limit,
                    )
                    .fetch_all(pool)
                    .await?
                }
            },
            Some(ArtifactFilter::NamePrefix(_)) => {
                unreachable!("name listings page by name through artifacts_by_name")
            }
        })
    }

    async fn artifacts_by_name(
        &self,
        caller: &Caller,
        prefix: &NamePrefix,
        after: Option<&ArtifactPosition>,
        size: PageSize<100>,
        now: DateTime<Utc>,
    ) -> Result<Vec<ArtifactRow>, ArtifactStoreError> {
        let f = caller.facts();
        let lower = prefix.as_str();
        let upper = format!("{lower}\u{10FFFF}");
        let limit = size.fetch_limit();
        let rows = match after {
            None => {
                sqlx::query_file_as!(
                    ArtifactRow,
                    "src/store/artifacts/artifacts_by_name_first.sql",
                    f.tenant as StoredTenant,
                    f.principal as StoredPrincipal,
                    f.principal_key,
                    &f.groups,
                    &f.group_levels,
                    f.work_context,
                    f.membership,
                    &f.labels,
                    now,
                    lower,
                    upper,
                    limit,
                )
                .fetch_all(self.pool())
                .await?
            }
            Some(after) => {
                let position = after;
                sqlx::query_file_as!(
                    ArtifactRow,
                    "src/store/artifacts/artifacts_by_name_next.sql",
                    f.tenant as StoredTenant,
                    f.principal as StoredPrincipal,
                    f.principal_key,
                    &f.groups,
                    &f.group_levels,
                    f.work_context,
                    f.membership,
                    &f.labels,
                    now,
                    lower,
                    upper,
                    position.name,
                    position.artifact_id as ArtifactId,
                    limit,
                )
                .fetch_all(self.pool())
                .await?
            }
        };
        Ok(rows)
    }
}

fn require(visible: &VisibleArtifact, needed: AccessLevel) -> Result<(), ArtifactStoreError> {
    if visible.level.allows(needed) {
        Ok(())
    } else {
        Err(ArtifactStoreError::Forbidden { needed })
    }
}
