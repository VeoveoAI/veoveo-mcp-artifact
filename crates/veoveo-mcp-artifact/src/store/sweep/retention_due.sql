-- veoveo: all tenants (the sweep deletes every tenant's artifacts whose retention passed)
-- Mark one batch of artifacts past their retention as deleting, oldest first.
WITH due AS (
    SELECT tenant, artifact_id
    FROM artifact.artifact
    WHERE retain_until <= $1 AND retain_until IS NOT NULL AND state IN ('verifying', 'available')
    ORDER BY retain_until
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
UPDATE artifact.artifact AS a
SET state = 'deleting', deleting_at = $1
FROM due
WHERE a.tenant = due.tenant AND a.artifact_id = due.artifact_id
