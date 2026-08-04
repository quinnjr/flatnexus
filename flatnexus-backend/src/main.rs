//! FlatNexus — a self-hosted Flatpak distribution point.
//!
//! The server does two jobs:
//!
//! 1. Serve a GraphQL API that publishers use to register applications and
//!    upload builds.
//! 2. Serve the resulting OSTree repository over plain HTTP, so `flatpak
//!    remote-add`, `flatpak install`, and `flatpak update` work against it with
//!    no client-side changes.

mod auth;
mod bundles;
mod config;
mod db;
mod error;
mod graphql;
mod http;
mod mail;
mod models;
mod ostree;
mod prelude;
mod screenshots;
mod state;
mod storage;

use crate::prelude::*;

use crate::auth::oauth::OAuthRegistry;
use crate::auth::otp::OtpService;
use crate::auth::token::TokenService;
use crate::config::Config;
use crate::error::AppResult;
use crate::http::graphql_controller::{GraphQLController, PlaygroundController};
use crate::http::health_controller::HealthController;
use crate::http::oauth_controller::OAuthController;
use crate::http::repo_controller::{RepoController, RepoDiscoveryController};
use crate::mail::Mailer;
use crate::ostree::RepoManager;
use crate::storage::BundleStore;
use crate::state::AppState;

#[module(
    controllers: [
        GraphQLController,
        PlaygroundController,
        OAuthController,
        RepoController,
        RepoDiscoveryController,
        HealthController,
    ]
)]
#[derive(Default, Clone)]
struct AppModule;

#[tokio::main]
async fn main() {
    // A `.env` is a development convenience; in production the environment is
    // expected to be populated already, so a missing file is not an error.
    let _ = dotenvy::dotenv();
    armature_log::init();

    if let Err(e) = run().await {
        eprintln!("fatal: {e}");
        std::process::exit(1);
    }
}

async fn run() -> AppResult<()> {
    let config = Config::from_env()?;
    let port = config.port;

    let db = db::connect(&config.database_url, config.db_max_connections).await?;

    let repo = RepoManager::new(
        config.repo_path.clone(),
        config.staging_path.clone(),
        config.gpg_key_id.clone(),
        config.repo_title.clone(),
        config.repo_description.clone(),
    );
    // Done before the listener opens: a server that accepts publishes into a
    // repository that was never initialized would fail every upload at the point
    // where the failure is most expensive to discover.
    repo.ensure_initialized().await?;

    // Connected before the listener opens for the same reason: a publish that
    // imports into the repository but cannot archive the bundle has already
    // half-succeeded by the time the failure is visible.
    let bundles = BundleStore::connect(&config.spaces).await?;

    let tokens = TokenService::new(&config.jwt_secret, config.session_ttl_secs)?;
    let otp = OtpService::new(config.totp_issuer.clone(), config.email_otp_ttl_secs);
    let mailer = Mailer::new(&config.smtp).await?;
    let oauth = OAuthRegistry::from_config(&config)?;

    let enabled_providers: Vec<&str> = oauth
        .descriptors(&config)
        .into_iter()
        .map(|d| d.id)
        .collect();

    armature_log::info!("repository:  {}", config.repo_url());
    armature_log::info!(
        "bundles:     s3://{}{} via {}",
        config.spaces.bucket,
        config
            .spaces
            .prefix
            .as_deref()
            .map(|p| format!("/{p}"))
            .unwrap_or_default(),
        config.spaces.endpoint
    );
    armature_log::info!("graphql:     {}/graphql", config.public_url);
    armature_log::info!("playground:  {}/playground", config.public_url);
    armature_log::info!(
        "oauth:       {}",
        if enabled_providers.is_empty() {
            "none configured (email/password sign-up only)".to_string()
        } else {
            enabled_providers.join(", ")
        }
    );
    if config.gpg_key_id.is_none() {
        armature_log::warn!(
            "no FLATNEXUS_GPG_KEY_ID set: commits are unsigned and clients cannot \
             verify what they install"
        );
    }

    state::init(AppState {
        config,
        db,
        repo,
        bundles,
        tokens,
        otp,
        mailer,
        oauth,
    })?;

    let app = Application::create::<AppModule>().await;
    app.listen(port)
        .await
        .map_err(|e| error::AppError::Internal(format!("server error: {e}")))
}
