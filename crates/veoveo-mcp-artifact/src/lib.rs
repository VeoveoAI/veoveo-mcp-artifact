//! The Artifact MCP server's library. The `contract` feature holds Artifact's public
//! types for consumers; the default `server` feature adds persistence in the server's
//! own PostgreSQL schema.

#[cfg(feature = "contract")]
pub mod contract;

#[cfg(feature = "server")]
use veoveo_types::ServerSlug;

/// The component name, taken from the crate name.
pub fn component() -> &'static str {
    env!("CARGO_PKG_NAME")
}

/// The server's slug, which names its schema `artifact`, its roles and its channel.
#[cfg(feature = "server")]
pub fn server() -> ServerSlug {
    ServerSlug::parse("artifact").expect("Artifact's slug")
}

#[cfg(test)]
mod tests {
    #[test]
    fn component_is_the_crate_name() {
        assert_eq!(super::component(), "veoveo-mcp-artifact");
    }
}
