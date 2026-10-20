-- One artifact and the caller's level on it, locked until the change commits.
-- Parameters $1 to $9 are the caller's facts for artifact.effective_level: tenant,
-- principal, principal key, groups, their roles, Work Context, membership, labels and time.
SELECT
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
    artifact.effective_level(a, $1, $2, $3, $4, $5, $6, $7, $8, $9) AS "level!: i16"
FROM artifact.artifact AS a
WHERE a.tenant = $1 AND a.artifact_id = $10
FOR UPDATE OF a
