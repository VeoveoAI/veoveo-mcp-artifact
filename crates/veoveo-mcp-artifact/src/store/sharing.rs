//! Sharing beyond grants: an artifact's release state, its share links and their
//! redemption by people without an account.
use std::{num::NonZeroU32, sync::Arc};

use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};
use veoveo_audit_contract::{
    AuditDraft, AuditOutcome, AuditRegistry, AuditRequest, AuditTransactionWrite,
};
use veoveo_store::{CursorScope, KeysetCursor, Page, PageSize, TenantId as StoredTenant};
use veoveo_types::{AccessLevel, PrincipalId, Sha256Digest};

use crate::contract::{
    ArtifactId, ArtifactName, ArtifactState, ListCursor, MediaType, ReleaseChanged, ReleaseState,
    ShareLinkCreated, ShareLinkId, ShareLinkRecord, ShareLinkRedeemed, ShareLinkRevoked,
};

use super::{
    ArtifactStore, ArtifactStoreError, Caller, VisibleArtifact,
    audit::{Decided, artifact_target, share_links_target},
};

/// The most live share links one artifact holds.
pub const MAX_LIVE_SHARE_LINKS: usize = 256;
/// The most share links one page holds.
pub const MAX_SHARE_LINKS_PER_PAGE: u32 = 100;
/// A page size of a share-link listing.
pub type ShareLinkPageSize = PageSize<MAX_SHARE_LINKS_PER_PAGE>;
/// One page of an artifact's share links and the cursor of the next.
pub type ShareLinkPage = Page<ShareLinkRecord, ListCursor>;

/// A share link to record: its artifact, the digest of its secret, its expiry and its
/// optional redemption limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewShareLink {
    pub artifact_id: ArtifactId,
    pub secret_digest: Sha256Digest,
    pub expires_at: DateTime<Utc>,
    pub redemption_limit: Option<NonZeroU32>,
}

struct ShareLinkRow {
    share_link_id: ShareLinkId,
    artifact_id: ArtifactId,
    expires_at: DateTime<Utc>,
    redemption_limit: Option<i32>,
    redemptions: i32,
    revoked_at: Option<DateTime<Utc>>,
    created_by: String,
    created_at: DateTime<Utc>,
}

impl ShareLinkRow {
    fn record(self) -> Result<ShareLinkRecord, ArtifactStoreError> {
        let corrupt = || ArtifactStoreError::Corrupt("share link");
        Ok(ShareLinkRecord {
            share_link_id: self.share_link_id,
            artifact_id: self.artifact_id,
            expires_at: self.expires_at,
            redemption_limit: self
                .redemption_limit
                .map(|limit| {
                    u32::try_from(limit)
                        .ok()
                        .and_then(NonZeroU32::new)
                        .ok_or_else(corrupt)
                })
                .transpose()?,
            redemptions: u32::try_from(self.redemptions).map_err(|_| corrupt())?,
            revoked_at: self.revoked_at,
            created_by: PrincipalId::parse(self.created_by).map_err(|_| corrupt())?,
            created_at: self.created_at,
        })
    }
}

/// A redemption counted inside its transaction. The caller signs the download URL, then
/// commits, which records the redemption; dropping it counts nothing.
pub struct Redemption {
    tx: Transaction<'static, Postgres>,
    registry: Arc<AuditRegistry>,
    pub tenant: StoredTenant,
    pub artifact_id: ArtifactId,
    pub share_link_id: ShareLinkId,
    /// The link's redemptions, this one included.
    pub redemptions: u32,
    pub name: ArtifactName,
    pub media_type: MediaType,
}

impl Redemption {
    /// Record the redemption, whose download URL lives until `expires_at`, and commit.
    pub async fn commit(mut self, expires_at: DateTime<Utc>) -> Result<(), ArtifactStoreError> {
        let detail = self.registry.detail(&ShareLinkRedeemed {
            artifact_id: self.artifact_id,
            share_link_id: self.share_link_id,
            redemptions: self.redemptions,
            expires_at,
        })?;
        // The holder of a link has no account, so the record names no actor.
        let draft = AuditDraft::builder(AuditRequest::background(), detail, AuditOutcome::Success)
            .target(artifact_target(self.artifact_id))
            .build()?;
        AuditTransactionWrite::new(&self.registry, draft)?
            .append_as_server(&mut self.tx)
            .await?;
        self.tx.commit().await?;
        Ok(())
    }
}

impl ArtifactStore {
    /// Move an available artifact the caller administers between private and released.
    /// Returning it to private ends its live share links in the same transaction.
    pub async fn set_release_state(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        release_state: ReleaseState,
        now: DateTime<Utc>,
    ) -> Result<VisibleArtifact, ArtifactStoreError> {
        let tenant = caller.tenant();
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
            "src/store/sharing/set_release_state.sql",
            tenant as StoredTenant,
            artifact_id as ArtifactId,
            release_state as ReleaseState,
        )
        .fetch_one(&mut *tx)
        .await?;
        let ended = match release_state {
            ReleaseState::Released => 0,
            ReleaseState::Private => sqlx::query_file_scalar!(
                "src/store/sharing/end_share_links.sql",
                tenant as StoredTenant,
                artifact_id as ArtifactId,
                now,
            )
            .fetch_all(&mut *tx)
            .await?
            .len(),
        };
        self.record(
            &mut tx,
            caller,
            artifact_target(artifact_id),
            &ReleaseChanged {
                artifact_id,
                release_state,
                ended_share_links: u32::try_from(ended).unwrap_or(u32::MAX),
            },
            Decided::Succeeded,
        )
        .await?;
        let changed = self
            .read_artifact(&mut tx, caller, artifact_id, now)
            .await?
            .ok_or(ArtifactStoreError::ArtifactNotFound)?;
        tx.commit().await?;
        Ok(changed)
    }

    /// Record a share link to a released, available artifact without labels that the
    /// caller administers. The artifact stays locked while its live links are counted.
    pub async fn create_share_link(
        &self,
        caller: &Caller,
        link: &NewShareLink,
        now: DateTime<Utc>,
    ) -> Result<ShareLinkRecord, ArtifactStoreError> {
        let tenant = caller.tenant();
        let mut tx = self.pool().begin().await?;
        let visible = self
            .lock_artifact(&mut tx, caller, link.artifact_id, AccessLevel::Admin, now)
            .await?;
        if visible.record.state != ArtifactState::Available {
            return Err(ArtifactStoreError::ArtifactState {
                state: visible.record.state,
            });
        }
        if visible.record.release_state != ReleaseState::Released {
            return Err(ArtifactStoreError::NotReleased);
        }
        if !visible.record.labels.is_empty() {
            return Err(ArtifactStoreError::LabelledArtifact);
        }
        let live = sqlx::query_file_scalar!(
            "src/store/sharing/live_share_links.sql",
            tenant as StoredTenant,
            link.artifact_id as ArtifactId,
            now,
        )
        .fetch_one(&mut *tx)
        .await?;
        if usize::try_from(live).unwrap_or(usize::MAX) >= MAX_LIVE_SHARE_LINKS {
            return Err(ArtifactStoreError::TooManyShareLinks(MAX_LIVE_SHARE_LINKS));
        }
        let limit = link
            .redemption_limit
            .map(|limit| i32::try_from(limit.get()))
            .transpose()
            .map_err(|_| {
                ArtifactStoreError::InvalidShareLink("the redemption limit is too large")
            })?;
        let row = sqlx::query_file_as!(
            ShareLinkRow,
            "src/store/sharing/insert_share_link.sql",
            tenant as StoredTenant,
            ShareLinkId::new() as ShareLinkId,
            link.artifact_id as ArtifactId,
            link.secret_digest.to_string(),
            link.expires_at,
            limit,
            caller.principal_key().as_str(),
            now,
        )
        .fetch_one(&mut *tx)
        .await?;
        let record = row.record()?;
        self.record(
            &mut tx,
            caller,
            share_links_target(link.artifact_id),
            &ShareLinkCreated {
                artifact_id: link.artifact_id,
                share_link_id: record.share_link_id,
                expires_at: record.expires_at,
                redemption_limit: record.redemption_limit,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        Ok(record)
    }

    /// Revoke one of the share links of an artifact the caller administers. Revoking a
    /// link again returns it unchanged.
    pub async fn revoke_share_link(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        share_link_id: ShareLinkId,
        now: DateTime<Utc>,
    ) -> Result<ShareLinkRecord, ArtifactStoreError> {
        let mut tx = self.pool().begin().await?;
        self.lock_artifact(&mut tx, caller, artifact_id, AccessLevel::Admin, now)
            .await?;
        let row = sqlx::query_file_as!(
            ShareLinkRow,
            "src/store/sharing/revoke_share_link.sql",
            caller.tenant() as StoredTenant,
            artifact_id as ArtifactId,
            share_link_id as ShareLinkId,
            now,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ArtifactStoreError::ShareLinkNotFound)?;
        self.record(
            &mut tx,
            caller,
            share_links_target(artifact_id),
            &ShareLinkRevoked {
                artifact_id,
                share_link_id,
            },
            Decided::Succeeded,
        )
        .await?;
        tx.commit().await?;
        row.record()
    }

    /// One page of the share links of an artifact the caller administers, newest first.
    pub async fn share_links_page(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        cursor: Option<&ListCursor>,
        size: ShareLinkPageSize,
        now: DateTime<Utc>,
    ) -> Result<ShareLinkPage, ArtifactStoreError> {
        self.artifact_requiring(caller, artifact_id, AccessLevel::Admin, now)
            .await?;
        let tenant = caller.tenant();
        let scope = CursorScope::new("artifact.share_links", tenant, &artifact_id)?;
        let after: Option<KeysetCursor<ShareLinkId>> =
            cursor.map(|wire| scope.parse(wire.as_str())).transpose()?;
        let limit = size.fetch_limit();
        let rows = match after {
            None => {
                sqlx::query_file_as!(
                    ShareLinkRow,
                    "src/store/sharing/share_links_first.sql",
                    tenant as StoredTenant,
                    artifact_id as ArtifactId,
                    limit,
                )
                .fetch_all(self.pool())
                .await?
            }
            Some(after) => {
                sqlx::query_file_as!(
                    ShareLinkRow,
                    "src/store/sharing/share_links_next.sql",
                    tenant as StoredTenant,
                    artifact_id as ArtifactId,
                    *after.position() as ShareLinkId,
                    limit,
                )
                .fetch_all(self.pool())
                .await?
            }
        };
        let page = size.page(rows, |last| last.share_link_id);
        let items = page
            .items
            .into_iter()
            .map(ShareLinkRow::record)
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

    /// Count one redemption of the live link whose secret has `secret_digest`, when it
    /// leads to a released, available artifact without labels. `None` for every other
    /// link, so a refusal names no reason.
    pub async fn redeem_share_link(
        &self,
        secret_digest: &Sha256Digest,
        now: DateTime<Utc>,
    ) -> Result<Option<Redemption>, ArtifactStoreError> {
        let mut tx = self.pool().begin().await?;
        let Some(row) = sqlx::query_file!(
            "src/store/sharing/redeem_share_link.sql",
            secret_digest.to_string(),
            now,
        )
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(Redemption {
            tx,
            registry: self.registry.clone(),
            tenant: row.tenant,
            artifact_id: row.artifact_id,
            share_link_id: row.share_link_id,
            redemptions: u32::try_from(row.redemptions)
                .map_err(|_| ArtifactStoreError::Corrupt("share link redemptions"))?,
            name: ArtifactName::parse(row.name)
                .map_err(|_| ArtifactStoreError::Corrupt("artifact name"))?,
            media_type: MediaType::parse(row.media_type)
                .map_err(|_| ArtifactStoreError::Corrupt("artifact media type"))?,
        }))
    }
}
