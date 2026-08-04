<script lang="ts">
  /**
   * Landing page for the OAuth redirect.
   *
   * The backend appends the session token as a URL *fragment*, which the browser
   * never transmits. It is read here, stored, and immediately stripped from the
   * address bar so it does not survive in history.
   */
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { signIn } from '$lib/stores/session';
  import Alert from '$lib/components/Alert.svelte';
  import Seo from '$lib/components/Seo.svelte';
  import { privateMeta } from '$lib/seo/meta';

  let error = $state('');

  onMount(async () => {
    const params = new URLSearchParams(window.location.hash.slice(1));
    const token = params.get('token');
    const expiresAt = Number(params.get('expires_at'));

    if (!token || !Number.isFinite(expiresAt) || expiresAt === 0) {
      error = 'Sign-in did not complete. Please try again.';
      return;
    }

    signIn(token, expiresAt);
    history.replaceState(null, '', window.location.pathname);
    await goto('/dashboard', { replaceState: true });
  });
</script>

<Seo meta={privateMeta('Signing in', 'Completing sign-in.', '/auth/callback')} />

<div class="mx-auto max-w-sm text-center">
  {#if error}
    <Alert>{error}</Alert>
    <p class="mt-4 text-sm"><a href="/login" class="underline">Back to sign in</a></p>
  {:else}
    <p style="color: var(--color-ink-muted)">Completing sign-in…</p>
  {/if}
</div>
