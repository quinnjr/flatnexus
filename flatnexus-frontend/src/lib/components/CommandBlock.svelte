<script lang="ts">
  /**
   * A copyable terminal command. Used anywhere the page shows something the
   * reader is expected to paste into a shell — which, on a Flatpak repository,
   * is most of the page.
   */
  let { command, label = '' }: { command: string; label?: string } = $props();

  let copied = $state(false);
  let failed = $state(false);

  /**
   * `navigator.clipboard` is undefined on a non-secure origin and rejects when
   * permission is denied. Unhandled, the button simply never changed — and this
   * is the component that shows the TOTP secret, where a silent copy failure is
   * easy to mistake for a successful one.
   */
  async function copy() {
    failed = false;
    try {
      if (!navigator.clipboard) throw new Error('clipboard unavailable');
      await navigator.clipboard.writeText(command);
      copied = true;
      setTimeout(() => (copied = false), 1600);
    } catch {
      failed = true;
      setTimeout(() => (failed = false), 4000);
    }
  }
</script>

<div>
  {#if label}
    <p class="field-label">{label}</p>
  {/if}
  <div class="flex items-stretch gap-2">
    <code class="command flex-1">{command}</code>
    <button type="button" class="btn btn-ghost shrink-0" onclick={copy}>
      {copied ? 'Copied' : failed ? 'Copy failed' : 'Copy'}
    </button>
  </div>
  {#if failed}
    <p class="mt-1 text-xs" style="color: var(--color-danger)" role="status">
      Could not reach the clipboard — select the text above and copy it manually.
    </p>
  {/if}
</div>
