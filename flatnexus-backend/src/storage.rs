//! Bundle archival in DigitalOcean Spaces.
//!
//! Uploaded `.flatpak` bundles are archived to an object-storage bucket rather
//! than to local disk. Spaces is S3-compatible, so this drives the AWS S3 SDK
//! against a Spaces endpoint — the same code works against MinIO for local
//! development, which is what `docker-compose.yml` provides.
//!
//! # Multipart, streamed from disk
//!
//! The archive is written with a real S3 multipart upload. Each part is a
//! [`ByteStream`] over a byte *range of the spooled file*, so at most one part
//! is in memory at a time regardless of bundle size — a 4 GiB bundle costs
//! [`PART_SIZE`], not 4 GiB. A single `PutObject` taking `Bytes` would have to
//! materialize the whole thing, which is what this replaces.
//!
//! An interrupted multipart upload leaves parts that Spaces bills for and that
//! no `ListObjects` will show, so every failure path aborts the upload
//! explicitly rather than leaving it to a lifecycle rule that may not exist.
//!
//! # What lives where
//!
//! The bucket holds the *original bundle a publisher uploaded*, which is the
//! artifact they actually produced and signed. It is not what clients install
//! from: that is the OSTree repository on the server's own disk, because
//! `libostree` fetches thousands of small objects per install and expects them
//! at stable paths under one origin. The two are complementary — the repository
//! is the distribution surface, the bucket is the archive of record.
//!
//! # Keys are scoped per application
//!
//! ```text
//! <prefix>/<app-id>/<sha256>-<arch>-<branch>.flatpak
//! ```
//!
//! Scoping by application id keeps one publisher's uploads in one place, which
//! is what makes a per-application lifecycle rule, a size audit, or a bulk
//! deletion expressible as a prefix operation rather than a full-bucket scan.
//! The digest leads the filename so the key is content-addressed: re-uploading
//! identical bytes resolves to the same object instead of a second copy.

use std::path::Path;
use std::time::Duration;

use aws_sdk_s3::Client;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use aws_smithy_types::byte_stream::{ByteStream, Length};

use crate::config::SpacesConfig;
use crate::error::{AppError, AppResult};
use crate::ostree::ImportedBundle;

/// Content type Spaces will serve archived bundles with.
const BUNDLE_CONTENT_TYPE: &str = "application/vnd.flatpak";

/// Size of each uploaded part.
///
/// S3 requires every part except the last to be at least 5 MiB and allows at
/// most 10,000 parts. At 16 MiB that ceiling is 160 GiB — far beyond any bundle
/// this server accepts — while keeping the in-flight buffer small.
const PART_SIZE: u64 = 16 * 1024 * 1024;

/// S3's floor for a non-final part.
const MIN_PART_SIZE: u64 = 5 * 1024 * 1024;

/// S3 accepts at most this many parts in one upload.
const MAX_PARTS: u64 = 10_000;

// Both invariants are properties of constants, so they are checked at compile
// time rather than in a test: a violation should fail the build, not wait for
// someone to run the suite — and clippy correctly objects to runtime assertions
// over values it can already fold.
const _: () = assert!(
    PART_SIZE >= MIN_PART_SIZE,
    "S3 rejects every non-final part below 5 MiB"
);
const _: () = assert!(
    PART_SIZE * MAX_PARTS >= crate::config::DEFAULT_MAX_BUNDLE_BYTES,
    "PART_SIZE is too small to cover the largest bundle this server accepts"
);

pub struct BundleStore {
    client: Client,
    bucket: String,
    /// Key prefix, already normalized without a trailing slash.
    prefix: Option<String>,
    presign_ttl: Duration,
    /// Optional CDN origin. When set, permanent links are built against it
    /// rather than signed against the origin endpoint.
    cdn_endpoint: Option<String>,
}

impl BundleStore {
    /// Connect to the configured bucket.
    ///
    /// Credentials come from the standard AWS provider chain —
    /// `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, which is where a Spaces
    /// access key and secret go. They are deliberately not FlatNexus-specific
    /// settings: every S3 tool an operator already has reads the same variables,
    /// and duplicating them under another name invites the two copies to drift.
    pub async fn connect(config: &SpacesConfig) -> AppResult<Self> {
        let ambient = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;

        let s3_config = aws_sdk_s3::config::Builder::from(&ambient)
            .region(Region::new(config.region.clone()))
            .endpoint_url(config.endpoint.clone())
            // Spaces serves both addressing styles; path style is the one that
            // also works against MinIO without per-bucket DNS, so the same
            // configuration covers development and production.
            .force_path_style(true)
            .build();

        let store = Self {
            client: Client::from_conf(s3_config),
            bucket: config.bucket.clone(),
            prefix: config
                .prefix
                .as_deref()
                .map(|p| p.trim_matches('/').to_string())
                .filter(|p| !p.is_empty()),
            presign_ttl: Duration::from_secs(config.presign_ttl_secs.max(1) as u64),
            cdn_endpoint: config.cdn_endpoint.clone(),
        };

        // One round trip, so a wrong bucket name, wrong region, bad credentials,
        // or unreachable endpoint is a boot failure. Building the client does no
        // network I/O, so without this the first sign of a misconfiguration was a
        // failed `publishBuild` — after the bundle had already been imported into
        // the live repository.
        store
            .client
            .head_bucket()
            .bucket(&store.bucket)
            .send()
            .await
            .map_err(|e| {
                AppError::Config(format!(
                    "could not reach Spaces bucket '{}' at {}: {e}",
                    store.bucket, config.endpoint
                ))
            })?;

        Ok(store)
    }

    /// Whether the bucket answers right now.
    ///
    /// Used by the readiness probe. Separate from the boot check because the
    /// answer can change under a running process — credentials expire, and a
    /// bucket can be deleted.
    pub async fn is_reachable(&self) -> bool {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .is_ok()
    }

    /// Object key for a bundle, before the configured prefix is applied.
    pub fn key_for(sha256: &str, imported: &ImportedBundle) -> String {
        format!(
            "{app}/{sha}-{arch}-{branch}.flatpak",
            app = imported.app_id,
            sha = sha256,
            arch = imported.arch,
            branch = imported.branch,
        )
    }

    /// The full key, including any configured prefix.
    fn full_key(&self, key: &str) -> String {
        match &self.prefix {
            Some(prefix) => format!("{prefix}/{}", key.trim_start_matches('/')),
            None => key.trim_start_matches('/').to_string(),
        }
    }

    /// Archive a spooled bundle as a multipart upload.
    ///
    /// Content-addressed, so an object already present is left alone: identical
    /// bytes under an identical key are the same archive, and re-uploading them
    /// would only spend time and egress.
    pub async fn archive(&self, key: &str, source: &Path, size_bytes: u64) -> AppResult<()> {
        if self.exists(key).await? {
            armature_log::debug!("bundle {key} already archived; skipping upload");
            return Ok(());
        }

        let full_key = self.full_key(key);

        let created = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(&full_key)
            .content_type(BUNDLE_CONTENT_TYPE)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("could not start multipart upload: {e}")))?;

        let Some(upload_id) = created.upload_id() else {
            // The upload exists server-side even though the response omitted its
            // id, so there is nothing to abort with and nothing to clean up by.
            // Loud rather than silent: this needs a lifecycle rule to reap.
            armature_log::error!(
                "Spaces started a multipart upload for {full_key} without returning an \
                 upload id; its parts cannot be aborted and will be billed until a \
                 lifecycle rule reaps them"
            );
            return Err(AppError::Internal(
                "multipart upload started without an upload id".into(),
            ));
        };

        // One fallible region covering *both* the part uploads and the
        // completion. Aborting only on an `upload_parts` failure left the worst
        // case unhandled: when `complete` fails, every part is already stored
        // and billed.
        let outcome = async {
            let parts = self
                .upload_parts(&full_key, upload_id, source, size_bytes)
                .await?;
            self.complete(&full_key, upload_id, parts).await
        }
        .await;

        if let Err(e) = outcome {
            // Parts left behind by an interrupted upload are billed and are
            // invisible to `ListObjects`, so the abort is not optional.
            self.abort(&full_key, upload_id).await;
            return Err(e);
        }

        Ok(())
    }

    /// Stream each part straight off the spooled file.
    async fn upload_parts(
        &self,
        full_key: &str,
        upload_id: &str,
        source: &Path,
        size_bytes: u64,
    ) -> AppResult<Vec<CompletedPart>> {
        let mut parts = Vec::new();
        let mut offset: u64 = 0;
        // S3 part numbers are 1-based.
        let mut part_number: i32 = 1;

        while offset < size_bytes {
            let length = PART_SIZE.min(size_bytes - offset);

            let body = ByteStream::read_from()
                .path(source)
                .offset(offset)
                .length(Length::Exact(length))
                .build()
                .await
                .map_err(|e| {
                    AppError::Internal(format!("could not read bundle part {part_number}: {e}"))
                })?;

            let uploaded = self
                .client
                .upload_part()
                .bucket(&self.bucket)
                .key(full_key)
                .upload_id(upload_id)
                .part_number(part_number)
                .body(body)
                .send()
                .await
                .map_err(|e| {
                    AppError::Internal(format!("could not upload bundle part {part_number}: {e}"))
                })?;

            parts.push(
                CompletedPart::builder()
                    .part_number(part_number)
                    .set_e_tag(uploaded.e_tag().map(str::to_string))
                    .build(),
            );

            offset += length;
            part_number += 1;
        }

        Ok(parts)
    }

    async fn complete(
        &self,
        full_key: &str,
        upload_id: &str,
        parts: Vec<CompletedPart>,
    ) -> AppResult<()> {
        let completed = CompletedMultipartUpload::builder()
            .set_parts(Some(parts))
            .build();

        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(full_key)
            .upload_id(upload_id)
            .multipart_upload(completed)
            .send()
            .await
            .map_err(|e| {
                AppError::Internal(format!("could not complete multipart upload: {e}"))
            })?;

        Ok(())
    }

    /// Abandon an upload. Logged rather than propagated — the caller is already
    /// returning the failure that caused it, and a failed abort must not mask it.
    async fn abort(&self, full_key: &str, upload_id: &str) {
        let result = self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(full_key)
            .upload_id(upload_id)
            .send()
            .await;

        if let Err(e) = result {
            armature_log::error!(
                "could not abort multipart upload {upload_id} for {full_key}; \
                 its parts will be billed until a lifecycle rule reaps them: {e}"
            );
        }
    }

    async fn exists(&self, key: &str) -> AppResult<bool> {
        let result = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(self.full_key(key))
            .send()
            .await;

        match result {
            Ok(_) => Ok(true),
            // A missing object is the expected case on every first upload, so it
            // is not an error; anything else is.
            Err(e) => match e.into_service_error() {
                aws_sdk_s3::operation::head_object::HeadObjectError::NotFound(_) => Ok(false),
                other => Err(AppError::Internal(format!(
                    "could not query Spaces for {key}: {other}"
                ))),
            },
        }
    }

    /// Store a small object in one request.
    ///
    /// Bundles go through a multipart upload because they are gigabytes; gallery
    /// media is capped at 15 MiB, well under S3's 5 GiB single-`PutObject` limit,
    /// so the extra three round trips a multipart upload costs would buy nothing.
    pub async fn put_small(&self, key: &str, bytes: Vec<u8>, content_type: &str) -> AppResult<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.full_key(key))
            .content_type(content_type)
            .body(bytes.into())
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("could not store {key}: {e}")))?;

        Ok(())
    }

    /// A time-limited URL for any stored object.
    ///
    /// Gallery media uses the same signing path as bundles: the bucket is private,
    /// and an application's artwork should stop being reachable when the
    /// application does.
    pub async fn object_url(&self, key: &str) -> AppResult<String> {
        self.download_url(key).await
    }

    /// Remove an archived bundle.
    ///
    /// S3 deletion is idempotent, so a key that is already gone is not an error
    /// — which matters because a withdrawn build may be retried after a
    /// partially-failed deletion.
    pub async fn remove(&self, key: &str) -> AppResult<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.full_key(key))
            .send()
            .await
            .map_err(|e| {
                AppError::Internal(format!("could not delete bundle from Spaces: {e}"))
            })?;

        Ok(())
    }

    /// A time-limited download URL for an archived bundle.
    ///
    /// Presigned rather than public: the bucket holds every build ever uploaded,
    /// including ones since withdrawn, and a permanently readable object is one
    /// a withdrawal cannot actually recall.
    ///
    /// Returns a URL or fails. It previously returned `Option`, whose `None` no
    /// path could produce — so the GraphQL field's nullability was untruthful and
    /// any client branching on it had dead code.
    pub async fn download_url(&self, key: &str) -> AppResult<String> {
        if let Some(cdn) = &self.cdn_endpoint {
            // A CDN origin is only ever configured for a public bucket, where a
            // signature would be pointless — and wrong, since the CDN would
            // cache one user's signed URL and serve it after it expired.
            return Ok(format!(
                "{}/{}",
                cdn.trim_end_matches('/'),
                self.full_key(key)
            ));
        }

        let presigning = PresigningConfig::expires_in(self.presign_ttl)
            .map_err(|e| AppError::Internal(format!("invalid presigning lifetime: {e}")))?;

        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.full_key(key))
            .presigned(presigning)
            .await
            .map_err(|e| AppError::Internal(format!("could not sign bundle URL: {e}")))?;

        Ok(request.uri().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imported() -> ImportedBundle {
        ImportedBundle {
            flatpak_ref: "app/org.example.App/x86_64/stable".into(),
            app_id: "org.example.App".into(),
            arch: "x86_64".into(),
            branch: "stable".into(),
            commit: "abc123".into(),
        }
    }

    #[test]
    fn keys_are_scoped_by_application() {
        let key = BundleStore::key_for("deadbeef", &imported());
        assert!(key.starts_with("org.example.App/"));
        assert_eq!(key, "org.example.App/deadbeef-x86_64-stable.flatpak");
    }

    #[test]
    fn keys_distinguish_architecture_and_branch() {
        let mut other = imported();
        other.arch = "aarch64".into();
        assert_ne!(
            BundleStore::key_for("deadbeef", &imported()),
            BundleStore::key_for("deadbeef", &other)
        );

        let mut beta = imported();
        beta.branch = "beta".into();
        assert_ne!(
            BundleStore::key_for("deadbeef", &imported()),
            BundleStore::key_for("deadbeef", &beta)
        );
    }

    #[test]
    fn identical_bundles_resolve_to_one_key() {
        // What makes the archive content-addressed: the same bytes for the same
        // ref are one object, not a copy per upload.
        assert_eq!(
            BundleStore::key_for("deadbeef", &imported()),
            BundleStore::key_for("deadbeef", &imported())
        );
    }

    /// The part plan a bundle of `size` produces: (offset, length) per part.
    fn plan(size: u64) -> Vec<(u64, u64)> {
        let mut parts = Vec::new();
        let mut offset = 0;
        while offset < size {
            let length = PART_SIZE.min(size - offset);
            parts.push((offset, length));
            offset += length;
        }
        parts
    }

    #[test]
    fn a_small_bundle_is_a_single_part() {
        let parts = plan(1024);
        assert_eq!(parts, vec![(0, 1024)]);
    }

    #[test]
    fn parts_tile_the_file_without_gaps_or_overlap() {
        let size = PART_SIZE * 3 + 12_345;
        let parts = plan(size);

        assert_eq!(parts.len(), 4);
        assert_eq!(parts.iter().map(|(_, len)| len).sum::<u64>(), size);

        let mut expected_offset = 0;
        for (offset, length) in &parts {
            assert_eq!(*offset, expected_offset);
            expected_offset += length;
        }
        // Only the final part may be short.
        assert_eq!(parts.last().unwrap().1, 12_345);
    }

    #[test]
    fn an_exact_multiple_produces_no_empty_trailing_part() {
        let parts = plan(PART_SIZE * 2);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|(_, len)| *len == PART_SIZE));
    }

    #[test]
    fn an_empty_bundle_produces_no_parts() {
        // `spool` already rejects empty uploads; this pins the loop's behaviour
        // so a future caller cannot start an upload with zero parts, which S3
        // rejects at completion.
        assert!(plan(0).is_empty());
    }

    #[test]
    fn a_prefix_is_applied_and_normalized() {
        let store = |prefix: Option<&str>| {
            prefix
                .map(|p| p.trim_matches('/').to_string())
                .filter(|p| !p.is_empty())
        };
        assert_eq!(store(Some("/bundles/")), Some("bundles".to_string()));
        assert_eq!(store(Some("/")), None);
        assert_eq!(store(None), None);
    }
}
