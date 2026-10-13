-- The artifact's bytes arrived. With a SHA-256 that object storage checked, the artifact
-- is available and its retention starts; otherwise it is verifying until the server
-- records the digest. Triggers add the size to the tenant's stored bytes.
UPDATE artifact.artifact AS a
SET size_bytes = $3,
    sha256 = $4,
    state = CASE WHEN $4::text IS NULL THEN 'verifying' ELSE 'available' END,
    available_at = CASE WHEN $4::text IS NULL THEN NULL ELSE $5::timestamptz END,
    retain_until = CASE
        WHEN $4::text IS NOT NULL AND a.retention_days IS NOT NULL
            THEN $5::timestamptz + make_interval(days => a.retention_days)
    END
WHERE a.tenant = $1 AND a.artifact_id = $2 AND a.state = 'uploading'
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
