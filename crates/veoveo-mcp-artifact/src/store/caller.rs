//! The caller facts a statement needs, taken from the forwarded identity. They reach SQL
//! as bind parameters, and `artifact.effective_level` decides each artifact from them.
use std::collections::BTreeSet;

use veoveo_audit_contract::AuditContext;
use veoveo_sdk_rust::OpaqueTaskId;
use veoveo_store::{PrincipalId as StoredPrincipal, TenantId as StoredTenant};
use veoveo_types::{
    AccessLevel, DataLabelId, GroupId, PrincipalId, ServerSlug, TenantId, WorkContextId,
    WorkContextMembershipLevel,
};

use super::ArtifactStoreError;

/// The caller's role in one of its groups. A group grant gives the caller at most the
/// lower of the grant's level and this role.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupAccess {
    pub group: GroupId,
    pub role: AccessLevel,
}

/// Who calls, in which tenant and Work Context, with which groups and labels, and, when
/// another server calls for the caller, which server and Task. The audit context is the
/// verified attribution every record of the caller's decisions carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    tenant_key: TenantId,
    tenant: StoredTenant,
    principal_key: PrincipalId,
    principal: StoredPrincipal,
    groups: BTreeSet<GroupAccess>,
    work_context: WorkContextId,
    membership: WorkContextMembershipLevel,
    labels: BTreeSet<DataLabelId>,
    output_labels: BTreeSet<DataLabelId>,
    acting_server: Option<ServerSlug>,
    acting_task: Option<OpaqueTaskId>,
    audit: AuditContext,
}

/// The parts of a [`Caller`] beyond its tenant, principal and Work Context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallerContext {
    pub groups: BTreeSet<GroupAccess>,
    /// The caller's data labels: its clearance.
    pub labels: BTreeSet<DataLabelId>,
    /// The labels the Work Context's output policy stamps on every new artifact: its
    /// classification and data labels.
    pub output_labels: BTreeSet<DataLabelId>,
    pub acting_server: Option<ServerSlug>,
    /// The acting server's Task the call came from, when it named one.
    pub acting_task: Option<OpaqueTaskId>,
}

impl Caller {
    pub fn new(
        tenant: TenantId,
        principal: PrincipalId,
        work_context: WorkContextId,
        membership: WorkContextMembershipLevel,
        context: CallerContext,
        audit: AuditContext,
    ) -> Result<Self, ArtifactStoreError> {
        if audit.actor.tenant.as_ref() != Some(&tenant) {
            return Err(ArtifactStoreError::InvalidCaller);
        }
        let stored_tenant = veoveo_store::deterministic_tenant_id(tenant.as_str())
            .map_err(|_| ArtifactStoreError::InvalidCaller)?;
        let stored_principal =
            veoveo_store::deterministic_principal_id(tenant.as_str(), principal.as_str())
                .map_err(|_| ArtifactStoreError::InvalidCaller)?;
        Ok(Self {
            tenant_key: tenant,
            tenant: stored_tenant,
            principal_key: principal,
            principal: stored_principal,
            groups: context.groups,
            work_context,
            membership,
            labels: context.labels,
            output_labels: context.output_labels,
            acting_server: context.acting_server,
            acting_task: context.acting_task,
            audit,
        })
    }

    /// The attribution of the caller's audit records.
    pub fn audit(&self) -> &AuditContext {
        &self.audit
    }

    pub fn tenant_key(&self) -> &TenantId {
        &self.tenant_key
    }

    pub fn tenant(&self) -> StoredTenant {
        self.tenant
    }

    pub fn principal_key(&self) -> &PrincipalId {
        &self.principal_key
    }

    pub fn principal(&self) -> StoredPrincipal {
        self.principal
    }

    pub fn work_context(&self) -> &WorkContextId {
        &self.work_context
    }

    pub fn membership(&self) -> WorkContextMembershipLevel {
        self.membership
    }

    pub fn labels(&self) -> &BTreeSet<DataLabelId> {
        &self.labels
    }

    pub fn output_labels(&self) -> &BTreeSet<DataLabelId> {
        &self.output_labels
    }

    pub fn acting_server(&self) -> Option<&ServerSlug> {
        self.acting_server.as_ref()
    }

    pub fn acting_task(&self) -> Option<&OpaqueTaskId> {
        self.acting_task.as_ref()
    }

    /// Who an idempotency key belongs to: the acting server when another server calls,
    /// otherwise the caller.
    pub(crate) fn acting_identity(&self) -> String {
        match &self.acting_server {
            Some(server) => format!("server:{server}"),
            None => format!("principal:{}", self.principal_key),
        }
    }

    /// The caller's facts as the bind parameters of `artifact.effective_level`.
    pub(crate) fn facts(&self) -> CallerFacts {
        let (groups, group_levels) = self
            .groups
            .iter()
            .map(|access| (access.group.to_string(), level_text(access.role).to_owned()))
            .unzip();
        CallerFacts {
            tenant: self.tenant,
            principal: self.principal,
            principal_key: self.principal_key.to_string(),
            groups,
            group_levels,
            work_context: self.work_context.to_string(),
            membership: membership_text(self.membership).to_owned(),
            labels: self.labels.iter().map(ToString::to_string).collect(),
        }
    }
}

/// [`Caller`] as text and arrays at the database-driver boundary.
#[derive(Debug, Clone)]
pub(crate) struct CallerFacts {
    pub tenant: StoredTenant,
    pub principal: StoredPrincipal,
    pub principal_key: String,
    pub groups: Vec<String>,
    pub group_levels: Vec<String>,
    pub work_context: String,
    pub membership: String,
    pub labels: Vec<String>,
}

pub(crate) fn level_text(level: AccessLevel) -> &'static str {
    match level {
        AccessLevel::Read => "read",
        AccessLevel::Write => "write",
        AccessLevel::Admin => "admin",
    }
}

pub(crate) fn membership_text(level: WorkContextMembershipLevel) -> &'static str {
    match level {
        WorkContextMembershipLevel::Viewer => "viewer",
        WorkContextMembershipLevel::Contributor => "contributor",
        WorkContextMembershipLevel::Custodian => "custodian",
        WorkContextMembershipLevel::Owner => "owner",
    }
}

/// A rank from `artifact.effective_level` as a level: 0 is none.
pub(crate) fn level_from_rank(rank: i16) -> Option<AccessLevel> {
    match rank {
        1 => Some(AccessLevel::Read),
        2 => Some(AccessLevel::Write),
        3 => Some(AccessLevel::Admin),
        _ => None,
    }
}

pub(crate) fn level_from_text(text: &str) -> Option<AccessLevel> {
    match text {
        "read" => Some(AccessLevel::Read),
        "write" => Some(AccessLevel::Write),
        "admin" => Some(AccessLevel::Admin),
        _ => None,
    }
}
