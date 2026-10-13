-- veoveo: all tenants (the sweep removes every tenant's deleted artifacts in batches)
-- Claim one batch of artifacts deleted by `$2`, oldest first, for the rest of the
-- transaction.
-- The caller deletes each object, then each row, before it commits.
SELECT tenant AS "tenant: StoredTenant", artifact_id AS "artifact_id: ArtifactId"
FROM artifact.artifact
WHERE state = 'deleting' AND deleting_at <= $2
ORDER BY deleting_at
LIMIT $1
FOR UPDATE SKIP LOCKED
