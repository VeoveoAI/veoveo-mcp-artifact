-- veoveo: all tenants (the sweep prunes every tenant's expired grants in batches)
-- Delete one batch of grants past their expiry.
WITH due AS (
    SELECT tenant, artifact_id, subject_kind, subject
    FROM artifact.grant
    WHERE expires_at <= $1 AND expires_at IS NOT NULL
    ORDER BY expires_at
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
DELETE FROM artifact.grant AS g
USING due
WHERE g.tenant = due.tenant
    AND g.artifact_id = due.artifact_id
    AND g.subject_kind = due.subject_kind
    AND g.subject = due.subject
