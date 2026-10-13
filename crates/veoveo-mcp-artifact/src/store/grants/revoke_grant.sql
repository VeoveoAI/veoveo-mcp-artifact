-- Remove a subject's grant on an artifact.
DELETE FROM artifact.grant
WHERE tenant = $1 AND artifact_id = $2 AND subject_kind = $3 AND subject = $4
RETURNING subject
