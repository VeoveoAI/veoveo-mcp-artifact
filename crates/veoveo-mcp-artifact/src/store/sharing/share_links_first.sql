-- The first page of an artifact's share links, newest first, with one row of lookahead.
SELECT
    share_link_id AS "share_link_id: ShareLinkId",
    artifact_id AS "artifact_id: ArtifactId",
    expires_at,
    redemption_limit,
    redemptions,
    revoked_at,
    created_by,
    created_at
FROM artifact.share_link
WHERE tenant = $1 AND artifact_id = $2
ORDER BY share_link_id DESC
LIMIT $3
