-- The tenant's stored and reserved bytes. A tenant without a row has stored nothing.
SELECT stored_bytes, reserved_bytes
FROM artifact.storage_usage
WHERE tenant = $1
