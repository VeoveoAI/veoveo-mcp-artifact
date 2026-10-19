//! Access requests in the store: who may ask for which level, who may withdraw and
//! decide a request, the grant an approval writes, the records each step leaves and the
//! listings by role and state. Ada owns the artifacts in Work Context `ops`, where Vera
//! is a viewer; Carol works in another Work Context and cannot see them.
mod support;

use std::{sync::Arc, time::Duration};

use support::{Who, available, declared_store, now, upload_request};
use veoveo_audit::AuditStore;
use veoveo_audit_contract::{
    AuditDetailType, AuditPartition, AuditQuery, AuditReadScope, AuditRecord,
};
use veoveo_fixtures::store::TestDb;
use veoveo_mcp_artifact::{
    contract::{
        AccessRequestCancelled, AccessRequestDecided, AccessRequestDecision, AccessRequestRole,
        AccessRequestState, AccessRequested, ArtifactId, GrantArtifactRequest, GrantChanged, Note,
        RequestArtifactAccessRequest, audit_registry,
    },
    migrator, server,
    store::{ArtifactStore, ArtifactStoreError},
};
use veoveo_store::PlatformStore;
use veoveo_types::{AccessLevel, AccessSubject, PrincipalId, TenantId, WorkContextMembershipLevel};

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
    .expect("the access request test exceeded 60 seconds");
}

async fn classes(owner: PlatformStore) -> Vec<String> {
    let tenant = TenantId::parse("acme").unwrap();
    let reader = AuditStore::new(owner, Arc::new(audit_registry().unwrap()));
    let mut query = AuditQuery::new(AuditPartition::Tenant(tenant.clone()));
    query.limit = 1000;
    let records: Vec<AuditRecord> = reader
        .page(&AuditReadScope::new(Some(tenant), false), &query)
        .await
        .unwrap()
        .records;
    records
        .iter()
        .map(|record| record.draft.class().to_string())
        .collect()
}

fn ada() -> Who {
    Who::new("ada")
}

fn vera() -> Who {
    Who::new("vera").in_context("ops", WorkContextMembershipLevel::Viewer)
}

fn carol() -> Who {
    Who::new("carol").in_context("research", WorkContextMembershipLevel::Contributor)
}

fn asking(artifact: ArtifactId, level: AccessLevel) -> RequestArtifactAccessRequest {
    RequestArtifactAccessRequest {
        artifact_id: artifact,
        level,
        justification: Note::parse("I review this report").unwrap(),
    }
}

async fn level(store: &ArtifactStore, who: &Who, artifact: ArtifactId) -> Option<AccessLevel> {
    support::level(store, who, artifact, now()).await
}

#[tokio::test]
async fn an_approval_gives_the_requested_level_and_records_each_step() {
    with_database(|db, store| async move {
        let artifact = available(&store, &ada(), upload_request("report.txt", Some(8))).await;
        assert_eq!(
            level(&store, &vera(), artifact).await,
            Some(AccessLevel::Read)
        );
        let pending = store
            .request_access(
                &vera().caller(),
                &asking(artifact, AccessLevel::Write),
                now(),
            )
            .await
            .unwrap();
        assert_eq!(pending.state, AccessRequestState::Pending);
        assert_eq!(pending.requester, PrincipalId::parse("vera").unwrap());
        assert!(matches!(
            store
                .request_access(
                    &vera().caller(),
                    &asking(artifact, AccessLevel::Admin),
                    now()
                )
                .await,
            Err(ArtifactStoreError::AccessRequestPending)
        ));

        let decided = store
            .decide_access_request(
                &ada().caller(),
                pending.access_request_id,
                AccessRequestDecision::Approve,
                Some(&Note::parse("for the audit").unwrap()),
                now(),
            )
            .await
            .unwrap();
        assert_eq!(decided.access_request.state, AccessRequestState::Approved);
        assert_eq!(
            decided.access_request.decided_by,
            Some(PrincipalId::parse("ada").unwrap())
        );
        let grant = decided.grant.expect("an approval writes a grant");
        assert_eq!(
            grant.subject,
            AccessSubject::Principal(PrincipalId::parse("vera").unwrap())
        );
        assert_eq!(grant.level, AccessLevel::Write);
        assert_eq!(grant.expires_at, None);
        assert_eq!(
            level(&store, &vera(), artifact).await,
            Some(AccessLevel::Write)
        );
        // A settled request takes no second decision.
        assert!(matches!(
            store
                .decide_access_request(
                    &ada().caller(),
                    pending.access_request_id,
                    AccessRequestDecision::Deny,
                    None,
                    now(),
                )
                .await,
            Err(ArtifactStoreError::AccessRequestSettled {
                state: AccessRequestState::Approved
            })
        ));
        let recorded = classes(db.connect().await).await;
        for class in [
            AccessRequested::CLASS,
            AccessRequestDecided::CLASS,
            GrantChanged::CLASS,
        ] {
            assert!(
                recorded.iter().any(|seen| seen == class),
                "{class} in {recorded:?}"
            );
        }
    })
    .await;
}

#[tokio::test]
async fn a_request_needs_a_visible_artifact_and_a_level_above_the_one_held() {
    with_database(|_db, store| async move {
        let artifact = available(&store, &ada(), upload_request("report.txt", Some(8))).await;
        // Carol cannot see the artifact, so it is as unknown to her as an unused ID.
        assert!(matches!(
            store
                .request_access(
                    &carol().caller(),
                    &asking(artifact, AccessLevel::Read),
                    now()
                )
                .await,
            Err(ArtifactStoreError::ArtifactNotFound)
        ));
        assert!(matches!(
            store
                .request_access(
                    &vera().caller(),
                    &asking(artifact, AccessLevel::Read),
                    now()
                )
                .await,
            Err(ArtifactStoreError::AlreadyHolds {
                level: AccessLevel::Read
            })
        ));
        assert!(matches!(
            store
                .request_access(
                    &ada().caller(),
                    &asking(artifact, AccessLevel::Admin),
                    now()
                )
                .await,
            Err(ArtifactStoreError::AlreadyHolds {
                level: AccessLevel::Admin
            })
        ));
    })
    .await;
}

#[tokio::test]
async fn only_the_requester_withdraws_and_only_an_administrator_decides() {
    with_database(|db, store| async move {
        let artifact = available(&store, &ada(), upload_request("report.txt", Some(8))).await;
        let pending = store
            .request_access(
                &vera().caller(),
                &asking(artifact, AccessLevel::Write),
                now(),
            )
            .await
            .unwrap();
        // Another principal of the Work Context neither withdraws nor decides it.
        let other = Who::new("otto").in_context("ops", WorkContextMembershipLevel::Viewer);
        assert!(matches!(
            store
                .cancel_access_request(&other.caller(), pending.access_request_id, now())
                .await,
            Err(ArtifactStoreError::AccessRequestNotFound)
        ));
        assert!(matches!(
            store
                .decide_access_request(
                    &vera().caller(),
                    pending.access_request_id,
                    AccessRequestDecision::Approve,
                    None,
                    now(),
                )
                .await,
            Err(ArtifactStoreError::Forbidden {
                needed: AccessLevel::Admin
            })
        ));
        assert!(matches!(
            store
                .decide_access_request(
                    &carol().caller(),
                    pending.access_request_id,
                    AccessRequestDecision::Approve,
                    None,
                    now(),
                )
                .await,
            Err(ArtifactStoreError::AccessRequestNotFound)
        ));
        let cancelled = store
            .cancel_access_request(&vera().caller(), pending.access_request_id, now())
            .await
            .unwrap();
        assert_eq!(cancelled.state, AccessRequestState::Cancelled);
        assert!(matches!(
            store
                .cancel_access_request(&vera().caller(), pending.access_request_id, now())
                .await,
            Err(ArtifactStoreError::AccessRequestSettled { .. })
        ));

        // A denial writes no grant.
        let again = store
            .request_access(
                &vera().caller(),
                &asking(artifact, AccessLevel::Write),
                now(),
            )
            .await
            .unwrap();
        let denied = store
            .decide_access_request(
                &ada().caller(),
                again.access_request_id,
                AccessRequestDecision::Deny,
                Some(&Note::parse("not for this report").unwrap()),
                now(),
            )
            .await
            .unwrap();
        assert_eq!(denied.access_request.state, AccessRequestState::Denied);
        assert_eq!(
            denied
                .access_request
                .note
                .map(|note| note.as_str().to_owned()),
            Some("not for this report".to_owned())
        );
        assert!(denied.grant.is_none());
        assert_eq!(
            level(&store, &vera(), artifact).await,
            Some(AccessLevel::Read)
        );
        let recorded = classes(db.connect().await).await;
        assert!(
            recorded
                .iter()
                .any(|class| class == AccessRequestCancelled::CLASS)
        );
        assert!(!recorded.iter().any(|class| class == GrantChanged::CLASS));
    })
    .await;
}

#[tokio::test]
async fn an_approval_keeps_a_stronger_grant_the_requester_received_meanwhile() {
    with_database(|_db, store| async move {
        let artifact = available(&store, &ada(), upload_request("report.txt", Some(8))).await;
        let pending = store
            .request_access(
                &vera().caller(),
                &asking(artifact, AccessLevel::Write),
                now(),
            )
            .await
            .unwrap();
        store
            .grant_artifact(
                &ada().caller(),
                &GrantArtifactRequest {
                    artifact_id: artifact,
                    subject: AccessSubject::Principal(PrincipalId::parse("vera").unwrap()),
                    level: AccessLevel::Admin,
                    expires_at: None,
                },
                now(),
            )
            .await
            .unwrap();
        let decided = store
            .decide_access_request(
                &ada().caller(),
                pending.access_request_id,
                AccessRequestDecision::Approve,
                None,
                now(),
            )
            .await
            .unwrap();
        assert_eq!(
            decided.grant.map(|grant| grant.level),
            Some(AccessLevel::Admin)
        );
        assert_eq!(
            level(&store, &vera(), artifact).await,
            Some(AccessLevel::Admin)
        );
    })
    .await;
}

#[tokio::test]
async fn listings_page_newest_first_by_role_and_state() {
    with_database(|_db, store| async move {
        let mut artifacts = Vec::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            artifacts.push(available(&store, &ada(), upload_request(name, Some(8))).await);
        }
        let mut requests = Vec::new();
        for artifact in &artifacts {
            requests.push(
                store
                    .request_access(
                        &vera().caller(),
                        &asking(*artifact, AccessLevel::Write),
                        now(),
                    )
                    .await
                    .unwrap()
                    .access_request_id,
            );
        }
        store
            .cancel_access_request(&vera().caller(), requests[0], now())
            .await
            .unwrap();
        // Vera owns an artifact of her own, and Otto asks her for access to it.
        let contributor = vera().in_context("ops", WorkContextMembershipLevel::Contributor);
        let veras = available(&store, &contributor, upload_request("v.txt", Some(8))).await;
        let otto = Who::new("otto").in_context("ops", WorkContextMembershipLevel::Viewer);
        let ottos_request = store
            .request_access(&otto.caller(), &asking(veras, AccessLevel::Write), now())
            .await
            .unwrap()
            .access_request_id;

        let size = veoveo_types::PageSize::<100>::new(2).unwrap();
        let ids =
            |page: &veoveo_types::Page<veoveo_mcp_artifact::contract::AccessRequests>| -> Vec<_> {
                page.items()
                    .iter()
                    .map(|request| request.access_request_id)
                    .collect()
            };
        let made = store
            .access_requests_page(
                &vera().caller(),
                Some(AccessRequestRole::Requester),
                None,
                None,
                size,
                now(),
            )
            .await
            .unwrap();
        assert_eq!(ids(&made), [requests[2], requests[1]]);
        let rest = store
            .access_requests_page(
                &vera().caller(),
                Some(AccessRequestRole::Requester),
                None,
                made.next_cursor(),
                size,
                now(),
            )
            .await
            .unwrap();
        assert_eq!(ids(&rest), [requests[0]]);
        assert!(rest.next_cursor().is_none());

        let pending = store
            .access_requests_page(
                &ada().caller(),
                Some(AccessRequestRole::Decider),
                Some(AccessRequestState::Pending),
                None,
                veoveo_types::PageSize::<100>::new(10).unwrap(),
                now(),
            )
            .await
            .unwrap();
        assert_eq!(ids(&pending), [requests[2], requests[1]]);

        // Without a role, Vera sees what she asked and what she can decide.
        let involved = store
            .access_requests_page(
                &vera().caller(),
                None,
                None,
                None,
                veoveo_types::PageSize::<100>::new(10).unwrap(),
                now(),
            )
            .await
            .unwrap();
        assert_eq!(
            ids(&involved),
            [ottos_request, requests[2], requests[1], requests[0]]
        );
        // A cursor belongs to the listing, role and state that issued it.
        assert!(matches!(
            store
                .access_requests_page(
                    &vera().caller(),
                    None,
                    None,
                    made.next_cursor(),
                    size,
                    now()
                )
                .await,
            Err(ArtifactStoreError::Paging(
                veoveo_types::PagingError::Cursor(_)
            ))
        ));
    })
    .await;
}
