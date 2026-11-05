-- The digest of the token that names a share link's preview on the content domain, set when a
-- browser first opens the link. The token is drawn from the link's secret, so that a stolen
-- database names no preview and a preview's origin redeems nothing.
ALTER TABLE artifact.share_link
    ADD COLUMN preview_digest text CHECK (preview_digest ~ '^sha256:[0-9a-f]{64}$');

CREATE UNIQUE INDEX share_link_preview ON artifact.share_link (preview_digest);
