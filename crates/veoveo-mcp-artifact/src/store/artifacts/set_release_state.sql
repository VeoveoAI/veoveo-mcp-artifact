-- Move an artifact between private and released.
UPDATE artifact.artifact
SET release_state = $3
WHERE tenant = $1 AND artifact_id = $2 AND state <> 'deleting'
RETURNING artifact_id AS "artifact_id: ArtifactId"
