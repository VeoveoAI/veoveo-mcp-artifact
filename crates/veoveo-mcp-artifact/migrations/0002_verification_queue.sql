-- The sweep finds verifying artifacts whose verification Task is gone, so it can fail
-- them. Few artifacts verify at once, and a partial index holds only those.
CREATE INDEX artifact_verifying ON artifact.artifact (tenant, artifact_id)
    WHERE state = 'verifying';
