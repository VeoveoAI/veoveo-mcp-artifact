//! Artifact's checked contract: slug, scheme, scope, documents, the fixed resources it
//! lists and the templates it declares.
use std::sync::LazyLock;

use rmcp::model::{Resource, ResourceTemplate, ServerCapabilities, ServerConfig};
use veoveo_sdk_rust::{
    ServerSlug,
    docs::ServerDocs,
    server_contract::{
        McpResource, McpResourceTemplate, McpServerContract, McpServerSetup, McpSetupError,
    },
};
use veoveo_types::{ResourceScheme, ResourceTemplateUri};

use crate::contract::{ArtifactDocument, ArtifactResource, ArtifactScope};

/// The crate documents embedded at build time and served under `artifact://docs`,
/// `artifact://docs/{doc_id}`, `artifact://contract` and the admin document routes.
pub(crate) static SERVER_DOCS: LazyLock<ServerDocs> =
    LazyLock::new(|| veoveo_sdk_rust::server_docs!("artifact"));

pub struct ArtifactContract;

pub(crate) static SERVER_SETUP: LazyLock<McpServerSetup<ArtifactContract>> =
    LazyLock::new(|| McpServerSetup::new().expect("Artifact MCP contract setup"));

impl McpServerContract for ArtifactContract {
    type Scope = ArtifactScope;
    type Resource = ArtifactResource;

    fn slug() -> ServerSlug {
        ServerSlug::parse("artifact").expect("declared Artifact slug")
    }
    fn scheme() -> ResourceScheme {
        ResourceScheme::parse("artifact").expect("declared Artifact scheme")
    }
    fn scopes() -> &'static [ArtifactScope] {
        <ArtifactScope as veoveo_types::Vocabulary>::ALL
    }
    fn documents() -> &'static ServerDocs {
        &SERVER_DOCS
    }
    fn server_config() -> ServerConfig {
        server_config()
    }
    fn resources() -> Result<Vec<McpResource<ArtifactResource>>, McpSetupError> {
        resources()
    }
    fn resource_templates() -> Result<Vec<McpResourceTemplate>, McpSetupError> {
        resource_templates()
    }
}

fn described(
    address: ArtifactResource,
    title: &str,
    description: &str,
    mime: &str,
) -> Result<McpResource<ArtifactResource>, McpSetupError> {
    McpResource::new(address, |uri| {
        Resource::new(uri, title)
            .with_title(title)
            .with_description(description)
            .with_mime_type(mime)
    })
}

fn resources() -> Result<Vec<McpResource<ArtifactResource>>, McpSetupError> {
    let mut resources = vec![described(
        ArtifactResource::Docs,
        "Server documents",
        "Index of the crate documents embedded at build time.",
        "application/json",
    )?];
    for (document, title) in [
        (ArtifactDocument::Agents, "Agent work manual"),
        (ArtifactDocument::Design, "Domain design"),
    ] {
        resources.push(described(
            ArtifactResource::Document(document),
            title,
            "Crate document embedded at build time.",
            "text/markdown",
        )?);
    }
    resources.push(described(
        ArtifactResource::Contract,
        "Contract declaration",
        "Machine-readable contract revision and compliance declaration.",
        "application/json",
    )?);
    resources.push(described(
        ArtifactResource::Artifacts {
            work_context: None,
            owner: None,
            media_type: None,
            name: None,
            release_state: None,
            cursor: None,
        },
        "Artifacts",
        "The first page of up to 50 artifacts you may read, newest first. Filter by one of \
         work_context, owner, media_type, name (a prefix) or release_state, and read a later \
         page by adding the page's `nextCursor` as `cursor`.",
        "application/json",
    )?);
    resources.push(described(
        ArtifactResource::Uploads {
            state: None,
            cursor: None,
        },
        "Your uploads",
        "The first page of up to 50 of your uploads, newest first.",
        "application/json",
    )?);
    resources.push(described(
        ArtifactResource::AccessRequests {
            role: None,
            state: None,
            cursor: None,
        },
        "Access requests",
        "The first page of up to 50 access requests you made or can decide, newest first. \
         Filter by role (`requester` or `decider`) and state.",
        "application/json",
    )?);
    resources.push(described(
        ArtifactResource::StorageUsage,
        "Storage usage",
        "Your tenant's stored and reserved bytes against its quota.",
        "application/json",
    )?);
    Ok(resources)
}

fn resource_templates() -> Result<Vec<McpResourceTemplate>, McpSetupError> {
    [
        (
            "artifact://docs/{doc_id}",
            "Server document",
            "Embedded crate document body.",
            "text/markdown",
        ),
        (
            "artifact://artifacts{?work_context,owner,media_type,name,release_state,cursor}",
            "Artifacts",
            "A page of artifacts you may read, filtered by at most one of the parameters.",
            "application/json",
        ),
        (
            "artifact://artifacts/{artifact_id}",
            "Artifact",
            "One artifact's metadata and your level on it.",
            "application/json",
        ),
        (
            "artifact://artifacts/{artifact_id}/grants",
            "Artifact grants",
            "The grants on one artifact you administer.",
            "application/json",
        ),
        (
            "artifact://artifacts/{artifact_id}/share-links{?cursor}",
            "Artifact share links",
            "A page of the share links of one artifact you administer, without their secrets.",
            "application/json",
        ),
        (
            "artifact://uploads{?state,cursor}",
            "Your uploads",
            "A page of your uploads, optionally in one state.",
            "application/json",
        ),
        (
            "artifact://uploads/{upload_id}",
            "Upload",
            "One of your uploads.",
            "application/json",
        ),
        (
            "artifact://access-requests{?role,state,cursor}",
            "Access requests",
            "A page of the access requests you made or can decide, by role and state.",
            "application/json",
        ),
    ]
    .into_iter()
    .map(|(uri, title, description, mime)| {
        let template = ResourceTemplateUri::new(uri).map_err(|_| McpSetupError::InvalidTemplate)?;
        McpResourceTemplate::new(template, |uri| {
            ResourceTemplate::new(uri, title)
                .with_title(title)
                .with_description(description)
                .with_mime_type(mime)
        })
    })
    .collect()
}

fn server_config() -> ServerConfig {
    let mut capabilities = ServerCapabilities::builder()
        .enable_tools()
        .enable_resources()
        .enable_resources_subscribe()
        .enable_prompts()
        .enable_completions()
        .build();
    if let Some(registration) = super::declarations::REGISTRATION.get() {
        veoveo_sdk_rust::registration::mcp::advertise(
            &mut capabilities,
            registration,
            &super::requirements::AUTHORIZATION,
        );
    }
    let mut info = ServerConfig::default();
    info.capabilities = capabilities;
    info.server_info = rmcp::model::Implementation::new("artifact", env!("CARGO_PKG_VERSION"));
    info.instructions = Some(
        "Files for your tenant. To store a file, call `create_upload` with its name and media \
         type (and its size and SHA-256 when you know them), send the bytes to the signed URLs \
         it returns, then call `complete_upload`. To read a file, call `create_download` and \
         fetch the signed URL. Bytes never travel through MCP. To share a file with someone \
         without an account, release it with `set_release_state` and call \
         `create_share_link`; to reach a higher level on a file you can read, call \
         `request_artifact_access`."
            .to_owned(),
    );
    info
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_setup_round_trips_every_resource_and_template() {
        let setup = &*super::SERVER_SETUP;
        assert_eq!(setup.resources().len(), 8);
        assert_eq!(setup.resource_templates().len(), 8);
    }
}
