-- veoveo: all tenants (the sweep removes every tenant's failed artifacts in batches)
-- Remove one batch of artifacts that failed before the cutoff. A failed artifact holds no
-- object, so its row goes at once with its upload.
WITH due AS (
    SELECT tenant, artifact_id
    FROM artifact.artifact
    WHERE state = 'failed' AND failed_at < $1
    ORDER BY failed_at
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
DELETE FROM artifact.artifact AS a
USING due
WHERE a.tenant = due.tenant AND a.artifact_id = due.artifact_id
