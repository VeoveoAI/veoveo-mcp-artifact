//! The service against a fresh database migrated as the server's own login, a private
//! RustFS with a fresh bucket and a Task runtime with its own process instance.
#![allow(dead_code)]

use std::{collections::BTreeSet, future::Future, sync::Arc, time::Duration};

use sha2::{Digest as _, Sha256};
use veoveo_mcp_artifact::{
    migrator, server, service::ArtifactService, share::ShareBase, storage::ObjectStorage,
};
use veoveo_platform_store::Instance;
use veoveo_task_runtime::{PrincipalKind, TaskOwner, TaskRuntime, TaskSurface};
use veoveo_test_fixtures::store::TestDb;
use veoveo_types::{
    AccessSubject, DataLabelId, InvocationAuthority, InvocationProvenance, PolicyVersion,
    PrincipalId, Sha256Digest, TenantId, TokenIssuer, TokenSubject, WorkContextId,
    WorkContextOutputPolicy,
};

use super::{Who, rustfs::TestRustFs};

/// The origin the test service names in share links.
pub const SHARE_ORIGIN: &str = "http://artifact.test:8801";

/// The service and the object store behind it, for one test.
pub struct Harness {
    pub service: Arc<ArtifactService>,
    pub storage: ObjectStorage,
    _rustfs: TestRustFs,
    /// The database, whose owner connection reads the audit log.
    pub db: TestDb,
}

/// Run a service test with a 120-second bound.
pub async fn with_service<F, Fut>(test: F)
where
    F: FnOnce(Arc<Harness>) -> Fut,
    Fut: Future<Output = ()>,
{
    tokio::time::timeout(Duration::from_secs(120), async {
        let db = TestDb::new().await;
        db.migrate_server(&server(), migrator()).await;
        let platform = db.server_store(&server()).await;
        let instance = Arc::new(Instance::start(&platform, &server()).await.unwrap());
        let tasks = TaskRuntime::new(platform.clone(), server(), instance);
        let rustfs = TestRustFs::start().await;
        let storage = rustfs.storage().await;
        let store = super::declared_store(platform).await;
        let service = Arc::new(ArtifactService::new(
            store,
            storage.clone(),
            tasks,
            ShareBase::parse(SHARE_ORIGIN).unwrap(),
        ));
        let harness = Arc::new(Harness {
            service,
            storage,
            _rustfs: rustfs,
            db,
        });
        test(harness).await;
    })
    .await
    .expect("the service test exceeded 120 seconds");
}

/// The Task owner the forwarded identity of `who` makes.
pub fn owner(who: &Who) -> TaskOwner {
    let principal = PrincipalId::parse(who.principal).unwrap();
    let tenant = TenantId::parse(who.tenant).unwrap();
    TaskOwner {
        principal_key: principal.clone(),
        principal_kind: PrincipalKind::User,
        issuer: TokenIssuer::parse("https://idp.veoveo.test").unwrap(),
        subject: TokenSubject::parse(who.principal).unwrap(),
        surface: TaskSurface::parse("local").unwrap(),
        tenant_key: Some(tenant.clone()),
        data_labels: who
            .labels
            .iter()
            .map(|label| DataLabelId::parse(*label).unwrap())
            .collect(),
        authority: InvocationAuthority {
            work_context: WorkContextId::parse(who.work_context).unwrap(),
            tenant,
            membership: who.membership,
            policy_revision: PolicyVersion::parse("r1").unwrap(),
            output_policy: WorkContextOutputPolicy {
                owner: AccessSubject::Principal(principal.clone()),
                initial_grants: Vec::new(),
                classification: None,
                data_labels: BTreeSet::new(),
            },
            provenance: InvocationProvenance::Direct {
                initiator: principal,
            },
        },
    }
}

/// The SHA-256 of `bytes` in the digest spelling.
pub fn sha256(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::parse(format!("sha256:{}", hex::encode(Sha256::digest(bytes)))).unwrap()
}
