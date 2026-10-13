//! The Artifact MCP server's library. The `contract` feature holds Artifact's public
//! types for consumers; the default `server` feature adds persistence in the server's
//! own PostgreSQL schema.

#[cfg(feature = "contract")]
pub mod contract;
#[cfg(feature = "server")]
pub mod mcp;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod service;
#[cfg(feature = "server")]
pub mod storage;
#[cfg(feature = "server")]
pub mod store;
#[cfg(feature = "server")]
pub mod sweep;

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

/// The migrations in `migrations/`, which the server applies in its own schema at
/// startup and which record their history in `artifact._sqlx_migrations`.
#[cfg(feature = "server")]
pub fn migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn component_is_the_crate_name() {
        assert_eq!(super::component(), "veoveo-mcp-artifact");
    }
}
