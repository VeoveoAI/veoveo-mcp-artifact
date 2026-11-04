-- The knowledge contract through the SDK's PostgreSQL search: an artifact's words, its
-- vector and the model that made it, and the read rule its search shares with its
-- listings. pgvector lives in the installation's `_extensions` schema.

-- Its name, whole and split at `.`, `_` and `-` so `kite.png` is found by `kite`, and
-- the string values of its metadata, which PostgreSQL keeps current.
ALTER TABLE artifact.artifact
    ADD COLUMN knowledge_text tsvector GENERATED ALWAYS AS (
        to_tsvector('simple', name || ' ' || translate(name, '._-', '   '))
        || jsonb_to_tsvector('simple', metadata, '["string"]')
    ) STORED,
    ADD COLUMN knowledge_vector _extensions.vector(768),
    ADD COLUMN knowledge_model text CHECK (artifact.is_text(knowledge_model, 512));

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
