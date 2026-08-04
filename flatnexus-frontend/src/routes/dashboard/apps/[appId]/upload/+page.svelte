<script lang="ts">
  /**
   * Build upload.
   *
   * The OTP field is not optional and is not a formality: the server verifies it
   * before the bundle is imported, and rejects the upload outright if it is
   * missing, wrong, expired, or already spent.
   */
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { gql, gqlUpload, GraphQLRequestError, API_URL } from '$lib/graphql/client';
  import {
    ME,
    PUBLISH_BUILD,
    REQUEST_UPLOAD_OTP,
    type Build,
    type Viewer
  } from '$lib/graphql/operations';
  // `liveSession` is null once the token has expired, not merely when it is
  // absent — a session that lapses with the tab open must not keep rendering
  // the signed-in branch.
  import { liveSession as session } from '$lib/stores/session';
  import { formatBytes } from '$lib/format';
  import Alert from '$lib/components/Alert.svelte';
  import Seo from '$lib/components/Seo.svelte';
  import { privateMeta } from '$lib/seo/meta';
  import CommandBlock from '$lib/components/CommandBlock.svelte';

  const appId = page.params.appId;

  let viewer = $state<Viewer | null>(null);
  let bundle = $state<File | null>(null);
  let version = $state('');
  let otp = $state('');
  let uploading = $state(false);
  let progress = $state(0);
  let error = $state('');
  let notice = $state('');
  let published = $state<Build | null>(null);

  onMount(async () => {
    if (!$session) return;
    try {
      const data = await gql<{ me: Viewer | null }>(ME);
      viewer = data.me;
    } catch {
      // The form still works; only the "request a code" affordance depends on
      // knowing which method the account uses.
      viewer = null;
    }
  });

  function pickFile(event: Event) {
    const input = event.target as HTMLInputElement;
    bundle = input.files?.[0] ?? null;
  }

  async function requestCode() {
    error = '';
    notice = '';
    try {
      const data = await gql<{ requestUploadOtp: { sentTo: string; expiresInSecs: number } }>(
        REQUEST_UPLOAD_OTP
      );
      const minutes = Math.max(1, Math.round(data.requestUploadOtp.expiresInSecs / 60));
      notice = `Code sent to ${data.requestUploadOtp.sentTo}. It is valid for ${minutes} minute(s).`;
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    }
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!bundle) {
      error = 'Choose a .flatpak bundle to upload.';
      return;
    }

    uploading = true;
    progress = 0;
    error = '';
    notice = '';
    published = null;

    try {
      const data = await gqlUpload<{ publishBuild: Build }>(
        PUBLISH_BUILD,
        // `bundle: null` is the placeholder the multipart `map` fills in.
        { input: { appId, version, otp, bundle: null } },
        { bundle },
        { bundle: 'variables.input.bundle' },
        { onProgress: (percent) => (progress = percent) }
      );
      published = data.publishBuild;
      // The code is single-use; leaving it in the field would only invite a
      // retry that is guaranteed to fail.
      otp = '';
      bundle = null;
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'The upload failed.';
    } finally {
      uploading = false;
    }
  }

  const curlExample = $derived(`curl -X POST ${API_URL}/graphql \\
  -H "Authorization: Bearer $FLATNEXUS_TOKEN" \\
  -F operations='{"query":"mutation($i:PublishBuildInput!){publishBuild(input:$i){id flatpakRef commit}}","variables":{"i":{"appId":"${appId}","version":"1.0.0","otp":"123456","bundle":null}}}' \\
  -F map='{"0":["variables.i.bundle"]}' \\
  -F 0=@app.flatpak`);
</script>

<Seo
  meta={privateMeta(
    `Upload a build for ${appId}`,
    'Publish a Flatpak bundle to this repository.',
    `/dashboard/apps/${appId}/upload`
  )}
/>

<div class="max-w-2xl">
  <h1 class="mb-1 text-2xl font-semibold tracking-tight">Upload a build</h1>
  <p class="mb-8 font-mono text-sm" style="color: var(--color-ink-muted)">{appId}</p>

  {#if !$session}
    <Alert tone="info"><a href="/login" class="underline">Sign in</a> to publish.</Alert>
  {:else}
    {#if published}
      <div class="card mb-6">
        <h2 class="mb-2 font-medium" style="color: var(--color-ok)">Published</h2>
        <dl class="space-y-1 text-sm">
          <div><dt class="inline" style="color: var(--color-ink-muted)">Ref </dt><dd class="inline font-mono text-xs">{published.flatpakRef}</dd></div>
          <div><dt class="inline" style="color: var(--color-ink-muted)">Commit </dt><dd class="inline font-mono text-xs">{published.commit}</dd></div>
          <div><dt class="inline" style="color: var(--color-ink-muted)">Size </dt><dd class="inline">{formatBytes(published.sizeBytes)}</dd></div>
          <div><dt class="inline" style="color: var(--color-ink-muted)">SHA-256 </dt><dd class="inline font-mono text-xs break-all">{published.sha256}</dd></div>
        </dl>
        <p class="mt-3 text-sm" style="color: var(--color-ink-muted)">
          Clients will pick this up on their next <code class="font-mono">flatpak update</code>.
        </p>
      </div>
    {/if}

    <form class="space-y-5" onsubmit={submit}>
      <div>
        <label class="field-label" for="bundle">Bundle</label>
        <input
          id="bundle"
          class="field"
          type="file"
          accept=".flatpak"
          onchange={pickFile}
          required={!bundle}
        />
        <p class="mt-1 text-xs" style="color: var(--color-ink-muted)">
          Produced by <code class="font-mono">flatpak build-bundle</code>. The ref inside
          it must be for <code class="font-mono">{appId}</code> — the server checks this
          in a staging repository before touching the live one.
          {#if bundle}<br />Selected: {bundle.name} ({formatBytes(bundle.size)}){/if}
        </p>
      </div>

      <div>
        <label class="field-label" for="version">Version</label>
        <input id="version" class="field" placeholder="1.0.0" bind:value={version} />
      </div>

      <div>
        <label class="field-label" for="otp">One-time password</label>
        <div class="flex gap-2">
          <input
            id="otp"
            class="field font-mono"
            inputmode="numeric"
            autocomplete="one-time-code"
            bind:value={otp}
            required
          />
          {#if viewer?.otpMethod === 'EMAIL'}
            <button type="button" class="btn btn-ghost shrink-0" onclick={requestCode}>
              Email me a code
            </button>
          {/if}
        </div>
        <p class="mt-1 text-xs" style="color: var(--color-ink-muted)">
          {#if viewer?.otpMethod === 'TOTP'}
            The current code from your authenticator. Each code can be used once.
            <strong class="font-medium" style="color: var(--color-ink)">
              If you just signed in, that code is already spent
            </strong>
            — wait for your authenticator to roll over, then use the new one.
          {:else}
            Request a code, then enter it here. Codes are single-use, and separate
            from the one that signed you in.
          {/if}
          <a href="/settings/security" class="underline">Change method</a>
        </p>
      </div>

      {#if uploading}
        <div>
          <div class="h-1.5 w-full overflow-hidden rounded" style="background-color: var(--color-surface-sunken)">
            <div
              class="h-full transition-[width]"
              style="width: {progress}%; background-color: var(--color-accent)"
            ></div>
          </div>
          <p class="mt-2 text-xs" style="color: var(--color-ink-muted)">
            Uploading… {progress}%{progress === 100 ? ' — importing into the repository' : ''}
          </p>
        </div>
      {/if}

      {#if notice}<Alert tone="success">{notice}</Alert>{/if}
      {#if error}<Alert>{error}</Alert>{/if}

      <button class="btn btn-primary" type="submit" disabled={uploading}>
        {uploading ? 'Publishing…' : 'Publish build'}
      </button>
    </form>

    <section class="mt-12">
      <h2 class="mb-2 text-lg font-medium">From the command line</h2>
      <p class="mb-3 text-sm" style="color: var(--color-ink-muted)">
        The same mutation, over the GraphQL multipart request spec. Use the
        authenticator method for this — an emailed code cannot be fetched by a
        script.
      </p>
      <CommandBlock command={curlExample} />
      <p class="mt-3 text-sm" style="color: var(--color-ink-muted)">
        Signing in consumes a code too. Spent authenticator codes are tracked by
        their 30-second window, not by what they were used for, so a script that
        signs in and uploads back to back must wait for the next code — otherwise
        the upload is rejected as a replay. Reuse a session token across uploads
        instead of signing in each time.
      </p>
    </section>
  {/if}
</div>
