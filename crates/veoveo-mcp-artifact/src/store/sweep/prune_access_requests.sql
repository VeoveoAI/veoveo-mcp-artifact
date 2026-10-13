-- veoveo: all tenants (the sweep prunes every tenant's decided access requests in batches)
-- Delete one batch of access requests decided or cancelled before the cutoff.
WITH due AS (
    SELECT tenant, access_request_id
    FROM artifact.access_request
    WHERE decided_at < $1 AND decided_at IS NOT NULL
    ORDER BY decided_at
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
DELETE FROM artifact.access_request AS r
USING due
WHERE r.tenant = due.tenant AND r.access_request_id = due.access_request_id
