//! CORS for the browser client.
//!
//! Exactly one origin is allowed — the configured frontend. A wildcard would be
//! wrong here: the API accepts a bearer token, and `Access-Control-Allow-Origin:
//! *` cannot be combined with credentialed requests anyway.

use crate::prelude::HttpResponse;

use crate::state::state;

/// Attach CORS headers to a response.
pub fn apply(response: HttpResponse) -> HttpResponse {
    let origin = state().config.cors_origin.clone();

    response
        .with_header("Access-Control-Allow-Origin".into(), origin)
        .with_header(
            "Access-Control-Allow-Methods".into(),
            "GET, POST, OPTIONS".into(),
        )
        .with_header(
            "Access-Control-Allow-Headers".into(),
            "Authorization, Content-Type".into(),
        )
        .with_header("Access-Control-Max-Age".into(), "86400".into())
        // The allowed origin varies with configuration, not with the request,
        // but caches keyed only on the URL would still serve a stale value to a
        // differently-originated client.
        .with_header("Vary".into(), "Origin".into())
}
