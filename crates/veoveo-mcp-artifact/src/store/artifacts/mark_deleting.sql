-- Delete an artifact: it stops existing for every caller at once, and the sweep removes
-- its object and then its row, whose grants, uploads, share links and access requests
-- go with it.
UPDATE artifact.artifact
SET state = 'deleting', deleting_at = $3
WHERE tenant = $1 AND artifact_id = $2 AND state <> 'deleting'
RETURNING artifact_id AS "artifact_id: ArtifactId", state AS "state: ArtifactState"
