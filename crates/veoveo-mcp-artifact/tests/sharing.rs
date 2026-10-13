//! Release states and share links in the store: who may release and link an artifact,
//! what returning it to private ends, how a link pages, and how its redemptions count
//! under concurrency. Records are read back through the installation owner.
mod support;

use std::{num::NonZeroU32, sync::Arc, time::Duration};

use chrono::{DateTime, TimeDelta, Utc};
use support::{Who, available, create, declared_store, now, upload_request, with_labels};
use veoveo_audit::AuditStore;
use veoveo_audit_contract::{
    AuditDetailType, AuditPartition, AuditQuery, AuditReadScope, AuditRecord,
};
use veoveo_mcp_artifact::{
    contract::{
        ArtifactId, ArtifactState, ReleaseChanged, ReleaseState, ShareLinkCreated,
        ShareLinkRedeemed, ShareLinkRevoked, audit_registry,
    },
    migrator, server,
    store::{ArtifactStore, ArtifactStoreError, NewShareLink, ShareLinkPageSize},
};
use veoveo_platform_store::PlatformStore;
use veoveo_test_fixtures::store::TestDb;
use veoveo_types::{Sha256Digest, TenantId, WorkContextMembershipLevel};

/// A share-link test with the database at hand, bounded to 60 seconds.
async fn with_database<F, Fut>(test: F)
where
    F: FnOnce(Arc<TestDb>, ArtifactStore) -> Fut,
    Fut: Future<Output = ()>,
{
    tokio::time::timeout(Duration::from_secs(60), async {
        // The database lives until the test ends, whether or not the test uses it.
        let db = Arc::new(TestDb::new().await);
        db.migrate_server(&server(), migrator()).await;
        let store = declared_store(db.server_store(&server()).await).await;
        test(db.clone(), store).await;
        drop(db);
    })
    .await
    .expect("the sharing test exceeded 60 seconds");
}

/// The records of one partition, oldest first, read as the installation owner.
async fn records(owner: PlatformStore, partition: AuditPartition) -> Vec<AuditRecord> {
    let scope = match &partition {
        AuditPartition::Tenant(tenant) => AuditReadScope::new(Some(tenant.clone()), false),
        AuditPartition::Installation => AuditReadScope::new(None, true),
    };
    let reader = AuditStore::new(owner, Arc::new(audit_registry().unwrap()));
    let mut query = AuditQuery::new(partition);
    query.limit = 1000;
    reader
        .page(&scope, &query)
        .await
        .expect("read the audit records")
        .records
}

fn of_class<T: AuditDetailType>(records: &[AuditRecord]) -> Vec<&AuditRecord> {
    records
        .iter()
        .filter(|record| record.draft.class().to_string() == T::CLASS)
        .collect()
}

fn tenant(name: &str) -> AuditPartition {
    AuditPartition::Tenant(TenantId::parse(name).unwrap())
}

/// A link to `artifact` whose secret digest is `byte` repeated, living `days` from `at`.
fn link(artifact: ArtifactId, byte: u8, at: DateTime<Utc>, days: i64) -> NewShareLink {
    NewShareLink {
        artifact_id: artifact,
        secret_digest: Sha256Digest::from_bytes([byte; 32]),
        expires_at: at + TimeDelta::days(days),
        redemption_limit: None,
    }
}

fn digest(byte: u8) -> Sha256Digest {
    Sha256Digest::from_bytes([byte; 32])
}

async fn released(store: &ArtifactStore, who: &Who, name: &str) -> ArtifactId {
    let artifact = available(store, who, upload_request(name, Some(8))).await;
    store
        .set_release_state(&who.caller(), artifact, ReleaseState::Released, now())
        .await
        .unwrap();
    artifact
}

/// Redeem and commit at once, as the share route does after it signs.
async fn redeem(store: &ArtifactStore, byte: u8, at: DateTime<Utc>) -> Option<u32> {
    let redemption = store.redeem_share_link(&digest(byte), at).await.unwrap()?;
    let redemptions = redemption.redemptions;
    redemption.commit(at + TimeDelta::minutes(5)).await.unwrap();
    Some(redemptions)
}

#[tokio::test]
async fn returning_to_private_ends_live_links_and_every_change_is_recorded() {
    with_database(|db, store| async move {
        let ada = Who::new("ada");
        let caller = ada.caller();
        let artifact = released(&store, &ada, "report.txt").await;
        let first = store
            .create_share_link(&caller, &link(artifact, 1, now(), 7), now())
            .await
            .unwrap();
        let second = store
            .create_share_link(&caller, &link(artifact, 2, now(), 7), now())
            .await
            .unwrap();
        store
            .revoke_share_link(&caller, artifact, second.share_link_id, now())
            .await
            .unwrap();
        // Revoking again keeps the first revocation.
        let again = store
            .revoke_share_link(
                &caller,
                artifact,
                second.share_link_id,
                now() + TimeDelta::hours(1),
            )
            .await
            .unwrap();
        assert_eq!(again.revoked_at, Some(now()));

        let private = store
            .set_release_state(&caller, artifact, ReleaseState::Private, now())
            .await
            .unwrap();
        assert_eq!(private.record.release_state, ReleaseState::Private);
        let page = store
            .share_links_page(
                &caller,
                artifact,
                None,
                ShareLinkPageSize::new(10).unwrap(),
                now(),
            )
            .await
            .unwrap();
        assert!(page.items.iter().all(|link| link.revoked_at.is_some()));
        // Releasing again revives no link.
        store
            .set_release_state(&caller, artifact, ReleaseState::Released, now())
            .await
            .unwrap();
        assert_eq!(redeem(&store, 1, now()).await, None);

        let records = records(db.connect().await, tenant("acme")).await;
        let changes: Vec<serde_json::Value> = of_class::<ReleaseChanged>(&records)
            .into_iter()
            .map(|record| serde_json::Value::Object(record.draft.detail().as_map().clone()))
            .collect();
        let ended: Vec<u64> = changes
            .iter()
            .map(|detail| detail["endedShareLinks"].as_u64().unwrap())
            .collect();
        // Released, private (ending the one live link), released again.
        assert_eq!(ended, [0, 1, 0], "{changes:?}");
        assert_eq!(of_class::<ShareLinkCreated>(&records).len(), 2);
        assert_eq!(of_class::<ShareLinkRevoked>(&records).len(), 2);
        assert_eq!(first.redemptions, 0);
    })
    .await;
}

#[tokio::test]
async fn a_link_needs_an_administrator_and_a_released_available_artifact_without_labels() {
    with_database(|_db, store| async move {
        let ada = Who::new("ada");
        let private = available(&store, &ada, upload_request("private.txt", Some(8))).await;
        assert!(matches!(
            store
                .create_share_link(&ada.caller(), &link(private, 1, now(), 1), now())
                .await,
            Err(ArtifactStoreError::NotReleased)
        ));

        // A contributor of the Work Context writes but does not administer.
        let artifact = released(&store, &ada, "shared.txt").await;
        let bob = Who::new("bob");
        assert!(matches!(
            store
                .create_share_link(&bob.caller(), &link(artifact, 2, now(), 1), now())
                .await,
            Err(ArtifactStoreError::Forbidden { .. })
        ));
        assert!(matches!(
            store
                .set_release_state(&bob.caller(), artifact, ReleaseState::Private, now())
                .await,
            Err(ArtifactStoreError::Forbidden { .. })
        ));

        // A label limits who may read; a link reaches people without labels.
        let cleared = Who::new("ada").labels(&["restricted"]);
        let labelled = available(
            &store,
            &cleared,
            with_labels(upload_request("restricted.txt", Some(8)), &["restricted"]),
        )
        .await;
        store
            .set_release_state(&cleared.caller(), labelled, ReleaseState::Released, now())
            .await
            .unwrap();
        assert!(matches!(
            store
                .create_share_link(&cleared.caller(), &link(labelled, 3, now(), 1), now())
                .await,
            Err(ArtifactStoreError::LabelledArtifact)
        ));

        // An artifact whose bytes have not arrived is not released or linked.
        let uploading = create(&store, &ada, upload_request("pending.txt", Some(8))).await;
        assert!(matches!(
            store
                .set_release_state(
                    &ada.caller(),
                    uploading.artifact.artifact_id,
                    ReleaseState::Released,
                    now()
                )
                .await,
            Err(ArtifactStoreError::ArtifactState {
                state: ArtifactState::Uploading
            })
        ));
    })
    .await;
}

#[tokio::test]
async fn the_redemption_limit_holds_under_concurrent_redemptions() {
    with_database(|db, store| async move {
        let ada = Who::new("ada");
        let artifact = released(&store, &ada, "limited.txt").await;
        let mut limited = link(artifact, 9, now(), 7);
        limited.redemption_limit = Some(NonZeroU32::new(3).unwrap());
        store
            .create_share_link(&ada.caller(), &limited, now())
            .await
            .unwrap();

        let redemptions = futures::future::join_all((0..12).map(|_| {
            let store = store.clone();
            async move { redeem(&store, 9, now()).await }
        }))
        .await;
        let mut counted: Vec<u32> = redemptions.into_iter().flatten().collect();
        counted.sort_unstable();
        assert_eq!(counted, [1, 2, 3]);
        let page = store
            .share_links_page(
                &ada.caller(),
                artifact,
                None,
                ShareLinkPageSize::new(10).unwrap(),
                now(),
            )
            .await
            .unwrap();
        assert_eq!(page.items[0].redemptions, 3);

        // The holder has no account, so the record names no actor and stays in the
        // installation's partition.
        let records = records(db.connect().await, AuditPartition::Installation).await;
        let redeemed = of_class::<ShareLinkRedeemed>(&records);
        assert_eq!(redeemed.len(), 3);
        assert!(redeemed.iter().all(|record| record.draft.actor().is_none()));
    })
    .await;
}

#[tokio::test]
async fn expired_revoked_deleted_and_unknown_links_redeem_nothing() {
    with_database(|_db, store| async move {
        let ada = Who::new("ada");
        let caller = ada.caller();
        let artifact = released(&store, &ada, "report.txt").await;
        // A link created two days ago for one day.
        let past = now() - TimeDelta::days(2);
        store
            .create_share_link(&caller, &link(artifact, 1, past, 1), past)
            .await
            .unwrap();
        assert_eq!(redeem(&store, 1, now()).await, None);

        let revoked = store
            .create_share_link(&caller, &link(artifact, 2, now(), 1), now())
            .await
            .unwrap();
        assert_eq!(redeem(&store, 2, now()).await, Some(1));
        store
            .revoke_share_link(&caller, artifact, revoked.share_link_id, now())
            .await
            .unwrap();
        assert_eq!(redeem(&store, 2, now()).await, None);

        store
            .create_share_link(&caller, &link(artifact, 3, now(), 1), now())
            .await
            .unwrap();
        assert_eq!(
            redeem(&store, 3, now() + TimeDelta::hours(1)).await,
            Some(1)
        );
        store
            .delete_artifact(&caller, artifact, now())
            .await
            .unwrap();
        assert_eq!(redeem(&store, 3, now() + TimeDelta::hours(1)).await, None);
        assert_eq!(redeem(&store, 4, now()).await, None);

        // A dropped redemption counts nothing.
        let kept = released(&store, &ada, "kept.txt").await;
        let mut once = link(kept, 5, now(), 1);
        once.redemption_limit = Some(NonZeroU32::new(1).unwrap());
        store
            .create_share_link(&caller, &once, now())
            .await
            .unwrap();
        drop(store.redeem_share_link(&digest(5), now()).await.unwrap());
        assert_eq!(redeem(&store, 5, now()).await, Some(1));
    })
    .await;
}

#[tokio::test]
async fn share_links_page_newest_first_for_administrators() {
    with_database(|_db, store| async move {
        let ada = Who::new("ada");
        let caller = ada.caller();
        let artifact = released(&store, &ada, "report.txt").await;
        let mut created = Vec::new();
        for byte in 1..=3 {
            created.push(
                store
                    .create_share_link(&caller, &link(artifact, byte, now(), 1), now())
                    .await
                    .unwrap()
                    .share_link_id,
            );
        }
        let size = ShareLinkPageSize::new(2).unwrap();
        let first = store
            .share_links_page(&caller, artifact, None, size, now())
            .await
            .unwrap();
        let ids: Vec<_> = first.items.iter().map(|link| link.share_link_id).collect();
        assert_eq!(ids, [created[2], created[1]]);
        let cursor = first.next.expect("a second page");
        let second = store
            .share_links_page(&caller, artifact, Some(&cursor), size, now())
            .await
            .unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].share_link_id, created[0]);
        assert!(second.next.is_none());

        // A cursor belongs to its artifact's listing.
        let other = released(&store, &ada, "other.txt").await;
        assert!(matches!(
            store
                .share_links_page(&caller, other, Some(&cursor), size, now())
                .await,
            Err(ArtifactStoreError::Cursor(_))
        ));
        // A reader who does not administer the artifact sees no links.
        let viewer = Who::new("vera").in_context("ops", WorkContextMembershipLevel::Viewer);
        assert!(matches!(
            store
                .share_links_page(&viewer.caller(), artifact, None, size, now())
                .await,
            Err(ArtifactStoreError::Forbidden { .. })
        ));
    })
    .await;
}
