-- The upload an acting identity already created with this idempotency key.
SELECT
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
FROM artifact.upload
WHERE tenant = $1 AND acting_identity = $2 AND idempotency_key = $3
