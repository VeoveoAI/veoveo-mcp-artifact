//! Artifact's knowledge search through the SDK's in-process mesh, over a fresh database
//! with pgvector and a private SeaweedFS. Embedding is a synthetic stand-in on the mesh
//! that serves Embedding's own `embed` contract, so Artifact's `EmbeddingClient` calls it
//! as it calls Embedding: it fetches each media part through its signed GET and answers a
//! vector hashed from the words and the bytes, never a model's. These tests prove the
//! indexing paths, the ranking and the read rule, not retrieval quality.
mod support;

use std::{
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use chrono::{TimeDelta, Utc};
use rmcp::{
    RoleServer,
    model::{Implementation, ServerConfig},
    service::RequestContext,
};
use serde_json::{Value, json};
use support::{
    Who,
    seaweedfs::send,
    service::{Harness, with_service},
};
use tokio_util::sync::CancellationToken;
use veoveo_hub_sdk::{
    AccessTokenSubject, ForwardedRequest, HubInternalTokenIssuer, HubProfileId, HubRequestContext,
    IssuedHubInternalToken, OAuthClientId, ProtectedResourceId, ServerSlug, TokenIssuer,
    definition::Product,
    docs::ServerDocs,
    durable_tasks::TaskWorker,
    hosting::{
        DomainServer, ForwardedBearer, Hosted, RoutePolicy, Routes, ServerCalls, ToolError,
        hub_identity,
        testing::{self, MESH_PROFILE, RunningMesh, TestMesh},
    },
    knowledge::ContentPart,
    registration::ResourceIndicator,
    server_contract::{McpServerContract, McpServerSetup},
};
use veoveo_hub_sdk_fixture::ledger::{self, LedgerDomain, LedgerTasks};
use veoveo_mcp_artifact::{
    contract::{
        ArtifactId, ArtifactPermission, CompleteUploadRequest, CreateUploadResult,
        GrantArtifactRequest, MediaType, SignedTransfer, UploadTransfer,
    },
    mcp::{self as artifact_mcp, AUTHORIZATION, ArtifactApp, ArtifactMcp, TenantHubs},
};
use veoveo_mcp_embedding::contract::{
    Device, EffectiveProcessing, EmbedRequest, Embedding, EmbeddingPermission, EmbeddingProvider,
    EmbeddingResult, Execution, FfmpegIdentity, ModelIdentity, RuntimeIdentity, TokenUsage,
};
use veoveo_types::{
    AccessLevel, AccessSubject, Checked, GroupId, InvocationMode, PermissionName, ResourceScheme,
    Sha256Digest,
};

const SYNTHETIC_MODEL: &str = "synthetic/word-hash";

/// The synthetic image bytes of a kite, which nothing decodes.
const KITE: &[u8] = b"synthetic image bytes of a red kite";
const RIVER: &[u8] = b"synthetic image bytes of a slow river";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, veoveo_types::Vocabulary)]
#[vocabulary(scope)]
pub enum NoScope {}

/// Who one Embedding call came from and what it embedded.
#[derive(Debug, Clone)]
struct Seen {
    acting_server: Option<String>,
    acting_task: Option<String>,
    prior_servers: Vec<String>,
    texts: Vec<String>,
}

/// The stand-in for Embedding.
#[derive(Clone, Default)]
struct Synthetic {
    seen: Arc<Mutex<Vec<Seen>>>,
    http: reqwest::Client,
}

struct SyntheticContract;
impl McpServerContract for SyntheticContract {
    type Server = Synthetic;
    type Provider = EmbeddingProvider;
    type Scope = NoScope;
    fn scheme() -> ResourceScheme {
        ResourceScheme::parse("embedding").unwrap()
    }
    fn scopes() -> &'static [NoScope] {
        &[]
    }
    fn documents() -> &'static ServerDocs {
        static DOCS: LazyLock<ServerDocs> = LazyLock::new(|| {
            ServerDocs::new(ServerSlug::parse("embedding").unwrap())
                .with_doc("agents", "Agents", "# Synthetic Embedding")
                .with_doc("design", "Design", "# Synthetic tests only")
        });
        &DOCS
    }
    fn server_config() -> ServerConfig {
        let mut config = ServerConfig::default();
        config.server_info = Implementation::new("embedding", "1");
        config
    }
}

static SYNTHETIC_ROUTES: LazyLock<Routes<Synthetic>> = LazyLock::new(|| {
    Routes::builder(RoutePolicy::Authenticated)
        .tool::<EmbedRequest, _, _>(
            "Synthetic embeddings",
            RoutePolicy::permission(EmbeddingPermission::EmbeddingsCreate).unwrap(),
            |domain: Synthetic, request, context| async move { domain.embed(request, context).await },
        )
        .build()
});

impl DomainServer for Synthetic {
    type Contract = SyntheticContract;
    fn setup() -> &'static McpServerSetup<SyntheticContract> {
        static SETUP: LazyLock<McpServerSetup<SyntheticContract>> =
            LazyLock::new(|| McpServerSetup::new().unwrap());
        &SETUP
    }
    fn routes() -> &'static Routes<Self> {
        &SYNTHETIC_ROUTES
    }
}

fn slot(bytes: &[u8], modulus: usize) -> usize {
    bytes.iter().fold(7_usize, |hash, byte| {
        hash.wrapping_mul(31) + usize::from(*byte)
    }) % modulus
}

impl Synthetic {
    async fn embed(
        self,
        request: EmbedRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<EmbeddingResult, ToolError> {
        let identity = hub_identity(&context)?;
        let mut texts = Vec::new();
        let mut items = Vec::with_capacity(request.items.len());
        for (index, item) in request.items.iter().enumerate() {
            let mut vector = vec![0.0_f32; 768];
            for part in &item.content {
                match part {
                    ContentPart::Text { text } => {
                        texts.push(text.clone());
                        for word in text.split_whitespace() {
                            vector[slot(word.to_lowercase().as_bytes(), 700)] += 1.0;
                        }
                    }
                    media => {
                        let media = media.signed_media().expect("a media part");
                        let mut fetch = self.http.get(media.url.expose());
                        for (name, value) in media.headers.iter() {
                            fetch = fetch.header(name.as_str(), value.expose());
                        }
                        let response = fetch
                            .send()
                            .await
                            .map_err(|_| ToolError::Rejected("source_unavailable".into()))?;
                        if !response.status().is_success() {
                            return Err(ToolError::Rejected("source_refused".into()));
                        }
                        let bytes = response
                            .bytes()
                            .await
                            .map_err(|_| ToolError::Rejected("source_unavailable".into()))?;
                        vector[700 + slot(&bytes, 60)] += 3.0;
                    }
                }
            }
            if vector.iter().all(|value| *value == 0.0) {
                vector[767] = 1.0;
            }
            items.push(Embedding {
                index,
                vector,
                token_count: 1,
                media: Vec::new(),
            });
        }
        self.seen.lock().unwrap().push(Seen {
            acting_server: identity.acting_server().map(ToString::to_string),
            acting_task: identity.acting_task().map(ToString::to_string),
            prior_servers: identity
                .delegation_context
                .iter()
                .flat_map(|context| context.actors.iter())
                .map(|actor| actor.server.to_string())
                .collect(),
            texts,
        });
        let tokens = items.len() as u64;
        Ok(EmbeddingResult {
            request_id: uuid::Uuid::now_v7(),
            model: ModelIdentity {
                id: SYNTHETIC_MODEL.into(),
                revision: "1".into(),
            },
            processing_id: Sha256Digest::from_bytes([0; 32]),
            dimensions: request.dimensions,
            normalized: false,
            execution: Execution {
                device: Device::Mps,
                dtype: "float32".into(),
            },
            runtime: RuntimeIdentity {
                package: "synthetic".into(),
                version: "0".into(),
                source_revision: "synthetic".into(),
                python: "none".into(),
                sentence_transformers: "none".into(),
                transformers: "none".into(),
                torch: "none".into(),
                torchvision: "none".into(),
                pillow: "none".into(),
                torchcodec: "none".into(),
                ffmpeg: FfmpegIdentity {
                    version: "none".into(),
                    libavfilter: [0; 3],
                    libavutil: [0; 3],
                    libavcodec: [0; 3],
                    libavformat: [0; 3],
                },
            },
            processing: EffectiveProcessing {
                prompt_name: request.prompt_name,
                prompt_sha256: Sha256Digest::from_bytes([0; 32]),
                preprocessing: request.preprocessing.effective(),
            },
            items,
            usage: TokenUsage {
                input_tokens: tokens,
            },
        })
    }
}

/// A forwarded token for the test user, as the Hub forwards one to Ledger.
fn forwarded_to_ledger() -> IssuedHubInternalToken {
    let principal = testing::principal();
    let authority = testing::authority();
    let now = Utc::now();
    let context = HubRequestContext {
        execution: testing::execution_context(),
        audit: veoveo_audit_contract::AuditRequest::background(),
        access_token: AccessTokenSubject {
            issuer: principal.issuer.clone(),
            subject: principal.subject.clone(),
            oauth_client_id: OAuthClientId::parse(testing::TEST_CLIENT).unwrap(),
            session_family: None,
            audience: ProtectedResourceId::parse(testing::TEST_RESOURCE).unwrap(),
            invocation_mode: InvocationMode::Direct,
            initiator: Some(principal.id.clone()),
            delegation_id: None,
            scopes: principal.scopes.clone(),
            jwt_id: None,
            issued_at: now,
            not_before: None,
            expires_at: now + TimeDelta::minutes(5),
        },
        principal: principal.clone(),
    };
    HubInternalTokenIssuer::new(
        TokenIssuer::parse(testing::TEST_ISSUER).unwrap(),
        testing::signing_key(),
    )
    .unwrap()
    .issue(ForwardedRequest {
        deployment: testing::deployment_id(),
        profile: HubProfileId::parse(MESH_PROFILE).unwrap(),
        server: ServerSlug::parse("ledger").unwrap(),
        audience: ResourceIndicator::parse(testing::TEST_RESOURCE).unwrap(),
        client: OAuthClientId::parse(testing::TEST_CLIENT).unwrap(),
        actor: principal,
        authority,
        permissions: [PermissionName::parse("ledger.entries.read").unwrap()].into(),
        request_context: Some(context),
        acting_server: None,
        acting_task: None,
        delegation_context: None,
        expires_at: now + TimeDelta::minutes(5),
    })
    .unwrap()
}

/// Artifact, the synthetic Embedding and Ledger on one mesh, with Artifact's index Task
/// worker running.
struct Mesh {
    running: RunningMesh,
    embedding: Synthetic,
    ledger: ServerCalls,
    _worker: tokio_util::sync::DropGuard,
    watching: tokio::task::JoinHandle<()>,
}

impl Drop for Mesh {
    fn drop(&mut self) {
        self.watching.abort();
    }
}

impl Mesh {
    async fn start(harness: &Harness) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut mesh = TestMesh::bind().await;
        let calls = mesh.register((*AUTHORIZATION).clone());
        let hubs = Arc::new(TenantHubs::default());
        let watching = tokio::spawn(artifact_mcp::watch_changes(
            harness.platform.clone(),
            hubs.clone(),
        ));
        let app = Arc::new(ArtifactApp {
            service: (*harness.service).clone(),
            hubs,
            tasks: harness.tasks.clone(),
            library: support::library(),
        });
        let served = app.clone();
        mesh.serve(
            testing::for_domain::<ArtifactMcp>()
                .calls(calls.clone())
                .handler(move || artifact_mcp::hosted(served.clone()))
                .build(),
        );
        let embedding = Synthetic::default();
        mesh.register_server(
            ServerSlug::parse("embedding").unwrap(),
            Synthetic::routes().authorization(),
        );
        let synthetic = embedding.clone();
        mesh.serve(
            testing::for_domain::<Synthetic>()
                .handler(move || Hosted::new(synthetic.clone()))
                .build(),
        );
        let mut declaration = ledger::authorization().unwrap();
        declaration.uses.extend([
            ArtifactPermission::ArtifactsRead.name(),
            ArtifactPermission::ArtifactsWrite.name(),
        ]);
        let ledger = mesh.register(declaration);
        let tasks = LedgerTasks::new(Duration::from_secs(90));
        mesh.serve(
            testing::for_domain::<LedgerDomain>()
                .handler(move || ledger::hosted(tasks.clone()))
                .build(),
        );
        let running = mesh
            .granting([
                ArtifactPermission::ArtifactsRead.name(),
                ArtifactPermission::ArtifactsWrite.name(),
                PermissionName::parse(EmbeddingPermission::EmbeddingsCreate.as_str()).unwrap(),
                PermissionName::parse("ledger.entries.read").unwrap(),
            ])
            .start();
        let stop = CancellationToken::new();
        tokio::spawn(artifact_mcp::work(
            TaskWorker::new(harness.tasks.clone()).calls(calls),
            ArtifactMcp::new(app),
            stop.clone(),
        ));
        Self {
            running,
            embedding,
            ledger,
            _worker: stop.drop_guard(),
            watching,
        }
    }

    async fn call(&self, tool: &str, arguments: Value) -> Value {
        let answer = self.running.call("artifact", tool, arguments).await;
        assert!(answer["error"].is_null(), "{tool}: {answer}");
        answer
    }

    /// Upload `body` as the test user, calling Artifact directly, and return the
    /// artifact and the complete call's answer.
    async fn upload(
        &self,
        name: &str,
        media_type: &str,
        body: &[u8],
        metadata: Value,
    ) -> (ArtifactId, Value) {
        let created = self
            .call(
                "create_upload",
                json!({"name": name, "mediaType": media_type, "size": body.len(), "metadata": metadata}),
            )
            .await;
        let created: Product<CreateUploadResult> =
            serde_json::from_value(created["result"]["structuredContent"].clone()).unwrap();
        let created = created.metadata().clone();
        let Some(UploadTransfer::Single { put }) = &created.transfer else {
            panic!("a small upload takes one PUT");
        };
        assert!(send(put, body.to_vec(), &[]).await.status().is_success());
        let completed = self
            .call(
                "complete_upload",
                json!({"uploadId": created.upload.upload_id}),
            )
            .await;
        (created.artifact.artifact_id, completed)
    }

    /// The test user's view of Artifact's Task `task` once it settles.
    async fn settled(&self, task: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let answer = self.running.task("artifact", task).await;
                if answer["result"]["status"] != "working" {
                    return answer;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the index Task settles within 30 seconds")
    }

    /// The artifacts a search answers, in rank order.
    async fn search(&self, parts: Value) -> Vec<String> {
        let answer = self.call("search_knowledge", json!({"parts": parts})).await;
        answer["result"]["structuredContent"]["hits"]
            .as_array()
            .unwrap_or_else(|| panic!("{answer}"))
            .iter()
            .map(|hit| hit["uri"].as_str().unwrap().to_owned())
            .collect()
    }

    /// An image part of the artifact's bytes, through a download the test user signs.
    async fn image_of(&self, artifact: ArtifactId, size: usize) -> Value {
        let download = self
            .call("create_download", json!({"artifactId": artifact}))
            .await;
        let get: SignedTransfer =
            serde_json::from_value(download["result"]["structuredContent"]["get"].clone()).unwrap();
        json!({"type": "image", "url": get.url.as_str(), "headers": get.headers,
            "mediaType": "image/png", "size": size})
    }

    fn seen(&self, marker: &str) -> Vec<Seen> {
        self.embedding
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.texts.iter().any(|text| text.contains(marker)))
            .cloned()
            .collect()
    }
}

fn uri(artifact: ArtifactId) -> String {
    format!("artifact://artifacts/{artifact}")
}

fn related_task(answer: &Value) -> Option<&str> {
    answer["result"]["_meta"][veoveo_hub_sdk::RELATED_TASK_META_KEY]["taskId"].as_str()
}

async fn stored_model(harness: &Harness, artifact: ArtifactId) -> Option<String> {
    sqlx::query_scalar("SELECT knowledge_model FROM artifact.artifact WHERE artifact_id = $1")
        .bind(artifact.as_uuid())
        .fetch_one(harness.platform.pool())
        .await
        .unwrap()
}

fn text(query: &str) -> Value {
    json!([{"type": "text", "text": query}])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_uploaded_image_is_found_by_words_and_by_an_image_after_its_index_task() {
    with_service(|harness| async move {
        let mesh = Mesh::start(&harness).await;
        let (kite, completed) = mesh
            .upload(
                "kite.png",
                "image/png",
                KITE,
                json!({"caption": "a red kite over the dunes"}),
            )
            .await;
        // A person's own upload names its index Task, and says nothing else of it.
        let task = related_task(&completed)
            .unwrap_or_else(|| panic!("{completed}"))
            .to_owned();
        assert!(
            completed["result"]["structuredContent"]["indexed"].is_null(),
            "{completed}"
        );
        let settled = mesh.settled(&task).await;
        assert_eq!(settled["result"]["status"], "completed", "{settled}");
        assert_eq!(
            stored_model(&harness, kite).await.as_deref(),
            Some("synthetic/word-hash@1")
        );
        // Embedding was called for the uploader by Artifact from the index Task, with
        // the artifact's name, its metadata and its bytes behind Artifact's signed GET.
        let seen = mesh.seen("a red kite over the dunes");
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].acting_server.as_deref(), Some("artifact"));
        assert_eq!(seen[0].acting_task.as_deref(), Some(task.as_str()));
        assert!(seen[0].texts[0].starts_with("kite.png"), "{seen:?}");

        let (river, completed) = mesh
            .upload(
                "river.png",
                "image/png",
                RIVER,
                json!({"caption": "a slow river at dawn"}),
            )
            .await;
        let settled = mesh.settled(related_task(&completed).unwrap()).await;
        assert_eq!(settled["result"]["status"], "completed", "{settled}");

        // Found by words, and by an image of the same bytes.
        // Words rank the kite first; the river follows only as a distant vector.
        let found = mesh.search(text("kite")).await;
        assert_eq!(found.first(), Some(&uri(kite)), "{found:?}");
        let image = mesh.image_of(kite, KITE.len()).await;
        let found = mesh.search(json!([image])).await;
        assert_eq!(found.first(), Some(&uri(kite)), "{found:?}");
        let image = mesh.image_of(river, RIVER.len()).await;
        let found = mesh.search(json!([image])).await;
        assert_eq!(found.first(), Some(&uri(river)), "{found:?}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_grant_gained_or_lost_changes_what_search_returns() {
    with_service(|harness| async move {
        let mesh = Mesh::start(&harness).await;
        // Another person's artifact in the test user's tenant, found by its words alone.
        let owner = Who::new("owner-1").tenant("tenant-a");
        let mut request = support::upload_request("lantern-plan.txt", Some(5));
        request.media_type = MediaType::parse("text/plain").unwrap();
        let artifact = support::available(harness.service.store(), &owner, request).await;
        let query = text("lantern");
        assert!(mesh.search(query.clone()).await.is_empty());

        // The test user is in `engineering`; a grant to it lets the search find it.
        let grant = GrantArtifactRequest {
            artifact_id: artifact,
            subject: AccessSubject::Group(GroupId::parse("engineering").unwrap()),
            level: AccessLevel::Read,
            expires_at: None,
        };
        harness
            .service
            .grant_artifact(&owner.caller(), &grant, Utc::now())
            .await
            .unwrap();
        assert_eq!(mesh.search(query.clone()).await, [uri(artifact)]);

        harness
            .service
            .revoke_artifact_grant(&owner.caller(), artifact, &grant.subject, Utc::now())
            .await
            .unwrap();
        assert!(mesh.search(query).await.is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_servers_upload_is_indexed_before_its_completion_returns() {
    with_service(|harness| async move {
        let mesh = Mesh::start(&harness).await;
        let issued = forwarded_to_ledger();
        let calls = mesh
            .ledger
            .for_request(&issued.identity, &ForwardedBearer::new(issued.bearer_token))
            .unwrap();
        let mut request = support::upload_request("ledger-scan.png", Some(KITE.len() as u64));
        request.media_type = MediaType::parse("image/png").unwrap();
        request.metadata =
            serde_json::from_value(json!({"caption": "scanned ledger page"})).unwrap();
        let created = calls.call(Checked::new(request).unwrap()).await.unwrap();
        let created = created.metadata().clone();
        let Some(UploadTransfer::Single { put }) = &created.transfer else {
            panic!("a small upload takes one PUT");
        };
        assert!(send(put, KITE.to_vec(), &[]).await.status().is_success());
        let completed = calls
            .call(CompleteUploadRequest {
                upload_id: created.upload.upload_id,
            })
            .await
            .unwrap();
        let completed = completed.metadata().clone();
        let artifact = completed.artifact.artifact_id;
        // Indexed in the call, with its outcome, and no Task.
        let indexed = completed.indexed.expect("the call's indexing outcome");
        assert!(indexed.embedded, "{indexed:?}");
        assert_eq!(
            stored_model(&harness, artifact).await.as_deref(),
            Some("synthetic/word-hash@1")
        );
        let seen = mesh.seen("scanned ledger page");
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].acting_server.as_deref(), Some("artifact"));
        assert_eq!(seen[0].acting_task, None);
        assert_eq!(seen[0].prior_servers, ["ledger"]);
        assert_eq!(mesh.search(text("scanned")).await, [uri(artifact)]);
    })
    .await;
}
