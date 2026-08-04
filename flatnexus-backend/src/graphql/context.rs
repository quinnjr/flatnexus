//! Per-request GraphQL context.
//!
//! The HTTP controller verifies the `Authorization` header once and attaches the
//! result to the request as [`RequestIdentity`]. Resolvers read it from the
//! `async-graphql` context rather than re-parsing headers, so authentication is
//! decided in exactly one place.

use armature_graphql::Context;
use armature_graphql::Result as GqlResult;

use crate::auth::Viewer;
use crate::error::AppError;

/// Whether the caller presented a valid session token.
///
/// A *missing* token and an *invalid* token are both `Anonymous` by the time
/// resolvers see it: the distinction only matters at the HTTP boundary, and
/// collapsing it here keeps every resolver's check to one shape.
#[derive(Debug, Clone, Default)]
pub enum RequestIdentity {
    #[default]
    Anonymous,
    Authenticated(Viewer),
}

impl RequestIdentity {
    pub fn viewer(&self) -> Option<&Viewer> {
        match self {
            Self::Anonymous => None,
            Self::Authenticated(v) => Some(v),
        }
    }
}

/// The signed-in viewer, or `None` for anonymous callers.
pub fn optional_viewer<'a>(ctx: &'a Context<'_>) -> Option<&'a Viewer> {
    ctx.data_opt::<RequestIdentity>().and_then(|id| id.viewer())
}

/// The signed-in viewer, or an `UNAUTHENTICATED` error.
pub fn require_viewer<'a>(ctx: &'a Context<'_>) -> GqlResult<&'a Viewer> {
    optional_viewer(ctx).ok_or_else(|| AppError::Unauthenticated.into())
}
