/**
 * Site identity and absolute-URL construction.
 *
 * Every canonical, JSON-LD `@id`, sitemap entry, and Open Graph URL is built
 * from one origin here. They must agree: a canonical that disagrees with the
 * `@id` in the structured data is how a page ends up indexed under one URL and
 * described under another.
 */

import { PUBLIC_SITE_URL } from '$env/static/public';

/** Origin this frontend is served from. */
export const SITE_ORIGIN = (PUBLIC_SITE_URL || 'http://localhost:5173').replace(/\/$/, '');

export const SITE_NAME = 'FlatNexus';
export const SITE_TAGLINE = 'A self-hosted Flatpak distribution point';

export const SITE_DESCRIPTION =
  'Browse and install Flatpak applications from a self-hosted OSTree repository. ' +
  'Add the remote once and the flatpak CLI handles installs and updates.';

/**
 * Absolute URL for a site-absolute path.
 *
 * `origin` is passed explicitly by server-side callers so `sitemap.xml`,
 * `robots.txt`, and `llms.txt` stay correct behind a proxy or on a preview
 * deployment, where the configured origin and the request origin differ.
 */
export function absolute(path: string, origin: string | URL = SITE_ORIGIN): string {
  return new URL(path, origin.toString()).href;
}

/** Path to an application's public page. */
export function applicationPath(appId: string): string {
  return `/apps/${appId}`;
}

/** Path to an application's answer-engine markdown view. */
export function applicationMarkdownPath(appId: string): string {
  return `/apps/${appId}.md`;
}
