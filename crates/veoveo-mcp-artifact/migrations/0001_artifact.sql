-- Artifact's tables. Every row belongs to one tenant. `tenant` and `owner` hold the
-- identity schema's deterministic UUIDs; principals, groups, Work Contexts and data
-- labels that only the forwarded identity names travel as their text keys.

-- Nonempty text of at most `max` bytes without control characters.
CREATE FUNCTION artifact.is_text(value text, max integer) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    RETURN octet_length(value) BETWEEN 1 AND max AND value !~ '[[:cntrl:]]';

-- A level's rank: read 1, write 2, admin 3.
CREATE FUNCTION artifact.level_rank(level text) RETURNS smallint
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    RETURN CASE level WHEN 'read' THEN 1 WHEN 'write' THEN 2 WHEN 'admin' THEN 3 ELSE 0 END;

-- The level a Work Context membership grants on the Work Context's artifacts: a viewer
-- reads, a contributor writes, and a custodian or owner administers.
CREATE FUNCTION artifact.membership_rank(membership text) RETURNS smallint
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    RETURN CASE membership
        WHEN 'viewer' THEN 1
        WHEN 'contributor' THEN 2
        WHEN 'custodian' THEN 3
        WHEN 'owner' THEN 3
        ELSE 0
    END;

CREATE TABLE artifact.artifact (
    tenant uuid NOT NULL,
    artifact_id uuid NOT NULL,
    owner uuid NOT NULL,
    owner_key text NOT NULL CHECK (artifact.is_text(owner_key, 512)),
    work_context text NOT NULL CHECK (artifact.is_text(work_context, 256)),
    name text COLLATE "C" NOT NULL CHECK (
        artifact.is_text(name, 255)
        AND name !~ '[/\\]'
        AND name NOT IN ('.', '..')
    ),
    media_type text NOT NULL CHECK (
        media_type ~ '^[a-z0-9][a-z0-9!#$&^_.+-]{0,126}/[a-z0-9][a-z0-9!#$&^_.+-]{0,126}$'
    ),
    size_bytes bigint CHECK (size_bytes BETWEEN 0 AND 5497558138880),
    labels text[] NOT NULL CHECK (cardinality(labels) <= 64 AND array_position(labels, NULL) IS NULL),
    release_state text NOT NULL CHECK (release_state IN ('private', 'released')),
    state text NOT NULL CHECK (
        state IN ('uploading', 'available', 'failed', 'deleting')
    ),
    provenance jsonb NOT NULL CHECK (jsonb_typeof(provenance) = 'object'),
    metadata jsonb NOT NULL CHECK (jsonb_typeof(metadata) = 'object'),
    retention_days integer CHECK (retention_days BETWEEN 1 AND 36500),
    created_at timestamptz NOT NULL,
    available_at timestamptz,
    retain_until timestamptz,
    failed_at timestamptz,
    deleting_at timestamptz,
    PRIMARY KEY (tenant, artifact_id),
    -- Bytes exist from completion until the row goes, so only those states hold a size.
    CHECK ((size_bytes IS NULL) = (state IN ('uploading', 'failed'))),
    CHECK (state <> 'available' OR available_at IS NOT NULL),
    CHECK ((state = 'failed') = (failed_at IS NOT NULL)),
    CHECK ((state = 'deleting') = (deleting_at IS NOT NULL)),
    CHECK (retain_until IS NULL OR retention_days IS NOT NULL)
);


-- One index per listing filter, each ending in the artifact ID so a page continues from
-- its last key. Names compare bytewise, so a prefix is an index range.
CREATE INDEX artifact_work_context_page ON artifact.artifact (tenant, work_context, artifact_id);
CREATE INDEX artifact_owner_page ON artifact.artifact (tenant, owner, artifact_id);
CREATE INDEX artifact_media_type_page ON artifact.artifact (tenant, media_type, artifact_id);
CREATE INDEX artifact_release_page ON artifact.artifact (tenant, release_state, artifact_id);
CREATE INDEX artifact_name_page ON artifact.artifact (tenant, name, artifact_id);
-- The sweep's queues across tenants.
CREATE INDEX artifact_retention_due ON artifact.artifact (retain_until)
    WHERE retain_until IS NOT NULL AND state = 'available';
CREATE INDEX artifact_failed_since ON artifact.artifact (failed_at) WHERE state = 'failed';
CREATE INDEX artifact_deleting_since ON artifact.artifact (deleting_at) WHERE state = 'deleting';

COMMENT ON TABLE artifact.artifact IS
    'Retention: until deleted or its retention passes, when the sweep deletes the object and '
    'the row; a failed artifact holds no object and goes 7 days after it failed.';

CREATE TABLE artifact.grant (
    tenant uuid NOT NULL,
    artifact_id uuid NOT NULL,
    subject_kind text NOT NULL CHECK (subject_kind IN ('principal', 'group')),
    subject text NOT NULL CHECK (artifact.is_text(subject, 512)),
    level text NOT NULL CHECK (level IN ('read', 'write', 'admin')),
    expires_at timestamptz,
    granted_by text NOT NULL CHECK (artifact.is_text(granted_by, 512)),
    granted_at timestamptz NOT NULL,
    PRIMARY KEY (tenant, artifact_id, subject_kind, subject),
    FOREIGN KEY (tenant, artifact_id)
        REFERENCES artifact.artifact (tenant, artifact_id) ON DELETE CASCADE
);

CREATE INDEX grant_expiry ON artifact.grant (expires_at) WHERE expires_at IS NOT NULL;

COMMENT ON TABLE artifact.grant IS
    'Retention: with its artifact, or until its expiry, when the sweep deletes it.';

CREATE TABLE artifact.upload (
    tenant uuid NOT NULL,
    upload_id uuid NOT NULL,
    artifact_id uuid NOT NULL,
    creator uuid NOT NULL,
    -- `principal:<key>` or `server:<slug>`: who an idempotency key belongs to.
    acting_identity text NOT NULL CHECK (
        artifact.is_text(acting_identity, 600) AND acting_identity ~ '^(principal|server):'
    ),
    idempotency_key text CHECK (artifact.is_text(idempotency_key, 256)),
    form text NOT NULL CHECK (form IN ('single', 'multipart')),
    state text NOT NULL CHECK (state IN ('open', 'completed', 'aborted', 'expired')),
    declared_size bigint CHECK (declared_size BETWEEN 0 AND 5497558138880),
    max_size bigint CHECK (max_size BETWEEN 0 AND 5497558138880),
    reserved_bytes bigint NOT NULL CHECK (reserved_bytes BETWEEN 0 AND 5497558138880),
    object_upload text CHECK (artifact.is_text(object_upload, 1024)),
    part_size bigint CHECK (part_size BETWEEN 5242880 AND 5368709120),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    ended_at timestamptz,
    PRIMARY KEY (tenant, upload_id),
    UNIQUE (tenant, artifact_id),
    FOREIGN KEY (tenant, artifact_id)
        REFERENCES artifact.artifact (tenant, artifact_id) ON DELETE CASCADE,
    CHECK ((state = 'open') = (ended_at IS NULL)),
    CHECK ((form = 'multipart') = (part_size IS NOT NULL)),
    CHECK (expires_at > created_at)
);

CREATE UNIQUE INDEX upload_idempotency ON artifact.upload (tenant, acting_identity, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE INDEX upload_creator_page ON artifact.upload (tenant, creator, upload_id);
CREATE INDEX upload_creator_state_page ON artifact.upload (tenant, creator, state, upload_id);
CREATE INDEX upload_open_expiry ON artifact.upload (expires_at) WHERE state = 'open';
CREATE INDEX upload_ended ON artifact.upload (ended_at) WHERE ended_at IS NOT NULL;

COMMENT ON TABLE artifact.upload IS
    'Retention: 7 days after it ends, when the sweep deletes it, or with its artifact.';

CREATE TABLE artifact.share_link (
    tenant uuid NOT NULL,
    share_link_id uuid NOT NULL,
    artifact_id uuid NOT NULL,
    secret_digest text NOT NULL CHECK (secret_digest ~ '^sha256:[0-9a-f]{64}$'),
    expires_at timestamptz NOT NULL,
    redemption_limit integer CHECK (redemption_limit > 0),
    redemptions integer NOT NULL CHECK (
        redemptions >= 0 AND (redemption_limit IS NULL OR redemptions <= redemption_limit)
    ),
    revoked_at timestamptz,
    created_by text NOT NULL CHECK (artifact.is_text(created_by, 512)),
    created_at timestamptz NOT NULL,
    PRIMARY KEY (tenant, share_link_id),
    UNIQUE (secret_digest),
    FOREIGN KEY (tenant, artifact_id)
        REFERENCES artifact.artifact (tenant, artifact_id) ON DELETE CASCADE,
    CHECK (expires_at > created_at AND expires_at <= created_at + interval '30 days')
);

CREATE INDEX share_link_artifact_page ON artifact.share_link (tenant, artifact_id, share_link_id);
CREATE INDEX share_link_ended ON artifact.share_link ((coalesce(revoked_at, expires_at)));

COMMENT ON TABLE artifact.share_link IS
    'Retention: 30 days after it expires or is revoked, when the sweep deletes it, or with '
    'its artifact.';

CREATE TABLE artifact.access_request (
    tenant uuid NOT NULL,
    access_request_id uuid NOT NULL,
    artifact_id uuid NOT NULL,
    requester text NOT NULL CHECK (artifact.is_text(requester, 512)),
    level text NOT NULL CHECK (level IN ('read', 'write', 'admin')),
    justification text NOT NULL CHECK (
        octet_length(justification) BETWEEN 1 AND 1000
        AND justification !~ '[\x01-\x09\x0b-\x1f\x7f]'
    ),
    state text NOT NULL CHECK (state IN ('pending', 'approved', 'denied', 'cancelled')),
    decided_by text CHECK (artifact.is_text(decided_by, 512)),
    note text CHECK (
        octet_length(note) BETWEEN 1 AND 1000 AND note !~ '[\x01-\x09\x0b-\x1f\x7f]'
    ),
    created_at timestamptz NOT NULL,
    decided_at timestamptz,
    PRIMARY KEY (tenant, access_request_id),
    FOREIGN KEY (tenant, artifact_id)
        REFERENCES artifact.artifact (tenant, artifact_id) ON DELETE CASCADE,
    CHECK ((state = 'pending') = (decided_at IS NULL))
);

CREATE INDEX access_request_artifact_page
    ON artifact.access_request (tenant, artifact_id, access_request_id);
CREATE INDEX access_request_requester_page
    ON artifact.access_request (tenant, requester, access_request_id);
CREATE UNIQUE INDEX access_request_one_pending
    ON artifact.access_request (tenant, artifact_id, requester) WHERE state = 'pending';
CREATE INDEX access_request_decided ON artifact.access_request (decided_at)
    WHERE decided_at IS NOT NULL;

COMMENT ON TABLE artifact.access_request IS
    'Retention: 90 days after a decision or cancellation, when the sweep deletes it, or with '
    'its artifact.';

CREATE TABLE artifact.storage_usage (
    tenant uuid PRIMARY KEY,
    stored_bytes bigint NOT NULL CHECK (stored_bytes >= 0),
    reserved_bytes bigint NOT NULL CHECK (reserved_bytes >= 0),
    open_uploads integer NOT NULL CHECK (open_uploads >= 0)
);

COMMENT ON TABLE artifact.storage_usage IS
    'Retention: for the life of the tenant. Triggers on artifact.artifact and artifact.upload '
    'keep it current.';

CREATE TABLE artifact.upload_limits (
    tenant uuid PRIMARY KEY,
    quota_bytes bigint CHECK (quota_bytes >= 0),
    largest_object_bytes bigint NOT NULL CHECK (largest_object_bytes BETWEEN 0 AND 5497558138880),
    -- NULL admits every media type.
    allowed_media_types text[] CHECK (
        cardinality(allowed_media_types) <= 256
        AND array_position(allowed_media_types, NULL) IS NULL
    ),
    active_uploads integer NOT NULL CHECK (active_uploads BETWEEN 1 AND 10000),
    updated_by text NOT NULL CHECK (artifact.is_text(updated_by, 512)),
    updated_at timestamptz NOT NULL
);

COMMENT ON TABLE artifact.upload_limits IS
    'Retention: for the life of the tenant. A tenant without a row has the default limits.';

-- The caller's level on `target` at `as_of`, as a rank: 0 none, 1 read, 2 write, 3 admin.
-- An artifact of another tenant, one being deleted, one past its retention, and one
-- carrying a label outside the caller's labels give 0 whatever the grants say.
-- Otherwise the level is the highest of ownership, the caller's unexpired principal
-- grants, its unexpired group grants each capped by its role in the group, and its
-- membership in the artifact's Work Context while that is the caller's selected one.
-- `groups` and `group_levels` pair each of the caller's groups with its role there.
CREATE FUNCTION artifact.effective_level(
    target artifact.artifact,
    tenant uuid,
    caller uuid,
    caller_key text,
    groups text[],
    group_levels text[],
    work_context text,
    membership text,
    labels text[],
    as_of timestamptz
) RETURNS smallint
LANGUAGE sql STABLE PARALLEL SAFE
SET search_path = pg_catalog, pg_temp
RETURN CASE
    WHEN target.tenant IS DISTINCT FROM effective_level.tenant
        OR target.state = 'deleting'
        OR (target.retain_until IS NOT NULL AND target.retain_until <= as_of)
        OR NOT (target.labels <@ coalesce(effective_level.labels, '{}'))
        THEN 0::smallint
    ELSE greatest(
        CASE WHEN target.owner = caller THEN 3 ELSE 0 END,
        CASE WHEN target.work_context = effective_level.work_context
            THEN coalesce(artifact.membership_rank(membership), 0) ELSE 0 END,
        coalesce((
            SELECT max(artifact.level_rank(g.level))
            FROM artifact.grant AS g
            WHERE g.tenant = target.tenant
                AND g.artifact_id = target.artifact_id
                AND g.subject_kind = 'principal'
                AND g.subject = caller_key
                AND (g.expires_at IS NULL OR g.expires_at > as_of)
        ), 0),
        coalesce((
            SELECT max(least(artifact.level_rank(g.level), artifact.level_rank(m.level)))
            FROM unnest(groups, group_levels) AS m (grp, level)
            JOIN artifact.grant AS g
                ON g.tenant = target.tenant
                AND g.artifact_id = target.artifact_id
                AND g.subject_kind = 'group'
                AND g.subject = m.grp
            WHERE g.expires_at IS NULL OR g.expires_at > as_of
        ), 0)
    )::smallint
END;

-- Storage usage follows the rows. An open upload reserves its bytes and counts against
-- the open-upload limit until it ends; an artifact's bytes count from completion until
-- its row goes.
CREATE FUNCTION artifact.track_upload_usage() RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') AND OLD.state = 'open' THEN
        UPDATE artifact.storage_usage
        SET reserved_bytes = reserved_bytes - OLD.reserved_bytes,
            open_uploads = open_uploads - 1
        WHERE tenant = OLD.tenant;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') AND NEW.state = 'open' THEN
        INSERT INTO artifact.storage_usage AS u (tenant, stored_bytes, reserved_bytes, open_uploads)
        VALUES (NEW.tenant, 0, NEW.reserved_bytes, 1)
        ON CONFLICT (tenant) DO UPDATE
        SET reserved_bytes = u.reserved_bytes + excluded.reserved_bytes,
            open_uploads = u.open_uploads + 1;
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER track_usage AFTER INSERT OR UPDATE OF state, reserved_bytes OR DELETE
    ON artifact.upload FOR EACH ROW EXECUTE FUNCTION artifact.track_upload_usage();

CREATE FUNCTION artifact.track_stored_usage() RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    before bigint := CASE WHEN TG_OP IN ('UPDATE', 'DELETE') THEN coalesce(OLD.size_bytes, 0) ELSE 0 END;
    after bigint := CASE WHEN TG_OP IN ('INSERT', 'UPDATE') THEN coalesce(NEW.size_bytes, 0) ELSE 0 END;
    owner_tenant uuid := CASE WHEN TG_OP = 'DELETE' THEN OLD.tenant ELSE NEW.tenant END;
BEGIN
    -- A size first appears on a tenant whose upload created its usage row, so a change
    -- updates that row. An upsert would check its proposed row, which a decrease makes
    -- negative, before it found the conflict.
    IF before <> after THEN
        UPDATE artifact.storage_usage
        SET stored_bytes = stored_bytes + (after - before)
        WHERE tenant = owner_tenant;
        IF NOT FOUND THEN
            INSERT INTO artifact.storage_usage (tenant, stored_bytes, reserved_bytes, open_uploads)
            VALUES (owner_tenant, after - before, 0, 0);
        END IF;
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER track_usage AFTER INSERT OR UPDATE OF size_bytes OR DELETE
    ON artifact.artifact FOR EACH ROW EXECUTE FUNCTION artifact.track_stored_usage();

-- Every change wakes listeners on the `artifact` channel. The kernel trigger publishes the
-- table, the operation, the key column named by its first argument and the tenant column
-- named by its second. Listeners reread current rows; a payload is never data.
CREATE TRIGGER notify_change AFTER INSERT OR UPDATE OR DELETE ON artifact.artifact
    FOR EACH ROW EXECUTE FUNCTION kernel.notify_change('artifact_id', 'tenant');
CREATE TRIGGER notify_change AFTER INSERT OR UPDATE OR DELETE ON artifact.grant
    FOR EACH ROW EXECUTE FUNCTION kernel.notify_change('artifact_id', 'tenant');
CREATE TRIGGER notify_change AFTER INSERT OR UPDATE OR DELETE ON artifact.upload
    FOR EACH ROW EXECUTE FUNCTION kernel.notify_change('upload_id', 'tenant');
CREATE TRIGGER notify_change AFTER INSERT OR UPDATE OR DELETE ON artifact.share_link
    FOR EACH ROW EXECUTE FUNCTION kernel.notify_change('artifact_id', 'tenant');
CREATE TRIGGER notify_change AFTER INSERT OR UPDATE OR DELETE ON artifact.access_request
    FOR EACH ROW EXECUTE FUNCTION kernel.notify_change('access_request_id', 'tenant');
CREATE TRIGGER notify_change AFTER INSERT OR UPDATE OR DELETE ON artifact.storage_usage
    FOR EACH ROW EXECUTE FUNCTION kernel.notify_change('tenant', 'tenant');
CREATE TRIGGER notify_change AFTER INSERT OR UPDATE OR DELETE ON artifact.upload_limits
    FOR EACH ROW EXECUTE FUNCTION kernel.notify_change('tenant', 'tenant');
