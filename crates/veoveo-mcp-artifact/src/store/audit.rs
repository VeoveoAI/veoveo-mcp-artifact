//! Audit records of Artifact's decisions. Each decision appends its record on the
//! connection of the transaction that makes the change, so both commit or neither does.
//! The record names the caller's verified attribution, the artifact or server it is about
//! and one of Artifact's declared detail types.
use sqlx::PgConnection;
use veoveo_audit_contract::{
    AuditDetailType, AuditOutcome, AuditReason, AuditTarget, AuditTransactionWrite,
};
use veoveo_types::ResourceUri;

use crate::contract::{ArtifactId, ArtifactResource};

use super::{ArtifactStore, ArtifactStoreError, Caller};

/// How a decision ended, as its record states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decided {
    Succeeded,
    /// The decision failed the artifact; the reason names why.
    Failed(AuditReason),
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
        let (outcome, reason) = match decided {
            Decided::Succeeded => (AuditOutcome::Succeeded, AuditReason::Accepted),
            Decided::Failed(reason) => (AuditOutcome::Failed, reason),
        };
        let detail = self.registry.detail(detail)?;
        let draft = caller.audit().draft(target, detail, outcome, reason)?;
        AuditTransactionWrite::new(&self.registry, draft)?
            .append_as_server(connection)
            .await?;
        Ok(())
    }
}

/// The target of a record about one artifact: its resource address.
pub(crate) fn artifact_target(artifact_id: ArtifactId) -> AuditTarget {
    AuditTarget::Resource {
        uri: ResourceUri::new(ArtifactResource::Artifact(artifact_id).to_string())
            .expect("an artifact address is a resource URI"),
    }
}

/// The target of a record about the server's own configuration, such as a tenant's
/// upload limits.
pub(crate) fn server_target() -> AuditTarget {
    AuditTarget::Server {
        server: crate::server(),
    }
}
