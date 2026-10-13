//! Reading, listing, releasing and deleting artifacts, and a tenant's storage usage.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use veoveo_platform_store::{
    CursorScope, KeysetCursor, Page, PageSize, PrincipalId as StoredPrincipal,
    TenantId as StoredTenant,
};
use veoveo_types::{AccessLevel, PrincipalId, WorkContextId};

use crate::contract::{
    ArtifactId, ArtifactMetadataMap, ArtifactProvenance, ArtifactState, ListCursor, MediaType,
    NamePrefix, ReleaseState, StorageUsage,
};

use super::{
    ArtifactStore, ArtifactStoreError, Caller,
    rows::{ArtifactRow, VisibleArtifact},
};

/// The most artifacts one page holds.
pub const MAX_ARTIFACTS_PER_PAGE: u32 = 100;
/// A page size of an artifact listing.
pub type ArtifactPageSize = PageSize<MAX_ARTIFACTS_PER_PAGE>;
/// One page of readable artifacts and the cursor of the next.
pub type ArtifactPage = Page<VisibleArtifact, ListCursor>;

/// The one filter an artifact listing applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactFilter {
    WorkContext(WorkContextId),
    Owner(PrincipalId),
    MediaType(MediaType),
    /// Names that start with the prefix, ordered by name.
    NamePrefix(NamePrefix),
    ReleaseState(ReleaseState),
}

impl ArtifactFilter {
    /// The filter of a listing address, refusing an address with more than one.
    pub fn from_parts(
        work_context: Option<WorkContextId>,
        owner: Option<PrincipalId>,
        media_type: Option<MediaType>,
        name: Option<NamePrefix>,
        release_state: Option<ReleaseState>,
    ) -> Result<Option<Self>, ArtifactStoreError> {
        let filters: Vec<Self> = [
            work_context.map(Self::WorkContext),
            owner.map(Self::Owner),
            media_type.map(Self::MediaType),
            name.map(Self::NamePrefix),
            release_state.map(Self::ReleaseState),
        ]
        .into_iter()
        .flatten()
        .collect();
        match <[Self; 1]>::try_from(filters) {
            Ok([filter]) => Ok(Some(filter)),
            Err(filters) if filters.is_empty() => Ok(None),
            Err(_) => Err(ArtifactStoreError::MultipleFilters),
        }
    }

    fn scope_key(&self) -> (&'static str, String) {
        match self {
            Self::WorkContext(value) => ("work_context", value.to_string()),
            Self::Owner(value) => ("owner", value.to_string()),
            Self::MediaType(value) => ("media_type", value.to_string()),
            Self::NamePrefix(value) => ("name", value.to_string()),
            Self::ReleaseState(value) => ("release_state", value.to_string()),
        }
    }
}

/// Where a name-ordered page ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NamePosition {
    name: String,
    artifact_id: ArtifactId,
}

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
        tx.commit().await?;
        Ok(deleted.state)
    }

    /// Move an available artifact the caller administers between private and released.
    pub async fn set_release_state(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        release_state: ReleaseState,
        now: DateTime<Utc>,
    ) -> Result<VisibleArtifact, ArtifactStoreError> {
        let mut tx = self.pool().begin().await?;
        let visible = self
            .lock_artifact(&mut tx, caller, artifact_id, AccessLevel::Admin, now)
            .await?;
        if visible.record.state != ArtifactState::Available {
            return Err(ArtifactStoreError::ArtifactState {
                state: visible.record.state,
            });
        }
        sqlx::query_file_scalar!(
            "src/store/artifacts/set_release_state.sql",
            caller.tenant() as StoredTenant,
            artifact_id as ArtifactId,
            release_state as ReleaseState,
        )
        .fetch_one(&mut *tx)
        .await?;
        let changed = self
            .read_artifact(&mut tx, caller, artifact_id, now)
            .await?
            .ok_or(ArtifactStoreError::ArtifactNotFound)?;
        tx.commit().await?;
        Ok(changed)
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
        cursor: Option<&ListCursor>,
        size: ArtifactPageSize,
        now: DateTime<Utc>,
    ) -> Result<ArtifactPage, ArtifactStoreError> {
        let scope = CursorScope::new(
            "artifact.artifacts",
            caller.tenant(),
            &filter.map(ArtifactFilter::scope_key),
        )?;
        if let Some(ArtifactFilter::NamePrefix(prefix)) = filter {
            return self
                .artifacts_by_name(caller, prefix, &scope, cursor, size, now)
                .await;
        }
        let after: Option<KeysetCursor<ArtifactId>> =
            cursor.map(|wire| scope.parse(wire.as_str())).transpose()?;
        let rows = self
            .artifact_rows(
                caller,
                filter,
                after.map(|cursor| *cursor.position()),
                size,
                now,
            )
            .await?;
        let page = size.page(rows, |last| last.artifact_id);
        visible_page(
            page.items,
            page.next
                .map(|position| scope.cursor(position))
                .transpose()?,
        )
    }

    async fn artifact_rows(
        &self,
        caller: &Caller,
        filter: Option<&ArtifactFilter>,
        after: Option<ArtifactId>,
        size: ArtifactPageSize,
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
                let owner = veoveo_platform_store::deterministic_principal_id(
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
        scope: &CursorScope,
        cursor: Option<&ListCursor>,
        size: ArtifactPageSize,
        now: DateTime<Utc>,
    ) -> Result<ArtifactPage, ArtifactStoreError> {
        let f = caller.facts();
        let lower = prefix.as_str();
        let upper = format!("{lower}\u{10FFFF}");
        let limit = size.fetch_limit();
        let after: Option<KeysetCursor<NamePosition>> =
            cursor.map(|wire| scope.parse(wire.as_str())).transpose()?;
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
                let position = after.position();
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
        let page = size.page(rows, |last| NamePosition {
            name: last.name.clone(),
            artifact_id: last.artifact_id,
        });
        visible_page(
            page.items,
            page.next
                .map(|position| scope.cursor(position))
                .transpose()?,
        )
    }
}

fn require(visible: &VisibleArtifact, needed: AccessLevel) -> Result<(), ArtifactStoreError> {
    if visible.level.allows(needed) {
        Ok(())
    } else {
        Err(ArtifactStoreError::Forbidden { needed })
    }
}

fn visible_page<P: Serialize + serde::de::DeserializeOwned>(
    rows: Vec<ArtifactRow>,
    next: Option<KeysetCursor<P>>,
) -> Result<ArtifactPage, ArtifactStoreError> {
    let items = rows
        .into_iter()
        .map(|row| {
            row.visible()?.ok_or(ArtifactStoreError::Corrupt(
                "listed artifact without a level",
            ))
        })
        .collect::<Result<_, _>>()?;
    let next = next
        .map(|cursor| ListCursor::parse(cursor.as_str()))
        .transpose()
        .map_err(|_| ArtifactStoreError::Corrupt("issued cursor"))?;
    Ok(Page { items, next })
}
