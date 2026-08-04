//! GitHub and Google sign-in.
//!
//! OAuth is the one part of the API that is not GraphQL, and cannot be: the
//! authorization step is a browser redirect to a third party, and the callback
//! is a plain `GET` the provider performs. GraphQL still owns everything
//! downstream — the callback mints an ordinary session token and hands the
//! browser back to the frontend, which uses it exactly as it would a
//! password-login token.
//!
//! Accounts are keyed on `(provider, subject)`, never on email: a provider email
//! can be changed by its owner, and treating it as the identity would let
//! whoever later acquires an abandoned address inherit a publisher account.
//! Email is used only to *link* a first-time OAuth login to an existing local
//! account, and only when the provider asserts the address is verified.

use std::collections::HashMap;

use armature_auth::oauth2::{GenericOAuth2Provider, OAuth2Config, OAuth2Provider};
use armature_auth::providers::GitHubProvider;
use serde::Deserialize;

use crate::config::Config;
use crate::error::{AppError, AppResult};

/// Google's OIDC userinfo endpoint. Deliberately not the older
/// `/oauth2/v2/userinfo`, which returns `id`/`verified_email` rather than the
/// `sub`/`email_verified` claims the generic provider decodes.
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// GitHub's address list. Queried directly rather than through
/// `GitHubProvider::get_user_info`, which resolves an address like this:
///
/// ```text
/// emails.iter().find(|e| e.primary && e.verified).or_else(|| emails.first())
/// ```
///
/// — falling back to an *unverified* address, and returning a `GitHubUser` that
/// carries no `verified` field to tell the two apart. Since a verified address
/// is what authorizes linking an OAuth identity to an existing publisher,
/// accepting that fallback would let someone who merely *added* a victim's
/// address to their own GitHub account inherit the victim's publisher. This
/// endpoint is read here so the verified flag survives to the decision.
const GITHUB_EMAILS_URL: &str = "https://api.github.com/user/emails";

/// One entry from `GET /user/emails`.
#[derive(Debug, Deserialize)]
struct GitHubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

/// Providers this deployment has credentials for.
pub struct OAuthRegistry {
    providers: HashMap<&'static str, GenericOAuth2Provider>,
}

/// The normalized result of a completed OAuth round-trip.
#[derive(Debug, Clone)]
pub struct OAuthProfile {
    pub provider: &'static str,
    /// Stable provider-side identifier.
    pub subject: String,
    pub email: Option<String>,
    /// True only when the provider asserts the address is verified. An
    /// unverified address is never used to link to an existing account.
    pub email_verified: bool,
    pub display_name: Option<String>,
}

/// A provider offered to the frontend.
#[derive(Debug, Clone)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    /// Absolute URL the browser navigates to in order to start sign-in.
    pub start_url: String,
}

impl OAuthRegistry {
    pub fn from_config(config: &Config) -> AppResult<Self> {
        let mut providers = HashMap::new();

        if let Some(client) = &config.oauth.google {
            let oauth_config = OAuth2Config::new(
                client.client_id.clone(),
                client.client_secret.clone(),
                GOOGLE_AUTH_URL.to_string(),
                GOOGLE_TOKEN_URL.to_string(),
                config.oauth_redirect_url("google"),
            )
            .with_scopes(vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ])
            .with_user_info_url(GOOGLE_USERINFO_URL.to_string());

            let provider = GenericOAuth2Provider::new("google".to_string(), oauth_config)
                .map_err(|e| AppError::Config(format!("google oauth misconfigured: {e}")))?;
            providers.insert("google", provider);
        }

        if let Some(client) = &config.oauth.github {
            // GitHub's own config helper already sets the `user:email` scope and
            // the profile endpoint.
            let oauth_config = GitHubProvider::config(
                client.client_id.clone(),
                client.client_secret.clone(),
                config.oauth_redirect_url("github"),
            );
            let provider = GenericOAuth2Provider::new("github".to_string(), oauth_config)
                .map_err(|e| AppError::Config(format!("github oauth misconfigured: {e}")))?;
            providers.insert("github", provider);
        }

        Ok(Self { providers })
    }

    /// Providers the frontend should render sign-in buttons for.
    pub fn descriptors(&self, config: &Config) -> Vec<ProviderDescriptor> {
        let mut out: Vec<ProviderDescriptor> = self
            .providers
            .keys()
            .map(|id| ProviderDescriptor {
                id,
                display_name: display_name(id),
                start_url: format!("{}/auth/oauth/{}/start", config.public_url, id),
            })
            .collect();
        out.sort_by_key(|d| d.id);
        out
    }

    /// Build the provider's authorization URL along with the CSRF state value
    /// that must come back unchanged on the callback.
    pub fn authorization_url(&self, provider: &str) -> AppResult<(String, String)> {
        let (url, csrf) = self
            .lookup(provider)?
            .authorization_url()
            .map_err(|e| AppError::Internal(format!("could not build authorization URL: {e}")))?;

        Ok((url.to_string(), csrf.secret().to_string()))
    }

    /// Exchange the callback's authorization code for a normalized profile.
    pub async fn complete(&self, provider: &str, code: String) -> AppResult<OAuthProfile> {
        let configured = self.lookup(provider)?;
        let label = display_name(provider);

        let token = configured
            .exchange_code(code)
            .await
            .map_err(|e| AppError::Forbidden(format!("{label} sign-in failed: {e}")))?;

        match provider {
            "github" => {
                let user = GitHubProvider::get_user_info(&token.access_token)
                    .await
                    .map_err(|e| AppError::Forbidden(format!("{label} sign-in failed: {e}")))?;

                // The only address trusted for linking is one GitHub itself
                // reports as primary *and* verified.
                let verified = github_primary_verified_email(&token.access_token).await?;

                Ok(OAuthProfile {
                    provider: "github",
                    subject: user.id.to_string(),
                    email_verified: verified.is_some(),
                    // Fall back to the profile address purely so a new account
                    // gets a sensible email; with `email_verified` false it can
                    // never link to an existing publisher.
                    email: verified.or(user.email),
                    display_name: user.name.or(Some(user.login)),
                })
            }
            "google" => {
                let info = configured
                    .get_user_info(&token)
                    .await
                    .map_err(|e| AppError::Forbidden(format!("{label} sign-in failed: {e}")))?;

                Ok(OAuthProfile {
                    provider: "google",
                    subject: info.sub,
                    email: info.email,
                    email_verified: info.email_verified.unwrap_or(false),
                    display_name: info.name,
                })
            }
            // Unreachable while `lookup` resolves only the two providers above.
            // Explicit rather than a catch-all: the previous catch-all filed
            // every unknown provider's identities under `provider = "google"`,
            // which would collide in the `(provider, subject)` unique namespace
            // and cross-link accounts the first time a third provider was added.
            other => Err(AppError::Internal(format!(
                "no profile mapping for oauth provider '{other}'"
            ))),
        }
    }

    fn lookup(&self, provider: &str) -> AppResult<&GenericOAuth2Provider> {
        self.providers.get(provider).ok_or_else(|| {
            AppError::NotFound(format!("'{provider}' sign-in is not enabled on this server"))
        })
    }
}

/// The caller's primary, verified GitHub address, if they have one.
///
/// A failure to reach the endpoint returns `None` rather than an error: the
/// sign-in itself is already authenticated by the token exchange, and the only
/// thing this address governs is whether the identity may link to an existing
/// account. Failing closed here means a new account, not a rejected login.
async fn github_primary_verified_email(access_token: &str) -> AppResult<Option<String>> {
    let response = reqwest::Client::new()
        .get(GITHUB_EMAILS_URL)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "FlatNexus")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;

    let emails: Vec<GitHubEmail> = match response {
        Ok(response) => match response.json().await {
            Ok(emails) => emails,
            Err(e) => {
                armature_log::warn!("could not read GitHub addresses: {e}");
                return Ok(None);
            }
        },
        Err(e) => {
            armature_log::warn!("could not reach GitHub addresses endpoint: {e}");
            return Ok(None);
        }
    };

    Ok(emails
        .into_iter()
        .find(|entry| entry.primary && entry.verified)
        .map(|entry| entry.email))
}

fn display_name(id: &str) -> &'static str {
    match id {
        "github" => "GitHub",
        "google" => "Google",
        _ => "OAuth",
    }
}
