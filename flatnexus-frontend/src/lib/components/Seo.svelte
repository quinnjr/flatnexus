<script lang="ts">
  /**
   * Renders a page's metadata into `<head>`.
   *
   * One component so no route can half-describe itself — every page gets a
   * canonical, a robots directive, and Open Graph tags whether or not its author
   * thought about them.
   */
  import { page } from '$app/state';
  import { formatTitle, type PageMeta } from '$lib/seo/meta';
  import { SITE_NAME, absolute } from '$lib/seo/site';

  let { meta }: { meta: PageMeta } = $props();

  // Prefer the origin the request actually arrived on, so a preview deployment
  // or a proxy does not emit canonicals pointing at production.
  const origin = $derived(page.url?.origin || undefined);
  const canonical = $derived(absolute(meta.path, origin));
  const title = $derived(formatTitle(meta));
  const robots = $derived(
    meta.noindex ? 'noindex, nofollow' : 'index, follow, max-image-preview:large, max-snippet:-1'
  );
</script>

<svelte:head>
  <title>{title}</title>
  <meta name="description" content={meta.description} />
  <link rel="canonical" href={canonical} />
  <meta name="robots" content={robots} />

  <meta property="og:type" content="website" />
  <meta property="og:site_name" content={SITE_NAME} />
  <meta property="og:title" content={title} />
  <meta property="og:description" content={meta.description} />
  <meta property="og:url" content={canonical} />
  {#if meta.image}<meta property="og:image" content={meta.image} />{/if}

  <meta name="twitter:card" content={meta.image ? 'summary_large_image' : 'summary'} />
  <meta name="twitter:title" content={title} />
  <meta name="twitter:description" content={meta.description} />

  {#if meta.dateModified}
    <meta property="article:modified_time" content={meta.dateModified} />
  {/if}

  <!--
    Answer engines read a markdown mirror far more reliably than the DOM. The
    site index is always offered; a page that has its own markdown view declares
    it too, and that is the one an agent on that page should follow.
  -->
  {#each meta.alternates ?? [] as alternate (alternate.href)}
    <link
      rel="alternate"
      type={alternate.type}
      href={absolute(alternate.href, origin)}
      title={alternate.title}
    />
  {/each}
  <link rel="alternate" type="text/markdown" href={absolute('/llms.txt', origin)} title="llms.txt" />

  {#if !meta.noindex}
    {#each meta.schemas ?? [] as schema, index (index)}
      {@html `<script type="application/ld+json">${schema.replace(/</g, '\\u003c')}</script>`}
    {/each}
  {/if}
</svelte:head>
