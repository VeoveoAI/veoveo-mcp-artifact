-- veoveo: all tenants (a share link names its artifact by its secret alone, and the secret's digest is unique across tenants)
-- Count one redemption of a live link to a released, available artifact without labels,
-- and return what the download URL names. The row lock and the limit in the WHERE clause
-- let at most the link's limit of concurrent redemptions through.
UPDATE artifact.share_link AS s
SET redemptions = s.redemptions + 1
FROM artifact.artifact AS a
WHERE s.secret_digest = $1
    AND s.revoked_at IS NULL
    AND s.expires_at > $2
    AND (s.redemption_limit IS NULL OR s.redemptions < s.redemption_limit)
    AND a.tenant = s.tenant
    AND a.artifact_id = s.artifact_id
    AND a.release_state = 'released'
    AND a.state = 'available'
    AND cardinality(a.labels) = 0
    AND (a.retain_until IS NULL OR a.retain_until > $2)
RETURNING
    s.tenant AS "tenant: StoredTenant",
    s.share_link_id AS "share_link_id: ShareLinkId",
    s.artifact_id AS "artifact_id: ArtifactId",
    s.redemptions,
    a.name,
    a.media_type
