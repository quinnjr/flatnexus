<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { gql, GraphQLRequestError } from '$lib/graphql/client';
  import { OAUTH_PROVIDERS, REGISTER, type OAuthProvider } from '$lib/graphql/operations';
  import { signIn } from '$lib/stores/session';
  import Alert from '$lib/components/Alert.svelte';
  import Seo from '$lib/components/Seo.svelte';
  import { privateMeta } from '$lib/seo/meta';

  let email = $state('');
  let displayName = $state('');
  let password = $state('');
  let submitting = $state(false);
  let error = $state('');
  let providers = $state<OAuthProvider[]>([]);

  onMount(async () => {
    try {
      const data = await gql<{ oauthProviders: OAuthProvider[] }>(OAUTH_PROVIDERS);
      providers = data.oauthProviders;
    } catch {
      providers = [];
    }
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    submitting = true;
    error = '';
    try {
      const data = await gql<{ register: { token: string; expiresAt: number } }>(REGISTER, {
        input: { email, displayName, password }
      });
      signIn(data.register.token, data.register.expiresAt);
      await goto('/settings/security');
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    } finally {
      submitting = false;
    }
  }
</script>

<Seo meta={privateMeta('Create account', 'Create a publisher account to distribute Flatpak builds from this repository.', '/register')} />

<div class="mx-auto max-w-sm">
  <h1 class="mb-2 text-2xl font-semibold tracking-tight">Create an account</h1>
  <p class="mb-6 text-sm" style="color: var(--color-ink-muted)">
    Signing in and publishing both require a one-time password, so you will be
    asked to choose a second factor next. Until you do, codes are mailed to the
    address below.
  </p>

  {#if providers.length > 0}
    <div class="mb-6 space-y-2">
      {#each providers as provider (provider.id)}
        <a href={provider.startUrl} class="btn btn-ghost w-full" data-sveltekit-reload>
          Sign up with {provider.displayName}
        </a>
      {/each}
    </div>

    <div class="mb-6 flex items-center gap-3 text-xs" style="color: var(--color-ink-muted)">
      <span class="h-px flex-1" style="background-color: var(--color-edge)"></span>
      or with a password
      <span class="h-px flex-1" style="background-color: var(--color-edge)"></span>
    </div>
  {/if}

  <form class="space-y-4" onsubmit={submit}>
    <div>
      <label class="field-label" for="displayName">Display name</label>
      <input id="displayName" class="field" bind:value={displayName} required />
    </div>

    <div>
      <label class="field-label" for="email">Email</label>
      <input id="email" class="field" type="email" autocomplete="email" bind:value={email} required />
    </div>

    <div>
      <label class="field-label" for="password">Password</label>
      <input
        id="password"
        class="field"
        type="password"
        autocomplete="new-password"
        minlength="12"
        bind:value={password}
        required
      />
      <p class="mt-1 text-xs" style="color: var(--color-ink-muted)">At least 12 characters.</p>
    </div>

    {#if error}<Alert>{error}</Alert>{/if}

    <button class="btn btn-primary w-full" type="submit" disabled={submitting}>
      {submitting ? 'Creating…' : 'Create account'}
    </button>
  </form>
</div>
