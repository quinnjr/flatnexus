/**
 * Per-page metadata.
 *
 * A page describes itself by returning a [`PageMeta`] from its `load`, which
 * `<Seo />` renders. Anything a page does not set falls back to the site
 * defaults, so a new route is never silently untitled.
 */

import { SITE_DESCRIPTION, SITE_NAME, SITE_TAGLINE, absolute } from './site';

export interface PageAlternate {
  href: string;
  type: string;
  title?: string;
}

export interface PageMeta {
  /** Page title, without the site suffix. */
  title: string;
  description: string;
  /** Site-absolute path this page is canonically served at. */
  path: string;
  /**
   * Keep the page out of search indexes.
   *
   * Set on everything behind authentication. These pages render an empty shell
   * to a crawler anyway, and an indexed sign-in form competes with the pages
   * that should rank.
   */
  noindex?: boolean;
  /** Absolute URL of a social preview image. */
  image?: string;
  /** JSON-LD documents to embed, already serialized. */
  schemas?: string[];
  /**
   * Alternate representations of *this* page.
   *
   * The site-wide `llms.txt` used to be the only `rel="alternate"` emitted on
   * every route, so an agent doing head-only discovery on an application page was
   * pointed at the index rather than at that application's own markdown.
   */
  alternates?: PageAlternate[];
  /** ISO-8601 last-modified, when the page has a meaningful one. */
  dateModified?: string;
}

/** Full `<title>` text: page title plus the site name, except on the home page. */
export function formatTitle(meta: PageMeta): string {
  return meta.title === SITE_NAME ? `${SITE_NAME} — ${SITE_TAGLINE}` : `${meta.title} — ${SITE_NAME}`;
}

/** Metadata for a page that has nothing specific to say. */
export function defaultMeta(path: string): PageMeta {
  return {
    title: SITE_NAME,
    description: SITE_DESCRIPTION,
    path
  };
}

/**
 * Metadata for a page that must not be indexed.
 *
 * The description is still set: it is what a link preview shows when someone
 * pastes the URL into a chat, which happens to private pages constantly.
 */
export function privateMeta(title: string, description: string, path: string): PageMeta {
  return { title, description, path, noindex: true };
}

/** Canonical URL for a page. */
export function canonicalUrl(meta: PageMeta, origin?: string | URL): string {
  return absolute(meta.path, origin);
}
