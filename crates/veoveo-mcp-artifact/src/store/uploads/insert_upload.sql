-- The open upload that reserves the artifact's bytes. Triggers add the reservation to
-- the tenant's usage.
INSERT INTO artifact.upload (
    tenant, upload_id, artifact_id, creator, acting_identity, idempotency_key, form, state,
    declared_size, max_size, reserved_bytes, part_size, created_at, expires_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, 'open', $8, $9, $10, $11, $12, $13)
RETURNING
    upload_id AS "upload_id: UploadId",
    artifact_id AS "artifact_id: ArtifactId",
    creator AS "creator: StoredPrincipal",
    form AS "form: UploadForm",
    state AS "state: UploadState",
    declared_size,
    max_size,
    reserved_bytes,
    object_upload,
    part_size,
    created_at,
    expires_at,
    ended_at
