-- The server computed a verifying artifact's SHA-256: the artifact is available and its
-- retention starts.
UPDATE artifact.artifact AS a
SET sha256 = $3,
    state = 'available',
    available_at = $4::timestamptz,
    retain_until = CASE
        WHEN a.retention_days IS NOT NULL THEN $4::timestamptz + make_interval(days => a.retention_days)
    END
WHERE a.tenant = $1 AND a.artifact_id = $2 AND a.state = 'verifying'
RETURNING a.artifact_id AS "artifact_id: ArtifactId"
