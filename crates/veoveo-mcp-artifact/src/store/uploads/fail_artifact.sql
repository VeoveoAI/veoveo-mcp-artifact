-- The artifact's upload ended without usable bytes: aborted, expired, or holding bytes
-- that did not match what it declared. It holds no size, so triggers release any stored
-- bytes it counted.
UPDATE artifact.artifact
SET state = 'failed', size_bytes = NULL, failed_at = $3
WHERE tenant = $1 AND artifact_id = $2 AND state = 'uploading'
RETURNING artifact_id AS "artifact_id: ArtifactId"
