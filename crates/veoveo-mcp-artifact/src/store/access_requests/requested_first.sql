-- The first page of the requests the caller made, newest first, optionally in one state,
-- with one row of lookahead. A request on a deleted artifact is gone for its requester.
SELECT
    r.access_request_id AS "access_request_id: AccessRequestId",
    r.artifact_id AS "artifact_id: ArtifactId",
    r.requester,
    r.level,
    r.justification,
    r.state AS "state: AccessRequestState",
    r.decided_by,
    r.note,
    r.created_at,
    r.decided_at
FROM artifact.access_request AS r
JOIN artifact.artifact AS a ON a.tenant = r.tenant AND a.artifact_id = r.artifact_id
WHERE r.tenant = $1
    AND r.requester = $2
    AND ($3::text IS NULL OR r.state = $3)
    AND a.state <> 'deleting'
ORDER BY r.access_request_id DESC
LIMIT $4
