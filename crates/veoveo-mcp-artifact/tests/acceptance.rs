//! Acceptance through a running local installation: Artifact registered with the
//! Hub, called through the Hub by the installation's member and admin callers,
//! with bytes moving between the test and the installation's object store through the
//! URLs Artifact signs.
//!
//! The tests need a running installation, so they are ignored by default. Inside the
//! qualification environment, set `VEOVEO_ACCEPTANCE_HUB` to the native HTTPS Hub
//! origin and `VEOVEO_ACCEPTANCE_{MEMBER,ADMIN}_BEARER_ENV` to distinct variables
//! holding the prepared bearer/opaque references. Explicit tenant, admin principal and
//! Work Context inputs retain exact identity assertions; see the README. No caller
//! signing keys are copied.
//! Run the six private cases serially, excluding the public share-route case and
//! `restart_persistence_` phases; see the README for exact commands. The tests share
//! the tenant's usage and upload limits.
//!
//! The local installation has one tenant, so a read refused across tenants is covered by
//! the database tests and not here.
use std::{num::NonZeroUsize, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use futures::{StreamExt as _, stream};
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
use veoveo_hub_sdk::{
    calls::ResourceRead,
    client::{BearerSource as _, HubClient, StaticBearer},
};
use veoveo_mcp_artifact::client::{ArtifactClient, UploadBody};
use veoveo_mcp_artifact::contract::{
    AbortUploadResult, AccessRequestListing, AccessRequestResult, AccessRequestState,
    ArtifactEntry, ArtifactId, ArtifactName, ArtifactProvider, ArtifactResource, ArtifactResult,
    ArtifactState, CompleteUploadResult, CreateDownloadResult, CreateShareLinkResult,
    CreateUploadResult, DeleteArtifactResult, GrantArtifactResult, GrantListing, MediaType,
    ObjectSize, PartSize, ReadArtifact, ReleaseState, RevokeArtifactGrantResult, ShareLinkListing,
    SignedTransfer, StorageUsage, UploadLimits, UploadLimitsResult, UploadTransfer,
};
use veoveo_types::{AccessLevel, AccessSubject, PrincipalId, TenantId, WorkContextId};

#[derive(Clone, Copy)]
enum CallerRole {
    Member,
    Admin,
}

struct AcceptanceBearers {
    member: StaticBearer,
    admin: Option<StaticBearer>,
}

impl AcceptanceBearers {
    /// Injectable lookup keeps input regression tests independent of process state.
    fn load(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        ensure!(
            lookup("VEOVEO_ACCEPTANCE_KEY_DIRECTORY").is_none(),
            "VEOVEO_ACCEPTANCE_KEY_DIRECTORY is retired; supply prepared bearer inputs"
        );
        let member_env = lookup("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV")
            .context("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV must name the member bearer variable")?;
        let admin_env = lookup("VEOVEO_ACCEPTANCE_ADMIN_BEARER_ENV");
        ensure!(
            admin_env.as_ref() != Some(&member_env),
            "member and admin bearer selectors must name distinct variables"
        );
        let member = Self::read(&lookup, &member_env, "member")?;
        let admin = admin_env
            .as_deref()
            .map(|name| Self::read(&lookup, name, "admin"))
            .transpose()?;
        ensure!(
            admin.as_ref() != Some(&member),
            "member and admin bearer inputs must be distinct"
        );
        Ok(Self {
            member: StaticBearer::new(member),
            admin: admin.map(StaticBearer::new),
        })
    }

    fn read(lookup: &impl Fn(&str) -> Option<String>, name: &str, role: &str) -> Result<String> {
        ensure!(
            !name.is_empty()
                && name.bytes().enumerate().all(|(index, byte)| {
                    byte == b'_'
                        || byte.is_ascii_alphabetic()
                        || (index > 0 && byte.is_ascii_digit())
                }),
            "{role} bearer selector must be an environment variable name"
        );
        let value = lookup(name).with_context(|| format!("{role} bearer variable is missing"))?;
        ensure!(
            !value.is_empty() && !value.chars().any(char::is_whitespace),
            "{role} bearer variable must contain a nonempty bearer without whitespace"
        );
        Ok(value)
    }

    fn source(&self, role: CallerRole) -> Result<&StaticBearer> {
        match role {
            CallerRole::Member => Ok(&self.member),
            CallerRole::Admin => self
                .admin
                .as_ref()
                .context("VEOVEO_ACCEPTANCE_ADMIN_BEARER_ENV is required for admin qualification"),
        }
    }
}

const MEMBER: CallerRole = CallerRole::Member;
const ADMIN: CallerRole = CallerRole::Admin;
const MIB: u64 = 1024 * 1024;

struct Caller;

impl ClientHandler for Caller {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::builder().enable_tasks().build(),
            Implementation::new("veoveo-mcp-artifact-acceptance", env!("CARGO_PKG_VERSION")),
        )
    }
}

type Client = RunningService<RoleClient, Caller>;

struct AcceptanceCoordinates {
    tenant: TenantId,
    admin_principal: PrincipalId,
    work_context: WorkContextId,
}

impl AcceptanceCoordinates {
    fn load(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let required = |name: &str| lookup(name).with_context(|| format!("{name} is required"));
        Ok(Self {
            tenant: TenantId::parse(required("VEOVEO_ACCEPTANCE_TENANT")?).map_err(|_| {
                anyhow::anyhow!("VEOVEO_ACCEPTANCE_TENANT must be a valid tenant ID")
            })?,
            admin_principal: PrincipalId::parse(required("VEOVEO_ACCEPTANCE_ADMIN_PRINCIPAL")?)
                .map_err(|_| {
                    anyhow::anyhow!(
                        "VEOVEO_ACCEPTANCE_ADMIN_PRINCIPAL must be a valid principal ID"
                    )
                })?,
            work_context: WorkContextId::parse(required("VEOVEO_ACCEPTANCE_WORK_CONTEXT")?)
                .map_err(|_| {
                    anyhow::anyhow!(
                        "VEOVEO_ACCEPTANCE_WORK_CONTEXT must be a valid Work Context ID"
                    )
                })?,
        })
    }
}

struct Installation {
    hub: url::Url,
    coordinates: AcceptanceCoordinates,
    bearers: AcceptanceBearers,
    http: reqwest::Client,
}

impl Installation {
    fn from_env() -> Result<Self> {
        // reqwest is built without a TLS provider; Artifact uses rustls with ring.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let hub = std::env::var("VEOVEO_ACCEPTANCE_HUB")
            .context("VEOVEO_ACCEPTANCE_HUB names the Hub origin")?;
        let bearers = AcceptanceBearers::load(|name| std::env::var(name).ok())?;
        let coordinates = AcceptanceCoordinates::load(|name| std::env::var(name).ok())?;
        // Keep platform certificate verification, including provisioned Linux roots.
        let http = reqwest::Client::builder().build()?;
        Ok(Self {
            hub: url::Url::parse(&hub)?,
            coordinates,
            bearers,
            http,
        })
    }

    fn mcp(&self) -> Result<url::Url> {
        Ok(self.hub.join("/mcp/local")?)
    }

    async fn connect(&self, role: CallerRole) -> Result<Client> {
        let bearer = self.bearers.source(role)?.caller_bearer().await?;
        let transport = StreamableHttpClientTransport::with_client(
            self.http.clone(),
            StreamableHttpClientTransportConfig::with_uri(self.mcp()?.to_string())
                .auth_header(bearer.expose().to_owned()),
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

/// The Hub or Artifact refuses a tool call with a protocol error or a tool error
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

/// Test binding to Artifact's existing typed storage-usage resource.
struct ReadStorageUsage;

impl ResourceRead for ReadStorageUsage {
    type Provider = ArtifactProvider;
    type Address = ArtifactResource;
    type Content = StorageUsage;

    fn address(&self) -> ArtifactResource {
        ArtifactResource::StorageUsage
    }
}

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
    Ok(request
        .body(body)
        .send()
        .await
        .map_err(reqwest::Error::without_url)?)
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
        // The Hub lists the tools a caller's role may call: the member's role carries
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
            "cancel_access_request",
            "complete_upload",
            "create_download",
            "create_upload",
            "request_artifact_access",
            "set_release_state",
            "sign_upload_parts",
        ];
        let risky = [
            "create_share_link",
            "decide_access_request",
            "delete_artifact",
            "grant_artifact",
            "revoke_artifact_grant",
            "revoke_share_link",
            "set_upload_limits",
        ];
        let seen = listed(member.list_all_tools().await?);
        ensure!(seen == risk_free, "the Hub lists the member {seen:?}");
        let mut every: Vec<&str> = risk_free.iter().chain(&risky).copied().collect();
        every.sort_unstable();
        let seen = listed(admin.list_all_tools().await?);
        ensure!(seen == every, "the Hub lists the admin {seen:?}");
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
            "create_share_link",
            json!({"artifactId": artifact_id, "expiresInDays": 1}),
        )
        .await
        .context("the member created a share link")?;
        refused(
            &member,
            "set_upload_limits",
            json!({"tenant": installation.coordinates.tenant, "limits": UploadLimits::default()}),
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
        let owner = &installation.coordinates.admin_principal;
        ensure!(
            artifact.owner == *owner && artifact.provenance.caller == *owner,
            "the upload did not retain the admitted admin principal"
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
            fetched
                .bytes()
                .await
                .map_err(reqwest::Error::without_url)?
                .as_ref()
                == &body[1000..5000],
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
        let calls = HubClient::new(
            installation.mcp()?,
            installation.http.clone(),
            installation.bearers.source(ADMIN)?.clone(),
        );
        let artifacts = ArtifactClient::new(calls, installation.http.clone());
        let part = PartSize::DEFAULT;
        // Whole parts that make a gibibyte, then a shorter last part.
        let full = (1024 * MIB).div_ceil(part);
        let tail = 3 * MIB;
        let lengths: Vec<u64> = (0..full).map(|_| part).chain([tail]).collect();
        let mut whole = Sha256::new();
        for (index, length) in lengths.iter().enumerate() {
            whole.update(part_bytes(index as u64, *length));
        }
        let expected = format!("sha256:{}", hex::encode(whole.finalize()));
        let total: u64 = lengths.iter().sum();
        // Generate only the next chunk when the public client reads it. Its existing
        // bounded PUTPart retries retain the same bytes and part identity across a
        // transport interruption; create_upload and complete_upload are not replayed.
        let body = UploadBody::stream(stream::iter(
            lengths
                .into_iter()
                .enumerate()
                .map(|(index, length)| Ok(Bytes::from(part_bytes(index as u64, length)))),
        ));
        let completed = artifacts
            .upload(
                ArtifactName::parse("gibibyte.bin")?,
                MediaType::parse("application/octet-stream")?,
                body,
            )
            .max_size(ObjectSize::new(2 * 1024 * MIB)?)
            .parallelism(NonZeroUsize::new(4).unwrap())
            .await
            .context("the public client's multipart upload failed")?;
        let artifact_id = completed.artifact_id;
        ensure!(
            completed.state == ArtifactState::Verifying,
            "a gibibyte of unknown size completed {:?}",
            completed.state
        );
        // Reconcile resource state through the SDK's bounded subscription reopenings.
        // An initial or reopened snapshot may observe completion; this does not claim
        // an uninterrupted pushed update across credential-generation fencing.
        let observer = HubClient::new(
            installation.mcp()?,
            installation.http.clone(),
            installation.bearers.source(ADMIN)?.clone(),
        );
        let verified = observer
            .read(ReadArtifact {
                artifact: artifact_id,
            })
            .until(|entry| entry.artifact.state != ArtifactState::Verifying)
            .timeout(Duration::from_secs(600))
            .await
            .context("following artifact verification within 600 seconds")?
            .artifact;
        ensure!(
            verified.state == ArtifactState::Available,
            "the verified artifact is {:?}",
            verified.state
        );
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

        // Open the checker after verification rather than retain an idle connection
        // through credential renewal while the server hashes the object.
        let admin = installation
            .connect(ADMIN)
            .await
            .context("connecting the range and deletion checker")?;
        let uri = artifact_uri(artifact_id);
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
            fetched
                .bytes()
                .await
                .map_err(reqwest::Error::without_url)?
                .as_ref()
                == range.as_slice(),
            "the range across the part boundary holds other bytes"
        );

        // Establish the usage subscription and read its initial snapshot BEFORE
        // deleting once, so a change between deletion and the wait cannot be missed.
        let mut usage = observer.read(ReadStorageUsage).updates();
        let before = usage
            .next()
            .await
            .context("storage-usage subscription ended before its initial snapshot")?
            .context("reading the subscribed storage usage before deletion")?;
        delete(&admin, artifact_id)
            .await
            .context("deleting the verified artifact")?;
        ensure!(
            read::<ArtifactEntry>(&admin, &uri).await.is_err(),
            "a deleted artifact is still readable"
        );
        tokio::time::timeout(Duration::from_secs(180), async {
            while let Some(now) = usage.next().await {
                let now = now.context("following storage usage after deletion")?;
                if now.stored_bytes + total <= before.stored_bytes {
                    return Ok::<_, anyhow::Error>(());
                }
            }
            bail!("storage-usage subscription ended before the deleted bytes were released")
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
            json!({"tenant": installation.coordinates.tenant, "limits": limits}),
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
            json!({"tenant": installation.coordinates.tenant, "limits": UploadLimits::default()}),
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

#[tokio::test]
#[ignore = "requires a running local installation"]
async fn a_share_link_redeems_through_the_share_route_until_its_limit() -> Result<()> {
    bounded(120, async {
        let installation = Installation::from_env()?;
        let admin = installation.connect(ADMIN).await?;
        let body = part_bytes(4, 4096);
        let artifact_id = upload_declared(&admin, "shared.bin", &body).await?;
        let released: ArtifactResult = call(
            &admin,
            "set_release_state",
            json!({"artifactId": artifact_id, "releaseState": "released"}),
        )
        .await?;
        ensure!(
            released.artifact.release_state == ReleaseState::Released,
            "the artifact is {:?}",
            released.artifact.release_state
        );
        let link: CreateShareLinkResult = call(
            &admin,
            "create_share_link",
            json!({"artifactId": artifact_id, "expiresInDays": 1, "redemptionLimit": 1}),
        )
        .await?;
        // Someone without an account opens the link: Artifact answers a redirect to a
        // signed GET at the object store, which serves the bytes.
        let browser = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let redirected = browser
            .get(link.url.expose())
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        ensure!(
            redirected.status() == reqwest::StatusCode::SEE_OTHER,
            "the share route answered {}",
            redirected.status()
        );
        let location = redirected
            .headers()
            .get(reqwest::header::LOCATION)
            .context("the redirect names no location")?
            .to_str()?
            .to_owned();
        let fetched = browser
            .get(location)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        ensure!(
            fetched.status().is_success(),
            "the signed GET answered {}",
            fetched.status()
        );
        ensure!(
            fetched
                .bytes()
                .await
                .map_err(reqwest::Error::without_url)?
                .as_ref()
                == body,
            "the link serves other bytes"
        );
        let spent = browser
            .get(link.url.expose())
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        ensure!(
            spent.status() == reqwest::StatusCode::NOT_FOUND,
            "a spent link answered {}",
            spent.status()
        );
        let listed: ShareLinkListing = read(
            &admin,
            &format!("{}/share-links", artifact_uri(artifact_id)),
        )
        .await?;
        ensure!(
            listed.share_links.iter().any(|listed| listed.share_link_id
                == link.share_link.share_link_id
                && listed.redemptions == 1),
            "the listing shows {:?}",
            listed.share_links
        );
        delete(&admin, artifact_id).await
    })
    .await
}

#[tokio::test]
#[ignore = "requires a running local installation"]
async fn the_admin_approves_the_members_request_for_more_access() -> Result<()> {
    bounded(120, async {
        let installation = Installation::from_env()?;
        let admin = installation.connect(ADMIN).await?;
        let member = installation.connect(MEMBER).await?;
        let artifact_id = upload_declared(&admin, "requested.bin", &part_bytes(5, 1024)).await?;
        // The member writes through the Work Context and asks to administer.
        let requested: AccessRequestResult = call(
            &member,
            "request_artifact_access",
            json!({
                "artifactId": artifact_id,
                "level": "admin",
                "justification": "I curate this Work Context's reports"
            }),
        )
        .await?;
        let request_id = requested.access_request.access_request_id;
        let pending: AccessRequestListing = read(
            &admin,
            "artifact://access-requests?role=decider&state=pending",
        )
        .await?;
        ensure!(
            pending
                .access_requests
                .iter()
                .any(|request| request.access_request_id == request_id),
            "the admin cannot decide the request"
        );
        let decided: AccessRequestResult = call(
            &admin,
            "decide_access_request",
            json!({"accessRequestId": request_id, "decision": "approve", "note": "granted"}),
        )
        .await?;
        ensure!(
            decided.access_request.state == AccessRequestState::Approved
                && decided.grant.as_ref().map(|grant| grant.level) == Some(AccessLevel::Admin),
            "the decision is {decided:?}"
        );
        let seen: ArtifactEntry = read(&member, &artifact_uri(artifact_id)).await?;
        ensure!(
            seen.level == AccessLevel::Admin,
            "the member reads at {:?}",
            seen.level
        );
        delete(&admin, artifact_id).await
    })
    .await
}

/// Nonsecret domain coordinates carried across a restart controlled by the operator.
struct RestartInputs {
    artifact_id: ArtifactId,
    available_at: chrono::DateTime<chrono::Utc>,
}

impl RestartInputs {
    fn load(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let artifact_id = lookup("VEOVEO_ACCEPTANCE_RESTART_ARTIFACT_ID")
            .context("VEOVEO_ACCEPTANCE_RESTART_ARTIFACT_ID names the prepared artifact")?;
        let available_at = lookup("VEOVEO_ACCEPTANCE_RESTART_AVAILABLE_AT").context(
            "VEOVEO_ACCEPTANCE_RESTART_AVAILABLE_AT is the prepared availability timestamp",
        )?;
        Ok(Self {
            artifact_id: serde_json::from_value(json!(artifact_id))
                .context("the restart artifact ID must be a canonical Artifact identifier")?,
            available_at: chrono::DateTime::parse_from_rfc3339(&available_at)
                .context("the restart availability timestamp must be RFC3339")?
                .with_timezone(&chrono::Utc),
        })
    }
}

fn check_retained(
    entry: &ArtifactEntry,
    body: &[u8],
    coordinates: &AcceptanceCoordinates,
) -> Result<()> {
    let artifact = &entry.artifact;
    let owner = &coordinates.admin_principal;
    ensure!(
        artifact.name.as_str() == "restart-persistence.bin"
            && artifact.media_type.as_str() == "application/octet-stream"
            && artifact.state == ArtifactState::Available
            && artifact.release_state == ReleaseState::Private
            && artifact.size.map(|size| size.bytes()) == Some(body.len() as u64)
            && artifact.sha256.as_ref().map(ToString::to_string) == Some(digest(body))
            && artifact.owner == *owner
            && artifact.provenance.caller == *owner
            && artifact.work_context == coordinates.work_context
            && artifact.available_at.is_some(),
        "the prepared artifact's metadata, identity or integrity changed"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "explicit phase before a trusted operator restarts Artifact and Hub"]
async fn restart_persistence_prepare() -> Result<()> {
    bounded(120, async {
        let installation = Installation::from_env()?;
        let admin = installation.connect(ADMIN).await?;
        let body = part_bytes(23, 256 * 1024);
        let artifact_id = upload_declared(&admin, "restart-persistence.bin", &body).await?;
        let entry: ArtifactEntry = read(&admin, &artifact_uri(artifact_id)).await?;
        check_retained(&entry, &body, &installation.coordinates)?;
        println!("VEOVEO_ACCEPTANCE_RESTART_ARTIFACT_ID={artifact_id}");
        println!(
            "VEOVEO_ACCEPTANCE_RESTART_AVAILABLE_AT={}",
            entry
                .artifact
                .available_at
                .context("available timestamp")?
                .to_rfc3339()
        );
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "explicit phase after independently verified native Artifact and Hub restarts"]
async fn restart_persistence_verify() -> Result<()> {
    bounded(120, async {
        let inputs = RestartInputs::load(|name| std::env::var(name).ok())?;
        let installation = Installation::from_env()?;
        let member = installation.connect(MEMBER).await?;
        let uri = artifact_uri(inputs.artifact_id);
        let body = part_bytes(23, 256 * 1024);
        let mut listener = watch(&member, &uri).await?;
        next_update(&mut listener, &uri).await.context(
            "the fresh post-restart subscription must deliver its opening resource update",
        )?;
        let entry: ArtifactEntry = read(&member, &uri).await?;
        check_retained(&entry, &body, &installation.coordinates)?;
        ensure!(
            entry.artifact.available_at == Some(inputs.available_at)
                && entry.level == AccessLevel::Write,
            "availability timestamp or Work Context access changed across restart"
        );
        let download: CreateDownloadResult = call(
            &member,
            "create_download",
            json!({"artifactId": inputs.artifact_id}),
        )
        .await?;
        let response = send(&download.get, Vec::new(), None).await?;
        ensure!(
            response.status().is_success(),
            "retained download answered {}",
            response.status()
        );
        ensure!(
            response
                .bytes()
                .await
                .map_err(reqwest::Error::without_url)?
                .as_ref()
                == body,
            "the retained object holds different bytes"
        );
        drop(listener);
        let admin = installation.connect(ADMIN).await?;
        delete(&admin, inputs.artifact_id).await
    })
    .await
}

#[cfg(test)]
mod bearer_inputs {
    use super::*;
    use std::collections::BTreeMap;

    fn load(entries: &[(&str, &str)]) -> Result<AcceptanceBearers> {
        let values: BTreeMap<_, _> = entries.iter().copied().collect();
        AcceptanceBearers::load(|name| values.get(name).map(|value| (*value).to_owned()))
    }

    #[tokio::test]
    async fn selects_each_role_without_exposing_bearers() -> Result<()> {
        let bearers = load(&[
            ("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "MEMBER_REFERENCE"),
            ("VEOVEO_ACCEPTANCE_ADMIN_BEARER_ENV", "ADMIN_REFERENCE"),
            ("MEMBER_REFERENCE", "opaque-member-reference"),
            ("ADMIN_REFERENCE", "opaque-admin-reference"),
        ])?;
        assert_eq!(
            bearers.source(CallerRole::Member)?.caller_bearer().await?,
            veoveo_hub_sdk::client::Bearer::new("opaque-member-reference")
        );
        assert_eq!(
            bearers.source(CallerRole::Admin)?.caller_bearer().await?,
            veoveo_hub_sdk::client::Bearer::new("opaque-admin-reference")
        );
        Ok(())
    }

    #[test]
    fn member_only_input_does_not_require_admin_custody() -> Result<()> {
        let bearers = load(&[
            ("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "MEMBER_REFERENCE"),
            ("MEMBER_REFERENCE", "opaque-member-reference"),
        ])?;
        assert!(bearers.source(CallerRole::Member).is_ok());
        assert!(
            bearers
                .source(CallerRole::Admin)
                .err()
                .context("admin must be unavailable")?
                .to_string()
                .contains("ADMIN_BEARER_ENV")
        );
        Ok(())
    }

    #[test]
    fn rejects_missing_invalid_ambiguous_and_retired_inputs_without_secret_diagnostics() {
        let cases: &[&[(&str, &str)]] = &[
            &[],
            &[("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "MEMBER_REFERENCE")],
            &[("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "secret-selector!")],
            &[("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "9INVALID")],
            &[
                ("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "MEMBER_REFERENCE"),
                ("MEMBER_REFERENCE", ""),
            ],
            &[
                ("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "MEMBER_REFERENCE"),
                ("MEMBER_REFERENCE", "secret with-whitespace"),
            ],
            &[
                ("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "MEMBER_REFERENCE"),
                ("VEOVEO_ACCEPTANCE_ADMIN_BEARER_ENV", "MEMBER_REFERENCE"),
                ("MEMBER_REFERENCE", "secret-duplicate"),
            ],
            &[
                ("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "MEMBER_REFERENCE"),
                ("VEOVEO_ACCEPTANCE_ADMIN_BEARER_ENV", "ADMIN_REFERENCE"),
                ("MEMBER_REFERENCE", "secret-duplicate"),
                ("ADMIN_REFERENCE", "secret-duplicate"),
            ],
            &[
                ("VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV", "MEMBER_REFERENCE"),
                ("VEOVEO_ACCEPTANCE_ADMIN_BEARER_ENV", "ADMIN_REFERENCE"),
                ("MEMBER_REFERENCE", "opaque-member-reference"),
            ],
            &[("VEOVEO_ACCEPTANCE_KEY_DIRECTORY", "secret-key-path")],
        ];
        let expected = [
            "MEMBER_BEARER_ENV must name",
            "member bearer variable is missing",
            "member bearer selector must be an environment variable name",
            "member bearer selector must be an environment variable name",
            "member bearer variable must contain a nonempty bearer",
            "member bearer variable must contain a nonempty bearer",
            "selectors must name distinct variables",
            "bearer inputs must be distinct",
            "admin bearer variable is missing",
            "KEY_DIRECTORY is retired",
        ];
        assert_eq!(cases.len(), expected.len());
        for (entries, expected) in cases.iter().zip(expected) {
            let error = load(entries)
                .err()
                .expect("invalid input must fail")
                .to_string();
            assert!(
                !error.contains("secret"),
                "credential data reached diagnostics"
            );
            assert!(
                error.contains(expected),
                "input rejected for the wrong reason: {error}"
            );
        }
    }
}

#[cfg(test)]
mod restart_inputs {
    use super::*;

    #[test]
    fn requires_exact_domain_coordinates() -> Result<()> {
        let id = ArtifactId::new();
        let values = |name: &str| match name {
            "VEOVEO_ACCEPTANCE_RESTART_ARTIFACT_ID" => Some(id.to_string()),
            "VEOVEO_ACCEPTANCE_RESTART_AVAILABLE_AT" => Some("2026-10-17T12:00:00Z".to_owned()),
            _ => None,
        };
        assert_eq!(RestartInputs::load(values)?.artifact_id, id);
        assert!(RestartInputs::load(|_| None).is_err());
        for (field, invalid) in [
            ("VEOVEO_ACCEPTANCE_RESTART_ARTIFACT_ID", "not-an-artifact"),
            ("VEOVEO_ACCEPTANCE_RESTART_AVAILABLE_AT", "not-a-timestamp"),
        ] {
            assert!(
                RestartInputs::load(|name| {
                    if name == field {
                        Some(invalid.to_owned())
                    } else {
                        values(name)
                    }
                })
                .is_err()
            );
        }
        Ok(())
    }
}

#[tokio::test]
async fn signed_transfer_errors_do_not_expose_the_url_or_signature() -> Result<()> {
    let error = reqwest::Client::new()
        .get("https://storage.invalid/object?X-Amz-Signature=private-signature")
        .header("invalid header", "value")
        .send()
        .await
        .err()
        .context("an invalid header must fail before any network operation")?
        .without_url();
    ensure!(error.url().is_none(), "the error retained a signed URL");
    ensure!(
        !error.to_string().contains("private-signature"),
        "the error exposed a signature"
    );
    Ok(())
}

#[cfg(test)]
mod coordinate_inputs {
    use super::*;

    fn reference(name: &str) -> Option<String> {
        match name {
            "VEOVEO_ACCEPTANCE_TENANT" => Some("reference".into()),
            "VEOVEO_ACCEPTANCE_ADMIN_PRINCIPAL" => {
                Some("https://idp.example.com/tenant/v2.0#operator".into())
            }
            "VEOVEO_ACCEPTANCE_WORK_CONTEXT" => Some("reference-work".into()),
            _ => None,
        }
    }

    #[test]
    fn retains_explicit_installation_identity_without_local_defaults() -> Result<()> {
        let coordinates = AcceptanceCoordinates::load(reference)?;
        assert_eq!(coordinates.tenant.as_str(), "reference");
        assert_eq!(
            coordinates.admin_principal.as_str(),
            "https://idp.example.com/tenant/v2.0#operator"
        );
        assert_eq!(coordinates.work_context.as_str(), "reference-work");
        Ok(())
    }

    #[test]
    fn rejects_missing_and_invalid_coordinates_with_field_diagnostics() {
        for field in [
            "VEOVEO_ACCEPTANCE_TENANT",
            "VEOVEO_ACCEPTANCE_ADMIN_PRINCIPAL",
            "VEOVEO_ACCEPTANCE_WORK_CONTEXT",
        ] {
            for invalid in [None, Some(""), Some("invalid\ncoordinate")] {
                let error = AcceptanceCoordinates::load(|name| {
                    if name == field {
                        invalid.map(str::to_owned)
                    } else {
                        reference(name)
                    }
                })
                .err()
                .expect("invalid coordinates must fail")
                .to_string();
                assert!(error.contains(field), "wrong field diagnostic: {error}");
                assert!(!error.contains("invalid\ncoordinate"));
            }
        }
    }
}
