//! Write side of the API.
//!
//! Two mutations are privileged and require a one-time password on top of a
//! valid session: `publishBuild` and `deleteBuild`. Everything an OTP protects
//! is checked *before* the operation has any effect — see `publish_build`.

use armature_graphql::async_graphql::Upload;
use armature_graphql::{Context, ID, Object, Result as GqlResult};
use sha2::{Digest, Sha256};

use crate::auth::accounts;
use crate::auth::otp::{OtpMethod, OtpPurpose};
use crate::bundles;
use crate::http::graphql_controller::UploadRegistry;
use crate::screenshots;
use crate::storage::BundleStore;
use crate::error::{AppError, IntoGqlResult};
use crate::graphql::loaders::Loaders;
use crate::graphql::types::{
    Application, AuthPayload, Build, CreateApplicationInput, LoginChallenge, LoginInput,
    LoginResult, LoginStatus, OtpMethodGql, OtpPurposeGql, OtpRequestResult, PublishBuildInput,
    RegisterInput, ReplaceScreenshotsInput, TotpEnrollment, UpdateApplicationInput, Viewer,
    mask_email, viewer_publisher,
};
use crate::models::{self, application, build, publisher, screenshot};
use crate::models::PraxClientExt;
use crate::state::state;

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    // -- Accounts -----------------------------------------------------------

    /// Create an account with an email and password.
    ///
    /// GitHub and Google sign-up do not go through here: they are browser
    /// redirects, started from the URLs in `oauthProviders`.
    ///
    /// Returns a session directly. A new account has no second factor enrolled
    /// yet, so there is nothing to challenge; the next sign-in will be gated.
    async fn register(&self, input: RegisterInput) -> GqlResult<AuthPayload> {
        let state = state();
        let created =
            accounts::register(&state.db, &input.email, &input.display_name, &input.password)
                .await?;
        issue_session(&created)
    }

    /// Begin sign-in with an email and password.
    ///
    /// A correct password is the *first* factor, not the whole of it. This
    /// returns a session only for an account with no usable second factor;
    /// otherwise it returns a challenge that `verifyLogin` redeems with a
    /// one-time password. A password on its own is therefore never enough to
    /// obtain a token that can publish.
    ///
    /// For a publisher on the emailed-code method, the code is sent here, as
    /// part of this call — there is no separate "request a code" step to forget.
    async fn login(&self, input: LoginInput) -> GqlResult<LoginResult> {
        let state = state();
        let found = accounts::login(&state.db, &input.email, &input.password).await?;

        let method = OtpMethod::parse(&found.otp_method)?;

        // An account can be left without a usable factor: TOTP selected but the
        // enrollment never confirmed. Refusing sign-in outright would lock the
        // publisher out of the very page they need to fix it, so the password
        // alone completes here. `publishBuild` still demands an OTP, so this
        // cannot be used to reach a privileged operation.
        if method == OtpMethod::Totp && !found.totp_confirmed {
            armature_log::warn!(
                "{} signed in without a second factor: TOTP selected but unconfirmed",
                found.email
            );
            return Ok(LoginResult {
                status: LoginStatus::Authenticated,
                session: Some(issue_session(&found)?),
                challenge: None,
            });
        }

        let sent_to = match method {
            OtpMethod::Totp => None,
            OtpMethod::Email => {
                let code = state
                    .otp
                    .issue_email_code(&state.db, &found.id, OtpPurpose::Login)
                    .await?;
                state
                    .mailer
                    .send_otp(&found.email, &code, state.otp.email_ttl_secs(), OtpPurpose::Login)
                    .await?;
                Some(mask_email(&found.email))
            }
        };

        let challenge = state.tokens.issue_login_challenge(&found.id, &found.email)?;

        Ok(LoginResult {
            status: LoginStatus::OtpRequired,
            session: None,
            challenge: Some(LoginChallenge {
                challenge_token: challenge.token,
                method: OtpMethodGql::from_stored(method.as_str()),
                sent_to,
                expires_in_secs: state.tokens.challenge_ttl_secs(),
            }),
        })
    }

    /// Complete a sign-in by presenting the one-time password.
    ///
    /// `challengeToken` is what `login` returned. It is verified as a challenge
    /// specifically, so a session token cannot be passed here and a challenge
    /// token cannot be used as a session.
    async fn verify_login(
        &self,
        challenge_token: String,
        otp: String,
    ) -> GqlResult<AuthPayload> {
        let state = state();

        let claims = state.tokens.verify_login_challenge(&challenge_token)?;

        let publisher = state
            .db
            .publisher()
            .find_first()
            .r#where(publisher::id::equals(claims.sub.clone()))
            .exec()
            .await
            .gql()?
            .ok_or(AppError::Unauthenticated)?;

        state
            .otp
            .verify(&state.db, &publisher, OtpPurpose::Login, &otp)
            .await?;

        armature_log::info!("{} completed two-factor sign-in", publisher.email);
        issue_session(&publisher)
    }

    // -- Second factor ------------------------------------------------------

    /// Generate a TOTP secret for the signed-in publisher.
    ///
    /// The secret is returned once and stored unconfirmed. Until
    /// `confirmTotp` succeeds it cannot authorize anything, so a publisher who
    /// scans a QR code into the wrong device is never locked out.
    async fn enroll_totp(&self, ctx: &Context<'_>) -> GqlResult<TotpEnrollment> {
        let me = viewer_publisher(ctx).await?;
        let state = state();

        let enrollment = state.otp.generate_totp(&me.email);

        // Staged, not installed. Overwriting `totp_secret` and clearing
        // `totp_confirmed` here would put an account with working two-factor into
        // the state `login` treats as having no usable second factor — so merely
        // opening a re-enrollment and abandoning it downgraded the account to
        // password-only sign-in.
        state
            .db
            .publisher()
            .update_many()
            .r#where(publisher::id::equals(me.id.clone()))
            .set("totp_pending_secret", enrollment.secret.clone())
            .exec()
            .await
            .gql()?;

        Ok(TotpEnrollment {
            secret: enrollment.secret,
            uri: enrollment.uri,
        })
    }

    /// Confirm an enrolled authenticator by presenting a code from it.
    async fn confirm_totp(&self, ctx: &Context<'_>, code: String) -> GqlResult<Viewer> {
        let me = viewer_publisher(ctx).await?;
        let state = state();

        let Some(secret) = me.totp_pending_secret.as_deref() else {
            return Err(AppError::Validation(
                "Enroll an authenticator before confirming one.".into(),
            )
            .into());
        };

        if !state.otp.check_totp_code(secret, code.trim())? {
            return Err(AppError::OtpRejected("That authenticator code is not valid.".into()).into());
        }

        // Promote: the pending secret becomes the live one only now that a code
        // from the device has proved it works.
        state
            .db
            .publisher()
            .update_many()
            .r#where(publisher::id::equals(me.id.clone()))
            .set("totp_secret", secret.to_string())
            .set("totp_pending_secret", Option::<String>::None)
            .set("totp_confirmed", true)
            .exec()
            .await
            .gql()?;

        let refreshed = reload(&me.id).await?;
        Ok(Viewer::from(&refreshed))
    }

    /// Choose which second factor gates this account's uploads.
    async fn set_otp_method(
        &self,
        ctx: &Context<'_>,
        method: OtpMethodGql,
    ) -> GqlResult<Viewer> {
        let me = viewer_publisher(ctx).await?;
        let state = state();
        let method: OtpMethod = method.into();

        // Refusing this is the whole point of the confirm step: switching to an
        // unconfirmed authenticator would make every future upload impossible.
        if method == OtpMethod::Totp && !me.totp_confirmed {
            return Err(AppError::Validation(
                "Enroll and confirm an authenticator before switching to TOTP.".into(),
            )
            .into());
        }

        state
            .db
            .publisher()
            .update_many()
            .r#where(publisher::id::equals(me.id.clone()))
            .set("otp_method", method.as_str().to_string())
            .exec()
            .await
            .gql()?;

        let refreshed = reload(&me.id).await?;
        Ok(Viewer::from(&refreshed))
    }

    /// Request an emailed one-time code for a privileged operation.
    ///
    /// Only meaningful for publishers on the `EMAIL` method; TOTP users read the
    /// current code off their authenticator instead, which is what makes
    /// uploading from `curl` possible without a mailbox round-trip.
    ///
    /// `purpose` defaults to publishing. Emailed codes are scoped by purpose, so
    /// without a way to ask for a withdrawal code, `deleteBuild` was unreachable
    /// for every publisher on the emailed method — which is the default.
    async fn request_upload_otp(
        &self,
        ctx: &Context<'_>,
        #[graphql(default_with = "OtpPurposeGql::PublishBuild")] purpose: OtpPurposeGql,
    ) -> GqlResult<OtpRequestResult> {
        let me = viewer_publisher(ctx).await?;
        let state = state();
        let purpose: OtpPurpose = purpose.into();

        if OtpMethod::parse(&me.otp_method)? != OtpMethod::Email {
            return Err(AppError::Validation(
                "This account uses an authenticator app. Read the current code from it.".into(),
            )
            .into());
        }

        let code = state
            .otp
            .issue_email_code(&state.db, &me.id, purpose)
            .await?;

        state
            .mailer
            .send_otp(&me.email, &code, state.otp.email_ttl_secs(), purpose)
            .await?;

        Ok(OtpRequestResult {
            sent_to: mask_email(&me.email),
            expires_in_secs: state.otp.email_ttl_secs(),
        })
    }

    // -- Applications -------------------------------------------------------

    /// Register an application id. Must exist before builds can be uploaded.
    async fn create_application(
        &self,
        ctx: &Context<'_>,
        input: CreateApplicationInput,
    ) -> GqlResult<Application> {
        let me = viewer_publisher(ctx).await?;
        let state = state();

        let app_id = validate_app_id(&input.app_id)?;
        let name = require_text(&input.name, "name")?;
        let summary = require_text(&input.summary, "summary")?;

        if find_application(&app_id).await?.is_some() {
            return Err(AppError::Conflict(format!(
                "'{app_id}' is already registered on this server."
            ))
            .into());
        }

        let mut create = state
            .db
            .application()
            .create()
            .set("id", models::new_id())
            .set("app_id", app_id)
            .set("name", name)
            .set("summary", summary)
            .set("description", input.description.trim().to_string())
            .set("publisher_id", me.id.clone())
            .set("created_at", models::now_secs());

        if let Some(homepage) = clean_optional(input.homepage) {
            create = create.set("homepage", homepage);
        }
        if let Some(license) = clean_optional(input.license) {
            create = create.set("license", license);
        }

        // Checked before the row exists, so a rejected gallery does not leave a
        // registered application that can never publish. The format and count
        // rules are applied later, in `store_gallery`.
        if input.screenshots.is_empty() {
            return Err(AppError::Validation(
                "At least one screenshot is required to register an application.".into(),
            )
            .into());
        }

        let created = create.exec().await.gql()?;

        // If storing the gallery fails, the application must not survive: it
        // would be registered, unillustrated, and permanently unable to publish.
        if let Err(e) = store_gallery(ctx, &created.id, &created.app_id, &input.screenshots).await {
            state
                .db
                .application()
                .delete_many()
                .r#where(application::id::equals(created.id.clone()))
                .exec()
                .await
                .gql()?;
            return Err(e.into());
        }

        Ok(Application::from(&created))
    }

    /// Replace an application's gallery.
    ///
    /// Takes the complete new set, so the "at least one screenshot" rule is
    /// checked against what the application will actually end up with.
    async fn replace_screenshots(
        &self,
        ctx: &Context<'_>,
        input: ReplaceScreenshotsInput,
    ) -> GqlResult<Application> {
        let me = viewer_publisher(ctx).await?;
        let app = owned_application(&input.app_id, &me).await?;

        store_gallery(ctx, &app.id, &app.app_id, &input.screenshots).await?;

        Ok(Application::from(&app))
    }

    /// Edit an application's metadata. Its `appId` is immutable — it is baked
    /// into every published ref.
    async fn update_application(
        &self,
        ctx: &Context<'_>,
        app_id: String,
        input: UpdateApplicationInput,
    ) -> GqlResult<Application> {
        let me = viewer_publisher(ctx).await?;
        let existing = owned_application(&app_id, &me).await?;
        let state = state();

        let mut update = state
            .db
            .application()
            .update_many()
            .r#where(application::id::equals(existing.id.clone()));

        let mut changed = false;
        if let Some(name) = input.name {
            update = update.set("name", require_text(&name, "name")?);
            changed = true;
        }
        if let Some(summary) = input.summary {
            update = update.set("summary", require_text(&summary, "summary")?);
            changed = true;
        }
        if let Some(description) = input.description {
            update = update.set("description", description.trim().to_string());
            changed = true;
        }
        if let Some(homepage) = input.homepage {
            update = update.set("homepage", homepage.trim().to_string());
            changed = true;
        }
        if let Some(license) = input.license {
            update = update.set("license", license.trim().to_string());
            changed = true;
        }

        if changed {
            update.exec().await.gql()?;
        }

        let refreshed = find_application(&existing.app_id)
            .await?
            .ok_or_else(|| AppError::Internal("application vanished mid-update".into()))?;
        Ok(Application::from(&refreshed))
    }

    // -- Builds -------------------------------------------------------------

    /// Publish a `.flatpak` bundle to the repository.
    ///
    /// Requires a one-time password, verified before the bundle is imported into
    /// the repository or archived.
    ///
    /// The bundle itself is already on disk by then: a single-request multipart
    /// upload carries its bytes with it, and the transport has to spool them
    /// before the resolver exists to check anything. What that write costs is
    /// bounded — an anonymous request is refused before anything is written, and
    /// the size ceiling is applied at the transport, not here.
    ///
    /// The bundle's own ref is authoritative. `appId` says which application
    /// record the build is filed under, and the import is rejected if the two
    /// disagree, so a publisher cannot smuggle a bundle for one application into
    /// another's namespace.
    ///
    /// From `curl`:
    ///
    /// ```sh
    /// curl -X POST https://flatnexus.example/graphql \
    ///   -H "Authorization: Bearer $TOKEN" \
    ///   -F operations='{"query":"mutation($i:PublishBuildInput!){publishBuild(input:$i){id flatpakRef commit}}","variables":{"i":{"appId":"org.example.App","version":"1.2.0","otp":"123456","bundle":null}}}' \
    ///   -F map='{"0":["variables.i.bundle"]}' \
    ///   -F 0=@app.flatpak
    /// ```
    async fn publish_build(
        &self,
        ctx: &Context<'_>,
        input: PublishBuildInput,
    ) -> GqlResult<Build> {
        let me = viewer_publisher(ctx).await?;
        let state = state();

        let app = owned_application(&input.app_id, &me).await?;

        // Re-checked here, not just at registration: the gallery can be replaced
        // between the two, and a published application with no screenshot is one
        // nobody can evaluate before installing.
        let gallery = state
            .db
            .screenshot()
            .find_many()
            .r#where(screenshot::application_id::equals(app.id.clone()))
            .exec()
            .await
            .gql()?;

        if !gallery.iter().any(|item| item.format != "webm") {
            return Err(AppError::Validation(
                "This application needs at least one screenshot before a build can be \
                 published. A video on its own does not count."
                    .into(),
            )
            .into());
        }

        // Gate next. Nothing below this line is reachable without a valid,
        // single-use code.
        state
            .otp
            .verify(&state.db, &me, OtpPurpose::PublishBuild, &input.otp)
            .await?;

        // The HTTP layer already wrote this part to disk, once, before the
        // resolver ran — the `Upload` scalar is an index into that spool. Hashed
        // in place rather than copied again.
        let registry = ctx
            .data::<UploadRegistry>()
            .map_err(|_| AppError::Internal("upload registry missing from request".into()))?;

        let path = registry.path(input.bundle.0).ok_or_else(|| {
            AppError::UploadRejected("The bundle part was not uploaded.".into())
        })?;

        let spooled = bundles::hash_in_place(path, state.config.max_bundle_bytes).await?;

        // Staged import: validates the bundle and its ref before the live
        // repository is touched at all.
        let imported = state
            .repo
            .import_bundle(spooled.path(), &app.app_id)
            .await?;

        // Archived only after the import succeeded: a bundle the repository
        // rejected is not an artifact worth keeping, and uploading first would
        // fill the bucket with objects no build row ever references.
        // From here the live repository already carries the ref. Anything that
        // fails below must put it back, or the build is pullable by every client
        // while having no row — invisible in queries, and unwithdrawable through
        // `deleteBuild`, which looks builds up by row id.
        let bundle_key = BundleStore::key_for(spooled.sha256(), &imported);
        if let Err(e) = state
            .bundles
            .archive(&bundle_key, spooled.path(), spooled.size_bytes())
            .await
        {
            rollback_ref(&imported.flatpak_ref).await;
            return Err(e.into());
        }

        let now = models::now_secs();
        let version = input.version.trim().to_string();

        // Re-publishing a ref supersedes the previous build for it, matching the
        // repository, where the ref now points at the new commit. The superseded
        // rows are read first so their archived objects can be reclaimed —
        // dropping the rows alone would leave exactly the orphaned bucket
        // objects the archive-after-import ordering exists to prevent.
        let superseded = state
            .db
            .build()
            .find_many()
            .r#where(build::flatpak_ref::equals(imported.flatpak_ref.clone()))
            .exec()
            .await
            .gql()?;

        state
            .db
            .build()
            .delete_many()
            .r#where(build::flatpak_ref::equals(imported.flatpak_ref.clone()))
            .exec()
            .await
            .gql()?;

        for old in &superseded {
            // Identical bytes produce an identical key, so a re-upload of the
            // same bundle must not delete the object the new row points at.
            if old.bundle_key == bundle_key {
                continue;
            }
            // Best-effort: the new build is already live, and failing the
            // mutation over a stale object would be a worse outcome than leaving
            // one behind. Logged so it can be reclaimed.
            if let Err(e) = state.bundles.remove(&old.bundle_key).await {
                armature_log::warn!(
                    "could not remove superseded bundle {}: {e}",
                    old.bundle_key
                );
            }
        }

        let created = match state
            .db
            .build()
            .create()
            .set("id", models::new_id())
            .set("application_id", app.id.clone())
            .set("flatpak_ref", imported.flatpak_ref.clone())
            .set("arch", imported.arch.clone())
            .set("branch", imported.branch.clone())
            .set("version", version)
            .set("commit_id", imported.commit.clone())
            .set("bundle_key", bundle_key)
            .set("size_bytes", spooled.size_bytes() as i64)
            .set("sha256", spooled.sha256().to_string())
            .set("published_by", me.id.clone())
            .set("published_at", now)
            .exec()
            .await
        {
            Ok(created) => created,
            Err(e) => {
                rollback_ref(&imported.flatpak_ref).await;
                return Err(AppError::from(e).into());
            }
        };

        armature_log::info!(
            "published {} ({}) by {}",
            imported.flatpak_ref,
            imported.commit,
            me.email
        );

        Ok(Build::from(&created))
    }

    /// Withdraw a build. Also requires a one-time password.
    async fn delete_build(&self, ctx: &Context<'_>, id: ID, otp: String) -> GqlResult<bool> {
        let me = viewer_publisher(ctx).await?;
        let state = state();

        let target = state
            .db
            .build()
            .find_first()
            .r#where(build::id::equals(id.to_string()))
            .exec()
            .await
            .gql()?
            .ok_or_else(|| AppError::NotFound("No such build.".to_string()))?;

        let app = ctx
            .data::<Loaders>()?
            .applications
            .load_one(target.application_id.clone())
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::Internal("build references a missing application".into()))?;

        if app.publisher_id != me.id && !me.is_admin {
            return Err(AppError::Forbidden("You do not own this application.".into()).into());
        }

        state
            .otp
            .verify(&state.db, &me, OtpPurpose::DeleteBuild, &otp)
            .await?;

        state.repo.remove_ref(&target.flatpak_ref).await?;

        // Ordered after the ref removal and before the row delete: if this
        // fails, the build is already unreachable to clients and the row still
        // names the key, so the object can be reclaimed by hand. Dropping the
        // row first would strand the object with nothing pointing at it.
        state.bundles.remove(&target.bundle_key).await?;

        state
            .db
            .build()
            .delete_many()
            .r#where(build::id::equals(target.id.clone()))
            .exec()
            .await
            .gql()?;

        armature_log::info!("withdrew {} by {}", target.flatpak_ref, me.email);
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn issue_session(row: &models::Publisher) -> GqlResult<AuthPayload> {
    let state = state();
    let issued = state.tokens.issue(&row.id, &row.email)?;
    Ok(AuthPayload {
        token: issued.token,
        expires_at: issued.expires_at,
        viewer: Viewer::from(row),
    })
}

/// Validate, store, and persist a whole gallery for an application.
///
/// Replaces whatever was there. The count and format rules live in
/// [`crate::screenshots`]; this only moves bytes and rows.
///
/// Objects are content-addressed, so re-submitting an unchanged gallery
/// re-uploads nothing. Objects belonging to items that are gone are removed
/// afterwards, and only when no surviving item still points at the same key —
/// two gallery slots can legitimately hold identical bytes.
async fn store_gallery(
    ctx: &Context<'_>,
    application_id: &str,
    app_id: &str,
    uploads: &[Upload],
) -> Result<(), AppError> {
    let state = state();

    let registry = ctx
        .data::<UploadRegistry>()
        .map_err(|_| AppError::Internal("upload registry missing from request".into()))?;

    // Read every file before validating any, so a gallery is accepted or
    // rejected whole rather than half-written.
    let mut raw = Vec::with_capacity(uploads.len());
    for upload in uploads {
        let path = registry.path(upload.0).ok_or_else(|| {
            AppError::UploadRejected("A screenshot part was not uploaded.".into())
        })?;
        raw.push(tokio::fs::read(path).await?);
    }

    let validated = screenshots::validate_all(raw)?;

    let existing = state
        .db
        .screenshot()
        .find_many()
        .r#where(screenshot::application_id::equals(application_id.to_string()))
        .exec()
        .await?;

    let now = models::now_secs();
    let mut kept_keys = Vec::with_capacity(validated.len());

    for item in &validated {
        let sha256 = hex::encode(Sha256::digest(&item.bytes));
        let key = screenshots::key_for(app_id, &sha256, item.format);

        state
            .bundles
            .put_small(&key, item.bytes.clone(), item.format.content_type())
            .await?;

        kept_keys.push(key.clone());

        state
            .db
            .screenshot()
            .create()
            .set("id", models::new_id())
            .set("application_id", application_id.to_string())
            .set("object_key", key)
            .set("format", item.format.extension().to_string())
            .set("position", item.position)
            .set("size_bytes", item.size_bytes() as i64)
            .set("sha256", sha256)
            .set("created_at", now)
            .exec()
            .await?;
    }

    // Rows first: a stale row pointing at a live object is invisible clutter,
    // whereas a live row pointing at a deleted object is a broken gallery.
    for old in &existing {
        state
            .db
            .screenshot()
            .delete_many()
            .r#where(screenshot::id::equals(old.id.clone()))
            .exec()
            .await?;
    }

    for old in &existing {
        if kept_keys.iter().any(|key| key == &old.object_key) {
            continue;
        }
        if let Err(e) = state.bundles.remove(&old.object_key).await {
            armature_log::warn!("could not remove old screenshot {}: {e}", old.object_key);
        }
    }

    Ok(())
}

/// Withdraw a ref that was published but whose record could not be completed.
///
/// Best-effort and logged: the caller is already returning the failure that
/// prompted it, and a failed rollback must not replace that error with a less
/// useful one. What it must not do is stay silent — a ref left behind here is
/// serving content nothing in the database knows about.
async fn rollback_ref(flatpak_ref: &str) {
    if let Err(e) = state().repo.remove_ref(flatpak_ref).await {
        armature_log::error!(
            "could not roll back {flatpak_ref} after a failed publish; it is live in \
             the repository with no build row and must be removed by hand: {e}"
        );
    }
}

async fn reload(publisher_id: &str) -> Result<models::Publisher, AppError> {
    let state = state();
    state
        .db
        .publisher()
        .find_first()
        .r#where(publisher::id::equals(publisher_id.to_string()))
        .exec()
        .await?
        .ok_or(AppError::Unauthenticated)
}

async fn find_application(app_id: &str) -> Result<Option<models::Application>, AppError> {
    let state = state();
    Ok(state
        .db
        .application()
        .find_first()
        .r#where(application::app_id::equals(app_id.to_string()))
        .exec()
        .await?)
}

/// Fetch an application the caller is allowed to write to.
async fn owned_application(
    app_id: &str,
    me: &models::Publisher,
) -> Result<models::Application, AppError> {
    let found = find_application(app_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("No application '{app_id}'.")))?;

    if found.publisher_id != me.id && !me.is_admin {
        return Err(AppError::Forbidden(
            "You do not own this application.".into(),
        ));
    }
    Ok(found)
}

/// Validate a reverse-DNS Flatpak application id.
///
/// This is checked here as well as against the bundle because the record is
/// created before any bundle exists, and a malformed id would produce an
/// application that can never accept an upload.
fn validate_app_id(raw: &str) -> Result<String, AppError> {
    let app_id = raw.trim();

    let invalid = || {
        AppError::Validation(format!(
            "'{app_id}' is not a valid application id. Expected reverse-DNS form, \
             e.g. org.example.MyApp."
        ))
    };

    if app_id.len() < 3 || app_id.len() > 255 {
        return Err(invalid());
    }

    let segments: Vec<&str> = app_id.split('.').collect();
    if segments.len() < 3 {
        return Err(invalid());
    }

    for segment in &segments {
        if segment.is_empty() {
            return Err(invalid());
        }
        // A leading digit is invalid per the Flatpak/D-Bus name rules.
        if segment.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Err(invalid());
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(invalid());
        }
    }

    Ok(app_id.to_string())
}

fn require_text(raw: &str, field: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("A {field} is required.")));
    }
    Ok(trimmed.to_string())
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::validate_app_id;

    #[test]
    fn accepts_conventional_ids() {
        for good in [
            "org.gnome.Calculator",
            "com.example.my_app",
            "io.github.user-name.App",
        ] {
            assert!(validate_app_id(good).is_ok(), "{good} should be accepted");
        }
    }

    #[test]
    fn rejects_ids_that_could_never_match_a_ref() {
        for bad in [
            "",
            "app",
            "org.gnome",
            "org..Calculator",
            "org.gnome.Calc/ulator",
            "org.2gnome.Calculator",
            "org.gnome.Calc ulator",
        ] {
            assert!(validate_app_id(bad).is_err(), "{bad} should be rejected");
        }
    }
}
