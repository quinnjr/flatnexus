<script lang="ts">
  import { onMount } from 'svelte';
  import { gql, GraphQLRequestError } from '$lib/graphql/client';
  import { MY_APPLICATIONS, type Application } from '$lib/graphql/operations';
  // `liveSession` is null once the token has expired, not merely when it is
  // absent — a session that lapses with the tab open must not keep rendering
  // the signed-in branch.
  import { liveSession as session } from '$lib/stores/session';
  import { formatDate } from '$lib/format';
  import Alert from '$lib/components/Alert.svelte';
  import Seo from '$lib/components/Seo.svelte';
  import { privateMeta } from '$lib/seo/meta';

  let applications = $state<Application[]>([]);
  let loading = $state(true);
  let error = $state('');

  onMount(async () => {
    if (!$session) {
      loading = false;
      return;
    }
    try {
      const data = await gql<{ myApplications: Application[] }>(MY_APPLICATIONS);
      applications = data.myApplications;
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    } finally {
      loading = false;
    }
  });
</script>

<Seo meta={privateMeta('Publish', 'Applications you publish to this Flatpak repository.', '/dashboard')} />

<div class="mb-8 flex items-start justify-between gap-4">
  <div>
    <h1 class="mb-2 text-2xl font-semibold tracking-tight">Your applications</h1>
    <p class="max-w-2xl" style="color: var(--color-ink-muted)">
      An application id must be registered before a build can be uploaded for it.
      The id you register has to match the ref inside every bundle you upload.
    </p>
  </div>
  {#if $session}
    <a href="/dashboard/apps/new" class="btn btn-primary shrink-0">Register application</a>
  {/if}
</div>

{#if !$session}
  <Alert tone="info"><a href="/login" class="underline">Sign in</a> to publish.</Alert>
{:else if loading}
  <p style="color: var(--color-ink-muted)">Loading…</p>
{:else if error}
  <Alert>{error}</Alert>
{:else if applications.length === 0}
  <div class="card">
    <p class="mb-1">You have not registered an application yet.</p>
    <p class="text-sm" style="color: var(--color-ink-muted)">
      Register one, then upload a <code class="font-mono">.flatpak</code> bundle
      produced by <code class="font-mono">flatpak build-bundle</code>.
    </p>
  </div>
{:else}
  <ul class="space-y-4">
    {#each applications as app (app.id)}
      <li class="card">
        <div class="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h2 class="font-medium">{app.name}</h2>
            <p class="font-mono text-xs" style="color: var(--color-ink-muted)">{app.appId}</p>
          </div>
          <div class="flex gap-2">
            <a href="/apps/{app.appId}" class="btn btn-ghost">View</a>
            <a href="/dashboard/apps/{app.appId}/upload" class="btn btn-primary">Upload build</a>
          </div>
        </div>

        {#if app.builds && app.builds.length > 0}
          <ul class="mt-4 space-y-1 text-sm" style="color: var(--color-ink-muted)">
            {#each app.builds.slice(0, 5) as build (build.id)}
              <li class="flex flex-wrap gap-x-3">
                <code class="font-mono text-xs">{build.flatpakRef}</code>
                <span>{build.version || 'no version'}</span>
                <span>{formatDate(build.publishedAt)}</span>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="mt-4 text-sm" style="color: var(--color-ink-muted)">No builds published yet.</p>
        {/if}
      </li>
    {/each}
  </ul>
{/if}
