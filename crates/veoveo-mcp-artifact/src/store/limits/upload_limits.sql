-- A tenant's own upload limits. A tenant without a row has the default limits.
SELECT quota_bytes, largest_object_bytes, allowed_media_types, active_uploads
FROM artifact.upload_limits
WHERE tenant = $1
