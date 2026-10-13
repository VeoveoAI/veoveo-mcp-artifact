-- One access request, locked until the decision commits.
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
WHERE r.tenant = $1 AND r.access_request_id = $2
FOR UPDATE
