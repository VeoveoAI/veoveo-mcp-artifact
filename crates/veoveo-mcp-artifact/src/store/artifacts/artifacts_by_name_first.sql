-- The first page of the readable artifacts whose name starts with a prefix, ordered by
-- name, then ID. $8 is the prefix and $9 the prefix followed by U+10FFFF, which no name
-- holds, so the two bound exactly the names that start with the prefix.
-- Parameters $1 to $7 are the caller's facts for artifact.effective_level: tenant,
-- principal, principal key, groups, their roles, labels and time.
SELECT
    a.artifact_id AS "artifact_id: ArtifactId",
    a.owner_key,
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
    artifact.effective_level(a, $1, $2, $3, $4, $5, $6, $7) AS "level!: i16"
FROM artifact.artifact AS a
WHERE a.tenant = $1
    AND a.name >= $8
    AND a.name < $9
    AND a.state = 'available'
    AND artifact.effective_level(a, $1, $2, $3, $4, $5, $6, $7) >= 1
ORDER BY a.name, a.artifact_id
LIMIT $10
