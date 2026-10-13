//! Share links outside MCP: the secret a link carries, the origin its URL names, and the
//! route that redeems it for someone without an account.
mod route;
mod secret;

pub use route::routes;
pub use secret::{SHARE_TOKEN_CHARS, ShareBase, ShareLinkSecret};
