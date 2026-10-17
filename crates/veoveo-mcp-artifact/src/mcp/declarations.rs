//! Artifact's registration declaration. Artifact registers it with the Hub and
//! advertises the same value in MCP Discover, so both have one source.
use std::sync::OnceLock;

use veoveo_hub_sdk::{
    PublicDeployment,
    registration::{RegistrationDeclaration, ServerEndpoint, ServerVersion},
    server_contract::McpServerContract,
};

use super::setup::ArtifactContract;

/// The registration Artifact advertises. Startup sets it from the deployment before the
/// MCP setup is built; until then Discover advertises no declarations.
pub static REGISTRATION: OnceLock<RegistrationDeclaration> = OnceLock::new();

/// Artifact's identity and the URL where the Hub reaches its MCP endpoint:
/// `{public_base_url}/artifact/mcp`.
pub fn registration(deployment: &PublicDeployment) -> anyhow::Result<RegistrationDeclaration> {
    let endpoint = deployment.server(ArtifactContract::slug().as_str())?;
    let url = url::Url::parse(&format!("{}/mcp", endpoint.public_url()))?;
    Ok(RegistrationDeclaration {
        slug: ArtifactContract::slug(),
        uri_scheme: ArtifactContract::scheme(),
        version: ServerVersion::new(env!("CARGO_PKG_VERSION"))?,
        endpoint: ServerEndpoint::new(url)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registered_endpoint_is_the_mounted_mcp_url() {
        let deployment = PublicDeployment::new("http://vv-mcp-artifact.internal:8801").unwrap();
        let registration = registration(&deployment).unwrap();
        assert_eq!(
            registration.endpoint.url().as_str(),
            "http://vv-mcp-artifact.internal:8801/artifact/mcp"
        );
        assert_eq!(registration.slug.as_str(), "artifact");
        assert_eq!(registration.uri_scheme.as_str(), "artifact");
    }
}
