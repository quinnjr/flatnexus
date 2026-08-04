//! Authentication: accounts, sessions, OAuth linking, and one-time passwords.

pub mod accounts;
pub mod oauth;
pub mod otp;
pub mod password;
pub mod token;

use crate::error::{AppError, AppResult};
use crate::models::{Publisher, publisher};
use crate::models::PraxClientExt;
use crate::state::Db;

/// The authenticated caller, if any, attached to a GraphQL request.
#[derive(Debug, Clone)]
pub struct Viewer {
    pub publisher_id: String,
}

impl Viewer {
    /// Load the full publisher row for this viewer.
    pub async fn load(&self, db: &Db) -> AppResult<Publisher> {
        db.publisher()
            .find_first()
            .r#where(publisher::id::equals(self.publisher_id.clone()))
            .exec()
            .await?
            // The token verified, but the row is gone: the account was deleted
            // while a session was still live.
            .ok_or(AppError::Unauthenticated)
    }
}

/// Extract a bearer token from an `Authorization` header value.
///
/// The scheme is compared case-insensitively, as RFC 7235 §2.1 requires. Matching
/// only two spellings meant a conforming client sending `BEARER` was silently
/// treated as anonymous, surfacing later as a confusing `UNAUTHENTICATED`.
pub fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, rest) = header.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::bearer_token;

    #[test]
    fn parses_bearer_headers() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer abc"), Some("abc"));
        // RFC 7235 makes the scheme case-insensitive.
        assert_eq!(bearer_token("BEARER abc"), Some("abc"));
        assert_eq!(bearer_token("BeaRer abc"), Some("abc"));
        assert_eq!(bearer_token("Bearer   abc  "), Some("abc"));
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("abc"), None);
    }
}
