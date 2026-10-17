//! The one table that says which permission each of Artifact's targets requires. It
//! builds the `ai.veoveo/authorization` declaration the Hub evaluates, and every
//! handler checks the same declaration against the permissions the Hub granted.
use std::sync::LazyLock;

use rmcp::{ErrorData as McpError, RoleServer, service::RequestContext};
use veoveo_hub_sdk::{
    HubInternalIdentity, LocalToolName, PromptName,
    hosting::hub_identity,
    registration::{
        AuthorizationBuilder, AuthorizationDeclaration, DeclarationError, RequirementTarget,
    },
    server_contract::McpServerContract,
};
use veoveo_types::{PermissionName, ResourceTemplateUri, ResourceUri};

use super::setup::ArtifactContract;
use crate::contract::ArtifactPermission::{self, *};

/// Every tool and the permission it requires.
pub(crate) const TOOLS: &[(&str, ArtifactPermission)] = &[
    ("create_upload", ArtifactsWrite),
    ("sign_upload_parts", ArtifactsWrite),
    ("complete_upload", ArtifactsWrite),
    ("abort_upload", ArtifactsWrite),
    ("create_download", ArtifactsRead),
    ("grant_artifact", GrantsWrite),
    ("revoke_artifact_grant", GrantsWrite),
    ("set_release_state", ReleasesWrite),
    ("create_share_link", ShareLinksWrite),
    ("revoke_share_link", ShareLinksWrite),
    ("request_artifact_access", AccessRequestsWrite),
    ("cancel_access_request", AccessRequestsWrite),
    ("decide_access_request", GrantsWrite),
    ("delete_artifact", ArtifactsDelete),
    ("set_upload_limits", UploadLimitsAdminister),
];

/// Every fixed resource. Reads, subscriptions and the first page of a collection
/// require reading artifacts; each artifact's own access decides the rest.
pub(crate) const RESOURCES: &[(&str, ArtifactPermission)] = &[
    ("artifact://docs", ArtifactsRead),
    ("artifact://contract", ArtifactsRead),
    ("artifact://artifacts", ArtifactsRead),
    ("artifact://uploads", ArtifactsRead),
    ("artifact://access-requests", ArtifactsRead),
    ("artifact://storage-usage", ArtifactsRead),
];

/// Every parameterized resource, as simple templates that match the concrete URIs the
/// server reads. A trailing variable takes the rest of the URI, so
/// `artifact://artifacts/{artifact_id}` also covers the artifact's grants and share links.
pub(crate) const RESOURCE_TEMPLATES: &[(&str, ArtifactPermission)] = &[
    ("artifact://docs/{doc_id}", ArtifactsRead),
    ("artifact://artifacts?{query}", ArtifactsRead),
    ("artifact://artifacts/{artifact_id}", ArtifactsRead),
    ("artifact://uploads?{query}", ArtifactsRead),
    ("artifact://uploads/{upload_id}", ArtifactsRead),
    ("artifact://access-requests?{query}", ArtifactsRead),
];

/// Every prompt reads through resources, so each requires reading artifacts.
pub(crate) const PROMPTS: &[(&str, ArtifactPermission)] = &[
    ("review-artifact-sharing", ArtifactsRead),
    ("review-access-requests", ArtifactsRead),
];

/// Artifact's authorization declaration, checked against its namespace as the Hub
/// checks it.
pub static AUTHORIZATION: LazyLock<AuthorizationDeclaration> = LazyLock::new(|| {
    declaration().expect("Artifact's requirement table forms a valid declaration")
});

pub(crate) fn declaration() -> Result<AuthorizationDeclaration, DeclarationError> {
    let invalid = |field: &'static str| DeclarationError::Invalid {
        field,
        reason: "invalid Artifact target spelling",
    };
    let mut builder = AuthorizationBuilder::<ArtifactPermission>::new();
    for (tool, permission) in TOOLS {
        builder = builder.tool(
            LocalToolName::parse(*tool).map_err(|_| invalid("tool"))?,
            *permission,
        );
    }
    for (uri, permission) in RESOURCES {
        builder = builder.resource(
            ResourceUri::new(*uri).map_err(|_| invalid("resource"))?,
            *permission,
        );
    }
    for (template, permission) in RESOURCE_TEMPLATES {
        builder = builder.resource_template(
            ResourceTemplateUri::new(*template).map_err(|_| invalid("resource template"))?,
            *permission,
        );
    }
    for (prompt, permission) in PROMPTS {
        builder = builder.prompt(
            PromptName::parse(*prompt).map_err(|_| invalid("prompt"))?,
            *permission,
        );
    }
    builder.build(&ArtifactContract::slug(), &ArtifactContract::scheme())
}

/// A request target whose required permission comes from the declaration.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Target<'a> {
    Tool(&'a str),
    Resource(&'a str),
    Prompt(&'a str),
}

/// The verified Hub identity, when it grants the permission `target` requires.
pub(crate) fn require(
    context: &RequestContext<RoleServer>,
    target: Target<'_>,
) -> Result<HubInternalIdentity, McpError> {
    let identity = hub_identity(context)?;
    authorize(&identity, target)?;
    Ok(identity)
}

/// Check the permission `target` requires against the Hub's grants. A target the
/// declaration omits is denied.
pub(crate) fn authorize(
    identity: &HubInternalIdentity,
    target: Target<'_>,
) -> Result<(), McpError> {
    let permission = required(target)?;
    identity.grants(&permission).then_some(()).ok_or_else(|| {
        McpError::invalid_request(
            format!(
                "You don't have permission to make this request. Missing permission `{permission}`."
            ),
            None,
        )
    })
}

pub(crate) fn required(target: Target<'_>) -> Result<PermissionName, McpError> {
    let undeclared = || {
        McpError::invalid_request(
            "This request names no target Artifact declares a permission for.".to_owned(),
            None,
        )
    };
    let declared = match target {
        Target::Tool(tool) => {
            let tool = LocalToolName::parse(tool).map_err(|_| undeclared())?;
            AUTHORIZATION
                .required(RequirementTarget::Tool(&tool))
                .cloned()
        }
        Target::Resource(uri) => {
            let uri = ResourceUri::new(uri).map_err(|_| undeclared())?;
            AUTHORIZATION
                .required(RequirementTarget::Resource(&uri))
                .cloned()
        }
        Target::Prompt(prompt) => {
            let prompt = PromptName::parse(prompt).map_err(|_| undeclared())?;
            AUTHORIZATION
                .required(RequirementTarget::Prompt(&prompt))
                .cloned()
        }
    };
    declared
        .map(|declared| declared.permission)
        .ok_or_else(undeclared)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn the_declaration_declares_every_permission() {
        let declared: BTreeSet<String> = declaration()
            .unwrap()
            .permissions
            .iter()
            .map(|permission| permission.permission.as_str().to_owned())
            .collect();
        let expected: BTreeSet<String> = ArtifactPermission::ALL
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect();
        assert_eq!(declared, expected);
    }

    #[test]
    fn every_tool_the_server_lists_has_exactly_one_requirement() {
        let listed: BTreeSet<String> = super::super::ArtifactMcp::tools()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        let required: BTreeSet<String> = TOOLS.iter().map(|(tool, _)| (*tool).to_owned()).collect();
        assert_eq!(listed, required);
        assert_eq!(TOOLS.len(), required.len(), "a tool is listed twice");
    }

    #[test]
    fn concrete_addresses_resolve_and_undeclared_targets_are_denied() {
        let artifact = crate::contract::ArtifactId::new();
        let upload = crate::contract::UploadId::new();
        for concrete in [
            "artifact://docs/agents".to_owned(),
            "artifact://artifacts".to_owned(),
            "artifact://artifacts?owner=ada&cursor=abc".to_owned(),
            format!("artifact://artifacts/{artifact}"),
            format!("artifact://artifacts/{artifact}/grants"),
            "artifact://uploads?state=open".to_owned(),
            format!("artifact://uploads/{upload}"),
            format!("artifact://artifacts/{artifact}/share-links"),
            format!("artifact://artifacts/{artifact}/share-links?cursor=abc"),
            "artifact://access-requests".to_owned(),
            "artifact://access-requests?role=decider&state=pending".to_owned(),
            "artifact://storage-usage".to_owned(),
        ] {
            assert_eq!(
                required(Target::Resource(&concrete)).unwrap(),
                ArtifactsRead.name(),
                "{concrete}"
            );
        }
        for prompt in super::super::prompts::ArtifactPrompt::ALL {
            assert_eq!(
                required(Target::Prompt(prompt.name())).unwrap(),
                ArtifactsRead.name()
            );
        }
        assert!(required(Target::Tool("write_artifact")).is_err());
        assert!(required(Target::Prompt("unknown-prompt")).is_err());
        assert!(required(Target::Resource("time://events")).is_err());
    }

    #[test]
    fn tool_annotations_agree_with_the_declaration() {
        let tools = super::super::ArtifactMcp::tools().list_all();
        veoveo_hub_sdk::registration::mcp::check_tool_annotations(&AUTHORIZATION, &tools).unwrap();
    }
}
