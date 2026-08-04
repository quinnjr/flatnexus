<script lang="ts">
  import '../app.css';
  import { page } from '$app/state';
  import { liveSession as session, signOut } from '$lib/stores/session';
  import { goto } from '$app/navigation';

  let { children } = $props();

  const nav = [
    { href: '/', label: 'Browse' },
    { href: '/dashboard', label: 'Publish' },
    { href: '/settings/security', label: 'Security' }
  ];

  function isActive(href: string): boolean {
    return href === '/' ? page.url.pathname === '/' : page.url.pathname.startsWith(href);
  }

  async function handleSignOut() {
    signOut();
    await goto('/');
  }
</script>

<div class="flex min-h-full flex-col">
  <header class="border-b" style="border-color: var(--color-edge)">
    <div class="mx-auto flex max-w-5xl items-center gap-6 px-6 py-4">
      <a href="/" class="font-mono text-lg font-semibold tracking-tight">
        flat<span style="color: var(--color-accent)">nexus</span>
      </a>

      <nav class="flex items-center gap-1 text-sm">
        {#each nav as item (item.href)}
          <a
            href={item.href}
            class="rounded-md px-3 py-1.5 transition"
            style:color={isActive(item.href) ? 'var(--color-ink)' : 'var(--color-ink-muted)'}
            style:background-color={isActive(item.href) ? 'var(--color-accent-soft)' : 'transparent'}
          >
            {item.label}
          </a>
        {/each}
      </nav>

      <div class="ml-auto flex items-center gap-3 text-sm">
        {#if $session}
          <button type="button" class="btn btn-ghost" onclick={handleSignOut}>Sign out</button>
        {:else}
          <a href="/login" class="btn btn-ghost">Sign in</a>
          <a href="/register" class="btn btn-primary">Create account</a>
        {/if}
      </div>
    </div>
  </header>

  <main class="mx-auto w-full max-w-5xl flex-1 px-6 py-10">
    {@render children()}
  </main>

  <footer class="border-t px-6 py-6" style="border-color: var(--color-edge)">
    <div class="mx-auto max-w-5xl text-sm" style="color: var(--color-ink-muted)">
      A Flatpak distribution point. Builds are imported into an OSTree repository
      and served to the <code class="font-mono">flatpak</code> CLI directly.
    </div>
  </footer>
</div>
