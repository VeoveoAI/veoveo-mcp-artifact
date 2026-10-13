-- End an artifact's live share links when it returns to private. An artifact holds at
-- most 256 live links, which creation counts under the artifact's lock.
WITH live AS (
    SELECT tenant, share_link_id
    FROM artifact.share_link
    WHERE tenant = $1 AND artifact_id = $2 AND revoked_at IS NULL AND expires_at > $3
    LIMIT 256
    FOR UPDATE
)
UPDATE artifact.share_link AS s
SET revoked_at = $3
FROM live
WHERE s.tenant = live.tenant AND s.share_link_id = live.share_link_id
RETURNING s.share_link_id AS "share_link_id: ShareLinkId"
