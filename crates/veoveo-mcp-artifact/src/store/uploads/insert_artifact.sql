-- A new artifact waiting for its bytes. Its creator owns it, so the creator administers it.
INSERT INTO artifact.artifact AS a (
    tenant, artifact_id, owner, owner_key, work_context, name, media_type, labels,
    release_state, state, provenance, metadata, retention_days, created_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'private', 'uploading', $9, $10, $11, $12)
RETURNING
    a.artifact_id AS "artifact_id: ArtifactId",
    a.owner_key,
    a.work_context,
    a.name,
    a.media_type,
    a.size_bytes,
    a.sha256,
    a.labels,
    a.release_state AS "release_state: ReleaseState",
    a.state AS "state: ArtifactState",
    a.provenance AS "provenance: Json<ArtifactProvenance>",
    a.metadata AS "metadata: Json<ArtifactMetadataMap>",
    a.created_at,
    a.available_at,
    a.retain_until,
    3::smallint AS "level!: i16"
