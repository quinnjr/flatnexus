//! The `/graphql` endpoint.
//!
//! Handles both transports the API is specified for:
//!
//! * `application/json` — ordinary queries and mutations.
//! * `multipart/form-data` — the [GraphQL multipart request spec], which is how
//!   the `bundle: Upload!` argument on `publishBuild` is delivered. The same
//!   request shape works from a browser `FormData` and from `curl -F`.
//!
//! [GraphQL multipart request spec]: https://github.com/jaydenseric/graphql-multipart-request-spec

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use crate::prelude::*;
use armature_core::form::FormField;
use armature_graphql::async_graphql::{Request as GqlRequest, UploadValue, Variables};
use tempfile::NamedTempFile;

use crate::auth::{Viewer, bearer_token};
use crate::graphql::context::RequestIdentity;
use crate::graphql::loaders::Loaders;
use crate::graphql::{AppSchema, build_schema};
use crate::http::cors;
use crate::state::state;

/// The schema is immutable and cheap to clone, so it is built once.
static SCHEMA: OnceLock<AppSchema> = OnceLock::new();

fn schema() -> &'static AppSchema {
    SCHEMA.get_or_init(build_schema)
}

#[controller("/graphql")]
#[derive(Default, Clone)]
pub struct GraphQLController;

#[routes]
impl GraphQLController {
    /// Execute a GraphQL operation.
    #[post("")]
    async fn execute(req: HttpRequest) -> Result<HttpResponse, Error> {
        let identity = identify(&req);

        // Transport-level rejections are returned as CORS-decorated responses
        // rather than propagated with `?`. The framework's error path adds no
        // CORS headers, so a malformed-multipart or oversized-upload rejection
        // reached the browser as an opaque network failure instead of the
        // message it carries — on the one surface a browser actually posts to.
        let (gql_request, uploads) = match parse_request(&req, &identity) {
            Ok(parsed) => parsed,
            Err(e) => return Ok(cors::apply(error_response(&e))),
        };

        // Loaders are per request: their cache has no invalidation, so a shared
        // one would serve a build list that a publish had already superseded.
        // The registry owns the spooled files and outlives the execution, so a
        // resolver can reach them by the same index the `Upload` scalar carries.
        let response = schema()
            .execute(
                gql_request
                    .data(identity)
                    .data(uploads)
                    .data(Loaders::new()),
            )
            .await;
        let body = serde_json::to_vec(&response)
            .map_err(|e| Error::Serialization(e.to_string()))?;

        // A GraphQL response is 200 even when it carries errors; the transport
        // succeeded. Clients branch on `errors[].extensions.code`.
        Ok(cors::apply(
            HttpResponse::ok()
                .with_header("Content-Type".into(), "application/json".into())
                .with_body(body),
        ))
    }

    /// CORS preflight for the browser client.
    #[options("")]
    async fn preflight() -> Result<HttpResponse, Error> {
        Ok(cors::apply(HttpResponse::no_content()))
    }

    /// The schema as SDL. Useful for client codegen and for `curl` users who
    /// want to see what the server accepts.
    #[get("/schema")]
    async fn sdl() -> Result<HttpResponse, Error> {
        Ok(cors::apply(
            HttpResponse::ok()
                .with_header("Content-Type".into(), "text/plain; charset=utf-8".into())
                .with_body(crate::graphql::sdl().into_bytes()),
        ))
    }
}

/// The interactive GraphiQL IDE.
#[controller("/playground")]
#[derive(Default, Clone)]
pub struct PlaygroundController;

#[routes]
impl PlaygroundController {
    #[get("")]
    async fn playground() -> Result<HttpResponse, Error> {
        Ok(HttpResponse::html(armature_graphql::graphiql_html("/graphql")))
    }
}

/// Render a transport-level failure as a JSON body shaped like a GraphQL error.
///
/// Clients branch on `errors[].extensions.code`, and a rejection that never
/// reached the schema should not force them onto a second code path.
fn error_response(error: &Error) -> HttpResponse {
    let (status, code) = match error {
        Error::Unauthorized(_) => (401, "UNAUTHENTICATED"),
        Error::PayloadTooLarge(_) => (413, "UPLOAD_REJECTED"),
        Error::BadRequest(_) | Error::Deserialization(_) => (400, "VALIDATION_ERROR"),
        _ => (500, "INTERNAL_ERROR"),
    };

    let body = serde_json::json!({
        "errors": [{ "message": error.to_string(), "extensions": { "code": code } }]
    });

    HttpResponse::new(status)
        .with_header("Content-Type".into(), "application/json".into())
        .with_body(body.to_string().into_bytes())
}

// ---------------------------------------------------------------------------
// Request parsing
// ---------------------------------------------------------------------------

/// Resolve the caller's identity from the `Authorization` header.
///
/// A malformed or expired token is treated as anonymous rather than as an error:
/// the schema has public queries, and rejecting the whole request would make a
/// stale token in a browser tab break the public pages too. Resolvers that need
/// a viewer fail with `UNAUTHENTICATED` on their own.
fn identify(req: &HttpRequest) -> RequestIdentity {
    let Some(header) = req.headers.get("Authorization") else {
        return RequestIdentity::Anonymous;
    };
    let Some(token) = bearer_token(header) else {
        return RequestIdentity::Anonymous;
    };

    match state().tokens.verify(token) {
        Ok(claims) => RequestIdentity::Authenticated(Viewer {
            publisher_id: claims.sub,
        }),
        Err(_) => RequestIdentity::Anonymous,
    }
}

fn parse_request(
    req: &HttpRequest,
    identity: &RequestIdentity,
) -> Result<(GqlRequest, UploadRegistry), Error> {
    let content_type = req
        .headers
        .get("Content-Type")
        .unwrap_or("application/json");

    if content_type.starts_with("multipart/form-data") {
        parse_multipart_request(req, identity)
    } else {
        Ok((parse_json_request(req.body_ref())?, UploadRegistry::default()))
    }
}

/// The temporary files a multipart request spooled, in the order they were
/// registered as uploads.
///
/// `async_graphql::Upload` is an index into the request's upload list, and
/// `set_upload` assigns that index by push order — so pushing here in the same
/// loop iteration keeps the two in step, and a resolver can turn its `Upload`
/// straight into a path.
#[derive(Default)]
pub struct UploadRegistry {
    files: Vec<NamedTempFile>,
}

impl UploadRegistry {
    /// Path of the file behind an `Upload` index.
    pub fn path(&self, index: usize) -> Option<&Path> {
        self.files.get(index).map(|file| file.path())
    }
}

#[derive(serde::Deserialize, Clone)]
struct WireRequest {
    query: String,
    #[serde(default)]
    variables: Option<serde_json::Value>,
    #[serde(default, rename = "operationName")]
    operation_name: Option<String>,
}

fn parse_json_request(body: &[u8]) -> Result<GqlRequest, Error> {
    let wire: WireRequest = serde_json::from_slice(body)
        .map_err(|e| Error::Deserialization(format!("invalid GraphQL request: {e}")))?;
    Ok(build_request(wire))
}

fn build_request(wire: WireRequest) -> GqlRequest {
    let mut request = GqlRequest::new(wire.query);
    if let Some(vars) = wire.variables {
        request = request.variables(Variables::from_json(vars));
    }
    if let Some(name) = wire.operation_name {
        request = request.operation_name(name);
    }
    request
}

/// Assemble a request from a `multipart/form-data` body per the multipart spec.
///
/// The spec's three parts:
///
/// * `operations` — the JSON request, with `null` where each file belongs.
/// * `map` — `{"0": ["variables.input.bundle"], ...}`, wiring file parts to
///   those nulls.
/// * `0`, `1`, … — the file parts themselves.
fn parse_multipart_request(
    req: &HttpRequest,
    identity: &RequestIdentity,
) -> Result<(GqlRequest, UploadRegistry), Error> {
    // Nothing reaches disk for an anonymous caller. Every mutation that accepts
    // a file requires a session, so materializing parts before checking for one
    // let an unauthenticated request cost a full-size write per attempt.
    if identity.viewer().is_none() {
        return Err(Error::Unauthorized(
            "A session is required to upload a file.".into(),
        ));
    }

    let fields = req.multipart()?;

    let operations = text_field(&fields, "operations").ok_or_else(|| {
        Error::BadRequest(
            "multipart upload is missing the 'operations' field \
             (see the GraphQL multipart request spec)"
                .into(),
        )
    })?;

    let map_raw = text_field(&fields, "map").ok_or_else(|| {
        Error::BadRequest(
            "multipart upload is missing the 'map' field \
             (see the GraphQL multipart request spec)"
                .into(),
        )
    })?;

    let wire: WireRequest = serde_json::from_str(operations)
        .map_err(|e| Error::Deserialization(format!("invalid 'operations' payload: {e}")))?;

    let map: HashMap<String, Vec<String>> = serde_json::from_str(map_raw)
        .map_err(|e| Error::Deserialization(format!("invalid 'map' payload: {e}")))?;

    let variables = wire.variables.clone().unwrap_or(serde_json::Value::Null);

    // Bound the write before making it. This limit previously lived only in the
    // resolver, which meant the disk cost of an oversized upload was paid in
    // full before anything rejected it.
    let max_bytes = state().config.max_bundle_bytes;

    let mut request = build_request(wire);
    let mut registry = UploadRegistry::default();

    for (part_name, variable_paths) in map {
        let file = file_field(&fields, &part_name).ok_or_else(|| {
            Error::BadRequest(format!(
                "'map' references file part '{part_name}', which was not uploaded"
            ))
        })?;

        if file.data.len() as u64 > max_bytes {
            return Err(Error::PayloadTooLarge(format!(
                "'{part_name}' exceeds the {} MiB upload limit for this server.",
                max_bytes / (1024 * 1024)
            )));
        }

        for path in variable_paths {
            // `set_upload` silently does nothing when the path names no variable,
            // so a typo in `map` surfaced later as an unrelated "invalid value for
            // argument" error that never mentioned the map. The spec requires the
            // request be rejected; say which path was wrong while we know it.
            if !upload_path_exists(&variables, &path) {
                return Err(Error::BadRequest(format!(
                    "'map' names the variable path '{path}', which is not present \
                     in the operation's variables (it must be there, set to null)"
                )));
            }

            // The spec allows one part to fill several variables, so each gets
            // its own spooled copy and its own index.
            let (upload, spooled) = spool_part(file)?;
            request.set_upload(&path, upload);
            registry.files.push(spooled);
        }
    }

    Ok((request, registry))
}

/// Write a parsed file part to a named temporary file, once.
///
/// Returns the `UploadValue` async-graphql needs plus the named handle the
/// registry keeps alive. Named rather than anonymous because `flatpak` is given
/// a path, and hashing reads the same file rather than copying it — this used to
/// be two separate spools of the same bytes.
fn spool_part(
    file: &armature_core::form::FormFile,
) -> Result<(UploadValue, NamedTempFile), Error> {
    let mut temp = NamedTempFile::new()
        .map_err(|e| Error::Internal(format!("could not open upload scratch file: {e}")))?;
    temp.write_all(&file.data)
        .map_err(|e| Error::Internal(format!("could not write upload scratch file: {e}")))?;
    temp.flush()
        .map_err(|e| Error::Internal(format!("could not flush upload scratch file: {e}")))?;

    // A second descriptor on the same file, not a second copy of the bytes.
    let handle = std::fs::File::open(temp.path())
        .map_err(|e| Error::Internal(format!("could not reopen upload scratch file: {e}")))?;

    Ok((
        UploadValue {
            filename: file.filename.clone(),
            content_type: Some(file.content_type.clone()),
            content: handle,
        },
        temp,
    ))
}

/// Whether a `map` path such as `variables.input.bundle` names a real slot in
/// the operation's variables.
///
/// Mirrors how async-graphql walks the path: strip the `variables.` prefix, then
/// descend objects by key and lists by index.
fn upload_path_exists(variables: &serde_json::Value, path: &str) -> bool {
    let Some(rest) = path.strip_prefix("variables.") else {
        return false;
    };

    let mut current = variables;
    for segment in rest.split('.') {
        current = match current {
            serde_json::Value::Object(map) => match map.get(segment) {
                Some(value) => value,
                None => return false,
            },
            serde_json::Value::Array(items) => match segment.parse::<usize>() {
                Ok(index) => match items.get(index) {
                    Some(value) => value,
                    None => return false,
                },
                Err(_) => return false,
            },
            _ => return false,
        };
    }
    true
}

fn text_field<'a>(fields: &'a [FormField], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|f| f.name == name)
        .and_then(|f| f.value.as_deref())
}

fn file_field<'a>(
    fields: &'a [FormField],
    name: &str,
) -> Option<&'a armature_core::form::FormFile> {
    fields
        .iter()
        .find(|f| f.name == name)
        .and_then(|f| f.file.as_ref())
}

#[cfg(test)]
mod tests {
    use super::upload_path_exists;
    use serde_json::json;

    #[test]
    fn accepts_a_path_that_names_a_real_variable_slot() {
        let variables = json!({ "input": { "appId": "org.example.App", "bundle": null } });
        assert!(upload_path_exists(&variables, "variables.input.bundle"));
    }

    #[test]
    fn accepts_an_indexed_path_into_a_list() {
        let variables = json!({ "input": { "files": [null, null] } });
        assert!(upload_path_exists(&variables, "variables.input.files.1"));
        assert!(!upload_path_exists(&variables, "variables.input.files.2"));
    }

    #[test]
    fn rejects_paths_that_name_nothing() {
        let variables = json!({ "input": { "bundle": null } });
        // The failure this catches: a typo that `set_upload` would ignore,
        // surfacing much later as an unrelated argument error.
        assert!(!upload_path_exists(&variables, "variables.input.bundel"));
        assert!(!upload_path_exists(&variables, "variables.missing.bundle"));
        assert!(!upload_path_exists(&variables, "variables.input.bundle.deeper"));
        // The `variables.` prefix is required by the spec.
        assert!(!upload_path_exists(&variables, "input.bundle"));
    }
}
