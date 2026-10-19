//! Setup derives the advertised domain surface from its ready typed routes.
use crate::contract::{ArtifactProvider, ArtifactScope};
use rmcp::model::{ServerCapabilities, ServerConfig};
use std::sync::LazyLock;
use veoveo_hub_sdk::{
    docs::ServerDocs,
    server_contract::{McpServerContract, McpServerSetup},
};
use veoveo_types::ResourceScheme;
pub(crate) static SERVER_DOCS: LazyLock<ServerDocs> =
    LazyLock::new(|| veoveo_hub_sdk::server_docs!("artifact"));
pub struct ArtifactContract;
pub(crate) static SERVER_SETUP: LazyLock<McpServerSetup<ArtifactContract>> =
    LazyLock::new(|| McpServerSetup::new().expect("Artifact setup"));
impl McpServerContract for ArtifactContract {
    type Server = super::ArtifactMcp;
    type Provider = ArtifactProvider;
    type Scope = ArtifactScope;
    fn scheme() -> ResourceScheme {
        ResourceScheme::parse("artifact").unwrap()
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
        veoveo_hub_sdk::registration::mcp::advertise(
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
