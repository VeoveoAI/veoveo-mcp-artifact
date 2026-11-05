//! Setup derives the advertised domain surface from its ready typed routes.
use crate::contract::{ArtifactProvider, ArtifactScope};
use rmcp::model::{ServerCapabilities, ServerConfig};
use std::sync::LazyLock;
use veoveo_hub_sdk::{
    docs::ServerDocs,
    server_contract::{McpServerContract, McpServerSetup},
    skills::ServerSkills,
};
use veoveo_types::ResourceScheme;
pub(crate) static SERVER_DOCS: LazyLock<ServerDocs> =
    LazyLock::new(|| veoveo_hub_sdk::server_docs!("artifact"));
/// The skills under `skills/`, embedded at build time and served at `skill://artifact/…`.
pub(crate) static SERVER_SKILLS: LazyLock<ServerSkills> =
    LazyLock::new(|| veoveo_hub_sdk::server_skills!("artifact"));
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
    fn skills() -> &'static ServerSkills {
        &SERVER_SKILLS
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
    info.instructions = Some(INSTRUCTIONS.to_owned());
    info
}

/// What the server is and which skills say how to use it.
const INSTRUCTIONS: &str = "Artifact stores the files of the person you work for, each \
     owned by its creator and private until shared; bytes go straight to object storage. \
     Its skills say how: skill://artifact/upload-an-artifact/SKILL.md, \
     skill://artifact/download-an-artifact/SKILL.md, \
     skill://artifact/share-an-artifact/SKILL.md and \
     skill://artifact/find-artifacts/SKILL.md.";

#[cfg(test)]
mod tests {
    #[test]
    fn the_instructions_name_every_skill_the_server_serves() {
        let skills = &*super::SERVER_SKILLS;
        assert_eq!(skills.skills().len(), 4);
        for skill in skills.skills() {
            assert!(super::INSTRUCTIONS.contains(&skill.uri), "{}", skill.uri);
        }
    }

    #[test]
    fn the_setup_round_trips_every_resource_and_template() {
        let setup = &*super::SERVER_SETUP;
        assert_eq!(setup.resources().len(), 9);
        assert_eq!(setup.resource_templates().len(), 8);
    }
}
