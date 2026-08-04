<script lang="ts">
  import { goto } from '$app/navigation';
  import { gqlUpload, uploadList, GraphQLRequestError } from '$lib/graphql/client';
  import { CREATE_APPLICATION, type Application } from '$lib/graphql/operations';
  // `liveSession` is null once the token has expired, not merely when it is
  // absent — a session that lapses with the tab open must not keep rendering
  // the signed-in branch.
  import { liveSession as session } from '$lib/stores/session';
  import Alert from '$lib/components/Alert.svelte';
  import MediaPicker from '$lib/components/MediaPicker.svelte';
  import Seo from '$lib/components/Seo.svelte';
  import { privateMeta } from '$lib/seo/meta';

  let appId = $state('');
  let name = $state('');
  let summary = $state('');
  let description = $state('');
  let homepage = $state('');
  let license = $state('');
  let media = $state<File[]>([]);
  let submitting = $state(false);
  let progress = $state(0);
  let error = $state('');

  async function submit(event: SubmitEvent) {
    event.preventDefault();

    if (media.length === 0) {
      error = 'Add at least one screenshot.';
      return;
    }

    submitting = true;
    progress = 0;
    error = '';

    try {
      // `uploadList` produces the file map and the matching `null` placeholders,
      // so the JSON operations part and the multipart `map` cannot get out of
      // step over an index.
      const gallery = uploadList(media, 'input.screenshots');

      const data = await gqlUpload<{ createApplication: Application }>(
        CREATE_APPLICATION,
        {
          input: {
            appId,
            name,
            summary,
            description,
            homepage: homepage || null,
            license: license || null,
            screenshots: gallery.placeholders
          }
        },
        gallery.files,
        gallery.filePaths,
        { onProgress: (percent) => (progress = percent) }
      );
      await goto(`/dashboard/apps/${data.createApplication.appId}/upload`);
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    } finally {
      submitting = false;
    }
  }
</script>

<Seo meta={privateMeta('Register application', 'Register a Flatpak application id before publishing builds for it.', '/dashboard/apps/new')} />

<div class="max-w-xl">
  <h1 class="mb-2 text-2xl font-semibold tracking-tight">Register an application</h1>
  <p class="mb-8 text-sm" style="color: var(--color-ink-muted)">
    The application id is permanent — it is embedded in every ref published under
    it, so it cannot be changed later without breaking installs. At least one
    screenshot is required before you can publish a build.
  </p>

  {#if !$session}
    <Alert tone="info"><a href="/login" class="underline">Sign in</a> to continue.</Alert>
  {:else}
    <form class="space-y-4" onsubmit={submit}>
      <div>
        <label class="field-label" for="appId">Application id</label>
        <input
          id="appId"
          class="field font-mono"
          placeholder="org.example.MyApp"
          bind:value={appId}
          required
        />
        <p class="mt-1 text-xs" style="color: var(--color-ink-muted)">
          Reverse-DNS, at least three segments. Must match the ref inside your bundles.
        </p>
      </div>

      <div>
        <label class="field-label" for="name">Name</label>
        <input id="name" class="field" bind:value={name} required />
      </div>

      <div>
        <label class="field-label" for="summary">Summary</label>
        <input id="summary" class="field" maxlength="120" bind:value={summary} required />
      </div>

      <div>
        <label class="field-label" for="description">Description</label>
        <textarea id="description" class="field" rows="5" bind:value={description}></textarea>
      </div>

      <div class="grid gap-4 sm:grid-cols-2">
        <div>
          <label class="field-label" for="homepage">Homepage</label>
          <input id="homepage" class="field" type="url" bind:value={homepage} />
        </div>
        <div>
          <label class="field-label" for="license">License</label>
          <input id="license" class="field" placeholder="GPL-3.0-or-later" bind:value={license} />
        </div>
      </div>

      <MediaPicker bind:files={media} />

      {#if submitting && progress > 0}
        <div>
          <div class="h-1.5 w-full overflow-hidden rounded" style="background-color: var(--color-surface-sunken)">
            <div
              class="h-full transition-[width]"
              style="width: {progress}%; background-color: var(--color-accent)"
            ></div>
          </div>
          <p class="mt-2 text-xs" style="color: var(--color-ink-muted)">
            Uploading screenshots… {progress}%
          </p>
        </div>
      {/if}

      {#if error}<Alert>{error}</Alert>{/if}

      <button class="btn btn-primary" type="submit" disabled={submitting}>
        {submitting ? 'Registering…' : 'Register'}
      </button>
    </form>
  {/if}
</div>
