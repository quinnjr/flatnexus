//! GitHub and Google sign-in redirects.
//!
//! `GET /auth/oauth/:provider/start` sends the browser to the provider.
//! `GET /auth/oauth/:provider/callback` receives the code, mints a session, and
//! hands the browser back to the frontend.
//!
//! # Why the token travels in a fragment
//!
//! The callback redirects to `${CORS_ORIGIN}/auth/callback#token=…`. A URL
//! fragment is never sent to a server, so the session token stays out of the
//! frontend host's access logs, out of `Referer` headers, and out of any proxy
//! in between. The frontend reads it from `location.hash` and immediately clears
//! it. A query parameter would have leaked into all three.

use crate::prelude::*;

use crate::auth::accounts;
use crate::error::AppError;
use crate::state::state;

/// Cookie holding the OAuth CSRF state between the two legs of the flow.
const STATE_COOKIE: &str = "flatnexus_oauth_state";

#[controller("/auth/oauth")]
#[derive(Default, Clone)]
pub struct OAuthController;

#[routes]
impl OAuthController {
    /// Begin sign-in with `provider`.
    #[get("/:provider/start")]
    async fn start(req: HttpRequest) -> Result<HttpResponse, Error> {
        let provider = req.param("provider").unwrap_or_default().to_string();
        let state = state();

        let (url, csrf) = state.oauth.authorization_url(&provider)?;

        Ok(HttpResponse::redirect(url).with_header(
            "Set-Cookie".into(),
            // `SameSite=Lax` rather than `Strict`: the cookie has to survive the
            // provider's top-level redirect back here, which `Strict` would
            // suppress, breaking every sign-in.
            format!(
                "{STATE_COOKIE}={csrf}; Path=/auth/oauth; HttpOnly; SameSite=Lax; \
                 Max-Age=600{secure}",
                secure = secure_attribute(&state.config.public_url),
            ),
        ))
    }

    /// Complete sign-in and hand the browser back to the frontend.
    #[get("/:provider/callback")]
    async fn callback(req: HttpRequest) -> Result<HttpResponse, Error> {
        let provider = req.param("provider").unwrap_or_default().to_string();
        let state = state();

        // A provider that declines reports it here rather than by failing the
        // redirect, so surface its reason instead of a generic error.
        let query = req.query();
        if let Some(reason) = query.get("error") {
            return Err(AppError::Forbidden(format!("Sign-in was declined: {reason}")).into());
        }

        let code = query
            .get("code")
            .map(str::to_string)
            .ok_or_else(|| AppError::Validation("Missing authorization code.".into()))?;

        let returned_state = query
            .get("state")
            .map(str::to_string)
            .ok_or_else(|| AppError::Validation("Missing CSRF state.".into()))?;

        let expected = req
            .headers
            .get("Cookie")
            .and_then(|c| cookie_value(c, STATE_COOKIE))
            .ok_or_else(|| {
                AppError::Forbidden(
                    "Sign-in state was lost. Start again from the sign-in page.".into(),
                )
            })?;

        // Without this the callback would accept a code minted by an attacker
        // for their own account, silently signing the victim in as someone else.
        if expected != returned_state {
            return Err(AppError::Forbidden("Sign-in state did not match.".into()).into());
        }

        let profile = state.oauth.complete(&provider, code).await?;
        let publisher = accounts::upsert_from_oauth(&state.db, &profile).await?;
        let issued = state.tokens.issue(&publisher.id, &publisher.email)?;

        armature_log::info!("{} signed in via {}", publisher.email, profile.provider);

        let landing = format!(
            "{}#token={}&expires_at={}",
            state.config.frontend_auth_landing(),
            urlencoding::encode(&issued.token),
            issued.expires_at,
        );

        Ok(HttpResponse::redirect(landing).with_header(
            "Set-Cookie".into(),
            format!("{STATE_COOKIE}=; Path=/auth/oauth; HttpOnly; Max-Age=0"),
        ))
    }
}

/// Only mark the cookie `Secure` when this server is actually reachable over
/// HTTPS — otherwise local HTTP development would never receive it back.
fn secure_attribute(public_url: &str) -> &'static str {
    if public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    }
}

/// Pull one cookie's value out of a `Cookie` header.
fn cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::cookie_value;

    #[test]
    fn extracts_a_named_cookie() {
        let header = "theme=dark; flatnexus_oauth_state=abc123; other=x";
        assert_eq!(
            cookie_value(header, "flatnexus_oauth_state"),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn returns_none_when_absent_or_malformed() {
        assert_eq!(cookie_value("theme=dark", "flatnexus_oauth_state"), None);
        assert_eq!(cookie_value("", "flatnexus_oauth_state"), None);
        assert_eq!(cookie_value("novalue", "novalue"), None);
    }

    #[test]
    fn does_not_match_a_cookie_by_prefix() {
        // `flatnexus_oauth_state_other` must not satisfy a lookup for
        // `flatnexus_oauth_state`.
        assert_eq!(
            cookie_value("flatnexus_oauth_state_other=x", "flatnexus_oauth_state"),
            None
        );
    }
}
