//! The OSTree repository that `flatpak` talks to.
//!
//! FlatNexus does not reimplement OSTree. It drives the same `ostree` and
//! `flatpak` binaries a maintainer would run by hand, against an `archive`-mode
//! repository that is then served as plain static files over HTTP. That is
//! exactly what `flatpak remote-add` expects, so no client-side special-casing
//! is needed.
//!
//! # Import is staged, not direct
//!
//! Every mutation of the live repository is serialized behind one lock, so a
//! publish and a withdrawal cannot interleave and leave a summary describing
//! neither.
//!
//! An uploaded bundle is never imported straight into the live repository.
//! `flatpak build-import-bundle` is given a throwaway staging repo first, so the
//! ref and commit it produces can be read back and checked against what the
//! publisher *claimed* they were uploading before anything reaches the repo
//! clients pull from. A bundle for `org.evil.Thing` uploaded under an
//! `org.trusted.App` record is rejected while it is still in the staging
//! directory, and the live repo is never mutated.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;
use tokio::sync::Mutex;

use crate::error::{AppError, AppResult};

/// What a staged bundle turned out to contain.
#[derive(Debug, Clone)]
pub struct ImportedBundle {
    /// Full OSTree ref, e.g. `app/org.gnome.Calculator/x86_64/stable`.
    pub flatpak_ref: String,
    /// Reverse-DNS application id parsed out of the ref.
    pub app_id: String,
    pub arch: String,
    pub branch: String,
    /// Commit checksum the ref resolved to.
    pub commit: String,
}

pub struct RepoManager {
    repo_path: PathBuf,
    staging_path: PathBuf,
    gpg_key_id: Option<String>,
    repo_title: String,
    repo_description: String,
    /// Serializes every mutation of the live repository.
    ///
    /// `ostree` does its own locking per operation, but nothing coordinated two
    /// concurrent publishes at this level: a `pull-local` could interleave with
    /// another request's summary regeneration, or with a `refs --delete`, and the
    /// summary that resulted described neither request's view. Held across
    /// pull + summary so a publish is atomic from a client's perspective.
    write_lock: Mutex<()>,
}

impl RepoManager {
    pub fn new(
        repo_path: PathBuf,
        staging_path: PathBuf,
        gpg_key_id: Option<String>,
        repo_title: String,
        repo_description: String,
    ) -> Self {
        Self {
            repo_path,
            staging_path,
            gpg_key_id,
            repo_title,
            repo_description,
            write_lock: Mutex::new(()),
        }
    }

    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Create the repository if it does not exist and stamp its metadata.
    ///
    /// Safe to call on every boot: `ostree init` is a no-op on an initialized
    /// repo, and the metadata keys are idempotent writes.
    pub async fn ensure_initialized(&self) -> AppResult<()> {
        tokio::fs::create_dir_all(&self.repo_path).await?;
        tokio::fs::create_dir_all(&self.staging_path).await?;

        run(
            "ostree",
            &[
                &format!("--repo={}", self.repo_path.display()),
                "init",
                // `archive` stores each object as an individual file, which is
                // what makes the repo servable as static HTTP. The alternative
                // modes are for local checkouts.
                "--mode=archive",
            ],
        )
        .await?;

        // Advertised in the summary and shown by `flatpak remotes -d`.
        self.set_repo_metadata("xa.title", &self.repo_title).await?;
        self.set_repo_metadata("xa.comment", &self.repo_description)
            .await?;

        self.sweep_staging().await;
        self.update_summary().await?;
        Ok(())
    }

    /// Remove staging directories left by a previous process.
    ///
    /// The `Drop` guard covers this process; nothing covers a hard kill, and a
    /// leaked directory holds a full unpacked bundle. Best-effort and logged.
    async fn sweep_staging(&self) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.staging_path).await else {
            return;
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("import-") {
                continue;
            }
            if let Err(e) = tokio::fs::remove_dir_all(entry.path()).await {
                armature_log::warn!(
                    "could not sweep stale staging repo {}: {e}",
                    entry.path().display()
                );
            } else {
                armature_log::info!("swept stale staging repo {}", entry.path().display());
            }
        }
    }

    /// Import a `.flatpak` bundle, returning what it actually contained.
    ///
    /// `expected_app_id` is the application the caller claims this bundle
    /// belongs to. The import is aborted before it touches the live repository
    /// if the bundle disagrees.
    pub async fn import_bundle(
        &self,
        bundle: &Path,
        expected_app_id: &str,
    ) -> AppResult<ImportedBundle> {
        let staging = StagingRepo::create(&self.staging_path).await?;

        let staged = async {
            run(
                "ostree",
                &[
                    &format!("--repo={}", staging.path.display()),
                    "init",
                    "--mode=archive",
                ],
            )
            .await?;

            run(
                "flatpak",
                &[
                    "build-import-bundle",
                    &staging.path.display().to_string(),
                    &bundle.display().to_string(),
                ],
            )
            .await
            .map_err(|e| match e {
                // A malformed bundle is the publisher's problem, not a server
                // fault, so it is reported as a rejected upload.
                AppError::Repo(detail) => AppError::UploadRejected(format!(
                    "That file is not a valid Flatpak bundle: {}",
                    first_line(&detail)
                )),
                other => other,
            })?;

            let refs = list_refs(&staging.path).await?;
            let [flatpak_ref] = refs.as_slice() else {
                return Err(AppError::UploadRejected(format!(
                    "Expected the bundle to contain exactly one ref, found {}.",
                    refs.len()
                )));
            };

            let parsed = parse_ref(flatpak_ref)?;
            if parsed.app_id != expected_app_id {
                return Err(AppError::UploadRejected(format!(
                    "This bundle is for '{}', but it was uploaded under '{}'.",
                    parsed.app_id, expected_app_id
                )));
            }

            let commit = rev_parse(&staging.path, flatpak_ref).await?;

            Ok(ImportedBundle {
                flatpak_ref: flatpak_ref.clone(),
                app_id: parsed.app_id,
                arch: parsed.arch,
                branch: parsed.branch,
                commit,
            })
        }
        .await;

        // Whatever happened, the staging repo is scratch space.
        let staged = match staged {
            Ok(staged) => staged,
            Err(e) => {
                staging.discard().await;
                return Err(e);
            }
        };

        // Only now does anything reach the repository clients pull from, and only
        // one request at a time may.
        let _guard = self.write_lock.lock().await;

        let pull = run(
            "ostree",
            &[
                &format!("--repo={}", self.repo_path.display()),
                "pull-local",
                &staging.path.display().to_string(),
                &staged.flatpak_ref,
            ],
        )
        .await;

        staging.discard().await;
        pull?;

        self.update_summary_locked().await?;
        Ok(staged)
    }

    /// Drop a ref from the repository and republish the summary.
    ///
    /// The underlying objects are left for a later `ostree prune`; removing the
    /// ref is what makes the build disappear for clients.
    pub async fn remove_ref(&self, flatpak_ref: &str) -> AppResult<()> {
        let _guard = self.write_lock.lock().await;

        run(
            "ostree",
            &[
                &format!("--repo={}", self.repo_path.display()),
                "refs",
                "--delete",
                flatpak_ref,
            ],
        )
        .await?;

        self.update_summary_locked().await
    }

    /// Regenerate appstream data and the summary file.
    ///
    /// Clients read `summary` to discover what the remote offers, so this is what
    /// makes a newly imported build visible to `flatpak update`.
    ///
    /// Static deltas are **not** generated here. `--generate-static-deltas` costs
    /// O(whole repository) and produces the largest files this server serves, and
    /// it ran synchronously inside every publish and every withdrawal — on the
    /// request path, holding the mutation open. Deltas are an optimization for
    /// clients, not a correctness requirement: installs and updates work without
    /// them. Generate them out of band with
    /// `flatpak build-update-repo --generate-static-deltas <repo>` on a schedule.
    pub async fn update_summary(&self) -> AppResult<()> {
        let _guard = self.write_lock.lock().await;
        self.update_summary_locked().await
    }

    /// The body of [`update_summary`], for callers already holding the lock.
    async fn update_summary_locked(&self) -> AppResult<()> {
        let repo = self.repo_path.display().to_string();
        let mut args: Vec<String> = vec!["build-update-repo".to_string(), repo];

        if let Some(key) = &self.gpg_key_id {
            args.push(format!("--gpg-sign={key}"));
        }

        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        run("flatpak", &borrowed).await
    }

    async fn set_repo_metadata(&self, key: &str, value: &str) -> AppResult<()> {
        run(
            "ostree",
            &[
                &format!("--repo={}", self.repo_path.display()),
                "config",
                "set",
                &format!("flatpak.{key}"),
                value,
            ],
        )
        .await
    }
}

/// A scratch repository that is removed once the import decision is made.
struct StagingRepo {
    path: PathBuf,
}

impl Drop for StagingRepo {
    /// Remove the directory even when neither `discard` call is reached.
    ///
    /// "Whatever happened, the staging repo is scratch space" was not true of a
    /// panic, and — far more likely here — not true of cancellation: if a client
    /// disconnects while `build-import-bundle` is running, the future is dropped
    /// and a full unpacked copy of the bundle is left behind forever.
    ///
    /// Blocking removal on a blocking thread, because `Drop` cannot await.
    fn drop(&mut self) {
        let path = std::mem::take(&mut self.path);
        if path.as_os_str().is_empty() {
            // `discard` already ran and took the path.
            return;
        }
        std::thread::spawn(move || {
            if let Err(e) = std::fs::remove_dir_all(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                armature_log::warn!("could not remove staging repo {}: {e}", path.display());
            }
        });
    }
}

impl StagingRepo {
    async fn create(root: &Path) -> AppResult<Self> {
        tokio::fs::create_dir_all(root).await?;
        let path = root.join(format!("import-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&path).await?;
        Ok(Self { path })
    }

    /// Remove the staging directory. Failure is logged rather than propagated:
    /// leaked scratch space must not turn a successful publish into an error.
    ///
    /// Takes the path so the `Drop` fallback knows cleanup already happened.
    async fn discard(mut self) {
        let path = std::mem::take(&mut self.path);
        if let Err(e) = tokio::fs::remove_dir_all(&path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            armature_log::warn!("could not remove staging repo {}: {e}", path.display());
        }
    }
}

/// The three components a Flatpak ref carries beyond its `app`/`runtime` kind.
struct ParsedRef {
    app_id: String,
    arch: String,
    branch: String,
}

/// Split `app/org.gnome.Calculator/x86_64/stable` into its parts.
fn parse_ref(flatpak_ref: &str) -> AppResult<ParsedRef> {
    let parts: Vec<&str> = flatpak_ref.split('/').collect();
    let [kind, app_id, arch, branch] = parts.as_slice() else {
        return Err(AppError::UploadRejected(format!(
            "'{flatpak_ref}' is not a well-formed Flatpak ref."
        )));
    };

    if *kind != "app" && *kind != "runtime" {
        return Err(AppError::UploadRejected(format!(
            "Unsupported ref kind '{kind}'; expected 'app' or 'runtime'."
        )));
    }
    if app_id.is_empty() || arch.is_empty() || branch.is_empty() {
        return Err(AppError::UploadRejected(format!(
            "'{flatpak_ref}' has an empty component."
        )));
    }

    Ok(ParsedRef {
        app_id: (*app_id).to_string(),
        arch: (*arch).to_string(),
        branch: (*branch).to_string(),
    })
}

async fn list_refs(repo: &Path) -> AppResult<Vec<String>> {
    let output = capture("ostree", &[&format!("--repo={}", repo.display()), "refs"]).await?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

async fn rev_parse(repo: &Path, flatpak_ref: &str) -> AppResult<String> {
    let output = capture(
        "ostree",
        &[&format!("--repo={}", repo.display()), "rev-parse", flatpak_ref],
    )
    .await?;
    Ok(output.trim().to_string())
}

/// Run a command, discarding stdout and failing on a non-zero exit.
async fn run(program: &str, args: &[&str]) -> AppResult<()> {
    capture(program, args).await.map(|_| ())
}

/// Run a command and return its stdout.
async fn capture(program: &str, args: &[&str]) -> AppResult<String> {
    armature_log::debug!("running {program} {}", args.join(" "));

    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::Repo(format!(
                    "'{program}' is not installed. FlatNexus needs the ostree and \
                     flatpak command-line tools on PATH."
                ))
            } else {
                AppError::Repo(format!("could not run {program}: {e}"))
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AppError::Repo(format!(
            "{program} {} failed: {stderr}",
            args.first().copied().unwrap_or_default()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text).trim()
}

#[cfg(test)]
mod tests {
    use super::parse_ref;

    #[test]
    fn parses_a_well_formed_app_ref() {
        let parsed = parse_ref("app/org.gnome.Calculator/x86_64/stable").unwrap();
        assert_eq!(parsed.app_id, "org.gnome.Calculator");
        assert_eq!(parsed.arch, "x86_64");
        assert_eq!(parsed.branch, "stable");
    }

    #[test]
    fn parses_runtime_refs() {
        assert!(parse_ref("runtime/org.freedesktop.Platform/aarch64/23.08").is_ok());
    }

    #[test]
    fn rejects_malformed_refs() {
        for bad in [
            "app/org.gnome.Calculator/x86_64",
            "app/org.gnome.Calculator/x86_64/stable/extra",
            "screenshot/org.gnome.Calculator/x86_64/stable",
            "app//x86_64/stable",
            "",
        ] {
            assert!(parse_ref(bad).is_err(), "{bad} should be rejected");
        }
    }
}
