-- The tenant's usage and limits, with the usage row locked until the reservation
-- commits, so two uploads cannot both take the last of a quota.
SELECT
    u.stored_bytes,
    u.reserved_bytes,
    u.open_uploads,
    l.quota_bytes AS "quota_bytes?",
    l.largest_object_bytes AS "largest_object_bytes?",
    l.allowed_media_types AS "allowed_media_types?",
    l.active_uploads AS "active_uploads?"
FROM artifact.storage_usage AS u
LEFT JOIN artifact.upload_limits AS l ON l.tenant = u.tenant
WHERE u.tenant = $1
FOR UPDATE OF u
