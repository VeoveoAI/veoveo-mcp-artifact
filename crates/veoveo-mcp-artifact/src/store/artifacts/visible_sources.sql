-- How many of the listed artifacts the caller may read.
-- Parameters $1 to $9 are the caller's facts for artifact.effective_level: tenant,
-- principal, principal key, groups, their roles, Work Context, membership, labels and time.
SELECT count(*) AS "visible!"
FROM artifact.artifact AS a
WHERE a.tenant = $1
    AND a.artifact_id = ANY($10::uuid[])
    AND artifact.effective_level(a, $1, $2, $3, $4, $5, $6, $7, $8, $9) >= 1
