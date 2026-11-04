//! What the knowledge index reads of an artifact to embed it. The search's own
//! statements, its ranking and its vector writes, are the SDK's; this is the one read
//! Artifact gives it.
use sqlx::types::Json;
use veoveo_store::TenantId as StoredTenant;

use crate::contract::{
    ArtifactId, ArtifactMetadataMap, ArtifactName, ArtifactState, MediaType, ObjectSize,
};

use super::{ArtifactStore, ArtifactStoreError, rows::object_size};

struct KnowledgeRow {
    tenant: StoredTenant,
    name: String,
    media_type: String,
    size_bytes: Option<i64>,
    state: ArtifactState,
    metadata: Json<ArtifactMetadataMap>,
}

/// One artifact as its knowledge index embeds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeArtifact {
    pub tenant: StoredTenant,
    pub name: ArtifactName,
    pub media_type: MediaType,
    /// Known once its bytes arrived.
    pub size: Option<ObjectSize>,
    pub state: ArtifactState,
    pub metadata: ArtifactMetadataMap,
}

impl ArtifactStore {
    /// The artifact `artifact_id` of any tenant, for its knowledge index, which embeds the
    /// record itself for whoever wrote it.
    pub async fn knowledge_artifact(
        &self,
        artifact_id: ArtifactId,
    ) -> Result<Option<KnowledgeArtifact>, ArtifactStoreError> {
        let row = sqlx::query_file_as!(
            KnowledgeRow,
            "src/store/knowledge/knowledge_artifact.sql",
            artifact_id as ArtifactId,
        )
        .fetch_optional(self.pool())
        .await?;
        row.map(|row| {
            Ok(KnowledgeArtifact {
                tenant: row.tenant,
                name: ArtifactName::parse(row.name)
                    .map_err(|_| ArtifactStoreError::Corrupt("artifact name"))?,
                media_type: MediaType::parse(row.media_type)
                    .map_err(|_| ArtifactStoreError::Corrupt("artifact media type"))?,
                size: row.size_bytes.map(object_size).transpose()?,
                state: row.state,
                metadata: row.metadata.0,
            })
        })
        .transpose()
    }
}
