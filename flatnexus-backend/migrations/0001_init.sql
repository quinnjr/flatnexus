-- FlatNexus initial schema.
--
-- Applied by `scripts/migrate.sh` (or psql directly). Kept as plain SQL rather
-- than generated DDL so the deployed shape is reviewable in one file.
--
-- Timestamps are stored as BIGINT epoch-seconds rather than TIMESTAMPTZ: the
-- Prax value model maps cleanly onto integers, and every comparison this
-- application performs (token expiry, OTP windows) is arithmetic on instants
-- rather than calendar math.

BEGIN;

-- ---------------------------------------------------------------------------
-- Publishers
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS publishers (
    id              TEXT PRIMARY KEY,
    email           TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    -- NULL for accounts that only ever signed in through GitHub/Google.
    password_hash   TEXT,
    -- 'totp' or 'email'. Which second factor gates build uploads.
    otp_method      TEXT NOT NULL DEFAULT 'email',
    totp_secret     TEXT,
    totp_confirmed  BOOLEAN NOT NULL DEFAULT FALSE,
    is_admin        BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      BIGINT NOT NULL
);

-- Linked OAuth identities. A publisher may have several (GitHub *and* Google),
-- and the pair (provider, subject) is what uniquely identifies a remote account.
CREATE TABLE IF NOT EXISTS oauth_identities (
    id            TEXT PRIMARY KEY,
    publisher_id  TEXT NOT NULL REFERENCES publishers(id) ON DELETE CASCADE,
    provider      TEXT NOT NULL,
    subject       TEXT NOT NULL,
    email         TEXT,
    created_at    BIGINT NOT NULL,
    UNIQUE (provider, subject)
);

CREATE INDEX IF NOT EXISTS oauth_identities_publisher_idx
    ON oauth_identities (publisher_id);

-- ---------------------------------------------------------------------------
-- Applications and builds
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS applications (
    id            TEXT PRIMARY KEY,
    -- Reverse-DNS Flatpak application ID, e.g. org.gnome.Calculator.
    app_id        TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    summary       TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    homepage      TEXT,
    license       TEXT,
    publisher_id  TEXT NOT NULL REFERENCES publishers(id) ON DELETE RESTRICT,
    created_at    BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS applications_publisher_idx
    ON applications (publisher_id);

CREATE TABLE IF NOT EXISTS builds (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    -- Full OSTree ref, e.g. app/org.gnome.Calculator/x86_64/stable.
    flatpak_ref     TEXT NOT NULL,
    arch            TEXT NOT NULL,
    branch          TEXT NOT NULL,
    -- Human-facing version string taken from the bundle's appdata.
    version         TEXT NOT NULL DEFAULT '',
    -- OSTree commit checksum this ref resolved to after import.
    commit_id       TEXT NOT NULL,
    -- Key of the archived .flatpak bundle under FLATNEXUS_BUNDLE_PATH.
    bundle_key      TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    sha256          TEXT NOT NULL,
    published_by    TEXT NOT NULL REFERENCES publishers(id) ON DELETE RESTRICT,
    published_at    BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS builds_application_idx
    ON builds (application_id, published_at DESC);

-- One live build per (ref) — re-publishing the same ref supersedes the row.
CREATE UNIQUE INDEX IF NOT EXISTS builds_ref_key
    ON builds (flatpak_ref);

-- ---------------------------------------------------------------------------
-- One-time passwords
-- ---------------------------------------------------------------------------

-- Emailed codes. Stored as a SHA-256 hash so a database read does not hand an
-- attacker a usable code.
CREATE TABLE IF NOT EXISTS otp_challenges (
    id            TEXT PRIMARY KEY,
    publisher_id  TEXT NOT NULL REFERENCES publishers(id) ON DELETE CASCADE,
    purpose       TEXT NOT NULL,
    code_hash     TEXT NOT NULL,
    expires_at    BIGINT NOT NULL,
    consumed      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at    BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS otp_challenges_lookup_idx
    ON otp_challenges (publisher_id, purpose, consumed);

-- Spent TOTP time-steps. A TOTP code stays valid for its whole 30-second step
-- (plus skew), so without this a captured code could be replayed inside that
-- window. Rows older than a few minutes are pruned on write.
CREATE TABLE IF NOT EXISTS totp_replays (
    id            TEXT PRIMARY KEY,
    publisher_id  TEXT NOT NULL REFERENCES publishers(id) ON DELETE CASCADE,
    time_step     BIGINT NOT NULL,
    created_at    BIGINT NOT NULL,
    UNIQUE (publisher_id, time_step)
);

COMMIT;
