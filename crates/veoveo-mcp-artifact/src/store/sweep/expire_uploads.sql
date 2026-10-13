-- veoveo: all tenants (the sweep ends every tenant's expired uploads in batches)
-- End one batch of open uploads past their expiry, oldest first, and fail their
-- artifacts. Replicas sweeping at once take different rows. The caller aborts each
-- returned multipart upload in object storage.
WITH due AS (
    SELECT tenant, upload_id
    FROM artifact.upload
    WHERE state = 'open' AND expires_at <= $1
    ORDER BY expires_at
    LIMIT $2
    FOR UPDATE SKIP LOCKED
),
ended AS (
    UPDATE artifact.upload AS u
    SET state = 'expired', ended_at = $1
    FROM due
    WHERE u.tenant = due.tenant AND u.upload_id = due.upload_id
    RETURNING u.tenant, u.upload_id, u.artifact_id, u.object_upload
),
failed AS (
    UPDATE artifact.artifact AS a
    SET state = 'failed', failed_at = $1
    FROM ended
    WHERE a.tenant = ended.tenant
        AND a.artifact_id = ended.artifact_id
        AND a.state = 'uploading'
    RETURNING a.artifact_id
)
SELECT
    ended.tenant AS "tenant!: StoredTenant",
    ended.upload_id AS "upload_id!: UploadId",
    ended.artifact_id AS "artifact_id!: ArtifactId",
    ended.object_upload
FROM ended
