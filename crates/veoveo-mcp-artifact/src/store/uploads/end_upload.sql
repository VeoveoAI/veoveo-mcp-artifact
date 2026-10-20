-- End an open upload as completed, aborted or expired. Triggers release its reservation.
UPDATE artifact.upload
SET state = $3, ended_at = $4
-- `ended_at` is null exactly while the upload is open; testing it keeps the statement on
-- the primary key rather than the expiry queue's partial index.
WHERE tenant = $1 AND upload_id = $2 AND ended_at IS NULL
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
