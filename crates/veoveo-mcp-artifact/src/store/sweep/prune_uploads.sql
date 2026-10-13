-- veoveo: all tenants (the sweep prunes every tenant's ended uploads in batches)
-- Delete one batch of uploads that ended before the cutoff.
WITH due AS (
    SELECT tenant, upload_id
    FROM artifact.upload
    WHERE ended_at < $1 AND ended_at IS NOT NULL
    ORDER BY ended_at
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
DELETE FROM artifact.upload AS u
USING due
WHERE u.tenant = due.tenant AND u.upload_id = due.upload_id
