//! Registration with the Hub: Artifact's key, its declarations, and the client that
//! removes the registration when Artifact stops.
use anyhow::{Context, Result};
use veoveo_hub_sdk::registration::{
    ClientMetadata, RegistrationDeclaration,
    client::{KeyDirectory, Registration, RegistrationClient},
};

use super::config::Args;
use crate::mcp::AUTHORIZATION;

/// Register Artifact. Returns the client that later removes the registration, the
/// registration, and the HTTP client that reached the Hub, which also fetches the
/// Hub's token keys when a forwarded token names a key the registration lacks.
pub(super) async fn register(
    args: &Args,
    declaration: RegistrationDeclaration,
) -> Result<(RegistrationClient, Registration, reqwest::Client)> {
    let key = KeyDirectory::new(&args.key_directory)
        .load_or_generate()
        .context("loading Artifact's registration key")?;
    let metadata = ClientMetadata::new(*key.public_key(), declaration, AUTHORIZATION.clone())?;
    let http = reqwest::Client::builder()
        .build()
        .context("building the registration HTTP client")?;
    let endpoint = RegistrationClient::discover(&http, &args.hub_issuer)
        .await
        .context("discovering the Hub's registration endpoint")?;
    let client = RegistrationClient::new(
        http.clone(),
        endpoint,
        args.registration_secret.clone(),
        key,
        metadata,
    )?;
    let registration = client
        .register()
        .await
        .context("registering Artifact with the Hub")?;
    tracing::info!(client_id = %registration.client_id(), "registered with the Hub");
    Ok((client, registration, http))
}
