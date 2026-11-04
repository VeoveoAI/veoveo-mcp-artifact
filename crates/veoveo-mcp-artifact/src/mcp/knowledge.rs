//! Artifact's knowledge search through the SDK's ready-made PostgreSQL search: the
//! collection `artifact.artifacts`, its source, the index Tasks a person's uploads start
//! and the Task service that serves them.
use std::sync::LazyLock;

use chrono::Utc;
use rmcp::{
    ErrorData as McpError, RoleServer,
    model::{
        CallToolRequestParams, CreateTaskResult, GetTaskParams, GetTaskResult, UpdateTaskParams,
    },
    service::RequestContext,
};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use veoveo_hub_sdk::{
    HubInternalIdentity,
    client::HubClient,
    durable_tasks::{
        DurableTaskService, DurableTaskSubscription, TaskWorker, cancel_durable_task,
        get_durable_task, subscribe_durable_tasks, task_owner, update_durable_task,
    },
    hosting::{ToolError, hub_identity},
    knowledge::{
        ContentPart, ContentParts, HeaderName, HeaderValue, KnowledgeCollection, KnowledgeHit,
        KnowledgeValueError, MediaHeaders, MediaSize, Modality, PgKnowledge, PgKnowledgeSource,
        PgKnowledgeTable, SignedMedia, SignedUrl, Snippet,
    },
};
use veoveo_mcp_embedding::{client::EmbeddingClient, contract::MAX_SOURCE_BYTES};
use veoveo_task::{TaskError, TaskFailure, TaskRuntime, TaskTransition};
use veoveo_types::ResourceAddress as _;

use crate::{
    contract::{ArtifactId, ArtifactRecord, ArtifactResource, ArtifactState, MediaType},
    storage::{ObjectKey, StorageError},
    store::{ArtifactStoreError, KnowledgeArtifact},
};

use super::{ArtifactMcp, errors::store_error, identity};

/// Artifact's one searchable collection.
pub fn collection() -> KnowledgeCollection {
    KnowledgeCollection::parse("artifact.artifacts").expect("Artifact's collection")
}

/// Artifact's search: its artifacts, embedded through Embedding for their writers.
pub static KNOWLEDGE: LazyLock<PgKnowledge<ArtifactMcp, EmbeddingClient>> =
    LazyLock::new(|| PgKnowledge::new(EmbeddingClient).source(Artifacts));

/// The artifacts as a knowledge source: `artifact.artifact` by its `artifact_id`, under
/// `artifact.knowledge_readable`.
pub struct Artifacts;

/// Why an artifact's content could not be given to embed. No variant holds a signed URL.
#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error(transparent)]
    Store(#[from] ArtifactStoreError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("the artifact's content is not one Embedding takes: {0}")]
    Value(#[from] KnowledgeValueError),
}

impl PgKnowledgeSource<ArtifactMcp> for Artifacts {
    type Key = Uuid;
    type Error = ContentError;

    fn collection(&self) -> KnowledgeCollection {
        collection()
    }

    fn table(&self) -> PgKnowledgeTable {
        PgKnowledgeTable::new("artifact", "artifact_id", "knowledge_readable")
            .expect("Artifact's knowledge table")
    }

    fn pool<'a>(&self, domain: &'a ArtifactMcp) -> &'a PgPool {
        domain.app.service.store().pool()
    }

    async fn hits(
        &self,
        domain: &ArtifactMcp,
        request: &RequestContext<RoleServer>,
        keys: &[Uuid],
    ) -> Result<Vec<Option<KnowledgeHit>>, ToolError> {
        let caller = identity::caller(&hub_identity(request)?)?;
        let now = Utc::now();
        let mut hits = Vec::with_capacity(keys.len());
        for key in keys {
            let Ok(artifact_id) = ArtifactId::try_from(*key) else {
                hits.push(None);
                continue;
            };
            match domain
                .app
                .service
                .store()
                .artifact(&caller, artifact_id, now)
                .await
            {
                Ok(visible) => hits.push(Some(hit(&visible.record)?)),
                // Gone, or no longer readable, since the ranking.
                Err(ArtifactStoreError::ArtifactNotFound) => hits.push(None),
                Err(error) => return Err(store_error(error).into()),
            }
        }
        Ok(hits)
    }

    async fn content(
        &self,
        domain: &ArtifactMcp,
        _client: &HubClient,
        key: Uuid,
    ) -> Result<Option<ContentParts>, ContentError> {
        let Ok(artifact_id) = ArtifactId::try_from(key) else {
            return Ok(None);
        };
        let service = &domain.app.service;
        let Some(artifact) = service.store().knowledge_artifact(artifact_id).await? else {
            return Ok(None);
        };
        if artifact.state != ArtifactState::Available {
            return Ok(None);
        }
        let mut parts = vec![ContentPart::text(words(
            artifact.name.as_str(),
            artifact.metadata.as_object(),
        ))];
        if let Some(media) = media(service, artifact_id, &artifact).await? {
            parts.push(media);
        }
        Ok(Some(ContentParts::new(parts)?))
    }
}

/// The modality an artifact's media type embeds as, when Embedding takes it.
fn modality(media_type: &MediaType) -> Option<Modality> {
    match media_type.as_str().split('/').next() {
        Some("image") => Some(Modality::Image),
        Some("audio") => Some(Modality::Audio),
        Some("video") => Some(Modality::Video),
        _ => None,
    }
}

/// The artifact's media as Artifact's own signed GET, when its type is an image, audio or
/// video and its size within Embedding's source bound. The URL is on Artifact's signing
/// endpoint, the storage origin Embedding's runtime fetches from.
async fn media(
    service: &crate::service::ArtifactService,
    artifact_id: ArtifactId,
    artifact: &KnowledgeArtifact,
) -> Result<Option<ContentPart>, ContentError> {
    let (Some(modality), Some(size)) = (modality(&artifact.media_type), artifact.size) else {
        return Ok(None);
    };
    if size.bytes() == 0 || size.bytes() > MAX_SOURCE_BYTES {
        return Ok(None);
    }
    let key = ObjectKey::for_artifact(artifact.tenant, artifact_id);
    let get = service
        .storage()
        .sign_get(&key, &artifact.name, &artifact.media_type, Utc::now())
        .await?;
    let headers = MediaHeaders::new(
        get.headers
            .into_iter()
            .map(|(name, value)| Ok((HeaderName::parse(name)?, HeaderValue::parse(value)?)))
            .collect::<Result<Vec<_>, KnowledgeValueError>>()?,
    )?;
    let media = SignedMedia {
        url: SignedUrl::parse(String::from(get.url))?,
        headers,
        media_type: veoveo_hub_sdk::knowledge::MediaType::parse(artifact.media_type.as_str())?,
        size: MediaSize::new(size.bytes())?,
    };
    Ok(Some(ContentPart::media(modality, media)?))
}

/// The text an artifact is found by: its name, then each string of its metadata.
pub fn words(name: &str, metadata: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut text = name.to_owned();
    for value in metadata.values() {
        strings(value, &mut text);
    }
    text
}

fn strings(value: &serde_json::Value, text: &mut String) {
    match value {
        serde_json::Value::String(string) => {
            text.push('\n');
            text.push_str(string);
        }
        serde_json::Value::Array(values) => values.iter().for_each(|value| strings(value, text)),
        serde_json::Value::Object(object) => object.values().for_each(|value| strings(value, text)),
        _ => {}
    }
}

/// A hit: the artifact's address, its name as title and a snippet of its media type,
/// size and metadata.
fn hit(record: &ArtifactRecord) -> Result<KnowledgeHit, McpError> {
    let uri = ArtifactResource::Artifact(record.artifact_id)
        .to_uri()
        .map_err(|_| McpError::internal_error("an artifact's address", None))?;
    let mut snippet = record.media_type.as_str().to_owned();
    if let Some(size) = record.size {
        snippet.push_str(&format!(", {} bytes", size.bytes()));
    }
    let metadata = words("", record.metadata.as_object());
    let metadata = metadata.trim();
    if !metadata.is_empty() {
        snippet.push_str(": ");
        snippet.push_str(&metadata.replace('\n', "; "));
    }
    Ok(KnowledgeHit {
        uri,
        collection: collection(),
        title: record.name.as_str().to_owned(),
        snippet: Snippet::new(snippet),
        modified_at: record.available_at,
    })
}

/// Artifact's durable Tasks for their callers: the index Tasks a person's uploads start.
/// Artifact has no Task tool, so a tool call starts none.
#[derive(Clone)]
pub struct ArtifactTasks {
    runtime: TaskRuntime,
}

impl ArtifactTasks {
    pub fn new(runtime: TaskRuntime) -> Self {
        Self { runtime }
    }
}

impl DurableTaskService for ArtifactTasks {
    type Caller = HubInternalIdentity;

    fn authenticate(&self, context: &RequestContext<RoleServer>) -> Result<Self::Caller, McpError> {
        hub_identity(context)
    }

    async fn start_tool_task(
        &self,
        _: &Self::Caller,
        _: CallToolRequestParams,
    ) -> Result<Option<CreateTaskResult>, McpError> {
        Ok(None)
    }

    async fn get_task(
        &self,
        caller: &Self::Caller,
        request: GetTaskParams,
    ) -> Result<GetTaskResult, McpError> {
        get_durable_task(
            &self.runtime,
            &self.runtime.for_owner(&task_owner(caller)?),
            request,
        )
        .await
    }

    async fn update_task(
        &self,
        caller: &Self::Caller,
        request: UpdateTaskParams,
    ) -> Result<(), McpError> {
        update_durable_task(&self.runtime.for_owner(&task_owner(caller)?), request).await
    }

    async fn cancel_task(&self, caller: &Self::Caller, task_id: String) -> Result<(), McpError> {
        cancel_durable_task(&self.runtime.for_owner(&task_owner(caller)?), task_id).await
    }

    async fn subscribe_tasks(
        &self,
        caller: &Self::Caller,
        task_ids: Vec<String>,
    ) -> Result<DurableTaskSubscription, McpError> {
        subscribe_durable_tasks(
            &self.runtime,
            &self.runtime.for_owner(&task_owner(caller)?),
            task_ids,
        )
        .await
    }
}

/// Run Artifact's Tasks until `stop`: each index Task embeds its artifacts as their
/// writer, through the worker's calls.
pub async fn work(
    worker: TaskWorker,
    domain: ArtifactMcp,
    stop: CancellationToken,
) -> Result<(), TaskError> {
    worker
        .run(
            move |ctx| {
                let domain = domain.clone();
                async move {
                    if KNOWLEDGE.runs(ctx.task()) {
                        return Ok::<_, TaskError>(KNOWLEDGE.run_index(&domain, &ctx).await);
                    }
                    Ok(TaskTransition::Failed(TaskFailure::new(
                        "unknown_task",
                        "Artifact runs no Task of this type",
                    )))
                }
            },
            stop,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_artifact_is_found_by_its_name_and_its_metadatas_strings() {
        let metadata = serde_json::json!({
            "caption": "red kite",
            "tags": ["dunes", {"place": "coast"}],
            "count": 3
        });
        assert_eq!(
            words("kite.png", metadata.as_object().unwrap()),
            "kite.png\nred kite\ndunes\ncoast"
        );
        assert_eq!(
            modality(&MediaType::parse("video/mp4").unwrap()),
            Some(Modality::Video)
        );
        assert_eq!(modality(&MediaType::parse("text/plain").unwrap()), None);
    }
}
