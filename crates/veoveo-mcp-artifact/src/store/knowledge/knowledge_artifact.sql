-- veoveo: all tenants (the knowledge index embeds one artifact, whose ID is unique across tenants)
-- One artifact as its knowledge index embeds it: its tenant, its words, its media and its
-- state.
SELECT
    a.tenant AS "tenant: StoredTenant",
    a.name,
    a.media_type,
    a.size_bytes,
    a.state AS "state: ArtifactState",
    a.metadata AS "metadata: Json<ArtifactMetadataMap>"
FROM artifact.artifact AS a
WHERE a.artifact_id = $1
