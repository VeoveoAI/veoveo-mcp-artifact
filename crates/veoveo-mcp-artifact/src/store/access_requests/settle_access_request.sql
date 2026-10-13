-- Settle a pending request: cancelled by its requester, or approved or denied by an
-- administrator with an optional note.
UPDATE artifact.access_request AS r
SET state = $3, decided_by = $4, note = $5, decided_at = $6
WHERE r.tenant = $1 AND r.access_request_id = $2 AND r.state = 'pending'
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
