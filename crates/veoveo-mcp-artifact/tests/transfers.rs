//! Uploads and downloads end to end: the service signs, a client sends the bytes to a
//! private SeaweedFS, and the service completes, verifies, serves and sweeps them.
mod support;

use std::time::Duration;

use chrono::Utc;
use support::{
    Who,
    seaweedfs::send,
    service::{owner, sha256, with_service},
    upload_request, with_key,
};
use tokio_util::sync::CancellationToken;
use veoveo_mcp_artifact::{
    contract::{ArtifactState, MediaType, ObjectSize, UploadTransfer},
    server,
    service::{ArtifactError, run_verifications},
    storage::{ObjectKey, StorageError},
    store::ArtifactStoreError,
    sweep::sweep,
};
use veoveo_platform_store::PlatformStore;
use veoveo_types::Checked;

fn checked(
    request: veoveo_mcp_artifact::contract::CreateUploadRequest,
) -> Checked<veoveo_mcp_artifact::contract::CreateUploadRequest> {
    Checked::new(request).unwrap()
}

fn declared(
    mut request: veoveo_mcp_artifact::contract::CreateUploadRequest,
    bytes: &[u8],
) -> veoveo_mcp_artifact::contract::CreateUploadRequest {
    request.size = Some(ObjectSize::new(bytes.len() as u64).unwrap());
    request.sha256 = Some(sha256(bytes));
    request
}

fn octets(
    mut request: veoveo_mcp_artifact::contract::CreateUploadRequest,
) -> veoveo_mcp_artifact::contract::CreateUploadRequest {
    request.media_type = MediaType::parse("application/octet-stream").unwrap();
    request
}

#[tokio::test]
async fn a_declared_single_upload_is_available_and_downloads() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let body = b"a report".to_vec();
        let created = harness
            .service
            .create_upload(
                &caller,
                &checked(declared(upload_request("r.txt", None), &body)),
                Utc::now(),
            )
            .await
            .unwrap();
        let Some(UploadTransfer::Single { put }) = &created.transfer else {
            panic!("a small declared upload takes one PUT");
        };
        assert!(send(put, body.clone(), &[]).await.status().is_success());
        let artifact = harness
            .service
            .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(artifact.state, ArtifactState::Available);
        assert_eq!(artifact.sha256, Some(sha256(&body)));

        let download = harness
            .service
            .create_download(&caller, artifact.artifact_id, Utc::now())
            .await
            .unwrap();
        let fetched = send(&download.get, Vec::new(), &[]).await;
        assert!(fetched.status().is_success());
        assert_eq!(fetched.bytes().await.unwrap().as_ref(), body.as_slice());
        let usage = harness
            .service
            .store()
            .storage_usage(&caller)
            .await
            .unwrap();
        assert_eq!(usage.stored_bytes, body.len() as u64);
        assert_eq!(usage.reserved_bytes, 0);

        // Completing again returns the artifact.
        let again = harness
            .service
            .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(again.state, ArtifactState::Available);
    })
    .await;
}

#[tokio::test]
async fn an_undeclared_small_object_is_hashed_when_it_completes() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let body = vec![9_u8; 3000];
        let mut request = upload_request("blob.bin", Some(body.len() as u64));
        request = octets(request);
        let created = harness
            .service
            .create_upload(&caller, &checked(request), Utc::now())
            .await
            .unwrap();
        let Some(UploadTransfer::Single { put }) = &created.transfer else {
            panic!("one PUT");
        };
        assert!(!put.headers.contains_key("x-amz-checksum-sha256"));
        assert!(send(put, body.clone(), &[]).await.status().is_success());
        let artifact = harness
            .service
            .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(artifact.state, ArtifactState::Available);
        assert_eq!(artifact.sha256, Some(sha256(&body)));
    })
    .await;
}

#[tokio::test]
async fn a_completion_without_bytes_is_refused_and_an_abort_releases_the_reservation() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let body = b"expected".to_vec();
        let created = harness
            .service
            .create_upload(
                &caller,
                &checked(declared(upload_request("e.txt", None), &body)),
                Utc::now(),
            )
            .await
            .unwrap();
        let Some(UploadTransfer::Single { put }) = &created.transfer else {
            panic!("one PUT");
        };
        let refused = send(put, b"tampered".to_vec(), &[]).await;
        assert!(refused.status().is_client_error());
        assert!(matches!(
            harness
                .service
                .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
                .await,
            Err(ArtifactError::NoBytes)
        ));
        assert_eq!(
            harness
                .service
                .store()
                .storage_usage(&caller)
                .await
                .unwrap()
                .reserved_bytes,
            body.len() as u64
        );
        harness
            .service
            .abort_upload(&caller, created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(
            harness
                .service
                .store()
                .storage_usage(&caller)
                .await
                .unwrap()
                .reserved_bytes,
            0
        );
    })
    .await;
}

#[tokio::test]
async fn a_part_shorter_than_its_bound_length_is_refused() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let mut request = octets(upload_request("big.bin", None));
        request.size = Some(ObjectSize::new(150 * 1024 * 1024).unwrap());
        let created = harness
            .service
            .create_upload(&caller, &checked(request), Utc::now())
            .await
            .unwrap();
        let Some(UploadTransfer::Multipart { parts, .. }) = &created.transfer else {
            panic!("a large declared upload takes parts");
        };
        assert_eq!(parts.len(), 10, "150 MiB in 16 MiB parts");
        // The first part's length is bound, so a short part is refused; a client that
        // sends nothing leaves the upload without parts.
        let short = send(&parts[0].transfer, vec![0; 1024], &[]).await;
        assert!(short.status().is_client_error(), "{}", short.status());
        let refused = harness
            .service
            .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
            .await;
        assert!(
            matches!(refused, Err(ArtifactError::Storage(StorageError::NoParts))),
            "{refused:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn a_large_object_of_unknown_size_verifies_in_a_task() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let stop = CancellationToken::new();
        let worker = tokio::spawn(run_verifications(harness.service.clone(), stop.clone()));
        let created = harness
            .service
            .create_upload(
                &caller,
                &checked(octets(upload_request("video.bin", None))),
                Utc::now(),
            )
            .await
            .unwrap();
        let Some(UploadTransfer::Multipart { parts, .. }) = &created.transfer else {
            panic!("an unknown size takes parts");
        };
        let first = vec![1_u8; 5 * 1024 * 1024];
        let last = vec![2_u8; 4 * 1024 * 1024];
        assert!(
            send(&parts[0].transfer, first.clone(), &[])
                .await
                .status()
                .is_success()
        );
        assert!(
            send(&parts[1].transfer, last.clone(), &[])
                .await
                .status()
                .is_success()
        );
        let artifact = harness
            .service
            .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(artifact.state, ArtifactState::Verifying);
        let mut whole = first;
        whole.extend_from_slice(&last);
        let verified = loop {
            let current = harness
                .service
                .artifact(&caller, artifact.artifact_id, Utc::now())
                .await
                .unwrap();
            if current.state != ArtifactState::Verifying {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(verified.state, ArtifactState::Available);
        assert_eq!(verified.sha256, Some(sha256(&whole)));
        assert_eq!(
            verified.size,
            Some(ObjectSize::new(whole.len() as u64).unwrap())
        );
        stop.cancel();
        worker.await.unwrap().unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_verification_a_lost_connection_interrupts_runs_again_from_its_recovery() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let created = harness
            .service
            .create_upload(
                &caller,
                &checked(octets(upload_request("video.bin", None))),
                Utc::now(),
            )
            .await
            .unwrap();
        let Some(UploadTransfer::Multipart { parts, .. }) = &created.transfer else {
            panic!("an unknown size takes parts");
        };
        let first = vec![3_u8; 5 * 1024 * 1024];
        let last = vec![4_u8; 4 * 1024 * 1024];
        for (part, bytes) in [(&parts[0], &first), (&parts[1], &last)] {
            assert!(
                send(&part.transfer, bytes.clone(), &[])
                    .await
                    .status()
                    .is_success()
            );
        }
        let artifact = harness
            .service
            .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(artifact.state, ArtifactState::Verifying);

        // Another login of Artifact's group holds the artifact's row, so the verification
        // waits on its digest write with the statement in flight.
        let locker = harness.db.server_store(&server()).await;
        let mut lock = locker.pool().begin().await.unwrap();
        sqlx::query_file_scalar!(
            "tests/transfers/lock_artifact.sql",
            artifact.artifact_id.as_uuid()
        )
        .fetch_one(&mut *lock)
        .await
        .unwrap();
        let stop = CancellationToken::new();
        let worker = tokio::spawn(run_verifications(harness.service.clone(), stop.clone()));
        let interrupted = lock_waiter(&locker, None).await;

        // Every session of Artifact's login ends, the waiting one with them. The worker
        // gives the verification to its recovery, which queues it again, and the worker
        // claims it once more on a new connection.
        assert!(harness.db.end_sessions(&harness.platform).await > 0);
        lock_waiter(&locker, Some(interrupted)).await;
        lock.rollback().await.unwrap();
        let verified = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let current = harness
                    .service
                    .artifact(&caller, artifact.artifact_id, Utc::now())
                    .await
                    .unwrap();
                if current.state != ArtifactState::Verifying {
                    break current;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("the verification ran again within a minute");
        let mut whole = first;
        whole.extend_from_slice(&last);
        assert_eq!(verified.state, ArtifactState::Available);
        assert_eq!(verified.sha256, Some(sha256(&whole)));
        assert!(!worker.is_finished(), "the worker kept running");
        stop.cancel();
        worker.await.unwrap().unwrap();
        locker.close().await;
    })
    .await;
}

/// The process ID of a session waiting on a lock, other than `other_than`.
async fn lock_waiter(store: &PlatformStore, other_than: Option<i32>) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let waiting = sqlx::query_file_scalar!("tests/transfers/lock_waiters.sql")
            .fetch_all(store.pool())
            .await
            .unwrap();
        if let Some(pid) = waiting.into_iter().find(|pid| Some(*pid) != other_than) {
            return pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no verification waited on the artifact's row within a minute"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_multipart_object_that_misses_its_declared_digest_fails_and_is_removed() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let mut request = octets(upload_request("small.bin", None));
        request.sha256 = Some(sha256(b"other bytes"));
        request.max_size = Some(ObjectSize::new(6 * 1024 * 1024).unwrap());
        let created = harness
            .service
            .create_upload(&caller, &checked(request), Utc::now())
            .await
            .unwrap();
        let Some(UploadTransfer::Multipart { parts, .. }) = &created.transfer else {
            panic!("an unknown size takes parts");
        };
        assert!(
            send(&parts[0].transfer, vec![5_u8; 1000], &[])
                .await
                .status()
                .is_success()
        );
        assert!(matches!(
            harness
                .service
                .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
                .await,
            Err(ArtifactError::DigestMismatch)
        ));
        let key = ObjectKey::for_artifact(caller.tenant(), created.artifact.artifact_id);
        assert!(harness.storage.head(&key).await.unwrap().is_none());
        assert_eq!(
            harness
                .service
                .store()
                .storage_usage(&caller)
                .await
                .unwrap()
                .stored_bytes,
            0
        );
    })
    .await;
}

#[tokio::test]
async fn a_repeated_key_returns_an_ended_upload_without_a_transfer() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let body = b"once".to_vec();
        let request = with_key(
            declared(upload_request("once.txt", None), &body),
            "output-1",
        );
        let created = harness
            .service
            .create_upload(&caller, &checked(request.clone()), Utc::now())
            .await
            .unwrap();
        let Some(UploadTransfer::Single { put }) = &created.transfer else {
            panic!("one PUT");
        };
        assert!(send(put, body, &[]).await.status().is_success());
        harness
            .service
            .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        let repeated = harness
            .service
            .create_upload(&caller, &checked(request), Utc::now())
            .await
            .unwrap();
        assert_eq!(repeated.upload.upload_id, created.upload.upload_id);
        assert!(repeated.transfer.is_none());
        assert_eq!(repeated.artifact.state, ArtifactState::Available);
    })
    .await;
}

#[tokio::test]
async fn downloads_need_an_available_artifact_the_caller_reads() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let created = harness
            .service
            .create_upload(
                &caller,
                &checked(upload_request("later.txt", Some(5))),
                Utc::now(),
            )
            .await
            .unwrap();
        assert!(matches!(
            harness
                .service
                .create_download(&caller, created.artifact.artifact_id, Utc::now())
                .await,
            Err(ArtifactError::Store(
                ArtifactStoreError::ArtifactState { .. }
            ))
        ));
        let stranger = Who::new("eve").tenant("other").caller();
        assert!(matches!(
            harness
                .service
                .create_download(&stranger, created.artifact.artifact_id, Utc::now())
                .await,
            Err(ArtifactError::Store(ArtifactStoreError::ArtifactNotFound))
        ));
    })
    .await;
}

#[tokio::test]
async fn the_sweep_removes_objects_of_expired_deleted_and_stale_artifacts() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let service = &harness.service;

        // An upload whose bytes arrived but which never completed.
        let left = b"left behind".to_vec();
        let abandoned = service
            .create_upload(
                &caller,
                &checked(declared(upload_request("left.txt", None), &left)),
                Utc::now(),
            )
            .await
            .unwrap();
        let Some(UploadTransfer::Single { put }) = &abandoned.transfer else {
            panic!("one PUT");
        };
        assert!(send(put, left, &[]).await.status().is_success());

        // An available artifact that is then deleted.
        let kept = b"kept".to_vec();
        let deleted = service
            .create_upload(
                &caller,
                &checked(declared(upload_request("kept.txt", None), &kept)),
                Utc::now(),
            )
            .await
            .unwrap();
        let Some(UploadTransfer::Single { put }) = &deleted.transfer else {
            panic!("one PUT");
        };
        assert!(send(put, kept, &[]).await.status().is_success());
        service
            .complete_upload(&caller, &owner(&who), deleted.upload.upload_id, Utc::now())
            .await
            .unwrap();
        service
            .delete_artifact(&caller, deleted.artifact.artifact_id, Utc::now())
            .await
            .unwrap();

        // A large object that verifies with no worker running: its Task stays queued.
        let stale = service
            .create_upload(
                &caller,
                &checked(octets(upload_request("stale.bin", None))),
                Utc::now(),
            )
            .await
            .unwrap();
        let Some(UploadTransfer::Multipart { parts, .. }) = &stale.transfer else {
            panic!("parts");
        };
        assert!(
            send(&parts[0].transfer, vec![3_u8; 9 * 1024 * 1024], &[])
                .await
                .status()
                .is_success()
        );
        let verifying = service
            .complete_upload(&caller, &owner(&who), stale.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(verifying.state, ArtifactState::Verifying);

        let key = |artifact| ObjectKey::for_artifact(caller.tenant(), artifact);
        sweep(service, Utc::now() + chrono::Duration::hours(25))
            .await
            .unwrap();
        assert!(
            harness
                .storage
                .head(&key(abandoned.artifact.artifact_id))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            harness
                .storage
                .head(&key(deleted.artifact.artifact_id))
                .await
                .unwrap()
                .is_none()
        );
        // Its Task is queued, so the verification stands.
        assert!(
            harness
                .storage
                .head(&key(stale.artifact.artifact_id))
                .await
                .unwrap()
                .is_some()
        );
        let current = service
            .artifact(&caller, stale.artifact.artifact_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(current.state, ArtifactState::Verifying);
        assert_eq!(
            service
                .store()
                .storage_usage(&caller)
                .await
                .unwrap()
                .reserved_bytes,
            0
        );

        // The failed artifact of the abandoned upload goes after a week.
        sweep(service, Utc::now() + chrono::Duration::days(9))
            .await
            .unwrap();
        assert!(matches!(
            service
                .artifact(&caller, abandoned.artifact.artifact_id, Utc::now())
                .await,
            Err(ArtifactError::Store(ArtifactStoreError::ArtifactNotFound))
        ));
    })
    .await;
}

#[tokio::test]
async fn the_sweep_fails_a_verification_whose_task_never_started() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let service = &harness.service;
        let created = service
            .create_upload(
                &caller,
                &checked(octets(upload_request("orphan.bin", None))),
                Utc::now(),
            )
            .await
            .unwrap();
        let Some(UploadTransfer::Multipart { parts, .. }) = &created.transfer else {
            panic!("parts");
        };
        let bytes = vec![4_u8; 9 * 1024 * 1024];
        assert!(
            send(&parts[0].transfer, bytes.clone(), &[])
                .await
                .status()
                .is_success()
        );
        let key = ObjectKey::for_artifact(caller.tenant(), created.artifact.artifact_id);
        let object = object_upload(&harness, &caller, created.upload.upload_id).await;
        harness
            .storage
            .complete_multipart(&key, &object)
            .await
            .unwrap();
        // The process stops after the store records the bytes and before it queues the
        // verification.
        let completed = service
            .store()
            .complete_upload(
                &caller,
                created.upload.upload_id,
                &veoveo_mcp_artifact::store::CompletedObject {
                    size: ObjectSize::new(bytes.len() as u64).unwrap(),
                    checked_sha256: None,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        assert_eq!(completed.state, ArtifactState::Verifying);

        sweep(service, Utc::now() + chrono::Duration::minutes(5))
            .await
            .unwrap();
        assert!(
            harness.storage.head(&key).await.unwrap().is_some(),
            "inside the deadline"
        );
        sweep(service, Utc::now() + chrono::Duration::minutes(20))
            .await
            .unwrap();
        assert!(harness.storage.head(&key).await.unwrap().is_none());
        assert_eq!(
            service
                .store()
                .storage_usage(&caller)
                .await
                .unwrap()
                .stored_bytes,
            0
        );
    })
    .await;
}

/// The object store's identifier of a multipart upload the service started.
async fn object_upload(
    harness: &support::service::Harness,
    caller: &veoveo_mcp_artifact::store::Caller,
    upload_id: veoveo_mcp_artifact::contract::UploadId,
) -> String {
    let (_, object) = harness
        .service
        .store()
        .upload(caller, upload_id)
        .await
        .unwrap();
    object.expect("a multipart upload records its object upload")
}
