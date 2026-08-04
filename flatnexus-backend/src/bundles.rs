//! Handling of uploaded `.flatpak` bundle files.
//!
//! The HTTP layer writes each uploaded part to a named temporary file exactly
//! once (see [`crate::http::graphql_controller`]). This module hashes that file
//! **in place** — it does not copy it again — and the archive step streams byte
//! ranges of the same file straight into a multipart upload. One inbound copy on
//! disk, start to finish.
//!
//! # A note on memory
//!
//! Armature buffers a request body in full before a handler runs, and its
//! multipart parser copies each part out of that buffer. Those two copies are
//! upstream of anything here and are what make `FLATNEXUS_MAX_BUNDLE_BYTES` a
//! memory-sizing decision as well as a disk quota. Nothing in this module or
//! below it adds another.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::error::{AppError, AppResult};

/// Read this much at a time when hashing.
const CHUNK_BYTES: usize = 1024 * 1024;

/// An uploaded bundle on disk, with its digest computed.
///
/// Borrows the path rather than owning the file: the temporary file is owned by
/// the request-scoped upload registry, which keeps it alive for exactly as long
/// as the request and removes it on drop.
pub struct SpooledBundle {
    path: PathBuf,
    size_bytes: u64,
    sha256: String,
}

impl SpooledBundle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Hash an already-spooled bundle and check it against the size ceiling.
///
/// `max_bytes` is re-checked here even though the HTTP layer already enforced
/// it: this is the boundary the resolver depends on, and a limit enforced in
/// only one of two places is one refactor away from being enforced in neither.
pub async fn hash_in_place(path: &Path, max_bytes: u64) -> AppResult<SpooledBundle> {
    let mut file = tokio::fs::File::open(path).await?;
    let size_bytes = file.metadata().await?.len();

    if size_bytes == 0 {
        return Err(AppError::UploadRejected("The uploaded bundle is empty.".into()));
    }
    if size_bytes > max_bytes {
        return Err(AppError::UploadRejected(format!(
            "Bundle exceeds the {} MiB limit for this server.",
            max_bytes / (1024 * 1024)
        )));
    }

    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_BYTES];

    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(SpooledBundle {
        path: path.to_path_buf(),
        size_bytes,
        sha256: hex::encode(hasher.finalize()),
    })
}
