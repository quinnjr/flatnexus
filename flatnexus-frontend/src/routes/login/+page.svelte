<script lang="ts">
  /**
   * Two-step sign-in.
   *
   * A correct password does not produce a session — it produces a challenge that
   * a one-time password redeems. The two steps are one page because they are one
   * task: navigating away between them would strand a live challenge and an
   * already-sent code.
   */
  import { onMount, tick } from 'svelte';
  import { goto } from '$app/navigation';
  import { gql, GraphQLRequestError } from '$lib/graphql/client';
  import {
    LOGIN,
    OAUTH_PROVIDERS,
    VERIFY_LOGIN,
    type AuthPayload,
    type LoginChallenge,
    type LoginResult,
    type OAuthProvider
  } from '$lib/graphql/operations';
  import { signIn } from '$lib/stores/session';
  import Alert from '$lib/components/Alert.svelte';
  import Seo from '$lib/components/Seo.svelte';
  import { privateMeta } from '$lib/seo/meta';

  let email = $state('');
  let password = $state('');
  let otp = $state('');
  let challenge = $state<LoginChallenge | null>(null);
  let submitting = $state(false);
  let error = $state('');
  let providers = $state<OAuthProvider[]>([]);
  let otpInput = $state<HTMLInputElement | null>(null);

  onMount(async () => {
    try {
      const data = await gql<{ oauthProviders: OAuthProvider[] }>(OAUTH_PROVIDERS);
      providers = data.oauthProviders;
    } catch {
      // Password sign-in still works without the provider list, so a failure
      // here is not worth blocking the form over.
      providers = [];
    }
  });

  async function complete(session: AuthPayload) {
    signIn(session.token, session.expiresAt);
    await goto('/dashboard');
  }

  async function submitPassword(event: SubmitEvent) {
    event.preventDefault();
    submitting = true;
    error = '';
    try {
      const data = await gql<{ login: LoginResult }>(LOGIN, { input: { email, password } });

      if (data.login.status === 'AUTHENTICATED' && data.login.session) {
        await complete(data.login.session);
        return;
      }

      challenge = data.login.challenge;
      // The password is spent at this point; holding it in memory through the
      // second step buys nothing.
      password = '';
      await tick();
      otpInput?.focus();
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    } finally {
      submitting = false;
    }
  }

  async function submitOtp(event: SubmitEvent) {
    event.preventDefault();
    if (!challenge) return;

    submitting = true;
    error = '';
    try {
      const data = await gql<{ verifyLogin: AuthPayload }>(VERIFY_LOGIN, {
        challengeToken: challenge.challengeToken,
        otp
      });
      await complete(data.verifyLogin);
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
      // A spent or expired challenge cannot be retried with a new code, so send
      // the publisher back to step one rather than leaving them entering codes
      // against a dead challenge.
      if (e instanceof GraphQLRequestError && e.code === 'FORBIDDEN') {
        challenge = null;
        otp = '';
      }
    } finally {
      submitting = false;
    }
  }

  function startOver() {
    challenge = null;
    otp = '';
    error = '';
  }
</script>

<Seo meta={privateMeta('Sign in', 'Sign in to publish Flatpak builds to this repository.', '/login')} />

<div class="mx-auto max-w-sm">
  <h1 class="mb-6 text-2xl font-semibold tracking-tight">
    {challenge ? 'Enter your code' : 'Sign in'}
  </h1>

  {#if challenge}
    <p class="mb-6 text-sm" style="color: var(--color-ink-muted)">
      {#if challenge.method === 'EMAIL'}
        A one-time code was sent to {challenge.sentTo}. It is valid for
        {Math.max(1, Math.round(challenge.expiresInSecs / 60))} minute(s) and can be used once.
      {:else}
        Enter the current code from your authenticator app.
      {/if}
    </p>

    <form class="space-y-4" onsubmit={submitOtp}>
      <div>
        <label class="field-label" for="otp">One-time password</label>
        <input
          id="otp"
          class="field font-mono"
          inputmode="numeric"
          autocomplete="one-time-code"
          bind:value={otp}
          bind:this={otpInput}
          required
        />
      </div>

      {#if error}<Alert>{error}</Alert>{/if}

      <button class="btn btn-primary w-full" type="submit" disabled={submitting}>
        {submitting ? 'Verifying…' : 'Verify and sign in'}
      </button>

      <button class="btn btn-ghost w-full" type="button" onclick={startOver}>
        Start over
      </button>
    </form>
  {:else}
    {#if providers.length > 0}
      <div class="mb-6 space-y-2">
        {#each providers as provider (provider.id)}
          <a href={provider.startUrl} class="btn btn-ghost w-full" data-sveltekit-reload>
            Continue with {provider.displayName}
          </a>
        {/each}
      </div>

      <div class="mb-6 flex items-center gap-3 text-xs" style="color: var(--color-ink-muted)">
        <span class="h-px flex-1" style="background-color: var(--color-edge)"></span>
        or with a password
        <span class="h-px flex-1" style="background-color: var(--color-edge)"></span>
      </div>
    {/if}

    <form class="space-y-4" onsubmit={submitPassword}>
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
          autocomplete="current-password"
          bind:value={password}
          required
        />
      </div>

      {#if error}<Alert>{error}</Alert>{/if}

      <button class="btn btn-primary w-full" type="submit" disabled={submitting}>
        {submitting ? 'Signing in…' : 'Continue'}
      </button>

      <p class="text-xs" style="color: var(--color-ink-muted)">
        You will be asked for a one-time password next.
      </p>
    </form>

    <p class="mt-6 text-sm" style="color: var(--color-ink-muted)">
      No account? <a href="/register" class="underline">Create one</a>.
    </p>
  {/if}
</div>
