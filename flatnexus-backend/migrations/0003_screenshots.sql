-- Application media: screenshots, and optionally a short WebM clip.
--
-- Every application must carry at least one *still* screenshot. That is enforced
-- when the application is registered and re-checked before a build can be
-- published; see `src/screenshots.rs` for why a video does not satisfy it on its
-- own.
--
-- Objects live in the same bucket as the bundles, under `screenshots/<app-id>/`,
-- so an application's whole footprint is reachable by two prefixes.

BEGIN;

CREATE TABLE IF NOT EXISTS screenshots (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    -- Object key in the bundle bucket. Content-addressed, so re-uploading the
    -- same bytes for the same application resolves to one object.
    object_key      TEXT NOT NULL,
    -- 'png', 'jpg', 'webp', or 'webm'. Sniffed from the file's leading bytes,
    -- never taken from the client's filename or Content-Type.
    format          TEXT NOT NULL,
    -- Order in the gallery, 0-based.
    position        BIGINT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    sha256          TEXT NOT NULL,
    created_at      BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS screenshots_application_idx
    ON screenshots (application_id, position);

-- One row per distinct object per application: re-uploading identical bytes
-- updates the existing row rather than accumulating duplicates.
CREATE UNIQUE INDEX IF NOT EXISTS screenshots_object_key
    ON screenshots (application_id, object_key);

COMMIT;
