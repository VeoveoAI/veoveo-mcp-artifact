//! Audit records of Artifact's decisions. Each decision appends its record on the
//! connection of the transaction that makes the change, so both commit or neither does.
//! The record names the caller's verified attribution, the server and Task that called for
//! the caller when another server did, the Artifact resource the decision concerns and one
//! of Artifact's declared detail types.
use sqlx::PgConnection;
use veoveo_audit_contract::{AuditDetailType, AuditOutcome, AuditTarget, AuditTransactionWrite};

use crate::contract::{ArtifactAuditReason, ArtifactId, ArtifactResource, UploadId};

use super::{ArtifactStore, ArtifactStoreError, Caller};

/// How a decision ended, as its record states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decided {
    Succeeded,
    /// The decision failed the artifact; the reason names why.
    Failed(ArtifactAuditReason),
}

impl ArtifactStore {
    /// Append the record of `caller`'s decision on `connection`, inside the decision's
    /// transaction.
    pub(crate) async fn record<T: AuditDetailType>(
        &self,
        connection: &mut PgConnection,
        caller: &Caller,
        target: AuditTarget,
        detail: &T,
        decided: Decided,
    ) -> Result<(), ArtifactStoreError> {
        let detail = self.registry.detail(detail)?;
        let draft = match decided {
            Decided::Succeeded => caller.audit().draft(detail, AuditOutcome::Success),
            Decided::Failed(reason) => caller
                .audit()
                .draft(detail, AuditOutcome::Failure)
                .reason(reason.code()),
        }
        .target(target)
        .build()?;
        AuditTransactionWrite::new(&self.registry, draft)?
            .append_as_server(connection)
            .await?;
        Ok(())
    }
}

/// The target of a record about one resource: its Artifact address.
fn resource_target(resource: ArtifactResource) -> AuditTarget {
    AuditTarget::parse(resource.to_string()).expect("an Artifact address is an absolute URI")
}

/// The target of a record about one artifact.
pub(crate) fn artifact_target(artifact_id: ArtifactId) -> AuditTarget {
    resource_target(ArtifactResource::Artifact(artifact_id))
}

/// The target of a record about one upload.
pub(crate) fn upload_target(upload_id: UploadId) -> AuditTarget {
    resource_target(ArtifactResource::Upload(upload_id))
}

/// The target of a record about an artifact's grants.
pub(crate) fn grants_target(artifact_id: ArtifactId) -> AuditTarget {
    resource_target(ArtifactResource::Grants(artifact_id))
}

/// The target of a record about an artifact's share links.
pub(crate) fn share_links_target(artifact_id: ArtifactId) -> AuditTarget {
    resource_target(ArtifactResource::ShareLinks {
        artifact_id,
        cursor: None,
    })
}

/// The target of a record about a tenant's storage: its upload limits.
pub(crate) fn storage_target() -> AuditTarget {
    resource_target(ArtifactResource::StorageUsage)
}
