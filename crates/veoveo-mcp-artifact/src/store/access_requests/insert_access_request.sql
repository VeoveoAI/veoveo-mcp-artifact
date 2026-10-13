-- Record a pending request. A requester holds at most one pending request per artifact,
-- so a second one inserts nothing.
INSERT INTO artifact.access_request AS r (
    tenant, access_request_id, artifact_id, requester, level, justification, state, created_at
)
VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7)
ON CONFLICT (tenant, artifact_id, requester) WHERE state = 'pending' DO NOTHING
RETURNING
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
