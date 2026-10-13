-- Revoke one of an artifact's share links. A link revoked earlier keeps its revocation.
UPDATE artifact.share_link
SET revoked_at = coalesce(revoked_at, $4)
WHERE tenant = $1 AND share_link_id = $3 AND artifact_id = $2
RETURNING
    share_link_id AS "share_link_id: ShareLinkId",
    artifact_id AS "artifact_id: ArtifactId",
    expires_at,
    redemption_limit,
    redemptions,
    revoked_at,
    created_by,
    created_at
