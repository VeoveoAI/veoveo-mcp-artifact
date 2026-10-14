//! The server's configuration. Every flag has its `VEOVEO_` environment variable. The
//! Gateway issues the database login and the access-token trust when Artifact
//! registers, so neither is configured here.
use std::path::PathBuf;

use clap::Parser;
use secrecy::{ExposeSecret, SecretString};
use url::Url;
use veoveo_sdk_rust::{PublicDeployment, PublicOrigin, parse_allowed_host_authority};

use crate::{share::ShareBase, storage::StorageConfig};

#[derive(Parser)]
#[command(name = "veoveo-mcp-artifact", about = "Artifact MCP server")]
pub(super) struct Args {
    #[arg(long, env = "VEOVEO_PORT", default_value_t = 8801)]
    pub port: u16,
    /// The origin where the Gateway reaches Artifact; Artifact serves MCP at
    /// `{public_base_url}/artifact/mcp` and registers that URL.
    #[arg(long, env = "VEOVEO_PUBLIC_BASE_URL")]
    pub public_base_url: String,
    /// The installation's public origin, such as `https://veoveo.example.com`. Share
    /// links name `{origin}/artifact/shares/{token}`, which reaches Artifact for someone
    /// without an account, and Artifact accepts the origin's host. Without it, share
    /// links name the public base URL.
    #[arg(long, env = "VEOVEO_PUBLIC_ORIGIN")]
    pub public_origin: Option<PublicOrigin>,
    /// The Gateway's RFC 8414 issuer; registration discovers its endpoint from there.
    #[arg(long, env = "VEOVEO_GATEWAY_ISSUER")]
    pub gateway_issuer: Url,
    /// The installation's registration secret, or the placeholder OpenShell substitutes.
    #[arg(
        long,
        env = "VEOVEO_REGISTRATION_SECRET",
        hide_env_values = true,
        value_parser = parse_secret
    )]
    pub registration_secret: SecretString,
    /// Holds Artifact's registration key, which keeps its namespace across restarts.
    #[arg(
        long,
        env = "VEOVEO_KEY_DIRECTORY",
        default_value = "/var/lib/veoveo/artifact/keys"
    )]
    pub key_directory: PathBuf,
    /// Where Artifact calls object storage on the installation network.
    #[arg(long, env = "VEOVEO_STORAGE_ENDPOINT")]
    pub storage_endpoint: Url,
    /// Where clients send signed requests. SigV4 binds this host, so every client must
    /// reach object storage by this name. Defaults to the storage endpoint.
    #[arg(long, env = "VEOVEO_STORAGE_SIGNING_ENDPOINT")]
    pub storage_signing_endpoint: Option<Url>,
    #[arg(long, env = "VEOVEO_STORAGE_REGION", default_value = "us-east-1")]
    pub storage_region: String,
    #[arg(long, env = "VEOVEO_STORAGE_BUCKET", default_value = "artifacts")]
    pub storage_bucket: String,
    /// The access key of Artifact's storage credential.
    #[arg(long, env = "VEOVEO_STORAGE_ACCESS_KEY", hide_env_values = true)]
    pub storage_access_key: String,
    /// The secret key of Artifact's storage credential. SigV4 signs with it inside the
    /// server, so the server holds the secret itself.
    #[arg(
        long,
        env = "VEOVEO_STORAGE_SECRET_KEY",
        hide_env_values = true,
        value_parser = parse_secret
    )]
    pub storage_secret_key: SecretString,
    /// Browser origins allowed to send signed requests to object storage.
    #[arg(
        long = "storage-cors-origin",
        env = "VEOVEO_STORAGE_CORS_ORIGINS",
        value_delimiter = ','
    )]
    pub storage_cors_origins: Vec<String>,
    #[arg(long, env = "VEOVEO_ALLOW_LOOPBACK_HOSTS", default_value_t = false)]
    pub allow_loopback_hosts: bool,
    #[arg(
        long = "allowed-host",
        env = "VEOVEO_ALLOWED_HOSTS",
        value_name = "HOST",
        value_delimiter = ',',
        value_parser = parse_allowed_host
    )]
    pub allowed_hosts: Vec<String>,
}

impl Args {
    pub fn public_deployment(&self) -> anyhow::Result<PublicDeployment> {
        Ok(PublicDeployment::new(&self.public_base_url)?.with_origin(self.public_origin.clone()))
    }

    /// The origin share links name: the installation's public origin, or else the public
    /// base URL.
    pub fn share_base(&self) -> anyhow::Result<ShareBase> {
        let origin = match &self.public_origin {
            Some(origin) => origin.to_string(),
            None => self.public_base_url.clone(),
        };
        ShareBase::parse(&origin).map_err(anyhow::Error::msg)
    }

    pub fn storage(&self) -> StorageConfig {
        StorageConfig {
            internal_endpoint: self.storage_endpoint.clone(),
            signing_endpoint: self
                .storage_signing_endpoint
                .clone()
                .unwrap_or_else(|| self.storage_endpoint.clone()),
            region: self.storage_region.clone(),
            bucket: self.storage_bucket.clone(),
            access_key: self.storage_access_key.clone(),
            secret_key: self.storage_secret_key.expose_secret().to_owned(),
            cors_origins: self.storage_cors_origins.clone(),
        }
    }
}

fn parse_secret(value: &str) -> Result<SecretString, String> {
    (!value.is_empty())
        .then(|| SecretString::from(value))
        .ok_or_else(|| "secret must not be empty".to_owned())
}

fn parse_allowed_host(value: &str) -> Result<String, String> {
    let value = value.trim();
    parse_allowed_host_authority(value)
        .map(|_| value.to_owned())
        .ok_or_else(|| "expected a host authority such as host.openshell.internal:8801".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signing_endpoint_defaults_to_the_storage_endpoint() {
        let args = Args::try_parse_from([
            "veoveo-mcp-artifact",
            "--public-base-url",
            "http://host.openshell.internal:8801",
            "--gateway-issuer",
            "http://host.openshell.internal:8080",
            "--registration-secret",
            "placeholder",
            "--storage-endpoint",
            "http://host.openshell.internal:9000",
            "--storage-access-key",
            "artifact",
            "--storage-secret-key",
            "not-printed",
        ])
        .unwrap();
        let storage = args.storage();
        assert_eq!(storage.signing_endpoint, storage.internal_endpoint);
        assert_eq!(
            args.share_base().unwrap().authority(),
            "host.openshell.internal:8801",
            "share links default to the public base URL"
        );
        assert_eq!(storage.bucket, "artifacts");
        assert!(!format!("{storage:?}").contains("not-printed"));
    }

    #[test]
    fn share_links_and_hosts_follow_the_installation_origin() {
        let args = Args::try_parse_from([
            "veoveo-mcp-artifact",
            "--public-base-url",
            "http://host.openshell.internal:8801",
            "--public-origin",
            "https://veoveo.example.com",
            "--gateway-issuer",
            "http://host.openshell.internal:8080",
            "--registration-secret",
            "placeholder",
            "--storage-endpoint",
            "http://host.openshell.internal:9000",
            "--storage-access-key",
            "artifact",
            "--storage-secret-key",
            "not-printed",
        ])
        .unwrap();
        assert_eq!(args.share_base().unwrap().authority(), "veoveo.example.com");
        let deployment = args.public_deployment().unwrap();
        assert_eq!(
            veoveo_sdk_rust::public_allowed_hosts(&deployment, false),
            vec!["host.openshell.internal:8801", "veoveo.example.com"]
        );
        assert!(
            Args::try_parse_from([
                "veoveo-mcp-artifact",
                "--public-base-url",
                "http://host.openshell.internal:8801",
                "--public-origin",
                "http://veoveo.example.com",
                "--gateway-issuer",
                "http://host.openshell.internal:8080",
                "--registration-secret",
                "placeholder",
                "--storage-endpoint",
                "http://host.openshell.internal:9000",
                "--storage-access-key",
                "artifact",
                "--storage-secret-key",
                "not-printed",
            ])
            .is_err(),
            "the public origin is an https origin"
        );
    }
}
