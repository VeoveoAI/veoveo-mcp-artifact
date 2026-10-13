-- How many of an artifact's share links are live: neither revoked nor expired.
SELECT count(*) AS "live!"
FROM artifact.share_link
WHERE tenant = $1 AND artifact_id = $2 AND revoked_at IS NULL AND expires_at > $3
