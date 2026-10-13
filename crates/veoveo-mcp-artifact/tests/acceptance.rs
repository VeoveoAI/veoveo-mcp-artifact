//! Acceptance through a running local installation: Artifact registered with the
//! Gateway, called through the Gateway by the installation's member and admin callers,
//! with bytes moving between the test and the installation's object store through the
//! URLs Artifact signs.
//!
//! The tests need a running installation, so they are ignored by default. Inside the
//! Artifact environment, set `VEOVEO_ACCEPTANCE_GATEWAY` to the Gateway origin and
//! `VEOVEO_ACCEPTANCE_KEY_DIRECTORY` to the directory holding the callers' keys, then
//! run `cargo nextest run --test acceptance --run-ignored only`. The tests share the
//! tenant's usage and upload limits, so nextest runs them one at a time.
//!
//! The local installation has one tenant, so a read refused across tenants is covered by
//! the database tests and not here.
use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use futures::{StreamExt as _, TryStreamExt as _};
use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion,
        ReadResourceRequestParams, ResourceContents, ServerNotification, SubscriptionFilter,
    },
    service::{RunningService, ServiceError, Subscription},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use veoveo_mcp_artifact::contract::{
    AbortUploadResult, ArtifactEntry, ArtifactId, ArtifactState, CompleteUploadResult,
    CreateDownloadResult, CreateUploadResult, DeleteArtifactResult, GrantArtifactResult,
    GrantListing, RevokeArtifactGrantResult, SignedTransfer, StorageUsage, UploadLimits,
    UploadLimitsResult, UploadTransfer,
};
use veoveo_oauth_client::{ClientAssertionKeyId, ClientAssertionSigner};
use veoveo_types::{AccessLevel, AccessSubject, OAuthClientId};

const MEMBER: &str = "acceptance-member";
const ADMIN: &str = "acceptance-admin";
/// The local installation's one tenant, whose upload limits the quota test changes.
const TENANT: &str = "local";
const MIB: u64 = 1024 * 1024;

struct Caller;

impl ClientHandler for Caller {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("veoveo-mcp-artifact-acceptance", env!("CARGO_PKG_VERSION")),
        )
    }
}

type Client = RunningService<RoleClient, Caller>;

struct Installation {
    gateway: url::Url,
    keys: PathBuf,
}

impl Installation {
    fn from_env() -> Result<Self> {
        // reqwest is built without a TLS provider; Artifact uses rustls with ring.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let gateway = std::env::var("VEOVEO_ACCEPTANCE_GATEWAY")
            .context("VEOVEO_ACCEPTANCE_GATEWAY names the Gateway origin")?;
        let keys = std::env::var("VEOVEO_ACCEPTANCE_KEY_DIRECTORY")
            .context("VEOVEO_ACCEPTANCE_KEY_DIRECTORY names the callers' key directory")?;
        Ok(Self {
            gateway: url::Url::parse(&gateway)?,
            keys: PathBuf::from(keys),
        })
    }

    fn mcp(&self) -> Result<url::Url> {
        Ok(self.gateway.join("/mcp/local")?)
    }

    /// An RFC 6749 client-credentials token, authenticated with an RFC 7523 assertion.
    async fn token(&self, caller: &str) -> Result<String> {
        let jwks: Value = serde_json::from_slice(&std::fs::read(
            self.keys.join(format!("{caller}.jwks.json")),
        )?)?;
        let kid = jwks["keys"][0]["kid"]
            .as_str()
            .context("caller key identifier")?;
        let pem = std::fs::read(self.keys.join(format!("{caller}.pem")))?;
        let signer = ClientAssertionSigner::from_rsa_pem(ClientAssertionKeyId::parse(kid)?, &pem)?;
        let endpoint = self.gateway.join("/oauth/token")?;
        let assertion = signer.assertion(&OAuthClientId::parse(caller)?, &endpoint)?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "client_credentials")
            .append_pair("client_id", caller)
            .append_pair(
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            )
            .append_pair("client_assertion", assertion.expose_secret())
            .append_pair("resource", self.mcp()?.as_str())
            .append_pair("scope", "mcp")
            .finish();
        let response = reqwest::Client::new()
            .post(endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await?;
        let status = response.status();
        ensure!(status.is_success(), "token endpoint answered {status}");
        let token: Value = response.json().await?;
        Ok(token["access_token"]
            .as_str()
            .context("access_token")?
            .to_owned())
    }

    async fn connect(&self, caller: &str) -> Result<Client> {
        let token = self.token(caller).await?;
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(self.mcp()?.to_string())
                .auth_header(token),
        );
        Ok(Caller
            .serve_with_lifecycle(
                transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            )
            .await?)
    }
}

fn tool(name: &str, arguments: Value) -> Result<CallToolRequestParams> {
    let arguments = arguments
        .as_object()
        .cloned()
        .context("tool arguments are an object")?;
    Ok(CallToolRequestParams::new(format!("artifact__{name}")).with_arguments(arguments))
}

async fn call<T: DeserializeOwned>(client: &Client, name: &str, arguments: Value) -> Result<T> {
    let result = client.call_tool(tool(name, arguments)?).await?;
    ensure!(
        !result.is_error.unwrap_or(false),
        "{name} returned an error: {:?}",
        result.content
    );
    serde_json::from_value(result.structured_content.context("structured result")?)
        .with_context(|| format!("{name} returned a result of another shape"))
}

/// The Gateway or Artifact refuses a tool call with a protocol error or a tool error
/// result. A transport failure proves no refusal.
async fn refused(client: &Client, name: &str, arguments: Value) -> Result<()> {
    match client.call_tool(tool(name, arguments)?).await {
        Err(ServiceError::McpError(_)) => Ok(()),
        Ok(result) if result.is_error.unwrap_or(false) => Ok(()),
        Ok(_) => bail!("artifact__{name} was served"),
        Err(error) => Err(error).context(format!("artifact__{name} failed without a refusal")),
    }
}

async fn read<T: DeserializeOwned>(client: &Client, uri: &str) -> Result<T> {
    let result = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await?;
    let text = result
        .contents
        .iter()
        .find_map(|content| match content {
            ResourceContents::TextResourceContents { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .context("text resource")?;
    serde_json::from_str(text).with_context(|| format!("{uri} holds another shape"))
}

fn artifact_uri(artifact_id: ArtifactId) -> String {
    format!("artifact://artifacts/{artifact_id}")
}

const USAGE: &str = "artifact://storage-usage";

/// Subscribe to one resource; Artifact must accept it unchanged.
async fn watch(client: &Client, uri: &str) -> Result<Subscription> {
    let filter = SubscriptionFilter::builder()
        .resource_subscriptions([uri.to_owned()])
        .build();
    let listener = client.listen(filter.clone()).await?;
    ensure!(
        listener.acknowledged() == &filter,
        "the subscription to {uri} was not acknowledged as requested"
    );
    Ok(listener)
}

/// The next update naming the watched resource. Updates carry no state, so the caller
/// reads the resource after each one.
async fn next_update(listener: &mut Subscription, uri: &str) -> Result<()> {
    loop {
        let notification = listener.next().await?.context("the subscription ended")?;
        if let ServerNotification::ResourceUpdatedNotification(update) = notification {
            ensure!(
                update.params.uri == uri,
                "a notification names {}",
                update.params.uri
            );
            return Ok(());
        }
    }
}

/// Send exactly the method, URL and headers Artifact signed. The HTTP client sets the
/// content length the signature binds from the body.
async fn send(
    transfer: &SignedTransfer,
    body: Vec<u8>,
    range: Option<&str>,
) -> Result<reqwest::Response> {
    let method = reqwest::Method::from_bytes(transfer.method.as_str().as_bytes())?;
    let mut request = reqwest::Client::new().request(method, transfer.url.as_str());
    for (name, value) in &transfer.headers {
        if !name.eq_ignore_ascii_case("content-length") {
            request = request.header(name, value);
        }
    }
    if let Some(range) = range {
        request = request.header(reqwest::header::RANGE, range);
    }
    Ok(request.body(body).send().await?)
}

/// Deterministic bytes for one part, so a test can regenerate any range it uploaded.
fn part_bytes(part: u64, length: u64) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ part.wrapping_mul(0xd1b5_4a32_d192_ed03);
    let mut bytes = Vec::with_capacity(usize::try_from(length).expect("a part fits memory"));
    while (bytes.len() as u64) < length {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let word = state.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes();
        let take = usize::try_from((length - bytes.len() as u64).min(8)).unwrap();
        bytes.extend_from_slice(&word[..take]);
    }
    bytes
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Upload a small object with a declared SHA-256 through one signed PUT and complete it.
async fn upload_declared(client: &Client, name: &str, body: &[u8]) -> Result<ArtifactId> {
    let created: CreateUploadResult = call(
        client,
        "create_upload",
        json!({
            "name": name,
            "mediaType": "application/octet-stream",
            "size": body.len(),
            "sha256": digest(body),
        }),
    )
    .await?;
    let Some(UploadTransfer::Single { put }) = &created.transfer else {
        bail!("a small declared upload did not take one PUT");
    };
    let sent = send(put, body.to_vec(), None).await?;
    ensure!(
        sent.status().is_success(),
        "the signed PUT answered {}",
        sent.status()
    );
    let completed: CompleteUploadResult = call(
        client,
        "complete_upload",
        json!({"uploadId": created.upload.upload_id}),
    )
    .await?;
    ensure!(
        completed.artifact.state == ArtifactState::Available,
        "a PUT whose SHA-256 object storage checked is {:?}",
        completed.artifact.state
    );
    Ok(completed.artifact.artifact_id)
}

async fn delete(admin: &Client, artifact_id: ArtifactId) -> Result<()> {
    let deleted: DeleteArtifactResult =
        call(admin, "delete_artifact", json!({"artifactId": artifact_id})).await?;
    ensure!(
        deleted.state == ArtifactState::Deleting,
        "a deleted artifact is {:?}",
        deleted.state
    );
    Ok(())
}

async fn bounded<F: Future<Output = Result<()>>>(seconds: u64, test: F) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(seconds), test)
        .await
        .with_context(|| format!("the test exceeded {seconds} seconds"))?
}

#[tokio::test]
#[ignore = "requires a running local installation"]
async fn each_role_lists_the_tools_it_may_call_and_the_member_is_refused_the_rest() -> Result<()> {
    bounded(120, async {
        let installation = Installation::from_env()?;
        // The Gateway lists the tools a caller's role may call: the member's role carries
        // no sensitive, destructive or administer risk, and the admin's carries all three.
        let member = installation.connect(MEMBER).await?;
        let admin = installation.connect(ADMIN).await?;
        let listed = |tools: Vec<rmcp::model::Tool>| -> Vec<String> {
            let mut names: Vec<String> = tools
                .into_iter()
                .filter_map(|tool| tool.name.strip_prefix("artifact__").map(str::to_owned))
                .collect();
            names.sort();
            names
        };
        let risk_free = [
            "abort_upload",
            "complete_upload",
            "create_download",
            "create_upload",
            "sign_upload_parts",
        ];
        let risky = [
            "delete_artifact",
            "grant_artifact",
            "revoke_artifact_grant",
            "set_upload_limits",
        ];
        let seen = listed(member.list_all_tools().await?);
        ensure!(seen == risk_free, "the Gateway lists the member {seen:?}");
        let mut every: Vec<&str> = risk_free.iter().chain(&risky).copied().collect();
        every.sort_unstable();
        let seen = listed(admin.list_all_tools().await?);
        ensure!(seen == every, "the Gateway lists the admin {seen:?}");
        // A call the member's role does not admit is refused, with arguments that would
        // otherwise be served.
        let artifact_id = upload_declared(&admin, "risks.bin", &part_bytes(1, 1024)).await?;
        let group = json!({"kind": "group", "id": "acceptance-readers"});
        refused(
            &member,
            "grant_artifact",
            json!({"artifactId": artifact_id, "subject": group, "level": "read"}),
        )
        .await
        .context("the member granted access")?;
        refused(
            &member,
            "delete_artifact",
            json!({"artifactId": artifact_id}),
        )
        .await
        .context("the member deleted an artifact")?;
        refused(
            &member,
            "set_upload_limits",
            json!({"tenant": TENANT, "limits": UploadLimits::default()}),
        )
        .await
        .context("the member set upload limits")?;
        delete(&admin, artifact_id).await
    })
    .await
}

#[tokio::test]
#[ignore = "requires a running local installation"]
async fn a_declared_upload_is_available_and_serves_a_range_to_the_work_context() -> Result<()> {
    bounded(120, async {
        let installation = Installation::from_env()?;
        let admin = installation.connect(ADMIN).await?;
        let body = part_bytes(2, 256 * 1024);
        let artifact_id = upload_declared(&admin, "report.bin", &body).await?;
        let entry: ArtifactEntry = read(&admin, &artifact_uri(artifact_id)).await?;
        let artifact = &entry.artifact;
        ensure!(
            artifact.name.as_str() == "report.bin"
                && artifact.media_type.as_str() == "application/octet-stream"
                && artifact.size.map(|size| size.bytes()) == Some(body.len() as u64)
                && artifact.sha256.as_ref().map(ToString::to_string) == Some(digest(&body)),
            "the artifact's metadata is {artifact:?}"
        );
        ensure!(
            entry.level == AccessLevel::Admin,
            "the owner reads at {:?}",
            entry.level
        );
        let usage: StorageUsage = read(&admin, USAGE).await?;
        ensure!(
            usage.stored_bytes >= body.len() as u64,
            "the tenant stores {} bytes",
            usage.stored_bytes
        );

        // A member of the artifact's Work Context reads it and fetches a range.
        let member = installation.connect(MEMBER).await?;
        let seen: ArtifactEntry = read(&member, &artifact_uri(artifact_id)).await?;
        ensure!(
            seen.level == AccessLevel::Write,
            "a contributor of the Work Context reads at {:?}",
            seen.level
        );
        let download: CreateDownloadResult = call(
            &member,
            "create_download",
            json!({"artifactId": artifact_id}),
        )
        .await?;
        let fetched = send(&download.get, Vec::new(), Some("bytes=1000-4999")).await?;
        ensure!(
            fetched.status() == reqwest::StatusCode::PARTIAL_CONTENT,
            "a ranged GET answered {}",
            fetched.status()
        );
        ensure!(
            fetched.bytes().await?.as_ref() == &body[1000..5000],
            "the range holds other bytes"
        );
        delete(&admin, artifact_id).await
    })
    .await
}

#[tokio::test]
#[ignore = "requires a running local installation"]
async fn a_gibibyte_of_unknown_size_verifies_serves_a_range_and_releases_its_bytes() -> Result<()> {
    bounded(900, async {
        let installation = Installation::from_env()?;
        let admin = installation.connect(ADMIN).await?;
        let created: CreateUploadResult = call(
            &admin,
            "create_upload",
            json!({
                "name": "gibibyte.bin",
                "mediaType": "application/octet-stream",
                "maxSize": 2 * 1024 * MIB,
            }),
        )
        .await?;
        let Some(UploadTransfer::Multipart { part_size, parts }) = &created.transfer else {
            bail!("an upload of unknown size did not take parts");
        };
        let part = part_size.bytes();
        // Whole parts that make a gibibyte, then a shorter last part.
        let full = (1024 * MIB).div_ceil(part);
        let tail = 3 * MIB;
        let lengths: Vec<u64> = (0..full).map(|_| part).chain([tail]).collect();
        ensure!(
            parts.len() >= lengths.len(),
            "create_upload signed {} parts for {} needed",
            parts.len(),
            lengths.len()
        );
        let mut whole = Sha256::new();
        for (index, length) in lengths.iter().enumerate() {
            whole.update(part_bytes(index as u64, *length));
        }
        let expected = format!("sha256:{}", hex::encode(whole.finalize()));
        let total: u64 = lengths.iter().sum();
        futures::stream::iter(lengths.iter().enumerate())
            .map(|(index, length)| {
                let transfer = &parts[index].transfer;
                async move {
                    let sent = send(transfer, part_bytes(index as u64, *length), None).await?;
                    ensure!(
                        sent.status().is_success(),
                        "part {} answered {}",
                        index + 1,
                        sent.status()
                    );
                    Ok::<_, anyhow::Error>(())
                }
            })
            .buffer_unordered(4)
            .try_collect::<Vec<_>>()
            .await?;
        let completed: CompleteUploadResult = call(
            &admin,
            "complete_upload",
            json!({"uploadId": created.upload.upload_id}),
        )
        .await?;
        let artifact_id = completed.artifact.artifact_id;
        ensure!(
            completed.artifact.state == ArtifactState::Verifying,
            "a gibibyte of unknown size completed {:?}",
            completed.artifact.state
        );

        // The server hashes the object in a Task; its update reaches the subscription.
        let uri = artifact_uri(artifact_id);
        let mut listener = watch(&admin, &uri).await?;
        let verified = tokio::time::timeout(Duration::from_secs(600), async {
            loop {
                next_update(&mut listener, &uri).await?;
                let entry: ArtifactEntry = read(&admin, &uri).await?;
                match entry.artifact.state {
                    ArtifactState::Verifying => {}
                    ArtifactState::Available => return Ok::<_, anyhow::Error>(entry.artifact),
                    state => bail!("the verified artifact is {state:?}"),
                }
            }
        })
        .await
        .context("the artifact was not verified within 600 seconds")??;
        drop(listener);
        ensure!(
            verified.size.map(|size| size.bytes()) == Some(total),
            "the artifact holds {:?} bytes of {total}",
            verified.size
        );
        ensure!(
            verified.sha256.as_ref().map(ToString::to_string) == Some(expected.clone()),
            "the server computed {:?}, the client {expected}",
            verified.sha256
        );

        // A range that crosses the first part boundary.
        let download: CreateDownloadResult = call(
            &admin,
            "create_download",
            json!({"artifactId": artifact_id}),
        )
        .await?;
        let start = part - 100;
        let fetched = send(
            &download.get,
            Vec::new(),
            Some(&format!("bytes={start}-{}", part + 99)),
        )
        .await?;
        ensure!(
            fetched.status() == reqwest::StatusCode::PARTIAL_CONTENT,
            "a ranged GET answered {}",
            fetched.status()
        );
        let mut range = part_bytes(0, part)[usize::try_from(start)?..].to_vec();
        range.extend_from_slice(&part_bytes(1, part)[..100]);
        ensure!(
            fetched.bytes().await?.as_ref() == range.as_slice(),
            "the range across the part boundary holds other bytes"
        );

        // Deletion hides the artifact at once, and the sweep releases its bytes.
        let before: StorageUsage = read(&admin, USAGE).await?;
        let mut usage = watch(&admin, USAGE).await?;
        delete(&admin, artifact_id).await?;
        ensure!(
            read::<ArtifactEntry>(&admin, &uri).await.is_err(),
            "a deleted artifact is still readable"
        );
        tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                next_update(&mut usage, USAGE).await?;
                let now: StorageUsage = read(&admin, USAGE).await?;
                if now.stored_bytes + total <= before.stored_bytes {
                    return Ok::<_, anyhow::Error>(());
                }
            }
        })
        .await
        .context("the sweep did not release the deleted artifact's bytes within 180 seconds")?
    })
    .await
}

#[tokio::test]
#[ignore = "requires a running local installation"]
async fn grants_are_given_listed_and_revoked() -> Result<()> {
    bounded(120, async {
        let installation = Installation::from_env()?;
        let admin = installation.connect(ADMIN).await?;
        let artifact_id = upload_declared(&admin, "shared.bin", &part_bytes(3, 2048)).await?;
        let subject = json!({"kind": "group", "id": "acceptance-readers"});
        let expires = (chrono::Utc::now() + chrono::TimeDelta::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let granted: GrantArtifactResult = call(
            &admin,
            "grant_artifact",
            json!({"artifactId": artifact_id, "subject": subject, "level": "read", "expiresAt": expires}),
        )
        .await?;
        let readers: AccessSubject = serde_json::from_value(subject.clone())?;
        ensure!(
            granted.grant.subject == readers && granted.grant.level == AccessLevel::Read,
            "the grant is {:?}",
            granted.grant
        );
        let grants_uri = format!("{}/grants", artifact_uri(artifact_id));
        let listed: GrantListing = read(&admin, &grants_uri).await?;
        ensure!(
            listed.grants.iter().any(|grant| grant.subject == readers),
            "the grant is not listed"
        );
        let revoked: RevokeArtifactGrantResult = call(
            &admin,
            "revoke_artifact_grant",
            json!({"artifactId": artifact_id, "subject": subject}),
        )
        .await?;
        ensure!(revoked.revoked, "the revocation found no grant");
        let listed: GrantListing = read(&admin, &grants_uri).await?;
        ensure!(
            !listed.grants.iter().any(|grant| grant.subject == readers),
            "a revoked grant is still listed"
        );
        delete(&admin, artifact_id).await
    })
    .await
}

#[tokio::test]
#[ignore = "requires a running local installation"]
async fn the_admins_quota_refuses_an_upload_that_would_exceed_it() -> Result<()> {
    bounded(120, async {
        let installation = Installation::from_env()?;
        let admin = installation.connect(ADMIN).await?;
        let member = installation.connect(MEMBER).await?;
        let usage: StorageUsage = read(&admin, USAGE).await?;
        let quota = usage.stored_bytes + usage.reserved_bytes + MIB;
        let limits = UploadLimits {
            quota_bytes: Some(quota),
            ..UploadLimits::default()
        };
        let set: UploadLimitsResult = call(
            &admin,
            "set_upload_limits",
            json!({"tenant": TENANT, "limits": limits}),
        )
        .await?;
        ensure!(set.limits.quota_bytes == Some(quota), "the limits are {:?}", set.limits);
        let refusal = async {
            let seen: StorageUsage = read(&member, USAGE).await?;
            ensure!(seen.quota_bytes == Some(quota), "the usage shows quota {:?}", seen.quota_bytes);
            refused(
                &member,
                "create_upload",
                json!({"name": "over.bin", "mediaType": "application/octet-stream", "size": 2 * MIB}),
            )
            .await
            .context("an upload over the quota was admitted")
        }
        .await;
        // Restore the default limits whatever the check found.
        let restored: UploadLimitsResult = call(
            &admin,
            "set_upload_limits",
            json!({"tenant": TENANT, "limits": UploadLimits::default()}),
        )
        .await?;
        refusal?;
        ensure!(restored.limits.quota_bytes.is_none(), "the quota was not lifted");
        // Without the quota the same upload is admitted; aborting releases it.
        let admitted: CreateUploadResult = call(
            &member,
            "create_upload",
            json!({"name": "over.bin", "mediaType": "application/octet-stream", "size": 2 * MIB}),
        )
        .await?;
        let _: AbortUploadResult = call(
            &member,
            "abort_upload",
            json!({"uploadId": admitted.upload.upload_id}),
        )
        .await?;
        Ok(())
    })
    .await
}
