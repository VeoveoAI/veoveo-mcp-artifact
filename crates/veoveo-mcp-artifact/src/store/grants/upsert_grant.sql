-- Give a subject a level on an artifact, replacing its earlier grant.
INSERT INTO artifact.grant AS g (
    tenant, artifact_id, subject_kind, subject, level, expires_at, granted_by, granted_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
ON CONFLICT (tenant, artifact_id, subject_kind, subject) DO UPDATE
SET level = excluded.level,
    expires_at = excluded.expires_at,
    granted_by = excluded.granted_by,
    granted_at = excluded.granted_at
RETURNING g.subject_kind, g.subject, g.level, g.expires_at, g.granted_by, g.granted_at
