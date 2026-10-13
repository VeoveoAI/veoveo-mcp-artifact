-- Remove a deleted artifact whose object is gone, with its grants, uploads, share links
-- and access requests.
DELETE FROM artifact.artifact
-- `deleting_at` is set exactly while the artifact is deleting; testing it keeps the
-- statement on the primary key rather than the sweep's partial index.
WHERE tenant = $1 AND artifact_id = $2 AND deleting_at IS NOT NULL
