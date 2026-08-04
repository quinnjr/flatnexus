//! Process-wide application state.
//!
//! Armature route handlers are associated functions without a `self` receiver,
//! so long-lived collaborators (connection pool, repository manager, mailer)
//! live in a process-global initialized once during boot. [`init`] is called
//! from `main` before the server starts listening; [`state`] is the accessor
//! every handler and resolver uses.

use std::sync::{Arc, OnceLock};

use prax_orm::PraxClient;
use prax_postgres::PgEngine;

use crate::auth::oauth::OAuthRegistry;
use crate::auth::otp::OtpService;
use crate::auth::token::TokenService;
use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::mail::Mailer;
use crate::ostree::RepoManager;
use crate::storage::BundleStore;

/// The concrete Prax client this application uses.
pub type Db = PraxClient<PgEngine>;

pub struct AppState {
    pub config: Config,
    pub db: Db,
    pub repo: RepoManager,
    /// Object storage archiving every uploaded bundle.
    pub bundles: BundleStore,
    pub tokens: TokenService,
    pub otp: OtpService,
    pub mailer: Mailer,
    pub oauth: OAuthRegistry,
}

static STATE: OnceLock<Arc<AppState>> = OnceLock::new();

/// Install the application state. Calling this twice is a programming error.
pub fn init(state: AppState) -> AppResult<()> {
    STATE
        .set(Arc::new(state))
        .map_err(|_| AppError::Internal("application state already initialized".into()))
}

/// Borrow the application state.
///
/// # Panics
///
/// Panics if called before [`init`]. Every caller runs inside a request, which
/// can only happen after `main` has initialized state, so this cannot trip at
/// runtime without a boot-order bug.
pub fn state() -> Arc<AppState> {
    STATE
        .get()
        .expect("application state accessed before init()")
        .clone()
}
