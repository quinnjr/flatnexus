//! Liveness and readiness.

use crate::prelude::*;

use crate::state::state;

#[controller("/health")]
#[derive(Default, Clone)]
pub struct HealthController;

#[routes]
impl HealthController {
    /// Liveness: the process is up and serving.
    #[get("")]
    async fn live() -> Result<HttpResponse, Error> {
        HttpResponse::json(&serde_json::json!({ "status": "ok" }))
    }

    /// Readiness: the dependencies a request actually needs are reachable.
    ///
    /// Every dependency is reported individually, so a degraded instance says
    /// *what* is degraded. Only the two a read request cannot proceed without —
    /// the database and the repository — gate the status code: an instance whose
    /// bucket is unreachable can still serve every query and every install, and
    /// pulling it from rotation would turn a publish-only outage into a total
    /// one. `bundles` is reported so the condition is visible without being
    /// fatal.
    ///
    /// Reports 503 rather than 500 when something is down, so a load balancer
    /// takes the instance out of rotation instead of treating it as a bug.
    #[get("/ready")]
    async fn ready() -> Result<HttpResponse, Error> {
        let state = state();

        let database = state
            .db
            .execute_raw(prax_query::raw::Sql::new("SELECT 1"))
            .await
            .is_ok();

        let repository = tokio::fs::metadata(state.repo.repo_path())
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false);

        let bundles = state.bundles.is_reachable().await;

        let ready = database && repository;
        let body = serde_json::json!({
            "status": if ready { "ready" } else { "degraded" },
            "database": database,
            "repository": repository,
            "bundles": bundles,
        });

        let response = HttpResponse::new(if ready { 200 } else { 503 })
            .with_json(&body)?;
        Ok(response)
    }
}
