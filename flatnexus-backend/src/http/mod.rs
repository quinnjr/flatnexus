//! HTTP surfaces.
//!
//! Deliberately thin. `/graphql` is the API; everything else here exists because
//! its client cannot speak GraphQL:
//!
//! * `/repo/**` — `libostree` fetching repository objects.
//! * `/flatnexus.flatpakrepo`, `/refs/*.flatpakref` — remote discovery files.
//! * `/auth/oauth/**` — browser redirects to GitHub and Google.
//! * `/health` — orchestrator probes.

pub mod cors;
pub mod graphql_controller;
pub mod health_controller;
pub mod oauth_controller;
pub mod repo_controller;
