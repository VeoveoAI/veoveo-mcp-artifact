//! Access requests: a caller who reads an artifact asks its administrators for a higher
//! level, withdraws the request, or has it approved, which writes the grant, or denied.
use chrono::{DateTime, Utc};
use veoveo_platform_store::{
    CursorScope, KeysetCursor, Page, PageSize, PrincipalId as StoredPrincipal,
    TenantId as StoredTenant,
};
use veoveo_types::{AccessLevel, AccessSubject, PrincipalId};

use crate::contract::{
    AccessRequestCancelled, AccessRequestDecided, AccessRequestDecision, AccessRequestId,
    AccessRequestRecord, AccessRequestRole, AccessRequestState, AccessRequested, ArtifactGrant,
    ArtifactId, GrantChanged, ListCursor, Note, RequestArtifactAccessRequest,
};

use super::{
    ArtifactStore, ArtifactStoreError, Caller, MAX_GRANTS_PER_ARTIFACT,
    audit::{Decided, artifact_target, grants_target},
    caller::{level_from_text, level_text},
    grants::GrantRow,
};

/// The most access requests one page holds.
pub const MAX_ACCESS_REQUESTS_PER_PAGE: u32 = 100;
/// A page size of an access-request listing.
pub type AccessRequestPageSize = PageSize<MAX_ACCESS_REQUESTS_PER_PAGE>;
/// One page of access requests and the cursor of the next.
pub type AccessRequestPage = Page<AccessRequestRecord, ListCursor>;

/// A decided request and the grant an approval wrote or found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecidedRequest {
    pub access_request: AccessRequestRecord,
    pub grant: Option<ArtifactGrant>,
}

struct AccessRequestRow {
    access_request_id: AccessRequestId,
    artifact_id: ArtifactId,
    requester: String,
    level: String,
    justification: String,
    state: AccessRequestState,
    decided_by: Option<String>,
    note: Option<String>,
    created_at: DateTime<Utc>,
    decided_at: Option<DateTime<Utc>>,
}

impl AccessRequestRow {
    fn record(self) -> Result<AccessRequestRecord, ArtifactStoreError> {
        let corrupt = || ArtifactStoreError::Corrupt("access request");
        Ok(AccessRequestRecord {
            access_request_id: self.access_request_id,
            artifact_id: self.artifact_id,
            requester: PrincipalId::parse(self.requester).map_err(|_| corrupt())?,
            level: level_from_text(&self.level).ok_or_else(corrupt)?,
            justification: Note::parse(self.justification).map_err(|_| corrupt())?,
            state: self.state,
            decided_by: self
                .decided_by
                .map(PrincipalId::parse)
                .transpose()
                .map_err(|_| corrupt())?,
            note: self
                .note
                .map(Note::parse)
                .transpose()
                .map_err(|_| corrupt())?,
            created_at: self.created_at,
            decided_at: self.decided_at,
        })
    }
}

impl ArtifactStore {
    /// Ask the administrators of an artifact the caller reads for a level above the one it
    /// holds. A caller holds at most one pending request per artifact.
    pub async fn request_access(
        &self,
        caller: &Caller,
        request: &RequestArtifactAccessRequest,
        now: DateTime<Utc>,
    ) -> Result<AccessRequestRecord, ArtifactStoreError> {
        let mut tx = self.pool().begin().await?;
        let visible = self
            .read_artifact(&mut tx, caller, request.artifact_id, now)
            .await?
            .ok_or(ArtifactStoreError::ArtifactNotFound)?;
        if visible.level.allows(request.level) {
            return Err(ArtifactStoreError::AlreadyHolds {
                level: visible.level,
            });
        }
        let row = sqlx::query_file_as!(
            AccessRequestRow,
            "src/store/access_requests/insert_access_request.sql",
            caller.tenant() as StoredTenant,
            AccessRequestId::new() as AccessRequestId,
            request.artifact_id as ArtifactId,
            caller.principal_key().as_str(),
            level_text(request.level),
            request.justification.as_str(),
            now,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ArtifactStoreError::AccessRequestPending)?;
        let record = row.record()?;
        self.record(
            &mut tx,
            caller,
            artifact_target(request.artifact_id),
            &AccessRequested {
                artifact_id: request.artifact_id,
                access_request_id: record.access_request_id,
                level: request.level,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Withdraw the caller's own pending request.
    pub async fn cancel_access_request(
        &self,
        caller: &Caller,
        access_request_id: AccessRequestId,
        now: DateTime<Utc>,
    ) -> Result<AccessRequestRecord, ArtifactStoreError> {
        let mut tx = self.pool().begin().await?;
        let pending = self
            .lock_request(&mut tx, caller, access_request_id)
            .await?
            .filter(|row| row.requester == caller.principal_key().as_str())
            .ok_or(ArtifactStoreError::AccessRequestNotFound)?;
        if pending.state != AccessRequestState::Pending {
            return Err(ArtifactStoreError::AccessRequestSettled {
                state: pending.state,
            });
        }
        let artifact_id = pending.artifact_id;
        let row = self
            .settle(
                &mut tx,
                caller,
                access_request_id,
                AccessRequestState::Cancelled,
                None,
                None,
                now,
            )
            .await?;
        self.record(
            &mut tx,
            caller,
            artifact_target(artifact_id),
            &AccessRequestCancelled {
                artifact_id,
                access_request_id,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        row.record()
    }

    /// Approve or deny a pending request on an artifact the caller administers. An
    /// approval gives the requester the level it asked for in the same transaction.
    pub async fn decide_access_request(
        &self,
        caller: &Caller,
        access_request_id: AccessRequestId,
        decision: AccessRequestDecision,
        note: Option<&Note>,
        now: DateTime<Utc>,
    ) -> Result<DecidedRequest, ArtifactStoreError> {
        let mut tx = self.pool().begin().await?;
        // The request names its artifact; read it unlocked to learn which one, then lock
        // the artifact before the request, the order every grant change takes.
        let artifact_id = sqlx::query_file_as!(
            AccessRequestRow,
            "src/store/access_requests/access_request.sql",
            caller.tenant() as StoredTenant,
            access_request_id as AccessRequestId,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ArtifactStoreError::AccessRequestNotFound)?
        .artifact_id;
        // A request on an artifact the caller cannot see is as unknown as the artifact.
        self.lock_artifact(&mut tx, caller, artifact_id, AccessLevel::Admin, now)
            .await
            .map_err(|error| match error {
                ArtifactStoreError::ArtifactNotFound => ArtifactStoreError::AccessRequestNotFound,
                other => other,
            })?;
        let pending = self
            .lock_request(&mut tx, caller, access_request_id)
            .await?
            .ok_or(ArtifactStoreError::AccessRequestNotFound)?;
        if pending.state != AccessRequestState::Pending {
            return Err(ArtifactStoreError::AccessRequestSettled {
                state: pending.state,
            });
        }
        let requested = pending.record()?;
        let state = match decision {
            AccessRequestDecision::Approve => AccessRequestState::Approved,
            AccessRequestDecision::Deny => AccessRequestState::Denied,
        };
        let grant = match decision {
            AccessRequestDecision::Deny => None,
            AccessRequestDecision::Approve => {
                Some(self.approve(&mut tx, caller, &requested, now).await?)
            }
        };
        let row = self
            .settle(
                &mut tx,
                caller,
                access_request_id,
                state,
                Some(caller.principal_key()),
                note,
                now,
            )
            .await?;
        self.record(
            &mut tx,
            caller,
            artifact_target(artifact_id),
            &AccessRequestDecided {
                artifact_id,
                access_request_id,
                requester: requested.requester.clone(),
                level: requested.level,
                decision,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok(DecidedRequest {
            access_request: row.record()?,
            grant,
        })
    }

    /// One page of access requests in the caller's tenant, newest first: those it made,
    /// those it can decide, or, without a role, both.
    pub async fn access_requests_page(
        &self,
        caller: &Caller,
        role: Option<AccessRequestRole>,
        state: Option<AccessRequestState>,
        cursor: Option<&ListCursor>,
        size: AccessRequestPageSize,
        now: DateTime<Utc>,
    ) -> Result<AccessRequestPage, ArtifactStoreError> {
        let f = caller.facts();
        let scope = CursorScope::new(
            "artifact.access_requests",
            f.tenant,
            &(
                f.principal_key.as_str(),
                role.map(|role| role.as_str()),
                state.map(|state| state.as_str()),
            ),
        )?;
        let after: Option<KeysetCursor<AccessRequestId>> =
            cursor.map(|wire| scope.parse(wire.as_str())).transpose()?;
        let after = after.map(|cursor| *cursor.position());
        let state = state.map(|state| state.as_str());
        let limit = size.fetch_limit();
        let pool = self.pool();
        let rows = match (role, after) {
            (Some(AccessRequestRole::Requester), None) => {
                sqlx::query_file_as!(
                    AccessRequestRow,
                    "src/store/access_requests/requested_first.sql",
                    f.tenant as StoredTenant,
                    f.principal_key,
                    state,
                    limit,
                )
                .fetch_all(pool)
                .await?
            }
            (Some(AccessRequestRole::Requester), Some(after)) => {
                sqlx::query_file_as!(
                    AccessRequestRow,
                    "src/store/access_requests/requested_next.sql",
                    f.tenant as StoredTenant,
                    f.principal_key,
                    state,
                    after as AccessRequestId,
                    limit,
                )
                .fetch_all(pool)
                .await?
            }
            (Some(AccessRequestRole::Decider), None) => {
                sqlx::query_file_as!(
                    AccessRequestRow,
                    "src/store/access_requests/decidable_first.sql",
                    f.tenant as StoredTenant,
                    f.principal as StoredPrincipal,
                    f.principal_key,
                    &f.groups,
                    &f.group_levels,
                    f.work_context,
                    f.membership,
                    &f.labels,
                    now,
                    state,
                    limit,
                )
                .fetch_all(pool)
                .await?
            }
            (Some(AccessRequestRole::Decider), Some(after)) => {
                sqlx::query_file_as!(
                    AccessRequestRow,
                    "src/store/access_requests/decidable_next.sql",
                    f.tenant as StoredTenant,
                    f.principal as StoredPrincipal,
                    f.principal_key,
                    &f.groups,
                    &f.group_levels,
                    f.work_context,
                    f.membership,
                    &f.labels,
                    now,
                    state,
                    after as AccessRequestId,
                    limit,
                )
                .fetch_all(pool)
                .await?
            }
            (None, None) => {
                sqlx::query_file_as!(
                    AccessRequestRow,
                    "src/store/access_requests/involved_first.sql",
                    f.tenant as StoredTenant,
                    f.principal as StoredPrincipal,
                    f.principal_key,
                    &f.groups,
                    &f.group_levels,
                    f.work_context,
                    f.membership,
                    &f.labels,
                    now,
                    state,
                    limit,
                )
                .fetch_all(pool)
                .await?
            }
            (None, Some(after)) => {
                sqlx::query_file_as!(
                    AccessRequestRow,
                    "src/store/access_requests/involved_next.sql",
                    f.tenant as StoredTenant,
                    f.principal as StoredPrincipal,
                    f.principal_key,
                    &f.groups,
                    &f.group_levels,
                    f.work_context,
                    f.membership,
                    &f.labels,
                    now,
                    state,
                    after as AccessRequestId,
                    limit,
                )
                .fetch_all(pool)
                .await?
            }
        };
        let page = size.page(rows, |last| last.access_request_id);
        let items = page
            .items
            .into_iter()
            .map(AccessRequestRow::record)
            .collect::<Result<_, _>>()?;
        let next = page
            .next
            .map(|position| scope.cursor(position))
            .transpose()?
            .map(|cursor| ListCursor::parse(cursor.as_str()))
            .transpose()
            .map_err(|_| ArtifactStoreError::Corrupt("issued cursor"))?;
        Ok(Page { items, next })
    }

    async fn lock_request(
        &self,
        tx: &mut sqlx::PgConnection,
        caller: &Caller,
        access_request_id: AccessRequestId,
    ) -> Result<Option<AccessRequestRow>, ArtifactStoreError> {
        Ok(sqlx::query_file_as!(
            AccessRequestRow,
            "src/store/access_requests/access_request_for_update.sql",
            caller.tenant() as StoredTenant,
            access_request_id as AccessRequestId,
        )
        .fetch_optional(&mut *tx)
        .await?)
    }

    #[allow(clippy::too_many_arguments)]
    async fn settle(
        &self,
        tx: &mut sqlx::PgConnection,
        caller: &Caller,
        access_request_id: AccessRequestId,
        state: AccessRequestState,
        decided_by: Option<&PrincipalId>,
        note: Option<&Note>,
        now: DateTime<Utc>,
    ) -> Result<AccessRequestRow, ArtifactStoreError> {
        sqlx::query_file_as!(
            AccessRequestRow,
            "src/store/access_requests/settle_access_request.sql",
            caller.tenant() as StoredTenant,
            access_request_id as AccessRequestId,
            state as AccessRequestState,
            decided_by.map(PrincipalId::as_str),
            note.map(Note::as_str),
            now,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ArtifactStoreError::AccessRequestNotFound)
    }

    /// Give the requester its requested level and record the grant change. A grant that
    /// already gives at least that level without an expiry stays as it is.
    async fn approve(
        &self,
        tx: &mut sqlx::PgConnection,
        caller: &Caller,
        requested: &AccessRequestRecord,
        now: DateTime<Utc>,
    ) -> Result<ArtifactGrant, ArtifactStoreError> {
        let tenant = caller.tenant();
        let subject = requested.requester.to_string();
        let counts = sqlx::query_file!(
            "src/store/grants/grant_count.sql",
            tenant as StoredTenant,
            requested.artifact_id as ArtifactId,
            "principal",
            subject,
        )
        .fetch_one(&mut *tx)
        .await?;
        if counts.held == 0
            && usize::try_from(counts.grants).unwrap_or(usize::MAX) >= MAX_GRANTS_PER_ARTIFACT
        {
            return Err(ArtifactStoreError::TooManyGrants(MAX_GRANTS_PER_ARTIFACT));
        }
        let wrote = sqlx::query_file_scalar!(
            "src/store/access_requests/approve_grant.sql",
            tenant as StoredTenant,
            requested.artifact_id as ArtifactId,
            subject,
            level_text(requested.level),
            caller.principal_key().as_str(),
            now,
        )
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        let grant = sqlx::query_file_as!(
            GrantRow,
            "src/store/access_requests/grant.sql",
            tenant as StoredTenant,
            requested.artifact_id as ArtifactId,
            "principal",
            subject,
        )
        .fetch_one(&mut *tx)
        .await?
        .record(requested.artifact_id)?;
        if wrote {
            self.record(
                &mut *tx,
                caller,
                grants_target(requested.artifact_id),
                &GrantChanged {
                    artifact_id: requested.artifact_id,
                    subject: AccessSubject::Principal(requested.requester.clone()),
                    level: Some(grant.level),
                    expires_at: grant.expires_at,
                },
                Decided::Succeeded,
            )
            .await?;
        }
        Ok(grant)
    }
}
