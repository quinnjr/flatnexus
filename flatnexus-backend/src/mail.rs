//! Outbound mail.
//!
//! Only used for emailed one-time codes. Publishers on the TOTP method never
//! touch this path, so a deployment that only allows TOTP can leave SMTP
//! unconfigured entirely.

use armature_mail::{Email, Mailer as ArmatureMailer, SmtpConfig};

use crate::auth::otp::OtpPurpose;
use crate::config::SmtpConfig as AppSmtpConfig;
use crate::error::{AppError, AppResult};

pub struct Mailer {
    inner: ArmatureMailer,
    from: String,
}

impl Mailer {
    pub async fn new(config: &AppSmtpConfig) -> AppResult<Self> {
        let mut smtp = SmtpConfig::new(config.host.clone()).port(config.port);
        if let (Some(user), Some(pass)) = (&config.username, &config.password) {
            smtp = smtp.credentials(user.clone(), pass.clone());
        }
        // 25 (local relay) and 1025 (MailHog, and the port docker-compose
        // publishes) are the two where STARTTLS is usually absent; anything else
        // is assumed to be a real submission port.
        if config.port != 25 && config.port != 1025 {
            smtp = smtp.starttls();
        }

        let inner = ArmatureMailer::smtp(smtp)
            .await
            .map_err(|e| AppError::Config(format!("could not initialize mailer: {e}")))?;

        Ok(Self {
            inner,
            from: config.from.clone(),
        })
    }

    /// Deliver a one-time code.
    ///
    /// The code is in the body only — never the subject — so it does not surface
    /// in a lock-screen notification preview.
    ///
    /// The wording depends on the purpose. One shared "publish code" message was
    /// actively misleading on the sign-in path: it told someone signing in that a
    /// build was being published, and reassured them the code was useless without
    /// a password — when on that path the code *is* the second half of the
    /// sign-in, and a leaked one matters.
    pub async fn send_otp(
        &self,
        to: &str,
        code: &str,
        ttl_secs: i64,
        purpose: OtpPurpose,
    ) -> AppResult<()> {
        let minutes = (ttl_secs / 60).max(1);

        let (subject, lead, caution) = match purpose {
            OtpPurpose::Login => (
                "Your FlatNexus sign-in code",
                "Someone entered your password and is signing in to FlatNexus.",
                "If this wasn't you, someone knows your password. Change it as soon \
                 as you can — this code is the only thing standing between them and \
                 your account.",
            ),
            OtpPurpose::PublishBuild => (
                "Your FlatNexus publish code",
                "Someone requested a one-time code to publish a Flatpak build to FlatNexus.",
                "If this wasn't you, no action is needed — the code alone cannot be \
                 used without an active session.",
            ),
            OtpPurpose::DeleteBuild => (
                "Your FlatNexus withdrawal code",
                "Someone requested a one-time code to withdraw a Flatpak build from FlatNexus.",
                "If this wasn't you, no action is needed — the code alone cannot be \
                 used without an active session.",
            ),
        };

        let body = format!(
            "{lead}\n\n\
             Code: {code}\n\n\
             It is valid for {minutes} minute(s) and can be used once.\n\n\
             {caution}\n"
        );

        let email = Email::new()
            .from(self.from.as_str())
            .to(to)
            .subject(subject)
            .text(body);

        self.inner
            .send(email)
            .await
            .map_err(|e| AppError::Mail(e.to_string()))?;

        Ok(())
    }
}
