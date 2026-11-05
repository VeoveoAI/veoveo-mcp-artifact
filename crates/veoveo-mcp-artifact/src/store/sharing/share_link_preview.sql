-- veoveo: all tenants (a preview names its link by a token alone, whose digest is unique across tenants)
-- What the preview's token names, while its link is live: not revoked or expired, to a released,
-- available artifact without labels. A link's redemption limit counts visits, so it does not end
-- a preview already opened.
SELECT
    s.tenant AS "tenant: StoredTenant",
    s.artifact_id AS "artifact_id: ArtifactId",
    a.name,
    a.media_type,
    a.size_bytes AS "size_bytes!"
FROM artifact.share_link AS s
JOIN artifact.artifact AS a ON a.tenant = s.tenant AND a.artifact_id = s.artifact_id
WHERE s.preview_digest = $1
    AND s.revoked_at IS NULL
    AND s.expires_at > $2
    AND a.release_state = 'released'
    AND a.state = 'available'
    AND cardinality(a.labels) = 0
    AND a.size_bytes IS NOT NULL
    AND (a.retain_until IS NULL OR a.retain_until > $2)
