//! Static serving of the OSTree repository, plus the discovery files that point
//! `flatpak` at it.
//!
//! This is the endpoint the Flatpak CLI actually talks to. It fetches
//! `summary`, `config`, and then individual objects under `objects/` — all
//! ordinary `GET`s of ordinary files. Nothing here is GraphQL, and nothing here
//! can be: the client is `libostree`, not a browser.
//!
//! Range support is not optional politeness. `flatpak` resumes interrupted
//! object fetches with `Range`, and a server that ignores the header hands back
//! a whole file with a `200`, which the client then treats as a corrupt resume.

use std::path::{Component, Path, PathBuf};

use crate::prelude::*;

use crate::state::state;
use crate::models::PraxClientExt;

#[controller("/repo")]
#[derive(Default, Clone)]
pub struct RepoController;

#[routes]
impl RepoController {
    /// Serve any file inside the repository.
    #[get("/*path")]
    async fn object(req: HttpRequest) -> Result<HttpResponse, Error> {
        let relative = req.param("path").unwrap_or_default().to_string();

        let state = state();
        let resolved = resolve(state.repo.repo_path(), &relative)?;

        serve_file(&resolved, req.headers.get("Range")).await
    }
}

/// The `.flatpakrepo` and `.flatpakref` discovery files.
#[controller("")]
#[derive(Default, Clone)]
pub struct RepoDiscoveryController;

#[routes]
impl RepoDiscoveryController {
    /// The one-step way to add this server as a remote:
    /// `flatpak remote-add --if-not-exists flatnexus <this url>`.
    #[get("/flatnexus.flatpakrepo")]
    async fn flatpakrepo() -> Result<HttpResponse, Error> {
        let state = state();
        let config = &state.config;

        let mut body = format!(
            "[Flatpak Repo]\n\
             Title={}\n\
             Comment={}\n\
             Url={}\n\
             Homepage={}\n",
            config.repo_title,
            config.repo_description,
            config.repo_url(),
            config.public_url,
        );

        // Without the key, clients cannot verify signatures and `flatpak` will
        // warn on every install, so it is only advertised when it exists.
        if let Some(key) = read_gpg_key().await {
            body.push_str(&format!("GPGKey={key}\n"));
        }

        Ok(HttpResponse::ok()
            .with_header(
                "Content-Type".into(),
                "application/vnd.flatpak.repo".into(),
            )
            .with_body(body.into_bytes()))
    }

    /// Per-application single-click install file.
    #[get("/refs/:file")]
    async fn flatpakref(req: HttpRequest) -> Result<HttpResponse, Error> {
        let file = req.param("file").unwrap_or_default().to_string();

        let app_id = file.strip_suffix(".flatpakref").ok_or_else(|| {
            Error::NotFound("expected a .flatpakref filename".into())
        })?;

        let state = state();
        let found = state
            .db
            .application()
            .find_first()
            .r#where(crate::models::application::app_id::equals(app_id.to_string()))
            .exec()
            .await
            .map_err(crate::error::AppError::from)?;

        let Some(app) = found else {
            return Err(Error::NotFound(format!("No application '{app_id}'.")));
        };

        // Branch is left to the client: `flatpak` resolves it from the remote's
        // summary, which is the authority on which branches actually exist.
        let mut body = format!(
            "[Flatpak Ref]\n\
             Title={}\n\
             Name={}\n\
             Url={}\n\
             RuntimeRepo={}/flatnexus.flatpakrepo\n\
             IsRuntime=false\n",
            app.name,
            app.app_id,
            state.config.repo_url(),
            state.config.public_url,
        );

        if let Some(key) = read_gpg_key().await {
            body.push_str(&format!("GPGKey={key}\n"));
        }

        Ok(HttpResponse::ok()
            .with_header("Content-Type".into(), "application/vnd.flatpak.ref".into())
            .with_body(body.into_bytes()))
    }
}

/// The armored public key, base64 with newlines stripped, as the `.flatpakrepo`
/// format wants it. Returns `None` when signing is not configured or the key
/// file cannot be read.
async fn read_gpg_key() -> Option<String> {
    let state = state();
    let path = state.config.gpg_public_key_path.as_ref()?;

    match tokio::fs::read_to_string(path).await {
        Ok(armored) => Some(
            armored
                .lines()
                .filter(|line| {
                    !line.starts_with("-----")
                        && !line.is_empty()
                        && !line.starts_with("Version:")
                        && !line.starts_with("Comment:")
                })
                .collect::<String>(),
        ),
        Err(e) => {
            armature_log::warn!("could not read GPG public key {}: {e}", path.display());
            None
        }
    }
}

/// Resolve a request path against the repository root, refusing anything that
/// escapes it.
///
/// The check is on the *lexical* components rather than the joined string: a
/// path like `objects/../../etc/passwd` normalizes away cleanly, and comparing
/// prefixes after a `join` would still let a symlinked directory inside the repo
/// point outward.
///
/// It does **not** canonicalize, so a symlink planted inside the repository and
/// pointing outward would be followed — by this approach and by a prefix
/// comparison alike. Accepted rather than overlooked: the tree is written only by
/// `ostree`. Add a `canonicalize` + `starts_with(root)` check if that changes.
fn resolve(root: &Path, relative: &str) -> Result<PathBuf, Error> {
    // No leading-slash trimming: an absolute path is refused outright by the
    // `RootDir` component check below. Trimming it first would quietly reinterpret
    // `/etc/passwd` as the repository-relative `etc/passwd`, turning a request
    // that should be rejected into one that is merely harmless.
    if relative.is_empty() {
        return Err(Error::NotFound("no object requested".into()));
    }

    let candidate = Path::new(relative);
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            // Anything else is either an escape attempt or meaningless here.
            _ => {
                return Err(Error::Forbidden(
                    "repository paths may not traverse directories".into(),
                ));
            }
        }
    }

    Ok(root.join(candidate))
}

/// What a `Range` header asked for.
enum RangeRequest {
    /// No range, or one this server does not serve piecewise. Send the whole file.
    Whole,
    /// A satisfiable range.
    Satisfiable { start: u64, end: u64 },
    /// A syntactically valid range lying entirely outside the file.
    ///
    /// Distinct from `Whole` on purpose. Collapsing the two answered an
    /// impossible range with `200` and the entire file — exactly the
    /// corrupt-resume failure that range support exists to prevent, reached from
    /// the other direction.
    Unsatisfiable,
}

/// Serve a file, honouring a single-range `Range` header.
async fn serve_file(path: &Path, range_header: Option<&str>) -> Result<HttpResponse, Error> {
    // Opened once. Taking metadata from a separate `stat` meant the size in
    // `Content-Range` could describe a file the body no longer matched, and a
    // file truncated in between produced a 500 from `read_exact`.
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(_) => return Err(Error::NotFound("no such repository object".into())),
    };

    let metadata = file.metadata().await?;
    if !metadata.is_file() {
        // A directory is a 404: the repository has no listings to expose.
        return Err(Error::NotFound("no such repository object".into()));
    }

    let total = metadata.len();
    let content_type = content_type_for(path);

    // Objects are content-addressed and never rewritten in place, so they are
    // immutable; `summary` and `config` change on every publish and must not be
    // cached.
    let cache_control = if path
        .components()
        .any(|c| matches!(c, Component::Normal(name) if name == "objects" || name == "deltas"))
    {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    match range_header.map_or(RangeRequest::Whole, |h| parse_range(h, total)) {
        RangeRequest::Unsatisfiable => Ok(HttpResponse::new(416)
            .with_header("Content-Type".into(), content_type.into())
            .with_header("Accept-Ranges".into(), "bytes".into())
            // RFC 9110 §14.4: a 416 states the current length so the client can
            // recompute a range that does exist.
            .with_header("Content-Range".into(), format!("bytes */{total}"))),

        RangeRequest::Whole => {
            let body = read_at(&mut file, 0, total).await?;
            Ok(HttpResponse::ok()
                .with_header("Content-Type".into(), content_type.into())
                .with_header("Cache-Control".into(), cache_control.into())
                .with_header("Accept-Ranges".into(), "bytes".into())
                .with_body(body))
        }

        RangeRequest::Satisfiable { start, end } => {
            let body = read_at(&mut file, start, end - start + 1).await?;
            // Describe what was actually read. A file rewritten between the open
            // and the read yields a short body, and a `Content-Range` promising
            // more would corrupt the client's resume.
            let sent_end = start + (body.len() as u64).saturating_sub(1);

            Ok(HttpResponse::new(206)
                .with_header("Content-Type".into(), content_type.into())
                .with_header("Cache-Control".into(), cache_control.into())
                .with_header("Accept-Ranges".into(), "bytes".into())
                .with_header(
                    "Content-Range".into(),
                    format!("bytes {start}-{sent_end}/{total}"),
                )
                .with_body(body))
        }
    }
}

/// Read up to `len` bytes from `offset` in an already-open file.
///
/// A short read is returned as-is rather than raising `UnexpectedEof`: the file
/// may have been rewritten since it was opened (`summary` and `config` are, on
/// every publish), which is a shorter representation, not a server fault.
async fn read_at(file: &mut tokio::fs::File, offset: u64, len: u64) -> Result<Vec<u8>, Error> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    file.seek(std::io::SeekFrom::Start(offset)).await?;

    let mut buffer = vec![0u8; len as usize];
    let mut filled = 0usize;
    while filled < buffer.len() {
        let read = file.read(&mut buffer[filled..]).await?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// Parse a single `bytes=` range.
///
/// Multi-range requests fall back to the whole file — a valid response, and one
/// `libostree` never asks to avoid. A range lying entirely past the end is
/// reported separately so it can be answered `416`.
fn parse_range(header: &str, total: u64) -> RangeRequest {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return RangeRequest::Whole;
    };
    if spec.contains(',') || total == 0 {
        return RangeRequest::Whole;
    }

    let Some((start_raw, end_raw)) = spec.split_once('-') else {
        return RangeRequest::Whole;
    };

    let (start, end) = match (start_raw.trim(), end_raw.trim()) {
        // `bytes=-500`: the final 500 bytes. `bytes=-0` asks for nothing, which
        // RFC 9110 makes unsatisfiable rather than a whole-file request.
        ("", suffix) => {
            let Ok(len) = suffix.parse::<u64>() else {
                return RangeRequest::Whole;
            };
            if len == 0 {
                return RangeRequest::Unsatisfiable;
            }
            (total.saturating_sub(len), total - 1)
        }
        // `bytes=500-`: from 500 to the end.
        (start, "") => match start.parse::<u64>() {
            Ok(start) => (start, total - 1),
            Err(_) => return RangeRequest::Whole,
        },
        (start, end) => match (start.parse::<u64>(), end.parse::<u64>()) {
            (Ok(start), Ok(end)) => (start, end.min(total - 1)),
            _ => return RangeRequest::Whole,
        },
    };

    if start >= total {
        return RangeRequest::Unsatisfiable;
    }
    if start > end {
        return RangeRequest::Whole;
    }
    RangeRequest::Satisfiable { start, end }
}

fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("commit") | Some("dirtree") | Some("dirmeta") | Some("filez") => {
            "application/octet-stream"
        }
        Some("sig") => "application/pgp-signature",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_ordinary_object_paths() {
        let root = Path::new("/srv/repo");
        let resolved = resolve(root, "objects/ab/cdef.filez").unwrap();
        assert_eq!(resolved, Path::new("/srv/repo/objects/ab/cdef.filez"));
    }

    #[test]
    fn refuses_traversal_and_absolute_paths() {
        let root = Path::new("/srv/repo");
        for bad in ["../etc/passwd", "objects/../../etc/passwd", "/etc/passwd", ""] {
            assert!(resolve(root, bad).is_err(), "{bad} should be refused");
        }
    }

    fn satisfiable(header: &str, total: u64) -> Option<(u64, u64)> {
        match parse_range(header, total) {
            RangeRequest::Satisfiable { start, end } => Some((start, end)),
            _ => None,
        }
    }

    #[test]
    fn parses_the_range_forms_ostree_uses() {
        assert_eq!(satisfiable("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(satisfiable("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(satisfiable("bytes=-100", 1000), Some((900, 999)));
        // Clamped to the file, not rejected.
        assert_eq!(satisfiable("bytes=0-99999", 1000), Some((0, 999)));
    }

    #[test]
    fn falls_back_to_the_whole_file_for_ranges_it_does_not_serve() {
        for header in ["bytes=0-10,20-30", "items=0-10", "bytes=50-10", "bytes=abc-def"] {
            assert!(
                matches!(parse_range(header, 1000), RangeRequest::Whole),
                "{header} should fall back to the whole file"
            );
        }
    }

    #[test]
    fn reports_ranges_past_the_end_as_unsatisfiable() {
        // These previously returned `None` and were answered with a 200 and the
        // entire file — the corrupt-resume case range support exists to avoid.
        for header in ["bytes=2000-3000", "bytes=1000-", "bytes=-0"] {
            assert!(
                matches!(parse_range(header, 1000), RangeRequest::Unsatisfiable),
                "{header} should be unsatisfiable"
            );
        }
    }
}
