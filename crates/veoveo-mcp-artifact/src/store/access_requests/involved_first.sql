-- The first page of the requests the caller made or can decide, newest first, optionally
-- in one state, with one row of lookahead.
-- Parameters $1 to $9 are the caller's facts for artifact.effective_level: tenant,
-- principal, principal key, groups, their roles, Work Context, membership, labels and time.
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
    AND ($10::text IS NULL OR r.state = $10)
    AND a.state <> 'deleting'
    AND (r.requester = $3
        OR artifact.effective_level(a, $1, $2, $3, $4, $5, $6, $7, $8, $9) = 3)
ORDER BY r.access_request_id DESC
LIMIT $11
