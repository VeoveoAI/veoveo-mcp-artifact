-- Name the preview of the link being redeemed, by the digest of the token its origin holds.
UPDATE artifact.share_link
SET preview_digest = $3
WHERE tenant = $1 AND share_link_id = $2
RETURNING share_link_id AS "share_link_id: ShareLinkId"
