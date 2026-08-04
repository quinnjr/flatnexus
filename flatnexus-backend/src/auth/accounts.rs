//! Account creation, sign-in, and OAuth identity linking.

use std::sync::LazyLock;

use crate::auth::oauth::OAuthProfile;
use crate::auth::otp::OtpMethod;
use crate::auth::password;
use crate::error::{AppError, AppResult};
use crate::models::{self, OauthIdentity, Publisher, oauth_identity, publisher};
use crate::models::PraxClientExt;
use crate::state::Db;

/// Register a publisher with an email and password.
pub async fn register(
    db: &Db,
    email: &str,
    display_name: &str,
    plaintext_password: &str,
) -> AppResult<Publisher> {
    let email = normalize_email(email)?;
    let display_name = display_name.trim();
    if display_name.is_empty() {
        return Err(AppError::Validation("A display name is required.".into()));
    }
    password::validate_strength(plaintext_password)?;

    if find_by_email(db, &email).await?.is_some() {
        return Err(AppError::Conflict(
            "An account with that email already exists.".into(),
        ));
    }

    let now = models::now_secs();
    let created = db
        .publisher()
        .create()
        .set("id", models::new_id())
        .set("email", email)
        .set("display_name", display_name.to_string())
        .set("password_hash", password::hash(plaintext_password)?)
        // Email is the default second factor: it works before the publisher has
        // enrolled an authenticator, so a new account is never unable to publish.
        .set("otp_method", OtpMethod::Email.as_str().to_string())
        .set("totp_confirmed", false)
        .set("is_admin", false)
        .set("created_at", now)
        .exec()
        .await?;

    Ok(created)
}

/// Verify an email/password pair.
///
/// Every failure returns one message. Not just for tidiness: a distinct
/// "this account uses GitHub" reply would let any address be probed for
/// registration, which is the same disclosure a distinct "no such account"
/// would be. The frontend tells people about GitHub and Google by listing the
/// providers on the sign-in page, which needs no knowledge of the account.
pub async fn login(db: &Db, email: &str, plaintext_password: &str) -> AppResult<Publisher> {
    let email = normalize_email(email)?;
    let found = find_by_email(db, &email).await?;

    let invalid = || AppError::Forbidden("Incorrect email or password.".into());

    // Argon2 runs on every path, including the ones that are already doomed.
    // Returning early for an unknown address would make the response measurably
    // faster than one for a known address — a timing oracle that answers the
    // same question the shared message exists to refuse.
    let stored = found
        .as_ref()
        .and_then(|candidate| candidate.password_hash.as_deref())
        .unwrap_or(DUMMY_PASSWORD_HASH.as_str());

    let password_matches = password::verify(plaintext_password, stored);

    match found {
        Some(candidate) if password_matches && candidate.password_hash.is_some() => Ok(candidate),
        _ => Err(invalid()),
    }
}

/// A hash to verify against when there is no real one, so a doomed login costs
/// the same Argon2 work as a live one.
///
/// Generated at first use rather than written as a literal: a hand-written PHC
/// string that fails to parse would make `verify` return early, which is exactly
/// the fast path this exists to remove — and the test for it would still pass,
/// because a wrong password and an unparseable hash both return `false`.
static DUMMY_PASSWORD_HASH: LazyLock<String> = LazyLock::new(|| {
    password::hash("flatnexus timing equalization placeholder")
        .expect("hashing a fixed string cannot fail")
});

/// Resolve an OAuth profile to a publisher, creating or linking as needed.
///
/// Resolution order:
///
/// 1. An existing identity for `(provider, subject)` — the returning user.
/// 2. An existing publisher whose email matches a *verified* provider email —
///    the same person who previously signed up locally. The identity is linked.
/// 3. Otherwise a new publisher is created.
pub async fn upsert_from_oauth(db: &Db, profile: &OAuthProfile) -> AppResult<Publisher> {
    if let Some(identity) = find_identity(db, profile.provider, &profile.subject).await? {
        return db
            .publisher()
            .find_first()
            .r#where(publisher::id::equals(identity.publisher_id.clone()))
            .exec()
            .await?
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "oauth identity {} references a missing publisher",
                    identity.id
                ))
            });
    }

    // A provider address that fails normalization is treated as "no usable
    // email", not as a hard error: the sign-in itself already succeeded, and
    // aborting it over an address FlatNexus merely dislikes would strand the
    // publisher with no way in.
    let verified_email = profile
        .email
        .as_deref()
        .filter(|_| profile.email_verified)
        .and_then(|email| normalize_email(email).ok());

    let publisher_row = match &verified_email {
        Some(email) => match find_by_email(db, email).await? {
            Some(existing) => existing,
            None => create_from_oauth(db, profile, Some(email.clone())).await?,
        },
        // No verified address: there is nothing safe to match on, so this is
        // always a fresh account.
        None => create_from_oauth(db, profile, None).await?,
    };

    // A concurrent first-time login for the same identity may have won the race
    // to the `(provider, subject)` unique index. That is not a failure — it
    // means the identity now exists, so re-resolve and return the winner's row
    // rather than handing this caller a conflict for work that succeeded.
    if let Err(e) = link_identity(db, &publisher_row.id, profile, verified_email).await {
        let Some(identity) = find_identity(db, profile.provider, &profile.subject).await? else {
            return Err(e);
        };
        return db
            .publisher()
            .find_first()
            .r#where(publisher::id::equals(identity.publisher_id.clone()))
            .exec()
            .await?
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "oauth identity {} references a missing publisher",
                    identity.id
                ))
            });
    }

    Ok(publisher_row)
}

async fn create_from_oauth(
    db: &Db,
    profile: &OAuthProfile,
    email: Option<String>,
) -> AppResult<Publisher> {
    // Without a usable address, synthesize a stable placeholder so the NOT NULL
    // UNIQUE column still holds. The publisher can set a real one later.
    let email = email.unwrap_or_else(|| {
        format!("{}.{}@oauth.flatnexus.invalid", profile.provider, profile.subject)
    });

    let display_name = profile
        .display_name
        .clone()
        .unwrap_or_else(|| email.split('@').next().unwrap_or("publisher").to_string());

    let now = models::now_secs();
    let created = db
        .publisher()
        .create()
        .set("id", models::new_id())
        .set("email", email)
        .set("display_name", display_name)
        // No password: this account authenticates through its provider until
        // the publisher explicitly sets one.
        .set("otp_method", OtpMethod::Email.as_str().to_string())
        .set("totp_confirmed", false)
        .set("is_admin", false)
        .set("created_at", now)
        .exec()
        .await?;

    Ok(created)
}

async fn link_identity(
    db: &Db,
    publisher_id: &str,
    profile: &OAuthProfile,
    email: Option<String>,
) -> AppResult<()> {
    let now = models::now_secs();
    let mut create = db
        .oauth_identity()
        .create()
        .set("id", models::new_id())
        .set("publisher_id", publisher_id.to_string())
        .set("provider", profile.provider.to_string())
        .set("subject", profile.subject.clone())
        .set("created_at", now);

    if let Some(email) = email {
        create = create.set("email", email);
    }

    create.exec().await?;
    Ok(())
}

async fn find_identity(
    db: &Db,
    provider: &str,
    subject: &str,
) -> AppResult<Option<OauthIdentity>> {
    Ok(db
        .oauth_identity()
        .find_first()
        .r#where(oauth_identity::provider::equals(provider.to_string()))
        .r#where(oauth_identity::subject::equals(subject.to_string()))
        .exec()
        .await?)
}

pub async fn find_by_email(db: &Db, email: &str) -> AppResult<Option<Publisher>> {
    Ok(db
        .publisher()
        .find_first()
        .r#where(publisher::email::equals(email.to_string()))
        .exec()
        .await?)
}

/// Lowercase and validate an address.
///
/// Case-folding matters for correctness, not just tidiness: without it,
/// `Dev@example.com` and `dev@example.com` would be two accounts, and an OAuth
/// login could fail to match the local account it belongs to.
pub fn normalize_email(email: &str) -> AppResult<String> {
    let email = email.trim().to_ascii_lowercase();

    let domain_is_sane = |domain: &str| {
        // Every dot-separated label must be non-empty, so `dev@.com`, `dev@x.`
        // and `dev@...` are refused rather than stored as addresses that can
        // never receive a one-time code.
        domain.contains('.') && domain.split('.').all(|label| !label.is_empty())
    };

    let valid = email.len() >= 3
        && email.matches('@').count() == 1
        && !email.starts_with('@')
        && !email.ends_with('@')
        // Whitespace and control characters are header-injection material and
        // are never part of an address anyone can actually be mailed at.
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
        && email.split('@').nth(1).is_some_and(domain_is_sane);

    if !valid {
        return Err(AppError::Validation("That email address is not valid.".into()));
    }
    Ok(email)
}

#[cfg(test)]
mod tests {
    use super::normalize_email;

    #[test]
    fn normalizes_case_and_whitespace() {
        assert_eq!(
            normalize_email("  Dev@Example.COM ").unwrap(),
            "dev@example.com"
        );
    }

    #[test]
    fn rejects_malformed_addresses() {
        for bad in ["", "nope", "@example.com", "dev@", "a@b@c.com", "dev@localhost"] {
            assert!(normalize_email(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn rejects_addresses_that_could_never_be_mailed() {
        for bad in ["dev@.com", "dev@x.", "dev@...", "a b@example.com", "dev\n@x.com"] {
            assert!(normalize_email(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn the_dummy_hash_is_parseable_so_it_actually_costs_argon2_work() {
        // The property that matters is that `verify` reaches the KDF rather than
        // bailing out on a malformed hash. Asserting only "does not match" would
        // pass for an unparseable string and prove nothing.
        use argon2::password_hash::PasswordHash;
        let dummy = super::DUMMY_PASSWORD_HASH.as_str();
        assert!(
            PasswordHash::new(dummy).is_ok(),
            "dummy hash must parse, or the timing equalization is a no-op"
        );
        assert!(!crate::auth::password::verify("anything at all", dummy));
    }
}
