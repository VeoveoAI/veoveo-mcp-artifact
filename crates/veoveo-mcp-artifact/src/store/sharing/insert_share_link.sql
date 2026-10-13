-- Record a share link by its secret's digest; the secret itself is never stored.
INSERT INTO artifact.share_link (
    tenant, share_link_id, artifact_id, secret_digest, expires_at, redemption_limit,
    redemptions, created_by, created_at
)
VALUES ($1, $2, $3, $4, $5, $6, 0, $7, $8)
RETURNING
    share_link_id AS "share_link_id: ShareLinkId",
    artifact_id AS "artifact_id: ArtifactId",
    expires_at,
    redemption_limit,
    redemptions,
    revoked_at,
    created_by,
    created_at
