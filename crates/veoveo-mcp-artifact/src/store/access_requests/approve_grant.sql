-- Give an approved requester the level it asked for, unless a grant it already holds
-- gives at least that level; such a grant stays as it is.
INSERT INTO artifact.grant AS g (
    tenant, artifact_id, subject_kind, subject, level, expires_at, granted_by, granted_at
)
VALUES ($1, $2, 'principal', $3, $4, NULL, $5, $6)
ON CONFLICT (tenant, artifact_id, subject_kind, subject) DO UPDATE
SET level = excluded.level,
    expires_at = NULL,
    granted_by = excluded.granted_by,
    granted_at = excluded.granted_at
WHERE artifact.level_rank(g.level) < artifact.level_rank(excluded.level)
    OR (artifact.level_rank(g.level) = artifact.level_rank(excluded.level)
        AND g.expires_at IS NOT NULL)
RETURNING g.subject
