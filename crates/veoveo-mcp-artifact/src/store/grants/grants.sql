-- An artifact's grants in subject order. An artifact holds at most 256, so one row past
-- that would show a broken limit.
SELECT subject_kind, subject, level, expires_at, granted_by, granted_at
FROM artifact.grant
WHERE tenant = $1 AND artifact_id = $2
ORDER BY subject_kind, subject
LIMIT 257
