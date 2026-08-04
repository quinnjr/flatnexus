# FlatNexus

A self-hosted Flatpak distribution point. Publishers upload `.flatpak` bundles
through a GraphQL API; the server imports them into an OSTree repository and
serves that repository over plain HTTP, so the `flatpak` CLI installs and
updates from it with no client-side changes.

```
flatnexus-backend/    Rust — Armature + armature-graphql, Prax ORM, OSTree
flatnexus-frontend/   SvelteKit — browse, publish, second-factor settings
```

## What it does

| | |
|---|---|
| **API** | GraphQL only. There is no REST surface. |
| **Auth** | Email/password with a mandatory second factor, plus GitHub and Google sign-in. |
| **Publishing** | Every build upload requires a one-time password. |
| **Distribution** | An `archive`-mode OSTree repo served as static files at `/repo`. |
| **Bundle archive** | Uploaded `.flatpak` bundles go to a DigitalOcean Spaces bucket. |
| **Screenshots** | Every application needs at least one; WebM clips optional. |
| **Discovery** | SSR pages, JSON-LD, `sitemap.xml`, `robots.txt`, `llms.txt`. |

The non-GraphQL HTTP endpoints exist only because their clients cannot speak
GraphQL: `libostree` fetching repository objects, the browser performing OAuth
redirects, and orchestrator health probes.

## Requirements

- Rust 1.94.1+ (the MSRV of `armature-core` 0.8)
- PostgreSQL 14+
- A DigitalOcean Spaces bucket (or any S3-compatible endpoint — `docker-compose`
  provides MinIO for local development)
- `ostree` and `flatpak` on `PATH` — the server drives the real CLIs rather than
  reimplementing them
- Node 20+ and pnpm 11+ for the frontend

```sh
# Arch
sudo pacman -S ostree flatpak postgresql
# Debian/Ubuntu
sudo apt install ostree flatpak postgresql
```

## Running it

```sh
# 1. Dependencies: Postgres, MinIO (stands in for Spaces), MailHog
docker compose up -d
for m in flatnexus-backend/migrations/*.sql; do psql "$DATABASE_URL" -f "$m"; done

# 2. Backend
cd flatnexus-backend
cp .env.example .env
# Set at minimum:
#   DATABASE_URL
#   FLATNEXUS_JWT_SECRET          openssl rand -hex 32
#   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY   (your Spaces key pair)
#   FLATNEXUS_SPACES_BUCKET / _REGION
# For the docker-compose MinIO instead of real Spaces:
#   AWS_ACCESS_KEY_ID=flatnexus
#   AWS_SECRET_ACCESS_KEY=flatnexus-dev-secret
#   FLATNEXUS_SPACES_ENDPOINT=http://localhost:9000
cargo run

# 3. Frontend
cd ../flatnexus-frontend
cp .env.example .env
pnpm install
pnpm dev
```

The backend listens on `:4000`, the frontend on `:5173`.

| URL | |
|---|---|
| `http://localhost:4000/graphql` | the API |
| `http://localhost:4000/playground` | GraphiQL |
| `http://localhost:4000/graphql/schema` | SDL |
| `http://localhost:4000/repo` | the OSTree repository |
| `http://localhost:4000/flatnexus.flatpakrepo` | remote definition |
| `http://localhost:5173/llms.txt` | AI-readable catalog index |
| `http://localhost:5173/sitemap.xml` | generated sitemap |

### A note on the Armature dependency

`flatnexus-backend` depends on the individual `armature-*` crates from crates.io,
not on the `armature-framework` meta-crate. Two things make that necessary:

- **The meta-crate lags its siblings.** `armature-framework 0.5.0` resolves
  `armature-auth 0.1.2`, `armature-graphql 0.2.0` and `armature-jwt 0.1.1`, so
  pulling it in alongside the current crates puts two incompatible copies of
  `armature-core` in one graph. The prelude it exports is replaced by
  `src/prelude.rs`, which is a dozen lines of re-exports.

- **`rust-version` is load-bearing.** Edition 2024 selects Cargo's MSRV-aware
  resolver. `armature-core 0.8` declares an MSRV of 1.94.1, while every armature
  crate requires it as `armature-core = "0"` — so a crate declaring a lower
  `rust-version` makes the resolver satisfy those with `armature-core 0.2.3`
  (MSRV 1.89) while an explicit `armature-core = "0.8"` still pulls 0.8.2. The
  result is a split graph in which `armature-auth 0.2.0` fails to compile against
  `armature_core::crypto`, which does not exist in 0.2.3. `rust-version` here is
  therefore pinned to `1.94.1`; lowering it silently breaks the build.

Both are upstream packaging issues rather than anything this project needs to
carry forever. Once `armature-framework` depends on its current siblings, this
can collapse back to a single `armature-framework = "0.x"` dependency.

## Using the repository

```sh
flatpak remote-add --if-not-exists flatnexus http://localhost:4000/flatnexus.flatpakrepo
flatpak install flatnexus org.example.MyApp
flatpak update
```

## Publishing a build

### From the web UI

1. Register an application id at `/dashboard/apps/new`, **with at least one
   screenshot**.
2. Choose a second factor at `/settings/security`.
3. Upload the bundle at `/dashboard/apps/<appId>/upload`, with a one-time code.

### From curl

Uploads use the [GraphQL multipart request spec][spec], so the same mutation
works from a shell. Use the **authenticator** OTP method for this — an emailed
code cannot be fetched by a script.

Sign-in is two calls, because a password alone does not yield a session:

```sh
API=http://localhost:4000/graphql

# 1. Password → challenge.
CHALLENGE=$(curl -s $API -H 'Content-Type: application/json' \
  -d '{"query":"mutation($i:LoginInput!){login(input:$i){status challenge{challengeToken}}}",
       "variables":{"i":{"email":"dev@example.com","password":"..."}}}' \
  | jq -r .data.login.challenge.challengeToken)

# 2. Challenge + current authenticator code → session token.
TOKEN=$(curl -s $API -H 'Content-Type: application/json' \
  -d "{\"query\":\"mutation(\$c:String!,\$o:String!){verifyLogin(challengeToken:\$c,otp:\$o){token}}\",
       \"variables\":{\"c\":\"$CHALLENGE\",\"o\":\"$(oathtool --totp -b $TOTP_SECRET)\"}}" \
  | jq -r .data.verifyLogin.token)
```

Then publish. The upload needs a *different* code: the TOTP replay ledger is
per-publisher, so the 30-second step that signed you in is burned. A script that
signs in and uploads back-to-back must wait for the next step — around 30
seconds — or it will be rejected as a replay.

```sh
curl -X POST $API \
  -H "Authorization: Bearer $TOKEN" \
  -F operations='{"query":"mutation($i:PublishBuildInput!){publishBuild(input:$i){id flatpakRef commit sha256}}","variables":{"i":{"appId":"org.example.MyApp","version":"1.0.0","otp":"123456","bundle":null}}}' \
  -F map='{"0":["variables.i.bundle"]}' \
  -F 0=@myapp.flatpak
```

`oathtool` (from `oath-toolkit`) generates a TOTP code from the base32 secret
shown once at enrollment. Build a bundle to upload with:

```sh
flatpak build-bundle ~/.local/share/flatpak/repo myapp.flatpak org.example.MyApp
```

[spec]: https://github.com/jaydenseric/graphql-multipart-request-spec

## How sign-in and uploads are secured

**A password alone is never enough to sign in, and a session alone is never
enough to publish.** Sign-in is two steps, and `publishBuild` / `deleteBuild`
require a further one-time password on top of a valid session — so neither a
leaked password nor a stolen token is sufficient to push code to everyone who
added the remote.

### Two-step sign-in

`login` takes an email and password and returns a **challenge**, not a session.
`verifyLogin` redeems that challenge with a one-time password and returns the
token. For a publisher on the emailed-code method the code is sent as part of
`login`, so there is no separate request step to forget.

Withdrawing a build needs its own code: emailed codes are scoped by purpose, so
call `requestUploadOtp(purpose: DELETE_BUILD)` before `deleteBuild`.

Both tokens are signed with the same key and distinguished by a `typ` claim that
each verifier requires. Without it the challenge token handed out after step one
would be a valid session token, and the second factor would be skippable by
anyone who read the response body. There are unit tests for exactly that.

One deliberate exception: an account that selected TOTP but never confirmed the
enrollment has no usable factor, so `login` completes on the password alone and
logs a warning. Refusing would lock the publisher out of the settings page they
need in order to fix it, and `publishBuild` still demands an OTP, so this cannot
be used to reach a privileged operation.

OAuth sign-in is not gated by a FlatNexus OTP: GitHub and Google performed the
authentication, including whatever second factor the publisher has configured
there. If you want a FlatNexus-owned factor on that path too, gate the callback
in `src/http/oauth_controller.rs` the same way `login` is gated.

Two methods, per publisher, used for both sign-in and publishing:

- **TOTP** (RFC 6238) — read off an authenticator app. Works unattended from
  `curl`, which is what makes CI publishing possible.
- **Emailed code** — sent automatically during sign-in, and requested per
  privileged operation via `requestUploadOtp(purpose:)`. Valid for ten minutes.

Both are strictly single-use. Emailed codes are stored only as a SHA-256 hash and
claimed by a single conditional `UPDATE` whose predicate carries the hash, the
purpose, the expiry and `consumed = false` — so the check and the claim are one
statement, and a code cannot expire or be spent by a concurrent request in
between. TOTP codes are burned by recording their
30-second time-step in a unique-constrained table — without that, a captured
code stays valid for the remainder of its step and could be replayed inside it.

The OTP is verified **before** the bundle is imported into the repository or
archived. It is not verified before the bundle reaches disk, and cannot be: a
single-request multipart upload carries its bytes with it, and the transport has
to spool them before a resolver exists to check anything. What that costs is
bounded instead — an anonymous request is refused before a single byte is
written, and the size ceiling is applied at the transport rather than in the
resolver.

### Where a bundle ends up

Two destinations, deliberately:

| | |
|---|---|
| **OSTree repository** (server disk, `/repo`) | What clients install from. `libostree` fetches thousands of small objects per install and expects them at stable paths under one origin. |
| **Spaces bucket** | The archive of record: the original bundle the publisher produced, kept so a build can be re-verified against its `sha256` or re-imported later. |

Bucket keys are scoped per application:

```text
<prefix>/<app-id>/<sha256>-<arch>-<branch>.flatpak
```

Scoping by application id makes a per-application lifecycle rule, size audit, or
bulk deletion a prefix operation rather than a full-bucket scan. The digest leads
the filename, so the archive is content-addressed — re-uploading identical bytes
resolves to the same object instead of a second copy.

The upload is a real S3 multipart upload, with each part streamed as a byte
*range of the spooled file*. At most one 16 MiB part is in memory at a time, so
archival costs the same whether the bundle is 20 MiB or a gigabyte. Every failure
path aborts the upload explicitly — abandoned parts are billed by Spaces and are
invisible to `ListObjects`, so leaving them to a lifecycle rule that may not
exist is not good enough.

Archival happens **after** the staged import succeeds. A bundle the repository
rejected is not an artifact worth keeping, and uploading first would fill the
bucket with objects no build row ever references. `deleteBuild` removes the ref,
then the object, then the row — in that order, so a failure mid-way leaves the
build unreachable to clients with the row still naming the key, rather than
stranding an object nothing points at.

The `Build.bundleUrl` GraphQL field resolves a presigned, time-limited link to
the archived bundle. It is signed rather than public because the bucket holds
every build ever uploaded, including withdrawn ones, and a permanently readable
object is one a withdrawal cannot actually recall. Setting
`FLATNEXUS_SPACES_CDN_ENDPOINT` switches to unsigned CDN URLs — only correct for
a bucket you have deliberately made public, since a CDN would otherwise cache one
user's signed URL and keep serving it past expiry.

## Screenshots

An application cannot be registered without at least one screenshot, and the rule
is re-checked before every publish — the gallery can be replaced in between, and a
published application nobody can look at is one nobody will install.

| | |
|---|---|
| **Stills** | PNG, JPEG, WebP · 5 MiB each |
| **Video** | WebM · 15 MiB · optional |
| **Count** | 8 gallery items maximum |

A WebM clip is permitted alongside stills but **does not satisfy the
requirement**: it cannot be a thumbnail, will not render in a link preview, and
leaves a client that cannot play it with nothing.

**Type is sniffed, never trusted.** The declared `Content-Type` and the filename
are both client-controlled, so neither decides anything — the format comes from
the file's leading bytes, and the object is served with the content type that
sniffing produced. SVG is rejected outright: it is a scriptable document, and
serving one from the origin that holds sessions is an XSS vector rather than an
image. Two container ambiguities a naive check gets wrong are covered by tests —
a WAV file also begins `RIFF`, and Matroska shares WebM's EBML signature.

Media lives in the bundle bucket under `screenshots/<app-id>/<sha256>.<ext>`,
content-addressed like the bundles, and is served through short-lived signed URLs
so an application's artwork stops being reachable when the application does.

`replaceScreenshots` takes the *whole* new gallery rather than editing items
individually, so the "at least one screenshot" rule is checked against exactly
what the application will end up with.

Registration is therefore a multipart request too:

```sh
curl -X POST $API \
  -H "Authorization: Bearer $TOKEN" \
  -F operations='{"query":"mutation($i:CreateApplicationInput!){createApplication(input:$i){appId}}","variables":{"i":{"appId":"org.example.MyApp","name":"My App","summary":"Does a thing.","screenshots":[null,null]}}}' \
  -F map='{"0":["variables.i.screenshots.0"],"1":["variables.i.screenshots.1"]}' \
  -F 0=@shot-1.png \
  -F 1=@demo.webm
```

### Staged imports

An uploaded bundle is never imported straight into the live repository. It goes
into a throwaway staging repo first, so its actual ref and commit can be read
back and checked against the application it was filed under. A bundle for
`org.evil.Thing` uploaded under an `org.trusted.App` record is rejected while
still in staging, and the live repository is never mutated.

### TOTP re-enrollment

Enrolling an authenticator stages the new secret as *pending*; it replaces the
live one only when `confirmTotp` sees a code from it. So opening a re-enrollment
and abandoning it cannot downgrade an account that already had working TOTP.

### GPG signing

Unsigned by default, which means clients cannot verify what they install. For
anything real:

```sh
gpg --quick-generate-key "FlatNexus <repo@example.com>" default default never
gpg --armor --export <KEY_ID> > flatnexus-backend/data/repo.gpg.asc
# then set FLATNEXUS_GPG_KEY_ID and FLATNEXUS_GPG_PUBLIC_KEY_PATH
```

The public key is then advertised in the `.flatpakrepo` and `.flatpakref` files
automatically.

## OAuth setup

Both providers are optional; one with no credentials configured is simply not
advertised to the frontend.

| Provider | Callback URL |
|---|---|
| GitHub | `${FLATNEXUS_PUBLIC_URL}/auth/oauth/github/callback` |
| Google | `${FLATNEXUS_PUBLIC_URL}/auth/oauth/google/callback` |

```sh
FLATNEXUS_GITHUB_CLIENT_ID=...
FLATNEXUS_GITHUB_CLIENT_SECRET=...
FLATNEXUS_GOOGLE_CLIENT_ID=...
FLATNEXUS_GOOGLE_CLIENT_SECRET=...
```

Accounts are keyed on `(provider, subject)`, never on email — a provider email
can be changed by its owner, and treating it as the identity would let whoever
later acquires an abandoned address inherit a publisher account. A verified
provider email is used only to link a first-time OAuth login to an existing
local account.

## Tests

```sh
cd flatnexus-backend  && cargo test   # 64 unit tests, no database or bucket required
cd flatnexus-frontend && pnpm test    # 25 unit tests
pnpm run check                        # svelte-check, 0 errors
```

## Discovery: SEO and AEO

The public pages are server-rendered through `+page.ts` load functions rather
than fetched in `onMount`. This is the part that matters most: a crawler — and
an answer engine, which generally does not execute JavaScript at all — otherwise
receives an empty shell, and no amount of metadata makes an empty page rank or
get cited.

**Per-page metadata** is produced by `src/lib/seo/` and rendered by one `<Seo />`
component, so no route can half-describe itself. Everything behind
authentication is `noindex`.

**Structured data** (`src/lib/seo/schema.ts`) emits JSON-LD whose `@id` always
equals the page's canonical URL, so documents about the same thing merge rather
than compete:

| Page | Schema |
|---|---|
| Home | `WebSite` (+ `SearchAction`), `CollectionPage`/`ItemList`, `DataCatalog`, `FAQPage` |
| Application | `SoftwareApplication`, `BreadcrumbList`, `HowTo` (add remote → install) |

`SoftwareApplication` is the type that earns a citation: it lets an answer engine
say "yes, that is installable, here is the command" instead of paraphrasing a
page — and a paraphrased install command does not work.

**Answer-engine endpoints:**

| URL | |
|---|---|
| `/robots.txt` | Allows GPTBot, ClaudeBot, PerplexityBot and friends *explicitly*, so access survives a future default-deny rule. Private paths disallowed. |
| `/sitemap.xml` | Generated from the live catalog; `lastmod` from the newest build. |
| `/llms.txt` | The AI-readable index, per [llmstxt.org](https://llmstxt.org). |
| `/llms-full.txt` | The whole catalog as markdown in one fetch. Truncation, if any, is stated in the output rather than left silent. |
| `/apps/<app-id>.md` | One application as plain markdown, linked from the HTML page via `rel="alternate"`. |

The `FAQPage` markup on the home page describes three questions that the page
also renders visibly. Structured data for content a reader cannot see is a
guidelines violation and gets the markup ignored rather than rewarded — if you
edit one, edit both.

Canonicals, JSON-LD `@id`s, the sitemap and `llms.txt` are all built from the
**request** origin, falling back to `PUBLIC_SITE_URL` when there isn't one (the
`.md` and `.txt` endpoints, and any non-request context). That keeps a preview
deployment self-consistent instead of emitting production URLs. Set
`PUBLIC_SITE_URL` to the origin users actually reach, and make sure any reverse
proxy in front sets the forwarded-host headers SvelteKit honours — otherwise the
internal origin is what gets published.

## Known limits

- **The request body is buffered in memory, twice.** Armature reads a request
  body in full before a handler runs, and its multipart parser then copies each
  part out of that buffer. Peak usage is therefore about *twice*
  `FLATNEXUS_MAX_BUNDLE_BYTES` (default 1 GiB) per concurrent upload. Nothing
  below that adds to it — the bundle is spooled to disk once, hashed in place,
  and archived by streaming byte ranges of that same file — but both copies are
  upstream of anything this codebase controls, and they are why the default limit
  is deliberately modest.
- **Repository objects are served buffered, not streamed.** `serve_file` reads a
  file into memory before responding. Armature route handlers can only return
  `HttpResponse`, so its `StreamingResponse` type is not reachable from one.
  Static deltas — by far the largest files a repository holds — are no longer
  generated on the request path for this reason, but if you generate them out of
  band, size the process for concurrent delta fetches.
- **Static deltas are not generated automatically.** `flatpak build-update-repo
  --generate-static-deltas` costs O(whole repository) and used to run inside every
  publish. Installs and updates work without deltas; run it on a schedule if you
  want them.
- **Search filters in memory.** Prax `WhereParam`s compose with `AND`, so an
  `OR` across id/name/summary is not expressible in the query; a search request
  fetches and filters. Fine at this scale, not at ten thousand applications.
- **Withdrawn builds leave their objects behind.** `deleteBuild` removes the ref,
  which is what makes it disappear for clients. Reclaiming disk needs a periodic
  `ostree prune`.
- **No CLI.** Out of scope for now; the `curl` recipe above covers the same
  ground.
- **`llms-full.txt` is capped** at 250 applications per request. Past that it
  says so in its own output rather than silently truncating.
