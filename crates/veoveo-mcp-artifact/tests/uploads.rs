//! Uploads against the database: quota reservations under concurrent callers, the
//! open-upload limit, idempotent creates, completion with known and unknown sizes,
//! size validation, aborts, and the sweep that ends and removes what is left.
mod support;

use std::sync::Arc;

use chrono::Duration;
use support::*;
use veoveo_mcp_artifact::{
    contract::{
        ActiveUploadLimit, AllowedMediaTypes, ArtifactState, MediaType, ObjectSize, UploadForm,
        UploadLimits, UploadState,
    },
    store::{ArtifactStoreError, CompletedObject},
};
use veoveo_store::MaintenanceBatch;
use veoveo_types::{Checked, TenantId};

async fn set_limits(store: &veoveo_mcp_artifact::store::ArtifactStore, limits: UploadLimits) {
    store
        .set_upload_limits(
            &Who::new("admin").caller(),
            &TenantId::parse("acme").unwrap(),
            &limits,
            now(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn concurrent_reservations_never_exceed_the_quota() {
    with_store(|store| async move {
        set_limits(
            &store,
            UploadLimits {
                quota_bytes: Some(100),
                ..UploadLimits::default()
            },
        )
        .await;
        let store = Arc::new(store);
        let tasks: Vec<_> = (0..10)
            .map(|index| {
                let store = store.clone();
                tokio::spawn(async move {
                    let who = Who::new("owen");
                    let request = upload_request(&format!("part-{index}.txt"), Some(30));
                    store
                        .create_upload(&who.caller(), &Checked::new(request).unwrap(), now())
                        .await
                })
            })
            .collect();
        let mut admitted = 0;
        for task in tasks {
            match task.await.unwrap() {
                Ok(_) => admitted += 1,
                Err(ArtifactStoreError::QuotaExceeded { needed: 30, .. }) => {}
                Err(error) => panic!("unexpected refusal: {error}"),
            }
        }
        assert_eq!(admitted, 3);
        let usage = store
            .storage_usage(&Who::new("owen").caller())
            .await
            .unwrap();
        assert_eq!(
            (usage.stored_bytes, usage.reserved_bytes, usage.quota_bytes),
            (0, 90, Some(100))
        );
    })
    .await;
}

#[tokio::test]
async fn limits_refuse_large_objects_other_media_types_and_too_many_open_uploads() {
    with_store(|store| async move {
        set_limits(
            &store,
            UploadLimits {
                quota_bytes: None,
                largest_object: ObjectSize::new(1000).unwrap(),
                allowed_media_types: AllowedMediaTypes::Listed(
                    [MediaType::parse("text/plain").unwrap()].into(),
                ),
                active_uploads: ActiveUploadLimit::new(2).unwrap(),
            },
        )
        .await;
        let who = Who::new("owen");
        let attempt = |request| {
            let store = &store;
            let caller = who.caller();
            async move {
                store
                    .create_upload(&caller, &Checked::new(request).unwrap(), now())
                    .await
            }
        };
        assert!(matches!(
            attempt(upload_request("big.txt", Some(1001))).await,
            Err(ArtifactStoreError::TooLarge { largest: 1000 })
        ));
        let mut image = upload_request("photo.png", Some(10));
        image.media_type = MediaType::parse("image/png").unwrap();
        assert!(matches!(
            attempt(image).await,
            Err(ArtifactStoreError::MediaTypeNotAllowed(_))
        ));
        // Without a declared size the upload reserves the tenant's largest object.
        let unknown = attempt(upload_request("stream.txt", None)).await.unwrap();
        assert_eq!(unknown.upload.form, UploadForm::Multipart);
        assert_eq!(unknown.upload.reserved.bytes(), 1000);
        attempt(upload_request("second.txt", Some(5)))
            .await
            .unwrap();
        assert!(matches!(
            attempt(upload_request("third.txt", Some(5))).await,
            Err(ArtifactStoreError::TooManyOpenUploads { limit: 2 })
        ));
        // Aborting one frees a slot and its reservation.
        store
            .abort_upload(&who.caller(), unknown.upload.upload_id, now())
            .await
            .unwrap();
        attempt(upload_request("third.txt", Some(5))).await.unwrap();
        let usage = store.storage_usage(&who.caller()).await.unwrap();
        assert_eq!(usage.reserved_bytes, 10);
    })
    .await;
}

#[tokio::test]
async fn an_idempotency_key_returns_the_first_upload_for_its_acting_identity() {
    with_store(|store| async move {
        let person = Who::new("owen");
        let first = create(
            &store,
            &person,
            with_key(upload_request("out.mp4", Some(8)), "media:t1:output:0"),
        )
        .await;
        let again = create(
            &store,
            &person,
            with_key(
                upload_request("other-name.mp4", Some(9)),
                "media:t1:output:0",
            ),
        )
        .await;
        assert!(!first.repeated);
        assert!(again.repeated);
        assert_eq!(again.upload.upload_id, first.upload.upload_id);
        assert_eq!(again.artifact.name.as_str(), "out.mp4");

        // The same key from a server acting for the caller is a different identity, and
        // the server is recorded in the provenance.
        let media = Who::new("owen").acting("media");
        let by_server = create(
            &store,
            &media,
            with_key(upload_request("out.mp4", Some(8)), "media:t1:output:0"),
        )
        .await;
        assert!(!by_server.repeated);
        assert_ne!(by_server.upload.upload_id, first.upload.upload_id);
        assert_eq!(
            by_server
                .artifact
                .provenance
                .acting_server
                .as_ref()
                .map(|slug| slug.as_str()),
            Some("media")
        );
        let usage = store.storage_usage(&person.caller()).await.unwrap();
        assert_eq!(usage.reserved_bytes, 16);
    })
    .await;
}

#[tokio::test]
async fn completion_checks_sizes_and_makes_unknown_size_available() {
    with_store(|store| async move {
        let who = Who::new("owen");
        let caller = who.caller();

        // A declared size that differs fails the artifact.
        let wrong = create(&store, &who, upload_request("wrong.txt", Some(10))).await;
        let refused = store
            .complete_upload(
                &caller,
                wrong.upload.upload_id,
                &CompletedObject {
                    size: ObjectSize::new(11).unwrap(),
                },
                now(),
            )
            .await;
        assert!(matches!(
            refused,
            Err(ArtifactStoreError::SizeMismatch { received: 11 })
        ));
        let (upload, _) = store.upload(&caller, wrong.upload.upload_id).await.unwrap();
        assert_eq!(upload.state, UploadState::Completed);

        // An unknown size becomes available when its actual size fits the reservation.
        let mut stream = upload_request("stream.bin", None);
        stream.max_size = Some(ObjectSize::new(500).unwrap());
        let streamed = create(&store, &who, stream).await;
        let received = store
            .complete_upload(
                &caller,
                streamed.upload.upload_id,
                &CompletedObject {
                    size: ObjectSize::new(321).unwrap(),
                },
                now(),
            )
            .await
            .unwrap();
        assert_eq!(received.state, ArtifactState::Available);
        assert_eq!(received.size.map(ObjectSize::bytes), Some(321));
        let usage = store.storage_usage(&caller).await.unwrap();
        assert_eq!((usage.stored_bytes, usage.reserved_bytes), (321, 0));
        assert!(received.available_at.is_some());

        // A completed upload cannot complete again, and another caller cannot see it.
        let again = store
            .complete_upload(
                &caller,
                streamed.upload.upload_id,
                &CompletedObject {
                    size: ObjectSize::new(1).unwrap(),
                },
                now(),
            )
            .await;
        assert!(matches!(
            again,
            Err(ArtifactStoreError::UploadNotOpen {
                state: UploadState::Completed
            })
        ));
        assert!(matches!(
            store
                .upload(&Who::new("sam").caller(), streamed.upload.upload_id)
                .await,
            Err(ArtifactStoreError::UploadNotFound)
        ));
    })
    .await;
}

#[tokio::test]
async fn the_sweep_ends_expired_uploads_and_removes_deleted_and_failed_artifacts() {
    with_store(|store| async move {
        let who = Who::new("owen");
        let caller = who.caller();
        let batch = MaintenanceBatch::new(500).unwrap();
        let open = create(&store, &who, upload_request("left.txt", Some(10))).await;
        let kept = available(&store, &who, upload_request("kept.txt", Some(20))).await;

        // Nothing expires before its time.
        assert!(
            store
                .expire_uploads(now() + Duration::hours(23), batch)
                .await
                .unwrap()
                .is_empty()
        );
        let expired = store
            .expire_uploads(now() + Duration::hours(25), batch)
            .await
            .unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].upload_id, open.upload.upload_id);
        assert_eq!(
            store.storage_usage(&caller).await.unwrap().reserved_bytes,
            0
        );

        // Deleting marks the artifact; the deletion batch removes the row after the
        // object, and stored bytes fall with it.
        assert_eq!(store.storage_usage(&caller).await.unwrap().stored_bytes, 20);
        store.delete_artifact(&caller, kept, now()).await.unwrap();
        let mut deletions = store.claim_deletions(now(), batch).await.unwrap();
        assert_eq!(deletions.claimed.len(), 1);
        let claimed = deletions.claimed[0];
        deletions.remove(claimed).await.unwrap();
        deletions.commit().await.unwrap();
        assert_eq!(store.storage_usage(&caller).await.unwrap().stored_bytes, 0);

        // The expired upload's failed artifact and the ended uploads go after a week.
        let pruned = store.prune(now() + Duration::days(6), batch).await.unwrap();
        assert_eq!(pruned.rows, 0);
        let pruned = store.prune(now() + Duration::days(9), batch).await.unwrap();
        assert!(pruned.rows >= 1, "{pruned:?}");
        assert!(matches!(
            store.upload(&caller, open.upload.upload_id).await,
            Err(ArtifactStoreError::UploadNotFound)
        ));

        // The failed artifact goes after a week, through a batch that removes its object
        // first.
        let early = store
            .claim_failed(now() + Duration::days(6), batch)
            .await
            .unwrap();
        assert!(early.claimed.is_empty());
        early.commit().await.unwrap();
        let mut failed = store
            .claim_failed(now() + Duration::days(9), batch)
            .await
            .unwrap();
        assert_eq!(failed.claimed.len(), 1);
        assert_eq!(failed.claimed[0].artifact_id, open.artifact.artifact_id);
        let claimed = failed.claimed[0];
        failed.remove(claimed).await.unwrap();
        failed.commit().await.unwrap();
        assert!(matches!(
            store
                .artifact(&caller, open.artifact.artifact_id, now())
                .await,
            Err(ArtifactStoreError::ArtifactNotFound)
        ));
    })
    .await;
}
