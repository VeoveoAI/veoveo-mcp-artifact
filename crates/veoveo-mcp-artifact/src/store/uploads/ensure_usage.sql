-- Give the tenant a usage row, so a reservation can lock it.
INSERT INTO artifact.storage_usage (tenant, stored_bytes, reserved_bytes, open_uploads)
VALUES ($1, 0, 0, 0)
ON CONFLICT (tenant) DO NOTHING
