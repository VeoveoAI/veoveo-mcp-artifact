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
    AuditDetailType, AuditOutcome, AuditPartition, AuditQuery, AuditReadScope, AuditRecord,
    AuditRegistry, AuditTarget,
};
use veoveo_mcp_artifact::{
    contract::{
        ArtifactDeleted, ArtifactId, ArtifactVerified, DownloadIssued, GrantArtifactRequest,
        GrantChanged, UploadAborted, UploadCompleted, UploadCreated, UploadLimits, UploadLimitsSet,
        UploadTransfer, audit_registry,
    },
    migrator, server,
    store::{ArtifactStore, ArtifactStoreError},
};
use veoveo_platform_store::PlatformStore;
use veoveo_test_fixtures::store::TestDb;
use veoveo_types::{AccessLevel, AccessSubject, Checked, PrincipalId, ResourceUri, TenantId};

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

fn artifact_target(artifact_id: ArtifactId) -> AuditTarget {
    AuditTarget::Resource {
        uri: ResourceUri::new(format!("artifact://artifacts/{artifact_id}")).unwrap(),
    }
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
        assert_eq!(draft.outcome(), AuditOutcome::Succeeded);
        assert_eq!(
            draft.target(),
            &artifact_target(created.artifact.artifact_id)
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
        let draft = |index: usize| &records[index].draft;
        let verified: ArtifactVerified =
            registry.read(draft(2).class(), draft(2).detail()).unwrap();
        assert_eq!(verified.sha256, sha256(&body));
        let download: DownloadIssued = registry.read(draft(3).class(), draft(3).detail()).unwrap();
        assert_eq!(download.artifact_id, artifact_id);
        let revoked: GrantChanged = registry.read(draft(5).class(), draft(5).detail()).unwrap();
        assert_eq!((revoked.subject, revoked.level), (grantee, None));
        let limits: UploadLimitsSet = registry.read(draft(7).class(), draft(7).detail()).unwrap();
        assert_eq!(limits.tenant.as_str(), "acme");
        assert_eq!(draft(7).target(), &AuditTarget::Server { server: server() });
        for index in [0, 1, 2, 3, 4, 5, 6] {
            assert_eq!(
                draft(index).target(),
                &artifact_target(artifact_id),
                "{index}"
            );
            assert_eq!(draft(index).outcome(), AuditOutcome::Succeeded);
        }
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
        assert_eq!(last.outcome(), AuditOutcome::Failed);
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
