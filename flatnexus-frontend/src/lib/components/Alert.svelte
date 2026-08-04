<script lang="ts">
  /** Inline status message. `tone` drives colour only; the text carries meaning. */
  let {
    tone = 'error',
    children
  }: { tone?: 'error' | 'success' | 'info'; children?: import('svelte').Snippet } = $props();

  const styles = {
    error: 'border-[color:var(--color-danger)] text-[color:var(--color-danger)]',
    success: 'border-[color:var(--color-ok)] text-[color:var(--color-ok)]',
    info: 'border-[color:var(--color-edge)] text-[color:var(--color-ink-muted)]'
  };
</script>

<p class="rounded-md border px-3 py-2 text-sm {styles[tone]}" role={tone === 'error' ? 'alert' : 'status'}>
  {@render children?.()}
</p>
