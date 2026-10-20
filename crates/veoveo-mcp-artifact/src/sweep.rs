//! The sweep every replica runs. `FOR UPDATE SKIP LOCKED` hands each row to one replica,
//! and each batch commits on its own, so replicas share the work without coordinating.
//! A round ends expired uploads, deletes artifacts past their retention, removes the
//! objects and rows of deleted and failed artifacts, and prunes ended records.
use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;
use veoveo_store::MaintenanceBatch;

use crate::{
    service::{ArtifactError, ArtifactService},
    storage::ObjectKey,
    store::DeletionBatch,
};

/// How often a replica sweeps: every 30 seconds.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// Rows one statement handles at most.
const BATCH: u16 = 500;

/// Sweep every [`SWEEP_INTERVAL`] until `stop` is cancelled. A failed round is logged
/// and the next round tries again.
pub async fn run_sweep(service: Arc<ArtifactService>, stop: CancellationToken) {
    let mut ticks = tokio::time::interval(SWEEP_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = ticks.tick() => {}
        }
        if let Err(error) = sweep(&service, Utc::now()).await {
            tracing::warn!(%error, "an Artifact sweep round failed");
        }
    }
}

/// One sweep round at `now`.
pub async fn sweep(service: &ArtifactService, now: DateTime<Utc>) -> Result<(), ArtifactError> {
    let batch = MaintenanceBatch::new(BATCH).expect("a batch of 500 is admitted");
    let store = service.store();
    let storage = service.storage();
    loop {
        let expired = store.expire_uploads(now, batch).await?;
        for upload in &expired {
            let key = ObjectKey::for_artifact(upload.tenant, upload.artifact_id);
            if let Some(object_upload) = &upload.object_upload {
                storage.abort_multipart(&key, object_upload).await?;
            }
            storage.delete(&key).await?;
        }
        if expired.len() < BATCH as usize {
            break;
        }
    }
    store.delete_expired_artifacts(now, batch).await?;
    loop {
        let removed = remove_objects(service, store.claim_deletions(now, batch).await?).await?;
        if removed < BATCH as usize {
            break;
        }
    }
    loop {
        let removed = remove_objects(service, store.claim_failed(now, batch).await?).await?;
        if removed < BATCH as usize {
            break;
        }
    }
    store.prune(now, batch).await?;
    Ok(())
}

/// Delete each claimed artifact's object, then its row, and commit. An object storage
/// failure leaves the whole batch for the next round.
async fn remove_objects(
    service: &ArtifactService,
    mut batch: DeletionBatch,
) -> Result<usize, ArtifactError> {
    let claimed = batch.claimed.clone();
    for deletion in &claimed {
        service
            .storage()
            .delete(&ObjectKey::for_artifact(
                deletion.tenant,
                deletion.artifact_id,
            ))
            .await?;
        batch.remove(*deletion).await?;
    }
    batch.commit().await?;
    Ok(claimed.len())
}
