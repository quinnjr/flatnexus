//! The GraphQL schema.
//!
//! This is the entire application API — there is no REST resource surface. The
//! non-GraphQL HTTP routes exist only because their clients cannot speak
//! GraphQL, and they are, in full:
//!
//! * `/repo/**` — `libostree` fetching repository objects.
//! * `/flatnexus.flatpakrepo`, `/refs/*.flatpakref` — remote discovery files.
//! * `/auth/oauth/**` — browser redirects to GitHub and Google.
//! * `/health`, `/health/ready` — orchestrator probes.
//! * `/playground` — GraphiQL.
//! * `GET /graphql/schema` — the SDL, for codegen.

pub mod context;
pub mod loaders;
pub mod mutation;
pub mod query;
pub mod types;

use armature_graphql::{EmptySubscription, Schema};

use mutation::MutationRoot;
use query::QueryRoot;

pub type AppSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

/// Query depth ceiling. Deeply nested queries are how a public GraphQL endpoint
/// gets turned into an amplification attack; this schema's legitimate queries
/// nest three or four levels.
const MAX_DEPTH: usize = 12;
const MAX_COMPLEXITY: usize = 500;

/// Build the schema. Called once at boot; the result is cheap to clone.
pub fn build_schema() -> AppSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .limit_depth(MAX_DEPTH)
        .limit_complexity(MAX_COMPLEXITY)
        .finish()
}

/// Render the schema as SDL, for `GET /graphql/schema` and codegen.
pub fn sdl() -> String {
    build_schema().sdl()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_builds_and_exposes_the_documented_operations() {
        let sdl = sdl();

        for expected in [
            "publishBuild",
            "verifyLogin",
            "requestUploadOtp",
            "enrollTotp",
            "confirmTotp",
            "oauthProviders",
            "repoInfo",
        ] {
            assert!(sdl.contains(expected), "SDL should expose {expected}");
        }
    }

    #[test]
    fn login_cannot_return_a_session_without_a_status() {
        // `login` must not hand back a bare AuthPayload: if it regresses to that
        // shape, the second factor is skipped and nothing else in the schema
        // would show it.
        let sdl = sdl();
        assert!(sdl.contains("login(input: LoginInput!): LoginResult!"));
        assert!(sdl.contains("type LoginResult"));
        assert!(sdl.contains("status: LoginStatus!"));
    }

    #[test]
    fn bundle_uploads_use_the_upload_scalar() {
        // If this regresses to a String the multipart path silently stops
        // working while the schema still looks plausible.
        assert!(sdl().contains("bundle: Upload!"));
    }
}
