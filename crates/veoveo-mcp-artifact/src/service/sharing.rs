//! Sharing: release states, share links with their redemption, and access requests.
use chrono::{DateTime, TimeDelta, Utc};

use crate::{
    contract::{
        AccessRequestId, AccessRequestRecord, AccessRequestResult, ArtifactId, ArtifactRecord,
        CreateShareLinkRequest, CreateShareLinkResult, DecideAccessRequestRequest, ReleaseState,
        RequestArtifactAccessRequest, ShareLinkId, ShareLinkRecord, SignedTransfer,
    },
    share::ShareLinkSecret,
    storage::ObjectKey,
    store::{Caller, NewShareLink},
};

use super::{ArtifactError, ArtifactService};

impl ArtifactService {
    /// Move an available artifact the caller administers between private and released.
    pub async fn set_release_state(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        release_state: ReleaseState,
        now: DateTime<Utc>,
    ) -> Result<ArtifactRecord, ArtifactError> {
        Ok(self
            .store
            .set_release_state(caller, artifact_id, release_state, now)
            .await?
            .record)
    }

    /// Issue a share link to a released artifact the caller administers. The URL holds
    /// the link's secret; the server keeps only its digest, so this is the one time
    /// anyone sees it.
    pub async fn create_share_link(
        &self,
        caller: &Caller,
        request: &CreateShareLinkRequest,
        now: DateTime<Utc>,
    ) -> Result<CreateShareLinkResult, ArtifactError> {
        let secret = ShareLinkSecret::generate().map_err(ArtifactError::Random)?;
        let share_link = self
            .store
            .create_share_link(
                caller,
                &NewShareLink {
                    artifact_id: request.artifact_id,
                    secret_digest: secret.digest(),
                    expires_at: now + TimeDelta::days(i64::from(request.expires_in_days.days())),
                    redemption_limit: request.redemption_limit,
                },
                now,
            )
            .await?;
        Ok(CreateShareLinkResult {
            share_link,
            url: self.share_base.url(&secret),
        })
    }

    pub async fn revoke_share_link(
        &self,
        caller: &Caller,
        artifact_id: ArtifactId,
        share_link_id: ShareLinkId,
        now: DateTime<Utc>,
    ) -> Result<ShareLinkRecord, ArtifactError> {
        Ok(self
            .store
            .revoke_share_link(caller, artifact_id, share_link_id, now)
            .await?)
    }

    /// Redeem the share link `token` names: count the redemption, sign a GET of its
    /// artifact and record both. `None` when the token names no live link.
    pub async fn redeem_share_link(
        &self,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<SignedTransfer>, ArtifactError> {
        let Some(secret) = ShareLinkSecret::parse(token) else {
            return Ok(None);
        };
        let Some(redemption) = self.store.redeem_share_link(&secret.digest(), now).await? else {
            return Ok(None);
        };
        let key = ObjectKey::for_artifact(redemption.tenant, redemption.artifact_id);
        let get = self
            .storage
            .sign_get(&key, &redemption.name, &redemption.media_type, now)
            .await?;
        redemption.commit(get.expires_at).await?;
        Ok(Some(get))
    }

    pub async fn request_access(
        &self,
        caller: &Caller,
        request: &RequestArtifactAccessRequest,
        now: DateTime<Utc>,
    ) -> Result<AccessRequestRecord, ArtifactError> {
        Ok(self.store.request_access(caller, request, now).await?)
    }

    pub async fn cancel_access_request(
        &self,
        caller: &Caller,
        access_request_id: AccessRequestId,
        now: DateTime<Utc>,
    ) -> Result<AccessRequestRecord, ArtifactError> {
        Ok(self
            .store
            .cancel_access_request(caller, access_request_id, now)
            .await?)
    }

    /// Approve, which writes the requested grant, or deny a request on an artifact the
    /// caller administers.
    pub async fn decide_access_request(
        &self,
        caller: &Caller,
        request: &DecideAccessRequestRequest,
        now: DateTime<Utc>,
    ) -> Result<AccessRequestResult, ArtifactError> {
        let decided = self
            .store
            .decide_access_request(
                caller,
                request.access_request_id,
                request.decision,
                request.note.as_ref(),
                now,
            )
            .await?;
        Ok(AccessRequestResult {
            access_request: decided.access_request,
            grant: decided.grant,
        })
    }
}
