//! The caller facts Artifact decides on, taken from the identity the Hub forwards:
//! tenant, principal, groups and their roles, and data labels.
use std::collections::BTreeSet;

use rmcp::ErrorData as McpError;
use veoveo_hub_sdk::HubInternalIdentity;
use veoveo_types::AccessLevel;

use crate::store::{Caller, CallerContext, GroupAccess};

/// The store caller of a forwarded identity. A group the caller belongs to without a
/// named role counts as a read role.
pub fn caller(identity: &HubInternalIdentity) -> Result<Caller, McpError> {
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
    // The Hub forwards every MCP request with its signed request context, which
    // carries the attribution of the caller's audit records.
    let audit = identity.audit_context().map_err(|_| {
        McpError::invalid_request("the Hub forwarded no valid request context", None)
    })?;
    Caller::new(
        authority.tenant.clone(),
        actor.id.clone(),
        CallerContext {
            groups,
            labels: actor.data_labels.clone(),
            // A call another server made for the caller names that server, and the
            // server's Task when the call came from one.
            acting_server: identity.acting_server().cloned(),
            acting_task: identity.acting_task().cloned(),
        },
        audit,
    )
    .map_err(|_| McpError::invalid_request("the forwarded identity names no stored tenant", None))
}
