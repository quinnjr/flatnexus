//! Session and login-challenge tokens.
//!
//! Both are stateless HS256 JWTs. There is no server-side session table, so a
//! session token is valid until it expires; `FLATNEXUS_SESSION_TTL_SECS` is the
//! knob that bounds that window.
//!
//! # Why two token kinds
//!
//! Sign-in is two steps: a password proves the first factor and yields a
//! *challenge* token, and a one-time password redeems that challenge for a
//! session. Both are signed with the same key, so they are distinguished by a
//! `typ` claim that each verifier requires. Without it, the challenge token
//! handed out after step one would be a perfectly valid session token — the
//! second factor would be skippable by anyone who read the response body.

use std::time::Duration;

use armature_jwt::{JwtConfig, JwtManager};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// The `iss` value this server signs and requires.
const ISSUER: &str = "flatnexus";

/// `typ` of a full session token.
const TYPE_SESSION: &str = "session";
/// `typ` of a token that has passed the first factor only.
const TYPE_LOGIN_CHALLENGE: &str = "login_challenge";

/// How long a half-finished sign-in stays redeemable. Long enough to read an
/// emailed code, short enough that an abandoned attempt is not left open.
const CHALLENGE_TTL_SECS: i64 = 600;

/// Claims carried by a FlatNexus token.
///
/// `iss`, `nbf` and `exp` are all present because the verifier is configured to
/// check all three; a claim the verifier validates but the signer omits would
/// reject every token this server issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    /// Publisher id.
    pub sub: String,
    pub email: String,
    /// Token kind. Checked on every verification — see the module docs.
    pub typ: String,
    pub iss: String,
    /// Issued-at, epoch seconds.
    pub iat: i64,
    /// Not-before, epoch seconds.
    pub nbf: i64,
    /// Expiry, epoch seconds.
    pub exp: i64,
}

pub struct TokenService {
    manager: JwtManager,
    ttl_secs: i64,
}

impl TokenService {
    pub fn new(secret: &str, ttl_secs: i64) -> AppResult<Self> {
        let mut config = JwtConfig::new(secret.to_string()).with_issuer(ISSUER.to_string());
        config.expires_in = Duration::from_secs(ttl_secs.max(1) as u64);

        let manager = JwtManager::new(config)
            .map_err(|e| AppError::Config(format!("could not initialize JWT manager: {e}")))?;

        // Clamped into the field `sign` reads. Clamping only `JwtConfig::expires_in`
        // did nothing here: that value is consulted by `generate_token_pair`,
        // which this server never calls, while `exp` is computed from `ttl_secs`.
        Ok(Self {
            manager,
            ttl_secs: ttl_secs.max(1),
        })
    }

    /// Mint a full session token.
    pub fn issue(&self, publisher_id: &str, email: &str) -> AppResult<IssuedToken> {
        self.sign(publisher_id, email, TYPE_SESSION, self.ttl_secs)
    }

    /// Mint a token that proves only the first factor.
    ///
    /// It authorizes exactly one thing: presenting a one-time password to
    /// `verifyLogin`. It is not accepted as a session anywhere.
    pub fn issue_login_challenge(&self, publisher_id: &str, email: &str) -> AppResult<IssuedToken> {
        self.sign(publisher_id, email, TYPE_LOGIN_CHALLENGE, CHALLENGE_TTL_SECS)
    }

    /// Verify a bearer token and return its claims.
    pub fn verify(&self, token: &str) -> AppResult<SessionClaims> {
        self.verify_typed(token, TYPE_SESSION)
    }

    /// Verify a login-challenge token.
    pub fn verify_login_challenge(&self, token: &str) -> AppResult<SessionClaims> {
        self.verify_typed(token, TYPE_LOGIN_CHALLENGE).map_err(|_| {
            AppError::Forbidden(
                "This sign-in attempt is no longer valid. Start again from the sign-in page."
                    .into(),
            )
        })
    }

    pub fn challenge_ttl_secs(&self) -> i64 {
        CHALLENGE_TTL_SECS
    }

    fn sign(
        &self,
        publisher_id: &str,
        email: &str,
        typ: &str,
        ttl_secs: i64,
    ) -> AppResult<IssuedToken> {
        let now = chrono::Utc::now().timestamp();
        let exp = now + ttl_secs;

        let claims = SessionClaims {
            sub: publisher_id.to_string(),
            email: email.to_string(),
            typ: typ.to_string(),
            iss: ISSUER.to_string(),
            iat: now,
            nbf: now,
            exp,
        };

        let token = self
            .manager
            .sign(&claims)
            .map_err(|e| AppError::Internal(format!("could not sign token: {e}")))?;

        Ok(IssuedToken {
            token,
            expires_at: exp,
        })
    }

    fn verify_typed(&self, token: &str, expected_type: &str) -> AppResult<SessionClaims> {
        let claims = self
            .manager
            .verify::<SessionClaims>(token)
            .map_err(|_| AppError::Unauthenticated)?;

        // The signature is valid, but a token of the wrong kind is not a
        // credential for this operation.
        if claims.typ != expected_type {
            return Err(AppError::Unauthenticated);
        }

        Ok(claims)
    }
}

#[derive(Debug, Clone)]
pub struct IssuedToken {
    pub token: String,
    pub expires_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> TokenService {
        TokenService::new("test-secret-that-is-at-least-32-bytes-long", 3600).unwrap()
    }

    #[test]
    fn a_session_token_verifies_as_a_session() {
        let service = service();
        let issued = service.issue("pub-1", "dev@example.com").unwrap();
        let claims = service.verify(&issued.token).unwrap();
        assert_eq!(claims.sub, "pub-1");
        assert_eq!(claims.email, "dev@example.com");
    }

    #[test]
    fn a_login_challenge_is_not_accepted_as_a_session() {
        // The whole point of the `typ` claim: without this check, the token
        // handed out after a correct password would skip the second factor.
        let service = service();
        let challenge = service.issue_login_challenge("pub-1", "dev@example.com").unwrap();
        assert!(service.verify(&challenge.token).is_err());
        assert!(service.verify_login_challenge(&challenge.token).is_ok());
    }

    #[test]
    fn a_session_is_not_accepted_as_a_login_challenge() {
        let service = service();
        let session = service.issue("pub-1", "dev@example.com").unwrap();
        assert!(service.verify_login_challenge(&session.token).is_err());
    }

    #[test]
    fn a_token_signed_with_another_key_is_rejected() {
        let issued = service().issue("pub-1", "dev@example.com").unwrap();
        let other = TokenService::new("a-completely-different-secret-32-bytes", 3600).unwrap();
        assert!(other.verify(&issued.token).is_err());
    }
}
