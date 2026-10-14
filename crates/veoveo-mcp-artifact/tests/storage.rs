//! Object storage against a private SeaweedFS: signed single and multipart uploads, the
//! headers each signature binds, create-only writes, ranged downloads and deletion.
mod support;

use std::{future::Future, time::Duration};

use chrono::Utc;
use sha2::{Digest as _, Sha256};
use support::seaweedfs::{TestSeaweedFs, send};
use veoveo_mcp_artifact::{
    contract::{ArtifactId, ArtifactName, MediaType, ObjectSize, PartNumber},
    storage::{ObjectKey, ObjectStorage},
};
use veoveo_types::Sha256Digest;

async fn with_storage<F, Fut>(test: F)
where
    F: FnOnce(ObjectStorage) -> Fut,
    Fut: Future<Output = ()>,
{
    tokio::time::timeout(Duration::from_secs(120), async {
        let server = TestSeaweedFs::start().await;
        let storage = server.storage().await;
        test(storage).await;
        drop(server);
    })
    .await
    .expect("the storage test exceeded 120 seconds");
}

fn key() -> ObjectKey {
    let tenant = veoveo_platform_store::deterministic_tenant_id("acme").unwrap();
    ObjectKey::for_artifact(tenant, ArtifactId::new())
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", hex::encode(Sha256::digest(bytes)))).unwrap()
}

fn media() -> MediaType {
    MediaType::parse("application/octet-stream").unwrap()
}

fn size(bytes: usize) -> ObjectSize {
    ObjectSize::new(bytes as u64).unwrap()
}

#[tokio::test]
async fn a_single_put_binds_its_size_type_and_declared_sha256() {
    with_storage(|storage| async move {
        let body = b"hello artifact".to_vec();
        let key = key();
        let put = storage
            .sign_put(
                &key,
                size(body.len()),
                &media(),
                Some(&digest(&body)),
                Utc::now(),
            )
            .await
            .unwrap();
        for header in ["content-type", "if-none-match", "x-amz-checksum-sha256"] {
            assert!(put.headers.contains_key(header), "{header} is signed");
        }
        let signed = put.url.as_str();
        assert!(signed.contains("X-Amz-SignedHeaders="));
        assert!(signed.contains("content-length"));
        let response = send(&put, body.clone(), &[]).await;
        assert!(response.status().is_success(), "{}", response.status());
        let stored = storage.head(&key).await.unwrap().unwrap();
        assert_eq!(stored.size, size(body.len()));
        assert_eq!(stored.sha256, Some(digest(&body)));

        let again = storage
            .sign_put(&key, size(body.len()), &media(), None, Utc::now())
            .await
            .unwrap();
        let replaced = send(&again, body, &[]).await;
        assert_eq!(replaced.status().as_u16(), 412, "writes are create-only");
    })
    .await;
}

#[tokio::test]
async fn a_single_put_without_a_declared_sha256_stores_the_bytes() {
    with_storage(|storage| async move {
        let body = vec![7_u8; 4096];
        let key = key();
        let put = storage
            .sign_put(&key, size(body.len()), &media(), None, Utc::now())
            .await
            .unwrap();
        assert!(!put.headers.contains_key("x-amz-checksum-sha256"));
        assert!(send(&put, body.clone(), &[]).await.status().is_success());
        let (computed, length) = storage.sha256(&key).await.unwrap();
        assert_eq!(computed, digest(&body));
        assert_eq!(length, body.len() as u64);
    })
    .await;
}

#[tokio::test]
async fn object_storage_refuses_a_wrong_size_or_checksum() {
    with_storage(|storage| async move {
        let body = b"expected bytes".to_vec();
        let key = key();
        let put = storage
            .sign_put(
                &key,
                size(body.len()),
                &media(),
                Some(&digest(&body)),
                Utc::now(),
            )
            .await
            .unwrap();
        let shorter = send(&put, b"short".to_vec(), &[]).await;
        assert!(shorter.status().is_client_error(), "{}", shorter.status());
        let tampered = send(&put, b"tampered bytes".to_vec(), &[]).await;
        assert!(tampered.status().is_client_error(), "{}", tampered.status());
        assert!(storage.head(&key).await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
async fn a_multipart_upload_of_unknown_size_completes_create_only() {
    with_storage(|storage| async move {
        let key = key();
        let upload = storage.start_multipart(&key, &media()).await.unwrap();
        let first = vec![1_u8; 5 * 1024 * 1024];
        let last = vec![2_u8; 1234];
        let one = PartNumber::new(1).unwrap();
        let two = PartNumber::new(2).unwrap();
        let first_part = storage
            .sign_part(
                &key,
                &upload,
                one,
                Some(size(first.len())),
                Some(&digest(&first)),
                Utc::now(),
            )
            .await
            .unwrap();
        let response = send(&first_part, first.clone(), &[]).await;
        assert!(response.status().is_success(), "{}", response.status());
        let last_part = storage
            .sign_part(&key, &upload, two, None, None, Utc::now())
            .await
            .unwrap();
        let response = send(&last_part, last.clone(), &[]).await;
        assert!(response.status().is_success(), "{}", response.status());
        storage.complete_multipart(&key, &upload).await.unwrap();
        storage.complete_multipart(&key, &upload).await.unwrap();
        let stored = storage.head(&key).await.unwrap().unwrap();
        assert_eq!(stored.size, size(first.len() + last.len()));
        let mut whole = first;
        whole.extend_from_slice(&last);
        assert_eq!(storage.sha256(&key).await.unwrap().0, digest(&whole));
    })
    .await;
}

#[tokio::test]
async fn a_part_with_a_wrong_declared_checksum_is_refused() {
    with_storage(|storage| async move {
        let key = key();
        let upload = storage.start_multipart(&key, &media()).await.unwrap();
        let bytes = vec![3_u8; 1024];
        let part = storage
            .sign_part(
                &key,
                &upload,
                PartNumber::new(1).unwrap(),
                Some(size(bytes.len())),
                Some(&digest(b"other bytes")),
                Utc::now(),
            )
            .await
            .unwrap();
        let response = send(&part, bytes, &[]).await;
        assert!(response.status().is_client_error(), "{}", response.status());
        storage.abort_multipart(&key, &upload).await.unwrap();
        storage.abort_multipart(&key, &upload).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn downloads_serve_ranges_with_an_attachment_disposition() {
    with_storage(|storage| async move {
        let body: Vec<u8> = (0..=255).collect();
        let key = key();
        let put = storage
            .sign_put(&key, size(body.len()), &media(), None, Utc::now())
            .await
            .unwrap();
        assert!(send(&put, body.clone(), &[]).await.status().is_success());
        let name = ArtifactName::parse("bytes.bin").unwrap();
        let get = storage
            .sign_get(&key, &name, &media(), Utc::now())
            .await
            .unwrap();
        let ranged = send(&get, Vec::new(), &[("range", "bytes=10-19")]).await;
        assert_eq!(ranged.status().as_u16(), 206);
        let disposition = ranged
            .headers()
            .get("content-disposition")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        assert!(disposition.starts_with("attachment;"), "{disposition}");
        assert_eq!(ranged.bytes().await.unwrap().as_ref(), &body[10..20]);

        storage.delete(&key).await.unwrap();
        storage.delete(&key).await.unwrap();
        assert!(storage.head(&key).await.unwrap().is_none());
    })
    .await;
}

#[tokio::test]
async fn preparing_a_bucket_twice_changes_nothing() {
    with_storage(|storage| async move {
        storage.prepare().await.unwrap();
        assert!(storage.ready().await);
    })
    .await;
}
