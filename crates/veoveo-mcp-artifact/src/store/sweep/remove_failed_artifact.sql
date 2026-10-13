-- Remove a failed artifact whose object is gone, with its upload.
DELETE FROM artifact.artifact
-- `failed_at` is set exactly while the artifact is failed; testing it keeps the statement
-- on the primary key rather than the sweep's partial index.
WHERE tenant = $1 AND artifact_id = $2 AND failed_at IS NOT NULL
