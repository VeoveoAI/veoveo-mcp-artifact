//! Uploads and downloads end to end: the service signs, a client sends the bytes to a
//! private SeaweedFS, and the service completes, serves and sweeps them.
mod support;

use chrono::Utc;
use support::{
    Who,
    seaweedfs::send,
    service::{owner, with_service},
    upload_request, with_key,
};
use veoveo_mcp_artifact::{
    contract::{ArtifactState, MediaType, ObjectSize, UploadTransfer},
    service::ArtifactError,
    storage::{ObjectKey, StorageError},
    store::ArtifactStoreError,
    sweep::sweep,
};
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
            .complete_upload(&caller, created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(artifact.state, ArtifactState::Available);

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
            .complete_upload(&caller, created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(again, artifact);
    })
    .await;
}

#[tokio::test]
async fn a_small_object_is_available_when_it_completes() {
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
            .complete_upload(&caller, created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(artifact.state, ArtifactState::Available);
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
        let refused = send(put, b"short".to_vec(), &[]).await;
        assert!(refused.status().is_client_error());
        assert!(matches!(
            harness
                .service
                .complete_upload(&caller, created.upload.upload_id, Utc::now())
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
async fn a_completed_object_above_its_reservation_fails_and_removes_its_bytes() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let mut request = octets(upload_request("too-big.bin", None));
        request.max_size = Some(ObjectSize::new(5).unwrap());
        let request = checked(with_key(request, "too-big"));
        let created = harness
            .service
            .create_upload(&caller, &request, Utc::now())
            .await
            .unwrap();
        let Some(UploadTransfer::Multipart { parts, .. }) = &created.transfer else {
            panic!("an unknown size takes multipart storage");
        };
        assert!(
            send(&parts[0].transfer, vec![7; 6], &[])
                .await
                .status()
                .is_success()
        );
        let result = harness
            .service
            .complete_upload(&caller, created.upload.upload_id, Utc::now())
            .await;
        assert!(matches!(
            result,
            Err(ArtifactError::Store(ArtifactStoreError::SizeMismatch {
                received: 6
            }))
        ));
        let key = ObjectKey::for_artifact(caller.tenant(), created.artifact.artifact_id);
        assert!(harness.storage.head(&key).await.unwrap().is_none());
        let usage = harness
            .service
            .store()
            .storage_usage(&caller)
            .await
            .unwrap();
        assert_eq!((usage.stored_bytes, usage.reserved_bytes), (0, 0));
        let retained = harness
            .service
            .create_upload(&caller, &request, Utc::now())
            .await
            .unwrap();
        assert_eq!(retained.artifact.state, ArtifactState::Failed);
        assert_eq!(
            retained.upload.state,
            veoveo_mcp_artifact::contract::UploadState::Completed
        );
        assert!(
            retained.transfer.is_none(),
            "an idempotent retry cannot reopen failed bytes"
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
            .complete_upload(&caller, created.upload.upload_id, Utc::now())
            .await;
        assert!(
            matches!(refused, Err(ArtifactError::Storage(StorageError::NoParts))),
            "{refused:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn a_large_unknown_size_object_is_available_without_a_task() {
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
            .complete_upload(&caller, created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(artifact.state, ArtifactState::Available);
        assert_eq!(
            artifact.size,
            Some(ObjectSize::new((first.len() + last.len()) as u64).unwrap())
        );
        let tasks = harness
            .tasks
            .for_owner(&owner(&who))
            .page(veoveo_task::TaskPageOrder::OldestFirst, None, 10)
            .await
            .unwrap();
        assert!(
            tasks.items.is_empty(),
            "completing bytes starts no background Task"
        );
        let download = harness
            .service
            .create_download(&caller, artifact.artifact_id, Utc::now())
            .await
            .unwrap();
        let fetched = send(&download.get, Vec::new(), &[])
            .await
            .bytes()
            .await
            .unwrap();
        let mut expected = first;
        expected.extend_from_slice(&last);
        assert_eq!(fetched.as_ref(), expected.as_slice());
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
            .complete_upload(&caller, created.upload.upload_id, Utc::now())
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
            .complete_upload(&caller, deleted.upload.upload_id, Utc::now())
            .await
            .unwrap();
        service
            .delete_artifact(&caller, deleted.artifact.artifact_id, Utc::now())
            .await
            .unwrap();

        // A large completed object with no background worker.
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
        let completed = service
            .complete_upload(&caller, stale.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(completed.state, ArtifactState::Available);

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
        // Successful completed bytes remain available until explicitly deleted.
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
        assert_eq!(current.state, ArtifactState::Available);
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
