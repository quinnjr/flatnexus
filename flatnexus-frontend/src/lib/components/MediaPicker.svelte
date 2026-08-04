<script lang="ts">
  /**
   * Picks the gallery an application is registered with.
   *
   * At least one still image is required; a WebM clip may be included alongside
   * but does not satisfy the requirement on its own. That rule is enforced on the
   * server — it is mirrored here only so a publisher learns it before uploading
   * rather than after.
   */
  import { formatBytes } from '$lib/format';

  const MAX_ITEMS = 8;
  const MAX_IMAGE_BYTES = 5 * 1024 * 1024;
  const MAX_VIDEO_BYTES = 15 * 1024 * 1024;
  const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/webp'];

  let { files = $bindable([]) }: { files: File[] } = $props();

  let problem = $state('');

  const isVideo = (file: File) => file.type === 'video/webm' || file.name.endsWith('.webm');
  const stillCount = $derived(files.filter((file) => !isVideo(file)).length);

  function add(event: Event) {
    const input = event.target as HTMLInputElement;
    const picked = Array.from(input.files ?? []);
    problem = '';

    for (const file of picked) {
      if (files.length >= MAX_ITEMS) {
        problem = `A gallery holds at most ${MAX_ITEMS} items.`;
        break;
      }
      // Type is checked properly server-side by sniffing the bytes; this only
      // catches the obvious cases early.
      if (!isVideo(file) && !IMAGE_TYPES.includes(file.type)) {
        problem = `${file.name} is not a PNG, JPEG, WebP, or WebM file.`;
        continue;
      }
      const ceiling = isVideo(file) ? MAX_VIDEO_BYTES : MAX_IMAGE_BYTES;
      if (file.size > ceiling) {
        problem = `${file.name} is larger than ${formatBytes(ceiling)}.`;
        continue;
      }
      files = [...files, file];
    }

    // Clearing lets the same file be re-picked after being removed.
    input.value = '';
  }

  function remove(index: number) {
    files = files.filter((_, i) => i !== index);
    problem = '';
  }

  function move(index: number, delta: number) {
    const target = index + delta;
    if (target < 0 || target >= files.length) return;
    const next = [...files];
    [next[index], next[target]] = [next[target], next[index]];
    files = next;
  }
</script>

<div>
  <label class="field-label" for="media">Screenshots</label>
  <input
    id="media"
    class="field"
    type="file"
    accept="image/png,image/jpeg,image/webp,video/webm"
    multiple
    onchange={add}
  />
  <p class="mt-1 text-xs" style="color: var(--color-ink-muted)">
    At least one screenshot is required — an application nobody can look at is one
    nobody will install. PNG, JPEG or WebP up to 5&nbsp;MiB each; a WebM clip up to
    15&nbsp;MiB may be added alongside, but does not count as the screenshot.
    Up to {MAX_ITEMS} items; the first is the one shown first.
  </p>

  {#if problem}
    <p class="mt-2 text-xs" style="color: var(--color-danger)" role="alert">{problem}</p>
  {/if}

  {#if files.length > 0}
    <ul class="mt-3 space-y-2">
      {#each files as file, index (`${file.name}-${index}`)}
        <li class="flex items-center gap-3 rounded-md border px-3 py-2" style="border-color: var(--color-edge)">
          <span class="font-mono text-xs" style="color: var(--color-ink-muted)">{index + 1}</span>
          <span class="flex-1 truncate text-sm">{file.name}</span>
          <span class="text-xs" style="color: var(--color-ink-muted)">
            {isVideo(file) ? 'video' : 'image'} · {formatBytes(file.size)}
          </span>
          <button
            type="button"
            class="btn btn-ghost px-2 py-1 text-xs"
            aria-label="Move {file.name} earlier"
            disabled={index === 0}
            onclick={() => move(index, -1)}
          >
            ↑
          </button>
          <button
            type="button"
            class="btn btn-ghost px-2 py-1 text-xs"
            aria-label="Move {file.name} later"
            disabled={index === files.length - 1}
            onclick={() => move(index, 1)}
          >
            ↓
          </button>
          <button
            type="button"
            class="btn btn-ghost px-2 py-1 text-xs"
            aria-label="Remove {file.name}"
            onclick={() => remove(index)}
          >
            Remove
          </button>
        </li>
      {/each}
    </ul>

    {#if stillCount === 0}
      <p class="mt-2 text-xs" style="color: var(--color-danger)" role="alert">
        A video on its own is not enough — add at least one still image.
      </p>
    {/if}
  {/if}
</div>
