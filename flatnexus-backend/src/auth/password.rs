//! Password hashing with Argon2id.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core};
use argon2::Argon2;

use crate::error::{AppError, AppResult};

/// Hash a plaintext password for storage.
pub fn hash(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut rand_core::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(format!("password hashing failed: {e}")))
}

/// Check a plaintext password against a stored PHC-format hash.
///
/// Returns `false` for a wrong password *and* for a malformed stored hash —
/// both mean "cannot authenticate", and distinguishing them for the caller
/// would only widen what a failed login reveals.
pub fn verify(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Passwords rejected regardless of length, because a 12-character password
/// drawn from this set is not meaningfully better than a short one.
const COMMON_PASSWORDS: &[&str] = &[
    "password", "passw0rd", "password1", "password123", "passwordpassword",
    "123456789012", "1234567890123", "qwertyuiopas", "administrator",
    "letmeinletmein", "iloveyouiloveyou", "welcome12345", "changeme1234",
];

/// Enforce the password policy: a 12-character minimum, plus a small deny-list.
///
/// Deliberately length-first, per NIST SP 800-63B, which recommends a length
/// floor and a breach/common-password check over composition rules. The
/// deny-list here is a token gesture at the second half — wire in a real breach
/// corpus if this ever guards anything valuable.
pub fn validate_strength(password: &str) -> AppResult<()> {
    if password.chars().count() < 12 {
        return Err(AppError::Validation(
            "Password must be at least 12 characters.".into(),
        ));
    }

    let folded = password.to_lowercase();
    if COMMON_PASSWORDS.contains(&folded.as_str()) {
        return Err(AppError::Validation(
            "That password is among the most commonly used. Choose another.".into(),
        ));
    }

    // A single repeated character passes the length floor while carrying almost
    // no entropy.
    let mut chars = password.chars();
    if let Some(first) = chars.next()
        && chars.all(|c| c == first)
    {
        return Err(AppError::Validation(
            "Password must not be a single repeated character.".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{hash, validate_strength, verify};

    #[test]
    fn round_trips_a_password() {
        let stored = hash("a perfectly reasonable passphrase").unwrap();
        assert!(verify("a perfectly reasonable passphrase", &stored));
        assert!(!verify("something else entirely", &stored));
    }

    #[test]
    fn a_malformed_stored_hash_never_verifies() {
        assert!(!verify("anything", "not-a-phc-string"));
        assert!(!verify("anything", ""));
    }

    #[test]
    fn enforces_the_length_floor() {
        assert!(validate_strength("short").is_err());
        assert!(validate_strength("elevenchars").is_err());
        assert!(validate_strength("twelvechars!").is_ok());
    }

    #[test]
    fn rejects_the_passwords_a_length_floor_alone_would_admit() {
        assert!(validate_strength("passwordpassword").is_err());
        assert!(validate_strength("123456789012").is_err());
        assert!(validate_strength("aaaaaaaaaaaa").is_err());
        assert!(validate_strength("PasswordPassword").is_err()); // case-folded
    }
}
