-- Stored bytes passed size validation. Triggers settle reservation/stored usage.
UPDATE artifact.artifact AS a
SET size_bytes = $3,
    state = 'available',
    available_at = $4,
    retain_until = CASE
        WHEN a.retention_days IS NOT NULL
            THEN $4::timestamptz + make_interval(days => a.retention_days)
    END
WHERE a.tenant = $1 AND a.artifact_id = $2 AND a.state = 'uploading'
RETURNING
    a.artifact_id AS "artifact_id: ArtifactId",
    a.owner_key,
    a.work_context,
    a.name,
    a.media_type,
    a.size_bytes,
    a.labels,
    a.release_state AS "release_state: ReleaseState",
    a.state AS "state: ArtifactState",
    a.provenance AS "provenance: Json<ArtifactProvenance>",
    a.metadata AS "metadata: Json<ArtifactMetadataMap>",
    a.created_at,
    a.available_at,
    a.retain_until,
    3::smallint AS "level!: i16"
