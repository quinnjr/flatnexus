//! Unified error type.
//!
//! One error enum crosses every layer. It knows how to render itself as an
//! Armature HTTP error and as a GraphQL error with a machine-readable
//! `extensions.code`, so callers (browser or curl) get the same taxonomy
//! regardless of which surface they hit.

use armature_core::Error as ArmatureError;
use async_graphql::ErrorExtensions;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("{0}")]
    Validation(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("authentication required")]
    Unauthenticated,

    #[error("{0}")]
    Forbidden(String),

    /// The OTP was missing, wrong, expired, or already used.
    #[error("{0}")]
    OtpRejected(String),

    #[error("{0}")]
    Conflict(String),

    #[error("upload rejected: {0}")]
    UploadRejected(String),

    /// A `flatpak` / `ostree` subprocess failed. Carries the command's stderr.
    #[error("repository operation failed: {0}")]
    Repo(String),

    #[error("mail delivery failed: {0}")]
    Mail(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Stable, machine-readable code exposed as `extensions.code` in GraphQL
    /// errors and used by the frontend to branch on failures.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Config(_) => "CONFIG_ERROR",
            Self::Database(_) => "DATABASE_ERROR",
            Self::Validation(_) => "VALIDATION_ERROR",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Unauthenticated => "UNAUTHENTICATED",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::OtpRejected(_) => "OTP_REJECTED",
            Self::Conflict(_) => "CONFLICT",
            Self::UploadRejected(_) => "UPLOAD_REJECTED",
            Self::Repo(_) => "REPO_ERROR",
            Self::Mail(_) => "MAIL_ERROR",
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::Validation(_) => 400,
            Self::Unauthenticated => 401,
            Self::Forbidden(_) | Self::OtpRejected(_) => 403,
            Self::NotFound(_) => 404,
            Self::Conflict(_) => 409,
            Self::UploadRejected(_) => 413,
            _ => 500,
        }
    }

    /// True when the message is safe to show a caller verbatim. Internal
    /// failures leak implementation detail, so they are redacted instead.
    ///
    /// `Repo` is in the redacted set even though it is caused by publisher
    /// input: it carries raw `ostree`/`flatpak` stderr, which routinely names
    /// absolute server paths, staging directories, and GPG key ids. The
    /// publisher-actionable cases (a malformed bundle, a mismatched ref) are
    /// raised as `UploadRejected` with a written message instead.
    fn is_public(&self) -> bool {
        !matches!(
            self,
            Self::Config(_)
                | Self::Database(_)
                | Self::Internal(_)
                | Self::Mail(_)
                | Self::Repo(_)
        )
    }

    fn public_message(&self) -> String {
        if self.is_public() {
            self.to_string()
        } else {
            "An internal error occurred.".to_string()
        }
    }
}

/// Convert into a GraphQL error carrying `extensions.code`.
///
/// This cannot be a `From` impl: async-graphql already has a blanket
/// `impl<T: Display> From<T> for Error`, and adding another would collide. The
/// blanket impl would also silently drop `extensions.code`, which is exactly
/// what clients branch on — so conversions go through [`IntoGqlResult::gql`]
/// rather than through `?`.
pub fn to_gql_error(err: AppError) -> armature_graphql::Error {
    // Logged on status, not on publicity. Tying the two together meant a
    // redacted 500 could also be an unlogged one — the failure would reach the
    // client and exist nowhere else.
    if err.status() >= 500 {
        armature_log::error!("graphql resolver failed: {err}");
    }
    let code = err.code();
    armature_graphql::Error::new(err.public_message()).extend_with(|_, e| e.set("code", code))
}

/// Adapts any `Result` whose error becomes an [`AppError`] into a GraphQL
/// result, preserving the error code.
pub trait IntoGqlResult<T> {
    fn gql(self) -> armature_graphql::Result<T>;
}

impl<T, E: Into<AppError>> IntoGqlResult<T> for Result<T, E> {
    fn gql(self) -> armature_graphql::Result<T> {
        self.map_err(|e| to_gql_error(e.into()))
    }
}

impl From<AppError> for ArmatureError {
    fn from(err: AppError) -> Self {
        if err.status() >= 500 {
            armature_log::error!("request failed: {err}");
        }
        match err.status() {
            400 => ArmatureError::Validation(err.public_message()),
            401 => ArmatureError::Unauthorized(err.public_message()),
            403 => ArmatureError::Forbidden(err.public_message()),
            404 => ArmatureError::NotFound(err.public_message()),
            409 => ArmatureError::Conflict(err.public_message()),
            413 => ArmatureError::PayloadTooLarge(err.public_message()),
            _ => ArmatureError::Internal(err.public_message()),
        }
    }
}

impl From<prax_query::error::QueryError> for AppError {
    fn from(err: AppErrorQueryError) -> Self {
        // A unique-constraint violation is a conflict the caller can act on, not
        // an internal fault. Without this every lost check-then-insert race —
        // two people registering the same email, two first-time OAuth logins —
        // surfaces as an opaque 500.
        if is_unique_violation(&err) {
            return Self::Conflict(
                "That record already exists. Someone may have just created it.".into(),
            );
        }
        Self::Database(err.to_string())
    }
}

type AppErrorQueryError = prax_query::error::QueryError;

/// Recognize a Postgres unique-violation (SQLSTATE 23505) inside a Prax error.
///
/// Matched on the rendered error because Prax does not surface the SQLSTATE as a
/// typed field; both the code and the constraint-violation wording are checked
/// so a message merely containing the digits cannot be mistaken for one.
fn is_unique_violation(err: &AppErrorQueryError) -> bool {
    let text = err.to_string();
    text.contains("23505")
        || (text.contains("duplicate key") && text.contains("unique constraint"))
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        Self::Internal(format!("io: {err}"))
    }
}
