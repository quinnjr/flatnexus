<script lang="ts">
  import { API_URL } from '$lib/graphql/client';
  import { formatBytes, formatDate, shortHash } from '$lib/format';
  import CommandBlock from '$lib/components/CommandBlock.svelte';
  import Seo from '$lib/components/Seo.svelte';
  import { applicationMarkdownPath } from '$lib/seo/site';
  import type { PageData } from './$types';

  let { data }: { data: PageData } = $props();

  const app = $derived(data.app);
</script>

<Seo meta={data.meta} />

<nav aria-label="Breadcrumb" class="mb-6 text-sm" style="color: var(--color-ink-muted)">
  <a href="/" class="underline">Applications</a>
  <span aria-hidden="true"> / </span>
  <span aria-current="page">{app.name}</span>
</nav>

<header class="mb-8">
  <h1 class="mb-1 text-3xl font-semibold tracking-tight">{app.name}</h1>
  <p class="mb-4 font-mono text-sm" style="color: var(--color-ink-muted)">{app.appId}</p>
  <p class="max-w-2xl">{app.summary}</p>

  <dl class="mt-4 flex flex-wrap gap-x-8 gap-y-2 text-sm" style="color: var(--color-ink-muted)">
    {#if app.publisher}
      <div><dt class="inline">Published by </dt><dd class="inline">{app.publisher.displayName}</dd></div>
    {/if}
    {#if app.license}
      <div><dt class="inline">License </dt><dd class="inline">{app.license}</dd></div>
    {/if}
    {#if app.homepage}
      <div>
        <dt class="inline">Homepage </dt>
        <dd class="inline"><a class="underline" href={app.homepage} rel="noreferrer">{app.homepage}</a></dd>
      </div>
    {/if}
  </dl>
</header>

<section class="mb-10 space-y-3">
  <h2 class="text-lg font-medium">Install</h2>
  {#if data.repo}
    <CommandBlock label="1. Add the remote (once)" command={data.repo.addRemoteCommand} />
  {/if}
  <CommandBlock label="2. Install" command={app.installCommand} />
  <p class="text-sm" style="color: var(--color-ink-muted)">
    Or use the
    <a class="underline" href={app.flatpakrefUrl}>single-click <code class="font-mono">.flatpakref</code></a>,
    which adds the remote and installs in one step.
  </p>
</section>

{#if app.screenshots && app.screenshots.length > 0}
  <section class="mb-10">
    <h2 class="mb-3 text-lg font-medium">Screenshots</h2>
    <ul class="grid gap-4 sm:grid-cols-2">
      {#each app.screenshots as item (item.id)}
        <li>
          {#if item.isStillImage}
            <img
              src={item.url}
              alt="{app.name} screenshot {item.position + 1}"
              loading="lazy"
              class="w-full rounded-lg border"
              style="border-color: var(--color-edge)"
            />
          {:else}
            <!-- svelte-ignore a11y_media_has_caption -->
            <video
              src={item.url}
              controls
              preload="metadata"
              class="w-full rounded-lg border"
              style="border-color: var(--color-edge)"
              aria-label="{app.name} demonstration clip"
            ></video>
          {/if}
        </li>
      {/each}
    </ul>
  </section>
{/if}

{#if app.description}
  <section class="mb-10">
    <h2 class="mb-2 text-lg font-medium">About</h2>
    <p class="max-w-2xl whitespace-pre-line" style="color: var(--color-ink-muted)">
      {app.description}
    </p>
  </section>
{/if}

<section>
  <h2 class="mb-3 text-lg font-medium">Builds</h2>
  {#if !app.builds || app.builds.length === 0}
    <div class="card">
      <p style="color: var(--color-ink-muted)">
        This application is registered but has no published builds yet, so there
        is nothing to install.
      </p>
    </div>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-left text-sm">
        <caption class="sr-only">Published builds of {app.name}</caption>
        <thead style="color: var(--color-ink-muted)">
          <tr class="border-b" style="border-color: var(--color-edge)">
            <th scope="col" class="py-2 pr-4 font-medium">Ref</th>
            <th scope="col" class="py-2 pr-4 font-medium">Version</th>
            <th scope="col" class="py-2 pr-4 font-medium">Size</th>
            <th scope="col" class="py-2 pr-4 font-medium">Commit</th>
            <th scope="col" class="py-2 pr-4 font-medium">Published</th>
            <th scope="col" class="py-2 font-medium">Bundle</th>
          </tr>
        </thead>
        <tbody>
          {#each app.builds as build (build.id)}
            <tr class="border-b" style="border-color: var(--color-edge)">
              <td class="py-2 pr-4 font-mono text-xs">{build.flatpakRef}</td>
              <td class="py-2 pr-4">{build.version || '—'}</td>
              <td class="py-2 pr-4">{formatBytes(build.sizeBytes)}</td>
              <td class="py-2 pr-4 font-mono text-xs" title={build.commit}>
                {shortHash(build.commit)}
              </td>
              <td class="py-2 pr-4" style="color: var(--color-ink-muted)">
                <time datetime={new Date(build.publishedAt * 1000).toISOString()}>
                  {formatDate(build.publishedAt)}
                </time>
              </td>
              <td class="py-2">
                {#if build.bundleUrl}
                  <!--
                    The archived upload, for verifying against the sha256 above.
                    Not an install route — nofollow because the link is signed
                    and short-lived, so there is nothing durable to index.
                  -->
                  <a class="underline" href={build.bundleUrl} rel="nofollow">Download</a>
                {:else}
                  <span style="color: var(--color-ink-muted)">—</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="mt-3 text-xs" style="color: var(--color-ink-muted)">
      Repository served from <code class="font-mono">{API_URL}/repo</code>. A plain-text
      version of this page is at
      <a class="underline" href={applicationMarkdownPath(app.appId)}>{applicationMarkdownPath(app.appId)}</a>.
    </p>
  {/if}
</section>
