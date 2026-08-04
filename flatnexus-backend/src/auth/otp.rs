//! One-time passwords.
//!
//! Sign-in, build uploads, and build deletions all require a fresh OTP. A
//! publisher chooses their method:
//!
//! * [`OtpMethod::Totp`] — RFC 6238 authenticator app. Nothing to request; the
//!   publisher reads the current code off their device. This is the method that
//!   works unattended from `curl` without a mail round-trip.
//! * [`OtpMethod::Email`] — the server mails an 8-character code: automatically
//!   as part of sign-in, or on demand via `requestUploadOtp` before an upload.
//!
//! Both paths are single-use. Emailed codes are marked consumed; TOTP codes are
//! burned by recording their time-step, because a TOTP code is otherwise valid
//! for its entire 30-second step (plus the accepted skew) and could be replayed
//! within it.

use std::fmt;

use base32::Alphabet;
use rand::Rng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use totp_rs::{Algorithm, Secret, TOTP};

use crate::error::{AppError, AppResult};
use crate::models::{self, Publisher, TotpReplay, otp_challenge, totp_replay};
use crate::models::PraxClientExt;
use crate::state::Db;

/// TOTP parameters. Fixed rather than configurable: they are baked into the
/// `otpauth://` URI a publisher scans, so changing them would silently
/// invalidate every already-enrolled authenticator.
const TOTP_DIGITS: usize = 6;
const TOTP_STEP_SECS: u64 = 30;
/// Accept the previous and next step, tolerating ~30s of clock drift.
const TOTP_SKEW: u8 = 1;
const TOTP_SECRET_BYTES: usize = 20;

/// Emailed codes use an unambiguous alphabet: no `0`/`O`, no `1`/`I`.
const EMAIL_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const EMAIL_CODE_LEN: usize = 8;

/// Prune spent TOTP time-steps older than this. Comfortably beyond the widest
/// window any code can still be accepted in.
///
/// The ledger is keyed on `(publisher, time_step)` and deliberately not on
/// purpose: a code is a code, and one captured from a sign-in must not be
/// replayable against an upload. The cost is that two operations inside the
/// same 30-second step cannot both succeed — a script must wait for the next
/// code — which is the correct trade.
const REPLAY_RETENTION_SECS: i64 = 300;

/// Which second factor a publisher uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtpMethod {
    Totp,
    Email,
}

impl OtpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Totp => "totp",
            Self::Email => "email",
        }
    }

    pub fn parse(raw: &str) -> AppResult<Self> {
        match raw {
            "totp" => Ok(Self::Totp),
            "email" => Ok(Self::Email),
            other => Err(AppError::Validation(format!(
                "Unknown OTP method '{other}'."
            ))),
        }
    }
}

impl fmt::Display for OtpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an OTP authorizes.
///
/// Emailed codes are scoped by purpose in the database, so a code minted to
/// authorize an upload cannot be redirected into a deletion. TOTP codes have no
/// purpose — a code is a code — which is why the replay ledger burns a time-step
/// globally rather than per purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtpPurpose {
    /// Completing a sign-in whose first factor already passed.
    Login,
    PublishBuild,
    DeleteBuild,
}

impl OtpPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::PublishBuild => "publish_build",
            Self::DeleteBuild => "delete_build",
        }
    }
}

/// A freshly generated TOTP enrollment, shown once at enrollment time.
#[derive(Debug, Clone)]
pub struct TotpEnrollment {
    /// Base32 secret, for manual entry.
    pub secret: String,
    /// `otpauth://` URI to render as a QR code.
    pub uri: String,
}

pub struct OtpService {
    issuer: String,
    email_ttl_secs: i64,
}

impl OtpService {
    pub fn new(issuer: String, email_ttl_secs: i64) -> Self {
        Self {
            issuer,
            email_ttl_secs,
        }
    }

    // -- TOTP enrollment ----------------------------------------------------

    /// Generate an unconfirmed TOTP secret and its enrollment URI.
    ///
    /// The secret is stored on the publisher immediately but `totp_confirmed`
    /// stays false until [`confirm_totp`](Self::confirm_totp) sees a valid code,
    /// so a publisher cannot lock themselves out by scanning a broken QR code.
    pub fn generate_totp(&self, account: &str) -> TotpEnrollment {
        let mut raw = [0u8; TOTP_SECRET_BYTES];
        rand::thread_rng().fill(&mut raw);
        let secret = base32::encode(Alphabet::Rfc4648 { padding: false }, &raw);

        let label = format!(
            "{}:{}",
            urlencoding::encode(&self.issuer),
            urlencoding::encode(account)
        );
        let uri = format!(
            "otpauth://totp/{label}?secret={secret}&issuer={issuer}\
             &algorithm=SHA1&digits={digits}&period={period}",
            issuer = urlencoding::encode(&self.issuer),
            digits = TOTP_DIGITS,
            period = TOTP_STEP_SECS,
        );

        TotpEnrollment { secret, uri }
    }

    /// Check a code against an as-yet-unconfirmed secret, without touching the
    /// replay ledger — enrollment confirmation is not an authorization.
    pub fn check_totp_code(&self, secret: &str, code: &str) -> AppResult<bool> {
        let totp = build_totp(secret)?;
        totp.check_current(code)
            .map_err(|e| AppError::Internal(format!("system clock unusable for TOTP: {e}")))
    }

    // -- Emailed codes ------------------------------------------------------

    /// Mint an emailed one-time code, persist its hash, and return the
    /// plaintext for delivery. The plaintext exists only in this return value —
    /// it is never stored.
    pub async fn issue_email_code(
        &self,
        db: &Db,
        publisher_id: &str,
        purpose: OtpPurpose,
    ) -> AppResult<String> {
        // Supersede any outstanding code for the same purpose so a publisher
        // cannot accumulate a pool of live codes by spamming the mutation.
        db.otp_challenge()
            .update_many()
            .r#where(otp_challenge::publisher_id::equals(publisher_id.to_string()))
            .r#where(otp_challenge::purpose::equals(purpose.as_str().to_string()))
            .r#where(otp_challenge::consumed::equals(false))
            .set("consumed", true)
            .exec()
            .await?;

        let code = random_email_code();
        let now = models::now_secs();

        db.otp_challenge()
            .create()
            .set("id", models::new_id())
            .set("publisher_id", publisher_id)
            .set("purpose", purpose.as_str())
            .set("code_hash", hash_code(&code))
            .set("expires_at", now + self.email_ttl_secs)
            .set("consumed", false)
            .set("created_at", now)
            .exec()
            .await?;

        Ok(code)
    }

    pub fn email_ttl_secs(&self) -> i64 {
        self.email_ttl_secs
    }

    // -- Verification -------------------------------------------------------

    /// Verify `code` for `publisher` and `purpose`, consuming it on success.
    ///
    /// This is the single gate every privileged write goes through. It must be
    /// called *before* the operation it authorizes has any effect.
    pub async fn verify(
        &self,
        db: &Db,
        publisher: &Publisher,
        purpose: OtpPurpose,
        code: &str,
    ) -> AppResult<()> {
        let code = code.trim();
        if code.is_empty() {
            return Err(AppError::OtpRejected(
                "A one-time password is required for this operation.".into(),
            ));
        }

        match OtpMethod::parse(&publisher.otp_method)? {
            OtpMethod::Totp => self.verify_totp(db, publisher, code).await,
            OtpMethod::Email => self.verify_email(db, publisher, purpose, code).await,
        }
    }

    async fn verify_totp(&self, db: &Db, publisher: &Publisher, code: &str) -> AppResult<()> {
        let secret = publisher.totp_secret.as_deref().filter(|_| publisher.totp_confirmed);
        let Some(secret) = secret else {
            return Err(AppError::OtpRejected(
                "No confirmed authenticator is enrolled on this account.".into(),
            ));
        };

        let totp = build_totp(secret)?;
        let now = models::now_secs();

        // `check_current` accepts the neighbouring steps, so identify which step
        // actually matched: that is the one to burn.
        let Some(matched_step) = matching_step(&totp, code, now) else {
            return Err(AppError::OtpRejected(
                "That authenticator code is not valid.".into(),
            ));
        };

        // Burning the step is a unique-constrained insert, so two concurrent
        // requests carrying the same code cannot both succeed.
        let burned = db
            .totp_replay()
            .create()
            .set("id", models::new_id())
            .set("publisher_id", publisher.id.clone())
            .set("time_step", matched_step)
            .set("created_at", now)
            .exec()
            .await;

        if burned.is_err() {
            // Either the unique constraint fired (a genuine replay) or the write
            // failed. Both must fail closed.
            if replay_exists(db, &publisher.id, matched_step).await? {
                return Err(AppError::OtpRejected(
                    "That authenticator code has already been used. \
                     Wait for the next code."
                        .into(),
                ));
            }
            return Err(AppError::Internal(
                "could not record one-time password use".into(),
            ));
        }

        // Pruned occasionally rather than on every authentication: the ledger is
        // tiny per publisher and a DELETE on each sign-in and each publish is a
        // round trip spent on housekeeping.
        if should_prune(now) {
            prune_replays(db, &publisher.id, now).await;
        }
        Ok(())
    }

    async fn verify_email(
        &self,
        db: &Db,
        publisher: &Publisher,
        purpose: OtpPurpose,
        code: &str,
    ) -> AppResult<()> {
        let now = models::now_secs();
        let candidate_hash = hash_code(&code.to_ascii_uppercase());

        // One conditional UPDATE, not a read followed by a write. Every
        // condition — ownership, purpose, hash, expiry, unconsumed — is in the
        // predicate, so the check and the claim are the same statement and a
        // code cannot expire, or be spent by a concurrent request, in the window
        // between them.
        //
        // Matching the hash inside SQL gives up constant-time comparison. That
        // is not a meaningful loss here: the compared value is a SHA-256 digest
        // of a code drawn from a 32-character alphabet, so a timing signal would
        // have to leak digest bytes rather than code characters, and an attacker
        // who could grind digests offline would not need the oracle.
        let claimed = db
            .otp_challenge()
            .update_many()
            .r#where(otp_challenge::publisher_id::equals(publisher.id.clone()))
            .r#where(otp_challenge::purpose::equals(purpose.as_str().to_string()))
            .r#where(otp_challenge::consumed::equals(false))
            .r#where(otp_challenge::code_hash::equals(candidate_hash))
            .r#where(otp_challenge::expires_at::gte(now))
            .set("consumed", true)
            .exec()
            .await?;

        if claimed == 0 {
            // Deliberately one message for wrong, expired, already-used, and
            // never-requested. Distinguishing them would confirm to a caller
            // guessing codes that a code exists and how long they have.
            return Err(AppError::OtpRejected(
                "That one-time code is not valid. It may have expired or already \
                 been used — request a new one."
                    .into(),
            ));
        }

        Ok(())
    }
}

/// Which 30-second step `code` corresponds to, within the accepted skew.
fn matching_step(totp: &TOTP, code: &str, now_secs: i64) -> Option<i64> {
    let current = now_secs / TOTP_STEP_SECS as i64;
    let skew = TOTP_SKEW as i64;

    (current - skew..=current + skew).find(|step| {
        let at = (*step * TOTP_STEP_SECS as i64).max(0) as u64;
        // `generate` is deterministic for a given step, so comparing generated
        // output identifies the step the presented code came from.
        constant_time_eq(&totp.generate(at), code)
    })
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

fn build_totp(secret_base32: &str) -> AppResult<TOTP> {
    let bytes = Secret::Encoded(secret_base32.to_string())
        .to_bytes()
        .map_err(|e| AppError::Internal(format!("stored TOTP secret is malformed: {e:?}")))?;

    TOTP::new(
        Algorithm::SHA1,
        TOTP_DIGITS,
        TOTP_SKEW,
        TOTP_STEP_SECS,
        bytes,
    )
    .map_err(|e| AppError::Internal(format!("could not build TOTP: {e}")))
}

async fn replay_exists(db: &Db, publisher_id: &str, step: i64) -> AppResult<bool> {
    let found: Option<TotpReplay> = db
        .totp_replay()
        .find_first()
        .r#where(totp_replay::publisher_id::equals(publisher_id.to_string()))
        .r#where(totp_replay::time_step::equals(step))
        .exec()
        .await?;
    Ok(found.is_some())
}

/// Best-effort cleanup of spent steps. A failure here is not worth failing an
/// otherwise-valid authorization over, so it is logged and dropped.
async fn prune_replays(db: &Db, publisher_id: &str, now: i64) {
    let cutoff = now - REPLAY_RETENTION_SECS;
    let result = db
        .totp_replay()
        .delete_many()
        .r#where(totp_replay::publisher_id::equals(publisher_id.to_string()))
        .r#where(totp_replay::created_at::lt(cutoff))
        .exec()
        .await;

    if let Err(e) = result {
        armature_log::warn!("could not prune spent TOTP steps: {e}");
    }
}

/// Whether this authentication should also prune spent time-steps.
///
/// Sampled from the clock rather than a counter so it needs no shared state; the
/// retention window is minutes and the ledger is per-publisher, so pruning on
/// roughly one call in sixteen is ample.
fn should_prune(now: i64) -> bool {
    now % 16 == 0
}

fn random_email_code() -> String {
    let mut rng = rand::thread_rng();
    (0..EMAIL_CODE_LEN)
        .map(|_| EMAIL_CODE_ALPHABET[rng.gen_range(0..EMAIL_CODE_ALPHABET.len())] as char)
        .collect()
}

fn hash_code(code: &str) -> String {
    let digest = Sha256::digest(code.as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_codes_avoid_ambiguous_characters() {
        for _ in 0..64 {
            let code = random_email_code();
            assert_eq!(code.len(), EMAIL_CODE_LEN);
            assert!(!code.contains('0') && !code.contains('O'));
            assert!(!code.contains('1') && !code.contains('I'));
        }
    }

    #[test]
    fn generated_totp_secret_round_trips() {
        let service = OtpService::new("FlatNexus".into(), 600);
        let enrollment = service.generate_totp("dev@example.com");
        assert!(enrollment.uri.starts_with("otpauth://totp/"));
        // A malformed secret would fail to decode here.
        build_totp(&enrollment.secret).expect("generated secret must be usable");
    }

    #[test]
    fn matching_step_identifies_the_code_it_generated() {
        let service = OtpService::new("FlatNexus".into(), 600);
        let enrollment = service.generate_totp("dev@example.com");
        let totp = build_totp(&enrollment.secret).unwrap();

        let now = 1_700_000_000i64;
        let step = now / TOTP_STEP_SECS as i64;
        let code = totp.generate((step * TOTP_STEP_SECS as i64) as u64);

        assert_eq!(matching_step(&totp, &code, now), Some(step));
        assert_eq!(matching_step(&totp, "000000", now + 10_000), None);
    }

    #[test]
    fn otp_method_round_trips() {
        assert_eq!(OtpMethod::parse("totp").unwrap(), OtpMethod::Totp);
        assert_eq!(OtpMethod::parse("email").unwrap(), OtpMethod::Email);
        assert!(OtpMethod::parse("sms").is_err());
    }
}
