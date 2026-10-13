//! Audit records of Artifact's decisions: each commits with its decision or neither
//! does, and each carries one of Artifact's declared classes. Records are read back
//! through the installation owner, who alone reads the audit log.
mod support;

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use chrono::Utc;
use support::{
    Who, create, declared_store, now,
    rustfs::send,
    service::{owner, sha256, with_service},
    upload_request,
};
use veoveo_audit::AuditStore;
use veoveo_audit_contract::{
    AuditDetailType, AuditOutcome, AuditPartition, AuditQuery, AuditReadScope, AuditReason,
    AuditRecord, AuditRegistry, AuditTarget, AuditVia,
};
use veoveo_mcp_artifact::{
    contract::{
        AccessRequestCancelled, AccessRequestDecided, AccessRequestDecision, AccessRequested,
        ArtifactDeleted, ArtifactId, ArtifactVerified, CreateShareLinkRequest,
        DecideAccessRequestRequest, DownloadIssued, GrantArtifactRequest, GrantChanged, Note,
        ReleaseChanged, ReleaseState, RequestArtifactAccessRequest, ShareLinkCreated,
        ShareLinkDays, ShareLinkRedeemed, ShareLinkRevoked, UploadAborted, UploadCompleted,
        UploadCreated, UploadId, UploadLimits, UploadLimitsSet, UploadTransfer, audit_registry,
    },
    migrator, server,
    store::{ArtifactStore, ArtifactStoreError, CompletedObject},
};
use veoveo_platform_store::PlatformStore;
use veoveo_test_fixtures::store::TestDb;
use veoveo_types::{AccessLevel, AccessSubject, Checked, PrincipalId, ServerSlug, TenantId};

/// Every record of `tenant`, oldest first, read as the installation owner.
async fn records(owner: PlatformStore, tenant: &str) -> Vec<AuditRecord> {
    let tenant = TenantId::parse(tenant).unwrap();
    let reader = AuditStore::new(owner, Arc::new(audit_registry().unwrap()));
    let mut query = AuditQuery::new(AuditPartition::Tenant(tenant.clone()));
    query.limit = 1000;
    reader
        .page(&AuditReadScope::new(Some(tenant), false), &query)
        .await
        .expect("read the tenant's audit records")
        .records
}

fn target(uri: String) -> AuditTarget {
    AuditTarget::parse(uri).unwrap()
}

fn artifact_target(artifact_id: ArtifactId) -> AuditTarget {
    target(format!("artifact://artifacts/{artifact_id}"))
}

fn upload_target(upload_id: UploadId) -> AuditTarget {
    target(format!("artifact://uploads/{upload_id}"))
}

fn grants_target(artifact_id: ArtifactId) -> AuditTarget {
    target(format!("artifact://artifacts/{artifact_id}/grants"))
}

fn share_links_target(artifact_id: ArtifactId) -> AuditTarget {
    target(format!("artifact://artifacts/{artifact_id}/share-links"))
}

/// A store test with the database at hand, bounded to 60 seconds.
async fn with_database<F, Fut>(test: F)
where
    F: FnOnce(TestDb) -> Fut,
    Fut: Future<Output = ()>,
{
    tokio::time::timeout(Duration::from_secs(60), async {
        let db = TestDb::new().await;
        db.migrate_server(&server(), migrator()).await;
        test(db).await;
    })
    .await
    .expect("the audit test exceeded 60 seconds");
}

#[tokio::test]
async fn a_decision_and_its_record_commit_together() {
    with_database(|db| async move {
        let store = declared_store(db.server_store(&server()).await).await;
        let who = Who::new("ada");
        let created = create(&store, &who, upload_request("a.txt", Some(5))).await;

        let records = records(db.connect().await, "acme").await;
        assert_eq!(records.len(), 1, "{records:?}");
        let draft = &records[0].draft;
        assert_eq!(draft.class().to_string(), UploadCreated::CLASS);
        assert_eq!(draft.outcome(), AuditOutcome::Success);
        assert_eq!(draft.reason(), None);
        assert_eq!(draft.via(), None);
        assert_eq!(
            draft.target(),
            Some(&upload_target(created.upload.upload_id))
        );
        assert_eq!(
            draft.actor().map(|actor| &actor.principal),
            Some(&PrincipalId::parse("ada").unwrap())
        );
        assert_eq!(
            draft.partition(),
            &AuditPartition::Tenant(TenantId::parse("acme").unwrap())
        );
        let detail: UploadCreated = audit_registry()
            .unwrap()
            .read(draft.class(), draft.detail())
            .unwrap();
        assert_eq!(detail.artifact_id, created.artifact.artifact_id);
        assert_eq!(detail.upload_id, created.upload.upload_id);
        assert_eq!(detail.reserved, created.upload.reserved);
    })
    .await;
}

#[tokio::test]
async fn a_decision_rolls_back_when_its_record_fails() {
    with_database(|db| async move {
        let platform = db.server_store(&server()).await;
        let who = Who::new("ada");
        let caller = who.caller();

        // Artifact's classes are not declared yet, so the database refuses the record
        // and the reservation goes with it.
        let undeclared = ArtifactStore::new(platform.clone(), Arc::new(audit_registry().unwrap()));
        let refused = undeclared
            .create_upload(
                &caller,
                &Checked::new(upload_request("lost.txt", Some(5))).unwrap(),
                now(),
            )
            .await;
        assert!(
            matches!(refused, Err(ArtifactStoreError::AuditAppend(_))),
            "{refused:?}"
        );
        assert_eq!(
            undeclared
                .storage_usage(&caller)
                .await
                .unwrap()
                .reserved_bytes,
            0
        );

        // An abort whose record the registry cannot admit leaves the upload open.
        let store = declared_store(platform.clone()).await;
        let created = create(&store, &who, upload_request("kept.txt", Some(7))).await;
        let without_artifact = ArtifactStore::new(platform, Arc::new(AuditRegistry::platform()));
        let refused = without_artifact
            .abort_upload(&caller, created.upload.upload_id, now())
            .await;
        assert!(
            matches!(refused, Err(ArtifactStoreError::AuditDetail(_))),
            "{refused:?}"
        );
        let (upload, _) = store
            .upload(&caller, created.upload.upload_id)
            .await
            .unwrap();
        assert_eq!(
            upload.state,
            veoveo_mcp_artifact::contract::UploadState::Open
        );
        assert_eq!(
            store.storage_usage(&caller).await.unwrap().reserved_bytes,
            7
        );

        let classes: Vec<String> = records(db.connect().await, "acme")
            .await
            .iter()
            .map(|record| record.draft.class().to_string())
            .collect();
        assert_eq!(
            classes,
            [UploadCreated::CLASS],
            "only the committed decision"
        );
    })
    .await;
}

#[tokio::test]
async fn every_decision_records_its_declared_class() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let service = &harness.service;

        // A small upload without a declared digest: created, completed and verified.
        let body = b"report".to_vec();
        let created = service
            .create_upload(
                &caller,
                &Checked::new(upload_request("r.txt", Some(body.len() as u64))).unwrap(),
                Utc::now(),
            )
            .await
            .unwrap();
        let Some(UploadTransfer::Single { put }) = &created.transfer else {
            panic!("one PUT");
        };
        assert!(send(put, body.clone(), &[]).await.status().is_success());
        let artifact = service
            .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
            .await
            .unwrap();
        assert_eq!(artifact.sha256, Some(sha256(&body)));
        let artifact_id = artifact.artifact_id;

        service
            .create_download(&caller, artifact_id, Utc::now())
            .await
            .unwrap();
        let grantee = AccessSubject::Principal(PrincipalId::parse("bob").unwrap());
        service
            .grant_artifact(
                &caller,
                &GrantArtifactRequest {
                    artifact_id,
                    subject: grantee.clone(),
                    level: AccessLevel::Read,
                    expires_at: None,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        service
            .revoke_artifact_grant(&caller, artifact_id, &grantee, Utc::now())
            .await
            .unwrap();

        // Sharing: a release, a link redeemed and revoked, an approved request and a
        // withdrawn one.
        service
            .set_release_state(&caller, artifact_id, ReleaseState::Released, Utc::now())
            .await
            .unwrap();
        let link = service
            .create_share_link(
                &caller,
                &CreateShareLinkRequest {
                    artifact_id,
                    expires_in_days: ShareLinkDays::new(1).unwrap(),
                    redemption_limit: None,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        let token = link.url.expose().rsplit('/').next().unwrap().to_owned();
        assert!(
            service
                .redeem_share_link(&token, Utc::now())
                .await
                .unwrap()
                .is_some()
        );
        service
            .revoke_share_link(
                &caller,
                artifact_id,
                link.share_link.share_link_id,
                Utc::now(),
            )
            .await
            .unwrap();
        let ask = |who: &str| RequestArtifactAccessRequest {
            artifact_id,
            level: AccessLevel::Admin,
            justification: Note::parse(format!("{who} reviews it")).unwrap(),
        };
        let bob = Who::new("bob").caller();
        let approved = service
            .request_access(&bob, &ask("bob"), Utc::now())
            .await
            .unwrap();
        service
            .decide_access_request(
                &caller,
                &DecideAccessRequestRequest {
                    access_request_id: approved.access_request_id,
                    decision: AccessRequestDecision::Approve,
                    note: None,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        let cy = Who::new("cy").caller();
        let withdrawn = service
            .request_access(&cy, &ask("cy"), Utc::now())
            .await
            .unwrap();
        service
            .cancel_access_request(&cy, withdrawn.access_request_id, Utc::now())
            .await
            .unwrap();

        service
            .delete_artifact(&caller, artifact_id, Utc::now())
            .await
            .unwrap();
        service
            .set_upload_limits(
                &caller,
                &TenantId::parse("acme").unwrap(),
                &UploadLimits::default(),
                Utc::now(),
            )
            .await
            .unwrap();
        let abandoned = service
            .create_upload(
                &caller,
                &Checked::new(upload_request("left.txt", Some(3))).unwrap(),
                Utc::now(),
            )
            .await
            .unwrap();
        service
            .abort_upload(&caller, abandoned.upload.upload_id, Utc::now())
            .await
            .unwrap();

        let records = records(harness.db.connect().await, "acme").await;
        let registry = audit_registry().unwrap();
        let classes: Vec<String> = records
            .iter()
            .map(|record| record.draft.class().to_string())
            .collect();
        assert_eq!(
            classes,
            [
                UploadCreated::CLASS,
                UploadCompleted::CLASS,
                ArtifactVerified::CLASS,
                DownloadIssued::CLASS,
                GrantChanged::CLASS,
                GrantChanged::CLASS,
                ReleaseChanged::CLASS,
                ShareLinkCreated::CLASS,
                ShareLinkRevoked::CLASS,
                AccessRequested::CLASS,
                GrantChanged::CLASS,
                AccessRequestDecided::CLASS,
                AccessRequested::CLASS,
                AccessRequestCancelled::CLASS,
                ArtifactDeleted::CLASS,
                UploadLimitsSet::CLASS,
                UploadCreated::CLASS,
                UploadAborted::CLASS,
            ]
        );
        let declared: BTreeSet<String> = registry
            .declarations()
            .map(|declaration| declaration.class().to_string())
            .filter(|class| class.starts_with("artifact."))
            .collect();
        assert!(classes.iter().all(|class| declared.contains(class)));
        assert_eq!(declared.len(), 15);

        // Every detail reads back as its type, and names the artifact it is about.
        let of = |class: &str| -> Vec<&AuditRecord> {
            records
                .iter()
                .filter(|record| record.draft.class().to_string() == class)
                .collect()
        };
        let read =
            |record: &AuditRecord| (record.draft.class().clone(), record.draft.detail().clone());
        let (class, detail) = read(of(ArtifactVerified::CLASS)[0]);
        let verified: ArtifactVerified = registry.read(&class, &detail).unwrap();
        assert_eq!(verified.sha256, sha256(&body));
        let (class, detail) = read(of(DownloadIssued::CLASS)[0]);
        let download: DownloadIssued = registry.read(&class, &detail).unwrap();
        assert_eq!(download.artifact_id, artifact_id);
        let grants = of(GrantChanged::CLASS);
        let (class, detail) = read(grants[1]);
        let revoked: GrantChanged = registry.read(&class, &detail).unwrap();
        assert_eq!((revoked.subject, revoked.level), (grantee, None));
        let (class, detail) = read(grants[2]);
        let approval: GrantChanged = registry.read(&class, &detail).unwrap();
        assert_eq!(approval.level, Some(AccessLevel::Admin));
        let (class, detail) = read(of(AccessRequestDecided::CLASS)[0]);
        let decided: AccessRequestDecided = registry.read(&class, &detail).unwrap();
        assert_eq!(decided.decision, AccessRequestDecision::Approve);
        let limits = of(UploadLimitsSet::CLASS)[0];
        let (class, detail) = read(limits);
        let set: UploadLimitsSet = registry.read(&class, &detail).unwrap();
        assert_eq!(set.tenant.as_str(), "acme");
        // Each record names the resource it concerns, and every decision succeeded.
        let upload = created.upload.upload_id;
        let artifact = artifact_target(artifact_id);
        let grants_of = grants_target(artifact_id);
        let links = share_links_target(artifact_id);
        let expected = [
            upload_target(upload),
            upload_target(upload),
            artifact.clone(),
            artifact.clone(),
            grants_of.clone(),
            grants_of.clone(),
            artifact.clone(),
            links.clone(),
            links,
            artifact.clone(),
            grants_of,
            artifact.clone(),
            artifact.clone(),
            artifact.clone(),
            artifact,
            target("artifact://storage-usage".into()),
            upload_target(abandoned.upload.upload_id),
            upload_target(abandoned.upload.upload_id),
        ];
        assert_eq!(records.len(), expected.len());
        for (record, expected) in records.iter().zip(&expected) {
            assert_eq!(
                record.draft.target(),
                Some(expected),
                "{}",
                record.draft.class()
            );
            assert_eq!(record.draft.outcome(), AuditOutcome::Success);
            assert_eq!(record.draft.reason(), None);
            assert_eq!(record.draft.via(), None);
        }

        // The holder of a link has no account, so its redemption is the installation's.
        let reader = AuditStore::new(
            harness.db.connect().await,
            Arc::new(audit_registry().unwrap()),
        );
        let mut query = AuditQuery::new(AuditPartition::Installation);
        query.limit = 1000;
        let installation = reader
            .page(&AuditReadScope::new(None, true), &query)
            .await
            .unwrap()
            .records;
        let redeemed: Vec<_> = installation
            .iter()
            .filter(|record| record.draft.class().to_string() == ShareLinkRedeemed::CLASS)
            .collect();
        assert_eq!(redeemed.len(), 1);
        assert!(redeemed[0].draft.actor().is_none());
        assert_eq!(
            redeemed[0].draft.target(),
            Some(&artifact_target(artifact_id))
        );
    })
    .await;
}

#[tokio::test]
async fn a_digest_mismatch_records_a_failed_verification() {
    with_service(|harness| async move {
        let who = Who::new("ada");
        let caller = who.caller();
        let mut request = upload_request("wrong.bin", None);
        request.sha256 = Some(sha256(b"other bytes"));
        request.max_size =
            Some(veoveo_mcp_artifact::contract::ObjectSize::new(6 * 1024 * 1024).unwrap());
        let created = harness
            .service
            .create_upload(&caller, &Checked::new(request).unwrap(), Utc::now())
            .await
            .unwrap();
        let Some(UploadTransfer::Multipart { parts, .. }) = &created.transfer else {
            panic!("parts");
        };
        assert!(
            send(&parts[0].transfer, vec![1_u8; 100], &[])
                .await
                .status()
                .is_success()
        );
        assert!(
            harness
                .service
                .complete_upload(&caller, &owner(&who), created.upload.upload_id, Utc::now())
                .await
                .is_err()
        );
        let records = records(harness.db.connect().await, "acme").await;
        let last = &records.last().expect("records").draft;
        assert_eq!(last.class().to_string(), ArtifactVerified::CLASS);
        assert_eq!(last.outcome(), AuditOutcome::Failure);
        assert_eq!(
            last.reason(),
            Some(&AuditReason::parse("digest_mismatch").unwrap())
        );
        let detail: ArtifactVerified = audit_registry()
            .unwrap()
            .read(last.class(), last.detail())
            .unwrap();
        assert_eq!(
            detail.state,
            veoveo_mcp_artifact::contract::ArtifactState::Failed
        );
    })
    .await;
}

#[tokio::test]
async fn a_size_that_differs_records_a_failed_completion() {
    with_database(|db| async move {
        let store = declared_store(db.server_store(&server()).await).await;
        let who = Who::new("ada");
        let created = create(&store, &who, upload_request("wrong.txt", Some(10))).await;
        let refused = store
            .complete_upload(
                &who.caller(),
                created.upload.upload_id,
                &CompletedObject {
                    size: veoveo_mcp_artifact::contract::ObjectSize::new(11).unwrap(),
                    checked_sha256: None,
                },
                now(),
            )
            .await;
        assert!(matches!(
            refused,
            Err(ArtifactStoreError::SizeMismatch { received: 11 })
        ));

        let records = records(db.connect().await, "acme").await;
        let last = &records.last().expect("records").draft;
        assert_eq!(last.class().to_string(), UploadCompleted::CLASS);
        assert_eq!(last.outcome(), AuditOutcome::Failure);
        assert_eq!(
            last.reason(),
            Some(&AuditReason::parse("size_mismatch").unwrap())
        );
        assert_eq!(
            last.target(),
            Some(&upload_target(created.upload.upload_id))
        );
    })
    .await;
}

#[tokio::test]
async fn a_decision_another_server_asked_for_names_it_as_via() {
    with_database(|db| async move {
        let store = declared_store(db.server_store(&server()).await).await;
        let who = Who::new("ada").acting("media");
        let created = create(&store, &who, upload_request("for-media.txt", Some(5))).await;

        let records = records(db.connect().await, "acme").await;
        assert_eq!(records.len(), 1, "{records:?}");
        let draft = &records[0].draft;
        assert_eq!(
            draft.via(),
            Some(&AuditVia {
                server: ServerSlug::parse("media").unwrap(),
                task: None,
            })
        );
        assert_eq!(
            draft.actor().map(|actor| &actor.principal),
            Some(&PrincipalId::parse("ada").unwrap())
        );
        // The detail carries Artifact's facts only; the envelope names the acting server.
        let detail = draft.detail().as_map();
        assert!(!detail.contains_key("actingServer"), "{detail:?}");
        assert_eq!(
            created.artifact.provenance.acting_server,
            Some(ServerSlug::parse("media").unwrap())
        );
    })
    .await;
}
