//! Verification of a large object's SHA-256 as a durable Task. The Task's identifier is
//! the artifact's, so a repeated completion finds the same Task; it reads the object once
//! and records the digest. A replica resumes it when the instance that claimed it dies.
use std::{collections::BTreeSet, sync::Arc, time::Duration};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use veoveo_audit_contract::{AuditActor, AuditContext, AuditPrincipalKind, AuditRequest};
use veoveo_hub_sdk::durable_tasks::{TaskContext, TaskWorker};
use veoveo_task::{
    CreateTask, PrincipalKind, RecoveryClass, ResumeCancellationPolicy, TaskFailure, TaskOwner,
    TaskSnapshot, TaskStatus, TaskTransition,
};
use veoveo_types::{Sha256Digest, TaskId, TaskTypeName};

use crate::{
    contract::{ArtifactId, ArtifactResource, ArtifactState},
    storage::ObjectKey,
    store::{Caller, CallerContext},
};

use super::{ArtifactError, ArtifactService};

/// Objects up to 8 MiB are hashed while `complete_upload` runs.
pub const INLINE_DIGEST_BYTES: u64 = 8 * 1024 * 1024;
/// How long after its upload completed a verifying artifact may go without a live
/// verification Task before the sweep fails it.
pub const VERIFICATION_DEADLINE: Duration = Duration::from_secs(15 * 60);
/// The Task type of a verification.
pub const VERIFY_TASK: TaskTypeName = TaskTypeName::from_static("artifact.verify");
/// How long a verification Task stays readable after it ends.
const VERIFY_TASK_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// What a verification Task reads: the artifact and the SHA-256 its upload declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationRequest {
    pub artifact_id: ArtifactId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_sha256: Option<Sha256Digest>,
}

/// The Task that verifies an artifact carries the artifact's identifier.
pub fn verification_task_id(artifact_id: ArtifactId) -> TaskId {
    TaskId::from_uuid(artifact_id.as_uuid())
}

impl ArtifactService {
    /// Queue the artifact's verification, owned by the caller who completed it. A Task
    /// that already exists for the artifact stands.
    pub(super) async fn start_verification(
        &self,
        owner: &TaskOwner,
        artifact_id: ArtifactId,
        declared_sha256: Option<Sha256Digest>,
    ) -> Result<(), ArtifactError> {
        let task_id = verification_task_id(artifact_id);
        let request = VerificationRequest {
            artifact_id,
            declared_sha256,
        };
        let created = self
            .tasks
            .create(CreateTask {
                task_id,
                owner: owner.clone(),
                server: self.tasks.server().clone(),
                task_type: VERIFY_TASK,
                request: serde_json::to_value(&request).expect("a verification request serializes"),
                recovery_class: RecoveryClass::Resume,
                idempotency_key: Some(format!("verify:{artifact_id}")),
                ttl_ms: Some(VERIFY_TASK_TTL_MS),
                poll_interval_ms: None,
                retention_pins: BTreeSet::new(),
            })
            .await;
        match created {
            Ok(_) => Ok(()),
            // Another surface or caller already queued it under the same identifier.
            Err(error) => match self.tasks.get(task_id).await? {
                Some(_) => Ok(()),
                None => Err(error.into()),
            },
        }
    }

    /// Hash a claimed verification's object and record the digest.
    async fn verify(&self, snapshot: &TaskSnapshot) -> Result<ArtifactState, ArtifactError> {
        let request: VerificationRequest = serde_json::from_value(snapshot.request.clone())
            .map_err(|_| crate::store::ArtifactStoreError::Corrupt("verification request"))?;
        let caller = owner_caller(snapshot.task_id, &snapshot.owner)?;
        let key = ObjectKey::for_artifact(caller.tenant(), request.artifact_id);
        let (computed, _) = self.storage.sha256(&key).await?;
        let state = self
            .store
            .record_digest(
                &caller,
                request.artifact_id,
                request.declared_sha256.as_ref(),
                &computed,
                Utc::now(),
            )
            .await?;
        if state == ArtifactState::Failed {
            self.discard_object(&caller, request.artifact_id).await;
        }
        Ok(state)
    }

    /// Whether the artifact's verification Task can still finish.
    pub(crate) async fn verification_live(
        &self,
        artifact_id: ArtifactId,
    ) -> Result<bool, ArtifactError> {
        Ok(self
            .tasks
            .get(verification_task_id(artifact_id))
            .await?
            .is_some_and(|task| !matches!(task.status, TaskStatus::Failed | TaskStatus::Cancelled)))
    }
}

/// The store caller of a verification Task's owner: its tenant, principal and Work
/// Context. The Task runs outside any request, so its records carry a background request
/// that names the Task.
fn owner_caller(task_id: TaskId, owner: &TaskOwner) -> Result<Caller, ArtifactError> {
    let mut request = AuditRequest::background();
    request.task = Some(task_id);
    let audit = AuditContext {
        actor: AuditActor {
            principal: owner.principal_key.clone(),
            kind: match owner.principal_kind {
                PrincipalKind::User => AuditPrincipalKind::User,
                PrincipalKind::Service => AuditPrincipalKind::Service,
            },
            tenant: owner.tenant_key.clone(),
            work_context: Some(owner.authority.work_context.clone()),
        },
        via: None,
        request,
    };
    Ok(Caller::new(
        owner.authority.tenant.clone(),
        owner.principal_key.clone(),
        owner.authority.work_context.clone(),
        owner.authority.membership,
        CallerContext::default(),
        audit,
    )?)
}

/// Run every verification this process's instance claims until `stop` is cancelled,
/// through the SDK's [`TaskWorker`]. A Task whose instance died returns to the queue, so
/// a replica claims and resumes it, and a lost database connection hands a running
/// verification to its recovery. A recorded digest stands over a later cancellation
/// request, because the artifact's state already reflects it.
pub async fn run_verifications(
    service: Arc<ArtifactService>,
    stop: CancellationToken,
) -> anyhow::Result<()> {
    TaskWorker::new(service.tasks.clone())
        .cancellation(VERIFY_TASK, ResumeCancellationPolicy::PreserveOutcome)
        .run(
            move |context: TaskContext| {
                let service = service.clone();
                async move { service.verification(context.task()).await }
            },
            stop,
        )
        .await?;
    Ok(())
}

impl ArtifactService {
    /// The outcome of one claimed Task. A Task this version does not know fails, and so
    /// does an object the server cannot read; a lost database connection is returned as
    /// the error, which the worker hands to the Task's recovery.
    async fn verification(&self, snapshot: &TaskSnapshot) -> Result<TaskTransition, ArtifactError> {
        if snapshot.task_type != VERIFY_TASK {
            return Ok(TaskTransition::Failed(TaskFailure::new(
                "unsupported_artifact_task",
                "the Task type is not an Artifact verification",
            )));
        }
        match self.verify(snapshot).await {
            Ok(state) => {
                let artifact =
                    serde_json::from_value::<VerificationRequest>(snapshot.request.clone())
                        .map(|request| request.artifact_id)
                        .ok();
                Ok(TaskTransition::Succeeded {
                    message: format!("the artifact is {state}"),
                    result: serde_json::json!({ "state": state.as_str() }),
                    result_uri: artifact.and_then(|artifact_id| {
                        veoveo_types::ResourceUri::new(
                            ArtifactResource::Artifact(artifact_id).to_string(),
                        )
                        .ok()
                    }),
                })
            }
            Err(error) if error.is_connection() => Err(error),
            Err(error) => {
                tracing::warn!(%error, task_id = %snapshot.task_id, "verification failed");
                Ok(TaskTransition::Failed(TaskFailure::new(
                    "verification_failed",
                    "the server could not read the object to verify it",
                )))
            }
        }
    }
}
