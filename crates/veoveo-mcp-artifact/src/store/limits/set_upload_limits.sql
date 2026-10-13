-- Replace a tenant's upload limits.
INSERT INTO artifact.upload_limits (
    tenant, quota_bytes, largest_object_bytes, allowed_media_types, active_uploads,
    updated_by, updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (tenant) DO UPDATE
SET quota_bytes = excluded.quota_bytes,
    largest_object_bytes = excluded.largest_object_bytes,
    allowed_media_types = excluded.allowed_media_types,
    active_uploads = excluded.active_uploads,
    updated_by = excluded.updated_by,
    updated_at = excluded.updated_at
RETURNING quota_bytes, largest_object_bytes, allowed_media_types, active_uploads
