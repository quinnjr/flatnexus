//! Read side of the API.

use armature_graphql::{Context, Object, Result as GqlResult};

use crate::db::OrderByField;
use crate::error::{AppError, IntoGqlResult};
use crate::graphql::context::optional_viewer;
use crate::graphql::types::{
    Application, Build, OAuthProviderInfo, RepoInfo, Viewer, viewer_publisher,
};
use crate::models::{application, build};
use crate::models::PraxClientExt;
use crate::state::state;

/// Page size ceiling. A caller asking for more gets this instead of an error —
/// pagination limits are a server concern, not a client failure.
const MAX_PAGE_SIZE: i64 = 100;
const DEFAULT_PAGE_SIZE: i64 = 25;

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    /// Browse published applications, newest first.
    ///
    /// `search` matches the application id, name, and summary.
    async fn applications(
        &self,
        search: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> GqlResult<Vec<Application>> {
        let state = state();
        let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
        let offset = offset.unwrap_or(0).max(0);

        let needle = search
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_lowercase);

        let query = state
            .db
            .application()
            .find_many()
            .order_by(OrderByField::desc(application::created_at::COLUMN));

        // Prax `WhereParam`s compose with AND, so an OR across id/name/summary
        // is not expressible in the query. Matching on one column alone would
        // silently miss the other two, so a search is filtered in memory — and
        // therefore paged in memory too, otherwise `offset` would count rows
        // that were about to be discarded.
        let Some(needle) = needle else {
            let rows = query
                .take(limit as u64)
                .skip(offset as u64)
                .exec()
                .await
                .gql()?;
            return Ok(rows.iter().map(Application::from).collect());
        };

        let rows = query.exec().await.gql()?;
        Ok(rows
            .iter()
            .filter(|a| {
                // One allocation per row rather than up to three: the previous
                // form lowercased each column separately, and `||` only avoided
                // the later ones on a hit.
                let haystack =
                    format!("{} {} {}", a.app_id, a.name, a.summary).to_lowercase();
                haystack.contains(&needle)
            })
            .skip(offset as usize)
            .take(limit as usize)
            .map(Application::from)
            .collect())
    }

    /// One application by its reverse-DNS id.
    async fn application(&self, app_id: String) -> GqlResult<Option<Application>> {
        let state = state();
        let found = state
            .db
            .application()
            .find_first()
            .r#where(application::app_id::equals(app_id))
            .exec()
            .await
            .gql()?;

        Ok(found.as_ref().map(Application::from))
    }

    /// Builds for an application, newest first.
    async fn builds(&self, app_id: String) -> GqlResult<Vec<Build>> {
        let state = state();
        let Some(app) = state
            .db
            .application()
            .find_first()
            .r#where(application::app_id::equals(app_id.clone()))
            .exec()
            .await
            .gql()?
        else {
            return Err(AppError::NotFound(format!("No application '{app_id}'.")).into());
        };

        let rows = state
            .db
            .build()
            .find_many()
            .r#where(build::application_id::equals(app.id))
            .order_by(OrderByField::desc(build::published_at::COLUMN))
            .exec()
            .await
            .gql()?;

        Ok(rows.iter().map(Build::from).collect())
    }

    /// The signed-in publisher, or `null` when the request is anonymous.
    async fn me(&self, ctx: &Context<'_>) -> GqlResult<Option<Viewer>> {
        if optional_viewer(ctx).is_none() {
            return Ok(None);
        }
        let publisher = viewer_publisher(ctx).await?;
        Ok(Some(Viewer::from(&publisher)))
    }

    /// Applications owned by the signed-in publisher, newest first.
    ///
    /// Paged like the public query. It was previously unbounded, so a publisher
    /// with many applications fanned out one batch of field-resolver loads per
    /// row with nothing capping the row count.
    async fn my_applications(
        &self,
        ctx: &Context<'_>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> GqlResult<Vec<Application>> {
        let publisher = viewer_publisher(ctx).await?;
        let state = state();

        let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
        let offset = offset.unwrap_or(0).max(0);

        let rows = state
            .db
            .application()
            .find_many()
            .r#where(application::publisher_id::equals(publisher.id))
            .order_by(OrderByField::desc(application::created_at::COLUMN))
            .take(limit as u64)
            .skip(offset as u64)
            .exec()
            .await
            .gql()?;

        Ok(rows.iter().map(Application::from).collect())
    }

    /// How to add this server as a Flatpak remote.
    async fn repo_info(&self) -> RepoInfo {
        RepoInfo
    }

    /// Sign-in providers this deployment has configured.
    async fn oauth_providers(&self) -> Vec<OAuthProviderInfo> {
        let state = state();
        state
            .oauth
            .descriptors(&state.config)
            .into_iter()
            .map(|d| OAuthProviderInfo {
                id: d.id.to_string(),
                display_name: d.display_name.to_string(),
                start_url: d.start_url,
            })
            .collect()
    }
}
