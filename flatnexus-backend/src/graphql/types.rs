//! GraphQL output and input types.
//!
//! These are deliberately separate from the Prax models in [`crate::models`]:
//! the storage shape carries fields that must never leave the server
//! (`password_hash`, `totp_secret`), and the wire shape carries derived values
//! that have no column (install commands, ref URLs).

use armature_graphql::async_graphql::{ComplexObject, Enum, InputObject, SimpleObject, Upload};
use armature_graphql::{Context, ID, Object, Result as GqlResult};

use crate::auth::otp::{OtpMethod, OtpPurpose};
use crate::error::{AppError, IntoGqlResult, to_gql_error};
use crate::graphql::loaders::{LoadError, Loaders};
use crate::graphql::context::require_viewer;
use crate::models::{self};
use crate::state::state;

/// Which second factor a publisher uses to authorize uploads.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum OtpMethodGql {
    /// RFC 6238 authenticator app. Works unattended from `curl`.
    Totp,
    /// A code mailed on demand, requested via `requestUploadOtp`.
    Email,
}

impl From<OtpMethodGql> for OtpMethod {
    fn from(value: OtpMethodGql) -> Self {
        match value {
            OtpMethodGql::Totp => OtpMethod::Totp,
            OtpMethodGql::Email => OtpMethod::Email,
        }
    }
}

impl OtpMethodGql {
    pub fn from_stored(raw: &str) -> Self {
        match raw {
            "totp" => Self::Totp,
            _ => Self::Email,
        }
    }
}

/// A publisher, as seen by anyone.
#[derive(SimpleObject, Clone, Debug)]
pub struct PublisherProfile {
    pub id: ID,
    pub display_name: String,
}

impl From<&models::Publisher> for PublisherProfile {
    fn from(p: &models::Publisher) -> Self {
        Self {
            id: ID::from(p.id.clone()),
            display_name: p.display_name.clone(),
        }
    }
}

/// The signed-in publisher's own account, including settings only they see.
#[derive(SimpleObject, Clone, Debug)]
pub struct Viewer {
    pub id: ID,
    pub email: String,
    pub display_name: String,
    pub is_admin: bool,
    /// Which OTP method gates this account's uploads.
    pub otp_method: OtpMethodGql,
    /// True once an authenticator has been enrolled *and* confirmed. Switching
    /// `otpMethod` to `TOTP` is refused until this is true.
    pub totp_confirmed: bool,
    /// False for accounts that have only ever signed in through GitHub/Google.
    pub has_password: bool,
}

impl From<&models::Publisher> for Viewer {
    fn from(p: &models::Publisher) -> Self {
        Self {
            id: ID::from(p.id.clone()),
            email: p.email.clone(),
            display_name: p.display_name.clone(),
            is_admin: p.is_admin,
            otp_method: OtpMethodGql::from_stored(&p.otp_method),
            totp_confirmed: p.totp_confirmed,
            has_password: p.password_hash.is_some(),
        }
    }
}

/// A published application.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct Application {
    pub id: ID,
    /// Reverse-DNS Flatpak application id, e.g. `org.gnome.Calculator`.
    pub app_id: String,
    pub name: String,
    pub summary: String,
    pub description: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub created_at: i64,

    #[graphql(skip)]
    publisher_id: String,
}

#[ComplexObject]
impl Application {
    /// Builds published for this application, newest first.
    ///
    /// Batched: in a list query every application's builds are fetched by one
    /// `IN (...)`, not one query per row.
    async fn builds(&self, ctx: &Context<'_>) -> GqlResult<Vec<Build>> {
        let loaders = ctx.data::<Loaders>()?;
        let rows = loaders
            .builds
            .load_one(self.id.to_string())
            .await
            .map_err(|e| to_gql_error(clone_load_error(&e)))?
            .unwrap_or_default();

        Ok(rows.iter().map(Build::from).collect())
    }

    /// Who owns this application record. Batched, as with `builds`.
    async fn publisher(&self, ctx: &Context<'_>) -> GqlResult<Option<PublisherProfile>> {
        let loaders = ctx.data::<Loaders>()?;
        let found = loaders
            .publishers
            .load_one(self.publisher_id.clone())
            .await
            .map_err(|e| to_gql_error(clone_load_error(&e)))?;

        Ok(found.as_ref().map(PublisherProfile::from))
    }

    /// Screenshots and clips, in the publisher's chosen order.
    ///
    /// Never empty for a registered application: at least one still image is
    /// required to register, and re-checked before a build can be published.
    async fn screenshots(&self, ctx: &Context<'_>) -> GqlResult<Vec<Screenshot>> {
        let loaders = ctx.data::<Loaders>()?;
        let rows = loaders
            .screenshots
            .load_one(self.id.to_string())
            .await
            .map_err(|e| to_gql_error(clone_load_error(&e)))?
            .unwrap_or_default();

        Ok(rows.iter().map(Screenshot::from).collect())
    }

    /// The command a user runs to install this application from this remote.
    async fn install_command(&self) -> String {
        format!("flatpak install flatnexus {}", self.app_id)
    }

    /// URL of the single-click `.flatpakref` file for this application.
    async fn flatpakref_url(&self) -> String {
        format!("{}/refs/{}.flatpakref", state().config.public_url, self.app_id)
    }
}

impl From<&models::Application> for Application {
    fn from(a: &models::Application) -> Self {
        Self {
            id: ID::from(a.id.clone()),
            app_id: a.app_id.clone(),
            name: a.name.clone(),
            summary: a.summary.clone(),
            description: a.description.clone(),
            homepage: a.homepage.clone(),
            license: a.license.clone(),
            created_at: a.created_at,
            publisher_id: a.publisher_id.clone(),
        }
    }
}

/// A gallery item: a screenshot, or a short WebM clip.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct Screenshot {
    pub id: ID,
    /// `png`, `jpg`, `webp`, or `webm` — sniffed from the file, never from the
    /// name the client sent.
    pub format: String,
    /// Order in the gallery, 0-based.
    pub position: i64,
    pub size_bytes: i64,
    pub sha256: String,
    /// False for a WebM clip, which does not satisfy the screenshot requirement.
    pub is_still_image: bool,

    #[graphql(skip)]
    object_key: String,
}

#[ComplexObject]
impl Screenshot {
    /// A time-limited URL for the image or clip.
    ///
    /// Signed for the same reason bundles are: the bucket is private, and an
    /// application's artwork should stop being reachable when the application
    /// does.
    async fn url(&self) -> GqlResult<String> {
        state().bundles.object_url(&self.object_key).await.gql()
    }
}

impl From<&models::Screenshot> for Screenshot {
    fn from(s: &models::Screenshot) -> Self {
        Self {
            id: ID::from(s.id.clone()),
            format: s.format.clone(),
            position: s.position,
            size_bytes: s.size_bytes,
            sha256: s.sha256.clone(),
            is_still_image: s.format != "webm",
            object_key: s.object_key.clone(),
        }
    }
}

/// One imported bundle: a single ref at a single commit.
#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct Build {
    pub id: ID,
    /// Full OSTree ref, e.g. `app/org.gnome.Calculator/x86_64/stable`.
    pub flatpak_ref: String,
    pub arch: String,
    pub branch: String,
    pub version: String,
    /// OSTree commit checksum this ref resolved to.
    pub commit: String,
    pub size_bytes: i64,
    /// SHA-256 of the uploaded bundle, for out-of-band verification.
    pub sha256: String,
    pub published_at: i64,

    #[graphql(skip)]
    bundle_key: String,
}

#[ComplexObject]
impl Build {
    /// A time-limited link to the original `.flatpak` bundle in object storage.
    ///
    /// This is the archived artifact the publisher uploaded, not the install
    /// path — installing goes through the OSTree repository. It exists so a
    /// build can be independently re-verified against its `sha256`.
    ///
    /// Resolved lazily rather than stored on the row: a presigned URL expires,
    /// so one baked into a cached response would be dead on arrival.
    async fn bundle_url(&self) -> GqlResult<String> {
        state().bundles.download_url(&self.bundle_key).await.gql()
    }
}

impl From<&models::Build> for Build {
    fn from(b: &models::Build) -> Self {
        Self {
            id: ID::from(b.id.clone()),
            flatpak_ref: b.flatpak_ref.clone(),
            arch: b.arch.clone(),
            branch: b.branch.clone(),
            version: b.version.clone(),
            commit: b.commit_id.clone(),
            size_bytes: b.size_bytes,
            sha256: b.sha256.clone(),
            published_at: b.published_at,
            bundle_key: b.bundle_key.clone(),
        }
    }
}

/// Everything a client needs to add this server as a Flatpak remote.
pub struct RepoInfo;

#[Object]
impl RepoInfo {
    /// Human-readable remote title.
    async fn title(&self) -> String {
        state().config.repo_title.clone()
    }

    async fn description(&self) -> String {
        state().config.repo_description.clone()
    }

    /// Base URL of the OSTree repository.
    async fn url(&self) -> String {
        state().config.repo_url()
    }

    /// URL of the `.flatpakrepo` file, the one-step way to add this remote.
    async fn flatpakrepo_url(&self) -> String {
        format!("{}/flatnexus.flatpakrepo", state().config.public_url)
    }

    /// Copy-paste command that adds this remote.
    async fn add_remote_command(&self) -> String {
        format!(
            "flatpak remote-add --if-not-exists flatnexus {}/flatnexus.flatpakrepo",
            state().config.public_url
        )
    }

    /// True when commits are GPG-signed and clients will verify signatures.
    async fn gpg_signed(&self) -> bool {
        state().config.gpg_key_id.is_some()
    }
}

/// A sign-in provider this deployment has configured.
#[derive(SimpleObject, Clone, Debug)]
pub struct OAuthProviderInfo {
    /// Stable id, `github` or `google`.
    pub id: String,
    pub display_name: String,
    /// Navigate the browser here to begin sign-in.
    pub start_url: String,
}

/// A minted session.
#[derive(SimpleObject, Clone, Debug)]
pub struct AuthPayload {
    /// Bearer token for the `Authorization` header.
    pub token: String,
    /// Expiry, epoch seconds.
    pub expires_at: i64,
    pub viewer: Viewer,
}

/// What an emailed one-time code will be spent on.
///
/// Codes are scoped by purpose, so one requested for a publish cannot authorize
/// a withdrawal. Exposed because without it there is no way to obtain a
/// withdrawal code at all.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum OtpPurposeGql {
    PublishBuild,
    DeleteBuild,
}

impl From<OtpPurposeGql> for OtpPurpose {
    fn from(value: OtpPurposeGql) -> Self {
        match value {
            OtpPurposeGql::PublishBuild => OtpPurpose::PublishBuild,
            OtpPurposeGql::DeleteBuild => OtpPurpose::DeleteBuild,
        }
    }
}

/// How far a sign-in attempt has got.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum LoginStatus {
    /// Sign-in is complete; `session` carries the token.
    Authenticated,
    /// The password was correct but a one-time password is still required.
    /// `challenge` carries what is needed to finish.
    OtpRequired,
}

/// The outcome of `login`.
///
/// Modelled as one type with two nullable arms rather than a GraphQL union: a
/// client must branch on `status` either way, and a union would force every
/// caller to write inline fragments for a two-case decision.
#[derive(SimpleObject, Clone, Debug)]
pub struct LoginResult {
    pub status: LoginStatus,
    /// Present only when `status` is `AUTHENTICATED`.
    pub session: Option<AuthPayload>,
    /// Present only when `status` is `OTP_REQUIRED`.
    pub challenge: Option<LoginChallenge>,
}

/// A sign-in that has passed its first factor and is waiting on a code.
#[derive(SimpleObject, Clone, Debug)]
pub struct LoginChallenge {
    /// Opaque token identifying this attempt. Pass it back to `verifyLogin`.
    ///
    /// It is not a session token and is rejected everywhere a session is
    /// expected; it authorizes exactly one thing, redeeming this challenge.
    pub challenge_token: String,
    /// Which factor the account expects.
    pub method: OtpMethodGql,
    /// For `EMAIL`, the masked address the code was sent to. Null for `TOTP`,
    /// where nothing was sent — the code comes off the publisher's own device.
    pub sent_to: Option<String>,
    /// Seconds this challenge remains redeemable.
    pub expires_in_secs: i64,
}

/// A newly generated, not-yet-confirmed TOTP enrollment.
///
/// The secret is returned exactly once. Confirm it with `confirmTotp` before it
/// can authorize anything.
#[derive(SimpleObject, Clone, Debug)]
pub struct TotpEnrollment {
    /// Base32 secret, for manual entry into an authenticator.
    pub secret: String,
    /// `otpauth://` URI to render as a QR code.
    pub uri: String,
}

/// Outcome of requesting an emailed one-time code.
#[derive(SimpleObject, Clone, Debug)]
pub struct OtpRequestResult {
    /// Address the code was sent to, partially masked.
    pub sent_to: String,
    /// Seconds the code remains valid.
    pub expires_in_secs: i64,
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

#[derive(InputObject, Debug)]
pub struct RegisterInput {
    pub email: String,
    pub display_name: String,
    pub password: String,
}

#[derive(InputObject, Debug)]
pub struct LoginInput {
    pub email: String,
    pub password: String,
}

/// Registering an application.
///
/// Carries files, so this mutation goes over the GraphQL multipart request spec
/// exactly as `publishBuild` does.
#[derive(InputObject)]
pub struct CreateApplicationInput {
    /// Reverse-DNS id. Must match the ref inside every bundle uploaded for it.
    pub app_id: String,
    pub name: String,
    pub summary: String,
    #[graphql(default)]
    pub description: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    /// At least one screenshot. PNG, JPEG or WebP; a WebM clip may be included
    /// alongside but does not satisfy the requirement on its own.
    pub screenshots: Vec<Upload>,
}

/// Replacing an application's whole gallery.
///
/// Whole-gallery rather than per-item so the "at least one screenshot" rule is
/// checked against exactly what the application will end up with. A per-item
/// delete would have to re-derive that invariant on every call, and get it right
/// every time.
#[derive(InputObject)]
pub struct ReplaceScreenshotsInput {
    pub app_id: String,
    /// The complete new gallery, in order.
    pub screenshots: Vec<Upload>,
}

#[derive(InputObject, Debug)]
pub struct UpdateApplicationInput {
    pub name: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
}

/// A build upload.
///
/// `bundle` uses the GraphQL multipart request spec, so the same mutation works
/// from a browser `FormData` post and from `curl -F`.
#[derive(InputObject)]
pub struct PublishBuildInput {
    /// Application this bundle belongs to. Checked against the bundle's own ref.
    pub app_id: String,
    /// Human-facing version string, e.g. `46.2`.
    #[graphql(default)]
    pub version: String,
    /// The `.flatpak` bundle produced by `flatpak build-bundle`.
    pub bundle: Upload,
    /// A one-time password. Required — see `Mutation::publish_build`.
    pub otp: String,
}

/// Re-wrap a shared loader error as an owned one.
///
/// `DataLoader` hands the same `Arc<AppError>` to every resolver that batched
/// into one query, so the error cannot be moved out. The variant is preserved so
/// the GraphQL `extensions.code` still describes what actually failed.
fn clone_load_error(error: &LoadError) -> AppError {
    AppError::Database(error.to_string())
}

/// Convenience helper for masking an address in user-facing output.
pub fn mask_email(email: &str) -> String {
    let Some((local, domain)) = email.split_once('@') else {
        return "***".to_string();
    };
    let visible = local.chars().next().unwrap_or('*');
    format!("{visible}***@{domain}")
}

/// Shared guard used by resolvers that need a signed-in publisher.
pub async fn viewer_publisher(ctx: &Context<'_>) -> GqlResult<models::Publisher> {
    let viewer = require_viewer(ctx)?;
    let state = state();
    Ok(viewer.load(&state.db).await?)
}

#[cfg(test)]
mod tests {
    use super::mask_email;

    #[test]
    fn masks_the_local_part_only() {
        assert_eq!(mask_email("developer@example.com"), "d***@example.com");
        assert_eq!(mask_email("not-an-address"), "***");
    }
}
