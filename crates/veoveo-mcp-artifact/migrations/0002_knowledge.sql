-- The knowledge contract through the SDK's PostgreSQL search: the indexes over an
-- artifact's words and vector, which its table carries from migration 1, and the read rule
-- its search shares with its listings.

CREATE INDEX artifact_knowledge_text ON artifact.artifact USING gin (knowledge_text);
CREATE INDEX artifact_knowledge_vector ON artifact.artifact
    USING hnsw (knowledge_vector _extensions.vector_cosine_ops);
-- An artifact's ID is unique across tenants; the search's index reads and writes one
-- artifact by it.
CREATE UNIQUE INDEX artifact_knowledge_key ON artifact.artifact (artifact_id);

-- Whether the search may show `a` to `caller`, the SDK's `KnowledgeCaller`: an available
-- artifact whose level for the caller is read or more, decided as every listing decides
-- it, so a label outside the caller's data labels hides it.
CREATE FUNCTION artifact.knowledge_readable(a artifact.artifact, caller jsonb) RETURNS boolean
LANGUAGE sql STABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
RETURN a.state = 'available'
    AND artifact.effective_level(
        a,
        (caller->>'tenantId')::uuid,
        (caller->>'principalId')::uuid,
        caller->>'principalKey',
        ARRAY(
            SELECT e.membership->>'group'
            FROM jsonb_array_elements(caller->'groups') WITH ORDINALITY AS e (membership, position)
            ORDER BY e.position
        ),
        ARRAY(
            SELECT e.membership->>'role'
            FROM jsonb_array_elements(caller->'groups') WITH ORDINALITY AS e (membership, position)
            ORDER BY e.position
        ),
        ARRAY(SELECT jsonb_array_elements_text(caller->'dataLabels')),
        (caller->>'asOf')::timestamptz
    ) >= 1;
