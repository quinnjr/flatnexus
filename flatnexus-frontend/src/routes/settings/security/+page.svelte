<script lang="ts">
  /**
   * Second-factor settings.
   *
   * This is the page that decides whether a publisher can upload from `curl`
   * unattended: TOTP codes come off their own device, whereas emailed codes need
   * a mailbox round-trip per upload.
   */
  import { onMount } from 'svelte';
  import { gql, GraphQLRequestError } from '$lib/graphql/client';
  import {
    CONFIRM_TOTP,
    ENROLL_TOTP,
    ME,
    SET_OTP_METHOD,
    type Viewer
  } from '$lib/graphql/operations';
  // `liveSession` is null once the token has expired, not merely when it is
  // absent — a session that lapses with the tab open must not keep rendering
  // the signed-in branch.
  import { liveSession as session } from '$lib/stores/session';
  import Alert from '$lib/components/Alert.svelte';
  import Seo from '$lib/components/Seo.svelte';
  import { privateMeta } from '$lib/seo/meta';
  import CommandBlock from '$lib/components/CommandBlock.svelte';

  let viewer = $state<Viewer | null>(null);
  let enrollment = $state<{ secret: string; uri: string } | null>(null);
  let code = $state('');
  let busy = $state(false);
  let error = $state('');
  let notice = $state('');

  onMount(refresh);

  async function refresh() {
    error = '';
    try {
      const data = await gql<{ me: Viewer | null }>(ME);
      viewer = data.me;
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    }
  }

  async function enroll() {
    busy = true;
    error = '';
    notice = '';
    try {
      const data = await gql<{ enrollTotp: { secret: string; uri: string } }>(ENROLL_TOTP);
      enrollment = data.enrollTotp;
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    } finally {
      busy = false;
    }
  }

  async function confirm(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    error = '';
    try {
      const data = await gql<{ confirmTotp: Viewer }>(CONFIRM_TOTP, { code });
      viewer = data.confirmTotp;
      enrollment = null;
      code = '';
      notice = 'Authenticator confirmed.';
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    } finally {
      busy = false;
    }
  }

  async function setMethod(method: 'TOTP' | 'EMAIL') {
    busy = true;
    error = '';
    notice = '';
    try {
      const data = await gql<{ setOtpMethod: Viewer }>(SET_OTP_METHOD, { method });
      viewer = data.setOtpMethod;
      notice = `Uploads now require ${method === 'TOTP' ? 'an authenticator code' : 'an emailed code'}.`;
    } catch (e) {
      error = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    } finally {
      busy = false;
    }
  }
</script>

<Seo meta={privateMeta('Security', 'Manage the one-time password that gates sign-in and build uploads.', '/settings/security')} />

<h1 class="mb-2 text-2xl font-semibold tracking-tight">Security</h1>
<p class="mb-8 max-w-2xl" style="color: var(--color-ink-muted)">
  Signing in and publishing a build both require a one-time password. A password
  alone never yields a usable session, and a stolen session token is never enough
  on its own to push a build to everyone who has added this remote.
</p>

{#if !$session}
  <Alert tone="info">
    <a href="/login" class="underline">Sign in</a> to manage your second factor.
  </Alert>
{:else if error && !viewer}
  <Alert>{error}</Alert>
{:else if viewer}
  {#if notice}<div class="mb-4"><Alert tone="success">{notice}</Alert></div>{/if}
  {#if error}<div class="mb-4"><Alert>{error}</Alert></div>{/if}

  <section class="card mb-6">
    <h2 class="mb-3 font-medium">Method</h2>
    <div class="flex flex-wrap gap-3">
      <button
        class="btn {viewer.otpMethod === 'TOTP' ? 'btn-primary' : 'btn-ghost'}"
        disabled={busy || !viewer.totpConfirmed}
        onclick={() => setMethod('TOTP')}
      >
        Authenticator app
      </button>
      <button
        class="btn {viewer.otpMethod === 'EMAIL' ? 'btn-primary' : 'btn-ghost'}"
        disabled={busy}
        onclick={() => setMethod('EMAIL')}
      >
        Emailed code
      </button>
    </div>

    <p class="mt-3 text-sm" style="color: var(--color-ink-muted)">
      {#if viewer.otpMethod === 'TOTP'}
        Sign-in and uploads ask for the current code from your authenticator.
        This is the method to use for scripted or CI uploads — no mailbox is
        involved.
      {:else}
        Sign-in and uploads ask for a code mailed to {viewer.email}. Switch to an
        authenticator if you upload from scripts.
      {/if}
    </p>

    {#if !viewer.totpConfirmed}
      <p class="mt-2 text-xs" style="color: var(--color-ink-muted)">
        Enroll and confirm an authenticator below before you can select it.
      </p>
    {/if}
  </section>

  <section class="card">
    <h2 class="mb-3 font-medium">Authenticator app</h2>

    {#if viewer.totpConfirmed && !enrollment}
      <p class="text-sm" style="color: var(--color-ink-muted)">
        An authenticator is enrolled and confirmed.
      </p>
      <button class="btn btn-ghost mt-4" disabled={busy} onclick={enroll}>
        Re-enroll a new device
      </button>
      <p class="mt-2 text-xs" style="color: var(--color-ink-muted)">
        Re-enrolling replaces the current secret and marks it unconfirmed until
        you enter a code from the new device.
      </p>
    {:else if !enrollment}
      <button class="btn btn-primary" disabled={busy} onclick={enroll}>
        Enroll an authenticator
      </button>
    {/if}

    {#if enrollment}
      <div class="space-y-4">
        <p class="text-sm" style="color: var(--color-ink-muted)">
          Add this secret to your authenticator, then enter the code it shows.
          The secret is displayed once.
        </p>

        <CommandBlock label="Secret" command={enrollment.secret} />
        <CommandBlock label="otpauth URI (for a QR generator)" command={enrollment.uri} />

        <form class="flex items-end gap-3" onsubmit={confirm}>
          <div class="flex-1">
            <label class="field-label" for="code">Code from your authenticator</label>
            <input
              id="code"
              class="field font-mono"
              inputmode="numeric"
              autocomplete="one-time-code"
              maxlength="6"
              bind:value={code}
              required
            />
          </div>
          <button class="btn btn-primary" type="submit" disabled={busy}>Confirm</button>
        </form>
      </div>
    {/if}
  </section>
{/if}
