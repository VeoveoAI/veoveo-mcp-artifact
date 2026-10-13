//! Shared fixtures for the store tests: a fresh database migrated as the server's own
//! login, callers built the way the forwarded identity builds them, and artifacts made
//! available through the upload path. Requires the PostgreSQL 18 server programs on
//! `PATH`.
#![allow(dead_code)]

pub mod rustfs;
pub mod service;

use std::{collections::BTreeSet, future::Future, time::Duration};

use chrono::{DateTime, TimeZone, Utc};
use veoveo_mcp_artifact::{
    contract::{
        ArtifactId, ArtifactLabels, ArtifactName, CreateUploadRequest, IdempotencyKey, MediaType,
        ObjectSize, RetentionDays,
    },
    migrator, server,
    store::{ArtifactStore, Caller, CallerContext, CompletedObject, CreatedUpload},
};
use veoveo_test_fixtures::store::TestDb;
use veoveo_types::{
    AccessLevel, Checked, DataLabelId, GroupId, PrincipalId, ServerSlug, Sha256Digest, TenantId,
    WorkContextId, WorkContextMembershipLevel,
};

pub use veoveo_mcp_artifact::store::GroupAccess;

/// A fixed instant, so expiries and retention compare predictably.
pub fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap()
}

/// Run a store test against a fresh database with a 60-second bound.
pub async fn with_store<F, Fut>(test: F)
where
    F: FnOnce(ArtifactStore) -> Fut,
    Fut: Future<Output = ()>,
{
    tokio::time::timeout(Duration::from_secs(60), async {
        let db = TestDb::new().await;
        db.migrate_server(&server(), migrator()).await;
        let store = ArtifactStore::new(db.server_store(&server()).await);
        test(store).await;
        drop(db);
    })
    .await
    .expect("the store test exceeded 60 seconds");
}

/// A caller's identity, defaulting to a contributor in Work Context `ops` of `acme`
/// with no groups or labels.
#[derive(Clone)]
pub struct Who {
    pub tenant: &'static str,
    pub principal: &'static str,
    pub work_context: &'static str,
    pub membership: WorkContextMembershipLevel,
    pub groups: Vec<(&'static str, AccessLevel)>,
    pub labels: Vec<&'static str>,
    pub output_labels: Vec<&'static str>,
    pub acting_server: Option<&'static str>,
}

impl Who {
    pub fn new(principal: &'static str) -> Self {
        Self {
            tenant: "acme",
            principal,
            work_context: "ops",
            membership: WorkContextMembershipLevel::Contributor,
            groups: Vec::new(),
            labels: Vec::new(),
            output_labels: Vec::new(),
            acting_server: None,
        }
    }

    pub fn tenant(mut self, tenant: &'static str) -> Self {
        self.tenant = tenant;
        self
    }

    pub fn in_context(
        mut self,
        work_context: &'static str,
        membership: WorkContextMembershipLevel,
    ) -> Self {
        self.work_context = work_context;
        self.membership = membership;
        self
    }

    pub fn group(mut self, group: &'static str, role: AccessLevel) -> Self {
        self.groups.push((group, role));
        self
    }

    pub fn labels(mut self, labels: &[&'static str]) -> Self {
        self.labels = labels.to_vec();
        self
    }

    pub fn output_labels(mut self, labels: &[&'static str]) -> Self {
        self.output_labels = labels.to_vec();
        self
    }

    pub fn acting(mut self, server: &'static str) -> Self {
        self.acting_server = Some(server);
        self
    }

    pub fn caller(&self) -> Caller {
        let label_set = |labels: &[&str]| -> BTreeSet<DataLabelId> {
            labels
                .iter()
                .map(|label| DataLabelId::parse(*label).unwrap())
                .collect()
        };
        Caller::new(
            TenantId::parse(self.tenant).unwrap(),
            PrincipalId::parse(self.principal).unwrap(),
            WorkContextId::parse(self.work_context).unwrap(),
            self.membership,
            CallerContext {
                groups: self
                    .groups
                    .iter()
                    .map(|(group, role)| GroupAccess {
                        group: GroupId::parse(*group).unwrap(),
                        role: *role,
                    })
                    .collect(),
                labels: label_set(&self.labels),
                output_labels: label_set(&self.output_labels),
                acting_server: self
                    .acting_server
                    .map(|slug| ServerSlug::parse(slug).unwrap()),
                task: None,
            },
        )
        .unwrap()
    }
}

/// A request to upload `size` bytes named `name`, of `text/plain`.
pub fn upload_request(name: &str, size: Option<u64>) -> CreateUploadRequest {
    CreateUploadRequest {
        name: ArtifactName::parse(name).unwrap(),
        media_type: MediaType::parse("text/plain").unwrap(),
        size: size.map(|bytes| ObjectSize::new(bytes).unwrap()),
        sha256: None,
        max_size: None,
        labels: ArtifactLabels::default(),
        retention_days: None,
        metadata: Default::default(),
        derived_from: Default::default(),
        idempotency_key: None,
    }
}

pub fn with_labels(mut request: CreateUploadRequest, labels: &[&str]) -> CreateUploadRequest {
    request.labels = ArtifactLabels::new(
        labels
            .iter()
            .map(|label| DataLabelId::parse(*label).unwrap())
            .collect(),
    )
    .unwrap();
    request
}

pub fn with_retention(mut request: CreateUploadRequest, days: u16) -> CreateUploadRequest {
    request.retention_days = Some(RetentionDays::new(days).unwrap());
    request
}

pub fn with_key(mut request: CreateUploadRequest, key: &str) -> CreateUploadRequest {
    request.idempotency_key = Some(IdempotencyKey::parse(key).unwrap());
    request
}

/// A digest that stands for an object's bytes; the store never reads bytes.
pub fn digest(byte: u8) -> Sha256Digest {
    Sha256Digest::from_bytes([byte; 32])
}

pub async fn create(
    store: &ArtifactStore,
    who: &Who,
    request: CreateUploadRequest,
) -> CreatedUpload {
    store
        .create_upload(&who.caller(), &Checked::new(request).unwrap(), now())
        .await
        .unwrap()
}

/// An available artifact: a single upload with a declared size and SHA-256 that object
/// storage checked.
pub async fn available(
    store: &ArtifactStore,
    who: &Who,
    request: CreateUploadRequest,
) -> ArtifactId {
    let size = request
        .size
        .expect("an available fixture declares its size");
    let mut request = request;
    request.sha256 = Some(digest(7));
    let created = create(store, who, request).await;
    store
        .complete_upload(
            &who.caller(),
            created.upload.upload_id,
            &CompletedObject {
                size,
                checked_sha256: Some(digest(7)),
            },
            now(),
        )
        .await
        .unwrap();
    created.artifact.artifact_id
}

/// The caller's level on an artifact, or `None` when it does not exist for the caller.
pub async fn level(
    store: &ArtifactStore,
    who: &Who,
    artifact: ArtifactId,
    at: DateTime<Utc>,
) -> Option<AccessLevel> {
    match store.artifact(&who.caller(), artifact, at).await {
        Ok(visible) => Some(visible.level),
        Err(veoveo_mcp_artifact::store::ArtifactStoreError::ArtifactNotFound) => None,
        Err(error) => panic!("reading the artifact failed: {error}"),
    }
}
