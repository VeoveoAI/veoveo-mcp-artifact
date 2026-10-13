-- veoveo: all tenants (the sweep removes every tenant's failed artifacts in batches)
-- Claim one batch of artifacts that failed before `$2`, oldest first, for the rest of the
-- transaction. The caller deletes each object, then each row, before it commits.
SELECT tenant AS "tenant: StoredTenant", artifact_id AS "artifact_id: ArtifactId"
FROM artifact.artifact
WHERE state = 'failed' AND failed_at < $2
ORDER BY failed_at
LIMIT $1
FOR UPDATE SKIP LOCKED
