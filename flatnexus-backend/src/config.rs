//! Environment-backed configuration.
//!
//! Everything the server needs is resolved once at boot into [`Config`]. Values
//! that would make the server insecure or nonsensical if wrong (JWT secret
//! length, public URL shape) are validated here rather than at first use.

use std::path::PathBuf;

use crate::error::{AppError, AppResult};

/// Default ceiling on a single bundle upload: 1 GiB.
///
/// Sized by memory, not by disk. Armature buffers a request body in full before
/// a handler runs, so this much RAM is committed per concurrent upload no matter
/// what happens downstream — and the archive upload itself is streamed, so this
/// buffering is the only term that scales with bundle size. A GiB comfortably
/// covers ordinary desktop applications; raise it deliberately, with the
/// per-upload memory cost in mind, if you host something larger.
pub const DEFAULT_MAX_BUNDLE_BYTES: u64 = 1024 * 1024 * 1024;

/// Fully resolved server configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    /// Public origin clients reach this server on, without a trailing slash.
    pub public_url: String,
    pub cors_origin: String,

    pub database_url: String,
    pub db_max_connections: usize,

    pub jwt_secret: String,
    pub session_ttl_secs: i64,

    pub repo_path: PathBuf,
    pub staging_path: PathBuf,

    pub repo_title: String,
    pub repo_description: String,
    pub gpg_key_id: Option<String>,
    pub gpg_public_key_path: Option<PathBuf>,

    pub max_bundle_bytes: u64,
    pub spaces: SpacesConfig,

    pub email_otp_ttl_secs: i64,
    pub totp_issuer: String,

    pub smtp: SmtpConfig,
    pub oauth: OAuthConfig,
}

/// DigitalOcean Spaces bucket that archives uploaded bundles.
///
/// Required, not optional: an upload that is imported into the repository but
/// never archived leaves no way to re-verify or re-import that build later, and
/// a deployment that discovers this only on its first publish has already lost
/// the bundle. Booting without a reachable bucket configuration fails loudly
/// instead.
#[derive(Debug, Clone)]
pub struct SpacesConfig {
    pub bucket: String,
    /// Spaces datacenter, e.g. `nyc3`. Sent as the S3 region.
    pub region: String,
    /// Origin endpoint, e.g. `https://nyc3.digitaloceanspaces.com`. Point this
    /// at MinIO for local development.
    pub endpoint: String,
    /// Key prefix inside the bucket, so FlatNexus can share a bucket.
    pub prefix: Option<String>,
    /// Spaces CDN origin. Only set for a bucket whose objects are public.
    pub cdn_endpoint: Option<String>,
    /// Lifetime of a presigned download URL.
    pub presign_ttl_secs: i64,
}

impl SpacesConfig {
    fn from_env() -> AppResult<Self> {
        let region = env_or("FLATNEXUS_SPACES_REGION", "nyc3");

        // Credentials are read from the standard AWS variables by the SDK, not
        // from here. Checked at boot anyway so a misconfiguration surfaces now
        // rather than on the first upload.
        for key in ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"] {
            if optional(key).is_none() {
                return Err(AppError::Config(format!(
                    "{key} must be set: DigitalOcean Spaces access keys are read \
                     through the standard AWS credential variables"
                )));
            }
        }

        Ok(Self {
            bucket: require("FLATNEXUS_SPACES_BUCKET")?,
            endpoint: env_or(
                "FLATNEXUS_SPACES_ENDPOINT",
                &format!("https://{region}.digitaloceanspaces.com"),
            ),
            region,
            prefix: optional("FLATNEXUS_SPACES_PREFIX"),
            cdn_endpoint: optional("FLATNEXUS_SPACES_CDN_ENDPOINT"),
            presign_ttl_secs: positive("FLATNEXUS_SPACES_PRESIGN_TTL_SECS", 3600)?,
        })
    }
}

/// SMTP settings for emailed one-time codes.
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
}

/// OAuth client credentials. Either provider is optional; a provider with no
/// credentials configured is simply not advertised to the frontend.
#[derive(Debug, Clone, Default)]
pub struct OAuthConfig {
    pub github: Option<OAuthClient>,
    pub google: Option<OAuthClient>,
}

#[derive(Debug, Clone)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret: String,
}

impl Config {
    /// Load configuration from the process environment.
    pub fn from_env() -> AppResult<Self> {
        let public_url = validate_origin(
            "FLATNEXUS_PUBLIC_URL",
            &env_or("FLATNEXUS_PUBLIC_URL", "http://localhost:4000"),
        )?;

        let jwt_secret = require("FLATNEXUS_JWT_SECRET")?;
        if jwt_secret.len() < 32 {
            return Err(AppError::Config(
                "FLATNEXUS_JWT_SECRET must be at least 32 bytes; \
                 generate one with `openssl rand -hex 32`"
                    .into(),
            ));
        }

        Ok(Self {
            port: parse_env("FLATNEXUS_PORT", 4000u16)?,
            public_url,
            cors_origin: validate_origin(
                "FLATNEXUS_CORS_ORIGIN",
                &env_or("FLATNEXUS_CORS_ORIGIN", "http://localhost:5173"),
            )?,

            database_url: require("DATABASE_URL")?,
            db_max_connections: parse_env("FLATNEXUS_DB_MAX_CONNECTIONS", 16usize)?,

            jwt_secret,
            session_ttl_secs: positive("FLATNEXUS_SESSION_TTL_SECS", 86_400)?,

            repo_path: env_or("FLATNEXUS_REPO_PATH", "./data/repo").into(),
            staging_path: env_or("FLATNEXUS_STAGING_PATH", "./data/staging").into(),

            repo_title: env_or("FLATNEXUS_REPO_TITLE", "FlatNexus"),
            repo_description: env_or(
                "FLATNEXUS_REPO_DESCRIPTION",
                "Community-hosted Flatpak distribution point",
            ),
            gpg_key_id: optional("FLATNEXUS_GPG_KEY_ID"),
            gpg_public_key_path: optional("FLATNEXUS_GPG_PUBLIC_KEY_PATH").map(PathBuf::from),

            max_bundle_bytes: parse_env("FLATNEXUS_MAX_BUNDLE_BYTES", DEFAULT_MAX_BUNDLE_BYTES)?,
            spaces: SpacesConfig::from_env()?,

            email_otp_ttl_secs: positive("FLATNEXUS_EMAIL_OTP_TTL_SECS", 600)?,
            totp_issuer: env_or("FLATNEXUS_TOTP_ISSUER", "FlatNexus"),

            smtp: SmtpConfig {
                host: env_or("FLATNEXUS_SMTP_HOST", "localhost"),
                port: parse_env("FLATNEXUS_SMTP_PORT", 1025u16)?,
                username: optional("FLATNEXUS_SMTP_USERNAME"),
                password: optional("FLATNEXUS_SMTP_PASSWORD"),
                from: env_or("FLATNEXUS_MAIL_FROM", "no-reply@flatnexus.local"),
            },

            oauth: OAuthConfig {
                github: oauth_client("GITHUB"),
                google: oauth_client("GOOGLE"),
            },
        })
    }

    /// Redirect URI registered with the OAuth provider for `provider`.
    pub fn oauth_redirect_url(&self, provider: &str) -> String {
        format!("{}/auth/oauth/{}/callback", self.public_url, provider)
    }

    /// Where the browser lands after a successful OAuth round-trip. The session
    /// token is appended as a fragment so it never reaches a server log.
    pub fn frontend_auth_landing(&self) -> String {
        format!("{}/auth/callback", self.cors_origin.trim_end_matches('/'))
    }

    /// Base URL of the OSTree repository, as advertised to `flatpak`.
    pub fn repo_url(&self) -> String {
        format!("{}/repo", self.public_url)
    }
}

/// Validate an origin and strip any trailing slash.
///
/// Checked at boot rather than at first use because a malformed origin here is
/// not a cosmetic problem: it feeds the OAuth redirect URIs, the repository URL
/// advertised to `flatpak`, the `.flatpakrepo` file, and the `Secure` flag on the
/// OAuth state cookie. A missing scheme silently turns the last of those off.
fn validate_origin(key: &str, raw: &str) -> AppResult<String> {
    let trimmed = raw.trim().trim_end_matches('/');

    let bad = |why: &str| AppError::Config(format!("{key} is not a valid origin: {why}"));

    let rest = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .ok_or_else(|| bad("must start with http:// or https://"))?;

    let host = rest.split('/').next().unwrap_or_default();
    if host.is_empty() {
        return Err(bad("no host"));
    }
    if host.contains(char::is_whitespace) {
        return Err(bad("host contains whitespace"));
    }
    if rest.contains('/') {
        return Err(bad("must be an origin only, with no path"));
    }

    Ok(trimmed.to_string())
}

/// Parse a duration-like setting, rejecting zero and negative values.
///
/// A zero TTL is never what anyone means, and its effects are silent and
/// bewildering — a zero session TTL issues tokens that are expired before the
/// response reaches the client.
fn positive(key: &str, default: i64) -> AppResult<i64> {
    let value = parse_env(key, default)?;
    if value <= 0 {
        return Err(AppError::Config(format!(
            "{key} must be greater than zero (got {value})"
        )));
    }
    Ok(value)
}

fn oauth_client(prefix: &str) -> Option<OAuthClient> {
    let client_id = optional(&format!("FLATNEXUS_{prefix}_CLIENT_ID"))?;
    let client_secret = optional(&format!("FLATNEXUS_{prefix}_CLIENT_SECRET"))?;
    Some(OAuthClient {
        client_id,
        client_secret,
    })
}

/// Read an environment variable, trimmed.
///
/// Trimming is not cosmetic: a value sourced from a file or a secrets mount
/// commonly carries a trailing newline, and an untrimmed secret would count that
/// whitespace toward the 32-byte JWT minimum while an untrimmed URL would be
/// concatenated into malformed links.
fn optional(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

fn require(key: &str) -> AppResult<String> {
    optional(key).ok_or_else(|| AppError::Config(format!("{key} must be set")))
}

fn env_or(key: &str, default: &str) -> String {
    optional(key).unwrap_or_else(|| default.to_string())
}

fn parse_env<T>(key: &str, default: T) -> AppResult<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match optional(key) {
        None => Ok(default),
        Some(raw) => raw
            .parse()
            .map_err(|e| AppError::Config(format!("{key} is not a valid value: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::{positive, validate_origin};

    #[test]
    fn accepts_ordinary_origins_and_strips_the_trailing_slash() {
        assert_eq!(
            validate_origin("K", "https://flatnexus.example/").unwrap(),
            "https://flatnexus.example"
        );
        assert_eq!(
            validate_origin("K", "http://localhost:4000").unwrap(),
            "http://localhost:4000"
        );
    }

    #[test]
    fn rejects_origins_that_would_break_the_links_built_from_them() {
        for bad in [
            "flatnexus.example",              // no scheme; also silently drops cookie Secure
            "ftp://flatnexus.example",
            "https://",
            "https://flatnexus.example/repo", // path, not an origin
            "https://flat nexus.example",
        ] {
            assert!(validate_origin("K", bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn rejects_non_positive_durations() {
        // Safety: these mutate process env, but each key is unique to its test.
        unsafe { std::env::set_var("FLATNEXUS_TEST_TTL_ZERO", "0") };
        assert!(positive("FLATNEXUS_TEST_TTL_ZERO", 60).is_err());

        unsafe { std::env::set_var("FLATNEXUS_TEST_TTL_NEG", "-5") };
        assert!(positive("FLATNEXUS_TEST_TTL_NEG", 60).is_err());

        assert_eq!(positive("FLATNEXUS_TEST_TTL_UNSET", 60).unwrap(), 60);
    }
}
