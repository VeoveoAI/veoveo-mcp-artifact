-- veoveo: all tenants (the sweep prunes every tenant's ended share links in batches)
-- Delete one batch of share links that expired or were revoked before the cutoff.
WITH due AS (
    SELECT tenant, share_link_id
    FROM artifact.share_link
    WHERE coalesce(revoked_at, expires_at) < $1
    ORDER BY coalesce(revoked_at, expires_at)
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
DELETE FROM artifact.share_link AS s
USING due
WHERE s.tenant = due.tenant AND s.share_link_id = due.share_link_id
