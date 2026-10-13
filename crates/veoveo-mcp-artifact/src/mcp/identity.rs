//! The caller facts Artifact decides on, taken from the identity the Gateway forwards:
//! tenant, principal, groups and their roles, data labels, and the selected Work
//! Context with its membership and output policy.
use std::collections::BTreeSet;

use rmcp::ErrorData as McpError;
use veoveo_sdk_rust::GatewayInternalIdentity;
use veoveo_task_runtime::{TaskOwner, TaskSurface};
use veoveo_types::AccessLevel;

use crate::store::{Caller, CallerContext, GroupAccess};

/// The store caller of a forwarded identity. A group the caller belongs to without a
/// named role counts as a read role.
pub fn caller(identity: &GatewayInternalIdentity) -> Result<Caller, McpError> {
    let actor = &identity.actor;
    let authority = &identity.authority;
    let mut groups: BTreeSet<GroupAccess> = actor
        .group_roles
        .iter()
        .map(|membership| GroupAccess {
            group: membership.group.clone(),
            role: membership.role.access_level(),
        })
        .collect();
    for group in &actor.groups {
        if !groups.iter().any(|access| &access.group == group) {
            groups.insert(GroupAccess {
                group: group.clone(),
                role: AccessLevel::Read,
            });
        }
    }
    let policy = &authority.output_policy;
    let output_labels = policy
        .classification
        .iter()
        .chain(&policy.data_labels)
        .cloned()
        .collect();
    Caller::new(
        authority.tenant.clone(),
        actor.id.clone(),
        authority.work_context.clone(),
        authority.membership,
        CallerContext {
            groups,
            labels: actor.data_labels.clone(),
            output_labels,
            // Calls from other servers arrive through token exchange (plan item A5);
            // until then every call is the caller's own.
            acting_server: None,
            task: None,
        },
    )
    .map_err(|_| McpError::invalid_request("the forwarded identity names no stored tenant", None))
}

/// The owner of a Task the caller's call starts: its principal, authority and labels on
/// the Gateway profile the call arrived through.
pub fn task_owner(identity: &GatewayInternalIdentity) -> Result<TaskOwner, McpError> {
    let actor = &identity.actor;
    Ok(TaskOwner {
        principal_key: actor.id.clone(),
        principal_kind: match actor.kind {
            veoveo_sdk_rust::PrincipalKind::User => veoveo_task_runtime::PrincipalKind::User,
            veoveo_sdk_rust::PrincipalKind::Service => veoveo_task_runtime::PrincipalKind::Service,
        },
        issuer: actor.issuer.clone(),
        subject: actor.subject.clone(),
        surface: TaskSurface::parse(identity.profile.as_str())
            .map_err(|_| McpError::invalid_request("the Gateway profile names no surface", None))?,
        tenant_key: actor.tenant.clone(),
        data_labels: actor.data_labels.clone(),
        authority: identity.authority.clone(),
    })
}
