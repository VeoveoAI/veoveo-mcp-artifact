-- How many grants an artifact holds, and whether the subject already holds one.
SELECT
    count(*) AS "grants!",
    count(*) FILTER (WHERE subject_kind = $3 AND subject = $4) AS "held!"
FROM artifact.grant
WHERE tenant = $1 AND artifact_id = $2
