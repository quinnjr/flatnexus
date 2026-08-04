//! Database connection setup and query helpers.

use std::time::Duration;

use prax_orm::PraxClient;
use prax_postgres::{PgEngine, PgPool, PgPoolBuilder};

use crate::error::{AppError, AppResult};
use crate::state::Db;

/// Ordering helper re-exported so call sites can write
/// `OrderByField::desc(build::published_at::COLUMN)` without reaching into
/// `prax_query` directly. The `#[derive(Model)]` field modules expose a
/// `COLUMN` constant but no `asc()`/`desc()` constructors of their own.
pub use prax_query::types::OrderByField;

/// Build the connection pool and wrap it in a Prax client.
///
/// `max_connections` interacts directly with how many concurrent GraphQL
/// resolvers can make progress, so it is configurable rather than baked in like
/// every other operational knob in this process.
pub async fn connect(url: &str, max_connections: usize) -> AppResult<Db> {
    let pool: PgPool = PgPoolBuilder::new()
        .url(url.to_string())
        .max_connections(max_connections)
        .connection_timeout(Duration::from_secs(10))
        .build()
        .await
        .map_err(|e| AppError::Database(format!("could not connect to Postgres: {e}")))?;

    Ok(PraxClient::new(PgEngine::new(pool)))
}
