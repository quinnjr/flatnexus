//! Application media: screenshots, and optionally a short WebM clip.
//!
//! Every application must carry at least one *screenshot* — a still image. That
//! is required when the application is registered, and re-checked before a build
//! can be published. An entry with no screenshot is one nobody can evaluate
//! before installing, and its repository page is a name and a paragraph of text.
//!
//! WebM is accepted alongside stills, but does not satisfy the requirement on
//! its own. A video is a supplement to the gallery, not a substitute for it: it
//! cannot be shown as a thumbnail, it will not render in a feed or a link
//! preview, and a client that cannot play it is left with nothing.
//!
//! # Type is sniffed, never trusted
//!
//! The declared `Content-Type` on a multipart part and the extension on a
//! filename are both client-controlled, so neither decides anything here. The
//! format is determined from the leading bytes, and a file whose bytes do not
//! match a permitted format is rejected — including one whose bytes and
//! extension disagree, which is the shape of an upload trying to be one thing to
//! the validator and another to a browser.
//!
//! SVG is deliberately not permitted. It is a document format that can carry
//! script, and serving one from an origin that also holds sessions is an XSS
//! vector rather than an image.

use crate::error::{AppError, AppResult};

/// Largest single still image.
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Largest single video.
///
/// Higher than the image ceiling because 5 MiB of WebM is a few seconds at any
/// watchable bitrate — permitting the format under an image-sized limit would
/// be permitting it in name only.
pub const MAX_VIDEO_BYTES: u64 = 15 * 1024 * 1024;

/// Most gallery items one application may carry, stills and video together.
pub const MAX_MEDIA_ITEMS: usize = 8;

/// Fewest still images an application may have and still be publishable.
pub const MIN_SCREENSHOTS: usize = 1;

/// A media format this server will accept and serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaFormat {
    Png,
    Jpeg,
    WebP,
    WebM,
}

impl MediaFormat {
    /// Whether this format counts toward the screenshot requirement.
    pub fn is_still_image(self) -> bool {
        !matches!(self, Self::WebM)
    }

    /// The `Content-Type` this server will serve the object with.
    ///
    /// Taken from the sniffed format, never from the upload: serving a
    /// client-chosen content type is how a file that sniffs as an image gets
    /// served as something executable.
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::WebP => "image/webp",
            Self::WebM => "video/webm",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
            Self::WebM => "webm",
        }
    }

    /// The size ceiling that applies to this format.
    pub fn max_bytes(self) -> u64 {
        if self.is_still_image() {
            MAX_IMAGE_BYTES
        } else {
            MAX_VIDEO_BYTES
        }
    }

    /// Identify the format from the leading bytes of a file.
    ///
    /// Returns `None` for anything that is not a permitted format, which
    /// includes a valid GIF, SVG, or MP4 — "not media" and "not media we accept"
    /// are the same answer to a caller.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        // PNG: the 8-byte signature, whose CRLF/EOF bytes also detect a transfer
        // that mangled line endings.
        const PNG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        if bytes.starts_with(&PNG) {
            return Some(Self::Png);
        }

        // JPEG: SOI marker followed by any marker byte.
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Some(Self::Jpeg);
        }

        // WebP is a RIFF container: "RIFF" ⟨4-byte length⟩ "WEBP".
        // The length field sits between the two tags, so both must be checked —
        // a WAV file also starts with "RIFF".
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return Some(Self::WebP);
        }

        // WebM is an EBML container, and so is Matroska: the signature alone
        // does not distinguish them. The DocType element carries the string
        // "webm" and sits within the first few dozen bytes of the header, so a
        // bounded scan of that window is what separates the two.
        const EBML: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];
        if bytes.starts_with(&EBML) {
            let header = &bytes[..bytes.len().min(EBML_SCAN_BYTES)];
            if header.windows(4).any(|window| window == b"webm") {
                return Some(Self::WebM);
            }
        }

        None
    }

}

/// How far into an EBML header to look for the DocType string. The header is
/// small and fixed-ish; scanning further would only invite a false positive from
/// arbitrary payload bytes.
const EBML_SCAN_BYTES: usize = 64;

/// A gallery item that has passed validation.
#[derive(Debug, Clone)]
pub struct ValidatedMedia {
    pub format: MediaFormat,
    pub bytes: Vec<u8>,
    /// Position in the application's gallery, starting at 0.
    pub position: i64,
}

impl ValidatedMedia {
    pub fn size_bytes(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// Validate one uploaded gallery item.
///
/// `position` is the index it will occupy in the gallery.
pub fn validate(bytes: Vec<u8>, position: i64) -> AppResult<ValidatedMedia> {
    if bytes.is_empty() {
        return Err(AppError::UploadRejected(
            "One of the uploaded files is empty.".into(),
        ));
    }

    // Sniffed before the size check, so the ceiling reported is the one that
    // actually applies to the format the caller sent.
    let Some(format) = MediaFormat::sniff(&bytes) else {
        return Err(AppError::UploadRejected(
            "Screenshots must be PNG, JPEG, or WebP, and videos must be WebM. \
             The file's contents are not one of those formats."
                .into(),
        ));
    };

    if bytes.len() as u64 > format.max_bytes() {
        return Err(AppError::UploadRejected(format!(
            "{} files must be {} MiB or smaller.",
            format.extension().to_uppercase(),
            format.max_bytes() / (1024 * 1024)
        )));
    }

    Ok(ValidatedMedia {
        format,
        bytes,
        position,
    })
}

/// Validate a whole gallery, enforcing the count bound and the screenshot rule.
pub fn validate_all(items: Vec<Vec<u8>>) -> AppResult<Vec<ValidatedMedia>> {
    if items.is_empty() {
        return Err(AppError::Validation(
            "At least one screenshot is required.".into(),
        ));
    }

    if items.len() > MAX_MEDIA_ITEMS {
        return Err(AppError::UploadRejected(format!(
            "An application may have at most {MAX_MEDIA_ITEMS} gallery items."
        )));
    }

    let validated: Vec<ValidatedMedia> = items
        .into_iter()
        .enumerate()
        .map(|(index, bytes)| validate(bytes, index as i64))
        .collect::<AppResult<_>>()?;

    let stills = validated.iter().filter(|m| m.format.is_still_image()).count();
    if stills < MIN_SCREENSHOTS {
        return Err(AppError::Validation(
            "At least one screenshot is required. A video on its own does not \
             count: it cannot be shown as a thumbnail or in a link preview."
                .into(),
        ));
    }

    Ok(validated)
}

/// Object key for a gallery item in the bundle bucket.
///
/// Scoped under `screenshots/` and then by application id, mirroring how bundle
/// keys are scoped, so an application's whole footprint — bundles and media — is
/// reachable by two prefixes rather than a bucket scan.
pub fn key_for(app_id: &str, sha256: &str, format: MediaFormat) -> String {
    format!(
        "screenshots/{app}/{sha}.{ext}",
        app = app_id,
        sha = sha256,
        ext = format.extension()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(b"...rest of a png...");
        bytes
    }

    fn jpeg() -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
        bytes.extend_from_slice(b"...rest of a jpeg...");
        bytes
    }

    fn webp() -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&[0x24, 0x00, 0x00, 0x00]);
        bytes.extend_from_slice(b"WEBPVP8 ...");
        bytes
    }

    fn webm() -> Vec<u8> {
        let mut bytes = vec![0x1A, 0x45, 0xDF, 0xA3];
        // A realistic-enough EBML header: the DocType string sits a little way
        // in, not immediately after the signature.
        bytes.extend_from_slice(&[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F]);
        bytes.extend_from_slice(b"\x42\x86\x81\x01\x42\xF7\x81\x01\x42\x82\x84webm");
        bytes.extend_from_slice(b"...rest of a webm...");
        bytes
    }

    #[test]
    fn sniffs_the_permitted_formats() {
        assert_eq!(MediaFormat::sniff(&png()), Some(MediaFormat::Png));
        assert_eq!(MediaFormat::sniff(&jpeg()), Some(MediaFormat::Jpeg));
        assert_eq!(MediaFormat::sniff(&webp()), Some(MediaFormat::WebP));
        assert_eq!(MediaFormat::sniff(&webm()), Some(MediaFormat::WebM));
    }

    #[test]
    fn rejects_svg_however_it_is_dressed_up() {
        // The case that matters: an SVG is a scriptable document, and its
        // filename and declared content type are both attacker-chosen.
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
        assert_eq!(MediaFormat::sniff(svg), None);
        assert!(validate(svg.to_vec(), 0).is_err());
    }

    #[test]
    fn rejects_other_non_media() {
        for bad in [
            b"GIF89a".to_vec(),
            b"\x7fELF".to_vec(),
            b"#!/bin/sh\nrm -rf /".to_vec(),
            b"%PDF-1.7".to_vec(),
            vec![0u8; 32],
        ] {
            assert_eq!(MediaFormat::sniff(&bad), None, "{bad:?} should not sniff");
        }
    }

    #[test]
    fn a_riff_container_that_is_not_webp_is_not_accepted() {
        // A WAV file is also RIFF; only the second tag distinguishes it, so a
        // check on the first four bytes alone would let this through.
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&[0x24, 0x00, 0x00, 0x00]);
        wav.extend_from_slice(b"WAVEfmt ");
        assert_eq!(MediaFormat::sniff(&wav), None);
    }

    #[test]
    fn an_ebml_container_that_is_not_webm_is_not_accepted() {
        // Matroska shares WebM's signature; only the DocType separates them.
        let mut mkv = vec![0x1A, 0x45, 0xDF, 0xA3];
        mkv.extend_from_slice(b"\x42\x82\x88matroska");
        assert_eq!(MediaFormat::sniff(&mkv), None);
    }

    #[test]
    fn a_truncated_header_does_not_panic() {
        // Slicing bytes[8..12] on a short input would panic; these pin the
        // length guards that prevent it.
        assert_eq!(MediaFormat::sniff(b"RIFF"), None);
        assert_eq!(MediaFormat::sniff(b"RIFF\x00\x00\x00\x00WEB"), None);
        assert_eq!(MediaFormat::sniff(&[0x1A, 0x45, 0xDF, 0xA3]), None);
    }

    #[test]
    fn size_ceilings_are_per_format() {
        let mut big_png = png();
        big_png.resize((MAX_IMAGE_BYTES + 1) as usize, 0);
        assert!(validate(big_png, 0).is_err());

        // The same byte count is fine for video, which has its own ceiling.
        let mut medium_webm = webm();
        medium_webm.resize((MAX_IMAGE_BYTES + 1) as usize, 0);
        assert!(validate(medium_webm, 0).is_ok());

        let mut big_webm = webm();
        big_webm.resize((MAX_VIDEO_BYTES + 1) as usize, 0);
        assert!(validate(big_webm, 0).is_err());
    }

    #[test]
    fn rejects_an_empty_file() {
        assert!(validate(Vec::new(), 0).is_err());
    }

    #[test]
    fn requires_at_least_one_item() {
        assert!(validate_all(Vec::new()).is_err());
    }

    #[test]
    fn a_video_alone_does_not_satisfy_the_screenshot_requirement() {
        // The point of keeping "screenshot" meaningful once WebM is permitted.
        assert!(validate_all(vec![webm()]).is_err());
        assert!(validate_all(vec![webm(), webm()]).is_err());
    }

    #[test]
    fn a_still_plus_a_video_is_accepted() {
        let gallery = validate_all(vec![png(), webm()]).unwrap();
        assert_eq!(gallery.len(), 2);
        assert!(gallery[0].format.is_still_image());
        assert!(!gallery[1].format.is_still_image());
    }

    #[test]
    fn enforces_the_count_ceiling() {
        let too_many: Vec<Vec<u8>> = (0..MAX_MEDIA_ITEMS + 1).map(|_| png()).collect();
        assert!(validate_all(too_many).is_err());
    }

    #[test]
    fn assigns_gallery_positions_in_upload_order() {
        let validated = validate_all(vec![png(), jpeg(), webp()]).unwrap();

        assert_eq!(
            validated.iter().map(|s| s.position).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(validated[0].format, MediaFormat::Png);
        assert_eq!(validated[2].format, MediaFormat::WebP);
    }

    #[test]
    fn one_bad_item_rejects_the_whole_gallery() {
        // Partial acceptance would leave an application half-illustrated with no
        // signal to the publisher about which file failed.
        assert!(validate_all(vec![png(), b"not an image".to_vec()]).is_err());
    }

    #[test]
    fn keys_are_scoped_by_application_and_content_addressed() {
        let key = key_for("org.example.App", "deadbeef", MediaFormat::Png);
        assert_eq!(key, "screenshots/org.example.App/deadbeef.png");
        assert_eq!(
            key_for("org.example.App", "deadbeef", MediaFormat::WebM),
            "screenshots/org.example.App/deadbeef.webm"
        );

        // Same bytes, same key — a re-upload is one object, not a copy.
        assert_eq!(
            key_for("org.example.App", "deadbeef", MediaFormat::Png),
            key_for("org.example.App", "deadbeef", MediaFormat::Png)
        );
    }

    #[test]
    fn served_content_type_comes_from_the_sniffed_format() {
        assert_eq!(MediaFormat::Png.content_type(), "image/png");
        assert_eq!(MediaFormat::Jpeg.content_type(), "image/jpeg");
        assert_eq!(MediaFormat::WebP.content_type(), "image/webp");
        assert_eq!(MediaFormat::WebM.content_type(), "video/webm");
    }
}
