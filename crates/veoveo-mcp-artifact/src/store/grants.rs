//! Grants: giving a principal or group a level on an artifact, revoking it, and listing
//! an artifact's grants for its administrators.
use chrono::{DateTime, Utc};
use veoveo_platform_store::TenantId as StoredTenant;
use veoveo_types::{AccessLevel, AccessSubject, GroupId, PrincipalId};

use crate::contract::{ArtifactGrant, ArtifactId, GrantArtifactRequest, GrantChanged};

use super::{
    ArtifactStore, ArtifactStoreError, Caller,
    audit::{Decided, artifact_target},
    caller::{level_from_text, level_text},
};

/// The most grants one artifact holds.
pub const MAX_GRANTS_PER_ARTIFACT: usize = 256;

struct GrantRow {
    subject_kind: String,
    subject: String,
    level: String,
    expires_at: Option<DateTime<Utc>>,
    granted_by: String,
    granted_at: DateTime<Utc>,
}

impl GrantRow {
    fn record(self, artifact_id: ArtifactId) -> Result<ArtifactGrant, ArtifactStoreError> {
        let corrupt = |_| ArtifactStoreError::Corrupt("grant subject");
        let subject = match self.subject_kind.as_str() {
            "principal" => {
                AccessSubject::Principal(PrincipalId::parse(self.subject).map_err(corrupt)?)
            }
            "group" => AccessSubject::Group(GroupId::parse(self.subject).map_err(corrupt)?),
            _ => return Err(ArtifactStoreError::Corrupt("grant subject kind")),
        };
        Ok(ArtifactGrant {
            artifact_id,
            subject,
            level: level_from_text(&self.level)
                .ok_or(ArtifactStoreError::Corrupt("grant level"))?,
            expires_at: self.expires_at,
            granted_by: PrincipalId::parse(self.granted_by)
                .map_err(|_| ArtifactStoreError::Corrupt("grantor"))?,
            granted_at: self.granted_at,
        })
    }
}

fn subject_columns(subject: &AccessSubject) -> (&'static str, String) {
    match subject {
        AccessSubject::Principal(principal) => ("principal", principal.to_string()),
        AccessSubject::Group(group) => ("group", group.to_string()),
    }
}

impl ArtifactStore {
    /// Give a subject a level on an artifact the caller administers, replacing the
    /// subject's earlier grant. The artifact stays locked while the limit is checked.
    pub async fn grant_artifact(
        &self,
        caller: &Caller,
        request: &GrantArtifactRequest,
        now: DateTime<Utc>,
    ) -> Result<ArtifactGrant, ArtifactStoreError> {
        let tenant = caller.tenant();
        let (kind, subject) = subject_columns(&request.subject);
        let mut tx = self.pool().begin().await?;
        self.lock_artifact(
            &mut tx,
            caller,
            request.artifact_id,
            AccessLevel::Admin,
            now,
        )
        .await?;
        let counts = sqlx::query_file!(
            "src/store/grants/grant_count.sql",
            tenant as StoredTenant,
            request.artifact_id as ArtifactId,
            kind,
            subject,
        )
        .fetch_one(&mut *tx)
        .await?;
        if counts.held == 0
            && usize::try_from(counts.grants).unwrap_or(usize::MAX) >= MAX_GRANTS_PER_ARTIFACT
        {
            return Err(ArtifactStoreError::TooManyGrants(MAX_GRANTS_PER_ARTIFACT));
        }
        let row = sqlx::query_file_as!(
            GrantRow,
            "src/store/grants/upsert_grant.sql",
            tenant as StoredTenant,
            request.artifact_id as ArtifactId,
            kind,
            subject,
            level_text(request.level),
            request.expires_at,
            caller.principal_key().as_str(),
            now,
        )
        .fetch_one(&mut *tx)
        .await?;
        self.record(
            &mut tx,
            caller,
            artifact_target(request.artifact_id),
            &GrantChanged {
                artifact_id: request.artifact_id,
                subject: request.subject.clone(),
                level: Some(request.level),
                expires_at: request.expires_at,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        row.record(request.artifact_id)
    }

    /// Remove a subject's grant on an artifact the caller administers. False when the
    /// subject held none.
    pub async fn revoke_artifact_grant(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        subject: &AccessSubject,
        now: DateTime<Utc>,
    ) -> Result<bool, ArtifactStoreError> {
        let revoked_subject = subject.clone();
        let (kind, subject) = subject_columns(subject);
        let mut tx = self.pool().begin().await?;
        self.lock_artifact(&mut tx, caller, artifact_id, AccessLevel::Admin, now)
            .await?;
        let revoked = sqlx::query_file_scalar!(
            "src/store/grants/revoke_grant.sql",
            caller.tenant() as StoredTenant,
            artifact_id as ArtifactId,
            kind,
            subject,
        )
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        self.record(
            &mut tx,
            caller,
            artifact_target(artifact_id),
            &GrantChanged {
                artifact_id,
                subject: revoked_subject,
                level: None,
                expires_at: None,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok(revoked)
    }

    /// An artifact's grants, for a caller who administers it.
    pub async fn artifact_grants(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        now: DateTime<Utc>,
    ) -> Result<Vec<ArtifactGrant>, ArtifactStoreError> {
        self.artifact_requiring(caller, artifact_id, AccessLevel::Admin, now)
            .await?;
        let rows = sqlx::query_file_as!(
            GrantRow,
            "src/store/grants/grants.sql",
            caller.tenant() as StoredTenant,
            artifact_id as ArtifactId,
        )
        .fetch_all(self.pool())
        .await?;
        if rows.len() > MAX_GRANTS_PER_ARTIFACT {
            return Err(ArtifactStoreError::Corrupt("grant count"));
        }
        rows.into_iter()
            .map(|row| row.record(artifact_id))
            .collect()
    }
}
