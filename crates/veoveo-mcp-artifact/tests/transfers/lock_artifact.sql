SELECT a.artifact_id AS "artifact_id!"
FROM artifact.artifact AS a
WHERE a.artifact_id = $1
FOR UPDATE
