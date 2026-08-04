//! Request-scoped batch loaders.
//!
//! `Application::builds` and `Application::publisher` are field resolvers on a
//! type returned in lists, so each ran its own query per row: a hundred
//! applications cost a hundred build queries plus a hundred publisher queries,
//! on top of the one that fetched the applications. `myApplications` had no page
//! limit at all, so the fan-out was bounded only by how much a publisher owned.
//!
//! A [`DataLoader`] collapses each set into one `IN (...)` query. It is
//! constructed per request rather than shared: its cache is keyed by id with no
//! invalidation, so a process-wide loader would serve a build list that a publish
//! had already superseded.

use std::collections::HashMap;
use std::sync::Arc;

use armature_graphql::async_graphql::dataloader::{DataLoader, Loader};

use crate::error::AppError;
use crate::models::{self, PraxClientExt, application, build, publisher, screenshot};
use crate::state::state;

/// Errors are shared rather than cloned: `DataLoader` hands the same result to
/// every caller that batched into one query, so the error type must be `Clone`.
pub type LoadError = Arc<AppError>;

/// Builds, keyed by the application they belong to.
pub struct BuildsByApplication;

impl Loader<String> for BuildsByApplication {
    type Value = Vec<models::Build>;
    type Error = LoadError;

    async fn load(&self, keys: &[String]) -> Result<HashMap<String, Self::Value>, Self::Error> {
        let rows = state()
            .db
            .build()
            .find_many()
            .r#where(build::application_id::in_(keys.to_vec()))
            .exec()
            .await
            .map_err(|e| Arc::new(AppError::from(e)))?;

        let mut grouped: HashMap<String, Vec<models::Build>> = HashMap::new();
        for row in rows {
            grouped
                .entry(row.application_id.clone())
                .or_default()
                .push(row);
        }

        // Newest first, matching what the single-application query promised.
        // Sorting here rather than in SQL keeps one round trip: an ORDER BY
        // across the whole batch would still need regrouping.
        for builds in grouped.values_mut() {
            builds.sort_by_key(|build| std::cmp::Reverse(build.published_at));
        }

        Ok(grouped)
    }
}

/// Gallery media, keyed by the application it belongs to.
pub struct ScreenshotsByApplication;

impl Loader<String> for ScreenshotsByApplication {
    type Value = Vec<models::Screenshot>;
    type Error = LoadError;

    async fn load(&self, keys: &[String]) -> Result<HashMap<String, Self::Value>, Self::Error> {
        let rows = state()
            .db
            .screenshot()
            .find_many()
            .r#where(screenshot::application_id::in_(keys.to_vec()))
            .exec()
            .await
            .map_err(|e| Arc::new(AppError::from(e)))?;

        let mut grouped: HashMap<String, Vec<models::Screenshot>> = HashMap::new();
        for row in rows {
            grouped
                .entry(row.application_id.clone())
                .or_default()
                .push(row);
        }

        // Gallery order is the publisher's, not the database's.
        for items in grouped.values_mut() {
            items.sort_by_key(|item| item.position);
        }

        Ok(grouped)
    }
}

/// Publishers, keyed by id.
pub struct PublishersById;

impl Loader<String> for PublishersById {
    type Value = models::Publisher;
    type Error = LoadError;

    async fn load(&self, keys: &[String]) -> Result<HashMap<String, Self::Value>, Self::Error> {
        let rows = state()
            .db
            .publisher()
            .find_many()
            .r#where(publisher::id::in_(keys.to_vec()))
            .exec()
            .await
            .map_err(|e| Arc::new(AppError::from(e)))?;

        Ok(rows.into_iter().map(|row| (row.id.clone(), row)).collect())
    }
}

/// Applications, keyed by id.
pub struct ApplicationsById;

impl Loader<String> for ApplicationsById {
    type Value = models::Application;
    type Error = LoadError;

    async fn load(&self, keys: &[String]) -> Result<HashMap<String, Self::Value>, Self::Error> {
        let rows = state()
            .db
            .application()
            .find_many()
            .r#where(application::id::in_(keys.to_vec()))
            .exec()
            .await
            .map_err(|e| Arc::new(AppError::from(e)))?;

        Ok(rows.into_iter().map(|row| (row.id.clone(), row)).collect())
    }
}

/// Every loader a request needs, built fresh for that request.
pub struct Loaders {
    pub builds: DataLoader<BuildsByApplication>,
    pub publishers: DataLoader<PublishersById>,
    pub applications: DataLoader<ApplicationsById>,
    pub screenshots: DataLoader<ScreenshotsByApplication>,
}

impl Loaders {
    pub fn new() -> Self {
        Self {
            builds: DataLoader::new(BuildsByApplication, tokio::spawn),
            publishers: DataLoader::new(PublishersById, tokio::spawn),
            applications: DataLoader::new(ApplicationsById, tokio::spawn),
            screenshots: DataLoader::new(ScreenshotsByApplication, tokio::spawn),
        }
    }
}

impl Default for Loaders {
    fn default() -> Self {
        Self::new()
    }
}
