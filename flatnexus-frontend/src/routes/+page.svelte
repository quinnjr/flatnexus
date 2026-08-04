<script lang="ts">
  import { untrack } from 'svelte';
  import { goto } from '$app/navigation';
  import CommandBlock from '$lib/components/CommandBlock.svelte';
  import Alert from '$lib/components/Alert.svelte';
  import Seo from '$lib/components/Seo.svelte';
  import { faqEntries } from '$lib/seo/faq';
  import type { PageData } from './$types';

  let { data }: { data: PageData } = $props();

  // Seeded once from the server-rendered query. Thereafter this input owns the
  // value and pushes it into the URL; re-syncing from `data` on every navigation
  // would fight the user mid-keystroke.
  let search = $state(untrack(() => data.search));

  // Search is a URL parameter, not local state: it makes a result set
  // shareable, and it is what the `SearchAction` in the site's structured data
  // promises crawlers the query string does.
  let debounce: ReturnType<typeof setTimeout>;
  function onSearchInput() {
    clearTimeout(debounce);
    debounce = setTimeout(() => {
      const query = search ? `?search=${encodeURIComponent(search)}` : '';
      goto(`/${query}`, { keepFocus: true, replaceState: true, noScroll: true });
    }, 250);
  }
</script>

<Seo meta={data.meta} />

<section class="mb-10">
  <h1 class="mb-2 text-3xl font-semibold tracking-tight">Applications</h1>
  <p class="mb-6 max-w-2xl" style="color: var(--color-ink-muted)">
    Everything published here is a real Flatpak ref in an OSTree repository. Add
    the remote once and <code class="font-mono">flatpak update</code> picks up new
    builds the same way it does for Flathub.
  </p>

  {#if data.repo}
    <CommandBlock label="Add this remote" command={data.repo.addRemoteCommand} />
    {#if !data.repo.gpgSigned}
      <div class="mt-3">
        <Alert tone="info">
          This server does not sign its commits, so <code class="font-mono">flatpak</code>
          cannot verify what it installs from here.
        </Alert>
      </div>
    {/if}
  {/if}
</section>

<section class="mb-14">
  <input
    class="field mb-6"
    type="search"
    name="search"
    placeholder="Search by application id, name, or summary"
    aria-label="Search applications"
    bind:value={search}
    oninput={onSearchInput}
  />

  {#if data.error}
    <Alert>{data.error}</Alert>
  {:else if data.applications.length === 0}
    <div class="card text-center">
      <p class="mb-1">
        {data.search ? `Nothing matches “${data.search}”.` : 'Nothing published yet.'}
      </p>
      <p class="text-sm" style="color: var(--color-ink-muted)">
        <a href="/dashboard" class="underline">Register an application</a> to publish
        the first build.
      </p>
    </div>
  {:else}
    <ul class="grid gap-4 sm:grid-cols-2">
      {#each data.applications as app (app.id)}
        <li>
          <a href="/apps/{app.appId}" class="card block transition hover:border-[color:var(--color-accent)]">
            <h2 class="mb-1 font-medium">{app.name}</h2>
            <p class="mb-3 font-mono text-xs" style="color: var(--color-ink-muted)">{app.appId}</p>
            <p class="text-sm" style="color: var(--color-ink-muted)">{app.summary}</p>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</section>

{#if data.repo}
  <!--
    Rendered from the same list the FAQPage JSON-LD is built from
    (`$lib/seo/faq`). Structured data for content a reader cannot see is a
    guidelines violation, so the two must describe the same answers — sharing the
    source is what guarantees it rather than hoping.
  -->
  <section>
    <h2 class="mb-4 text-xl font-semibold tracking-tight">Common questions</h2>
    <dl class="space-y-5">
      {#each faqEntries(data.repo) as entry (entry.question)}
        <div>
          <dt class="mb-1 font-medium">{entry.question}</dt>
          <dd class="text-sm" style="color: var(--color-ink-muted)">{entry.answer}</dd>
        </div>
      {/each}
    </dl>
  </section>
{/if}
