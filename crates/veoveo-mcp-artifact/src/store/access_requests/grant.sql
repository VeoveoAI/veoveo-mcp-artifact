-- One subject's grant on an artifact.
SELECT subject_kind, subject, level, expires_at, granted_by, granted_at
FROM artifact.grant
WHERE tenant = $1 AND artifact_id = $2 AND subject_kind = $3 AND subject = $4
