-- The first page of the caller's uploads, newest first, with one row of lookahead.
SELECT
    upload_id AS "upload_id: UploadId",
    artifact_id AS "artifact_id: ArtifactId",
    creator AS "creator: StoredPrincipal",
    form AS "form: UploadForm",
    state AS "state: UploadState",
    declared_size,
    declared_sha256,
    max_size,
    reserved_bytes,
    object_upload,
    part_size,
    created_at,
    expires_at,
    ended_at
FROM artifact.upload
WHERE tenant = $1 AND creator = $2
ORDER BY upload_id DESC
LIMIT $3
