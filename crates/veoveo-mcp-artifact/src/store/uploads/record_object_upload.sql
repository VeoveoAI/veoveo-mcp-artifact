-- The object store's identifier for an open multipart upload.
UPDATE artifact.upload
SET object_upload = $3
-- `ended_at` is null exactly while the upload is open.
WHERE tenant = $1 AND upload_id = $2 AND ended_at IS NULL AND object_upload IS NULL
RETURNING upload_id AS "upload_id: UploadId"
