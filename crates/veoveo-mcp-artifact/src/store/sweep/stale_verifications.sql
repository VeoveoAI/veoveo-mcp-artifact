-- veoveo: all tenants (the sweep checks every tenant's verifying artifacts)
-- Verifying artifacts whose upload completed by `$1`, for the sweep to check that their
-- verification Task still runs.
SELECT a.tenant AS "tenant: StoredTenant", a.artifact_id AS "artifact_id: ArtifactId"
FROM artifact.artifact AS a
JOIN artifact.upload AS u ON u.tenant = a.tenant AND u.artifact_id = a.artifact_id
WHERE a.state = 'verifying' AND u.state = 'completed' AND u.ended_at <= $1
ORDER BY a.tenant, a.artifact_id
LIMIT $2
