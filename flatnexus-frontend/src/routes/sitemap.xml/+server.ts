/**
 * `/sitemap.xml`
 *
 * Generated from the live application list rather than maintained by hand, so a
 * newly published application is discoverable on its first crawl. `lastmod`
 * comes from the newest build's publish time — the only date on this site that
 * corresponds to the page's content actually changing.
 */

import type { RequestHandler } from './$types';
import { fetchCatalog } from '$lib/graphql/catalog';
import type { Application } from '$lib/graphql/operations';
import { absolute, applicationPath } from '$lib/seo/site';
import { newestBuild } from '$lib/seo/schema';

/** Upper bound on entries. A sitemap is capped at 50,000 URLs by the protocol. */
const MAX_ENTRIES = 5000;

interface Entry {
  path: string;
  lastmod?: string;
  changefreq: string;
  priority: string;
}

export const GET: RequestHandler = async ({ fetch, url }) => {
  const origin = url.origin;

  // Paged, because the server clamps a single request to 100 rows. A one-shot
  // `limit: 5000` returned the first hundred and silently dropped the rest.
  const { applications }: { applications: Application[] } = await fetchCatalog(
    MAX_ENTRIES,
    fetch
  );

  const entries: Entry[] = [
    { path: '/', changefreq: 'hourly', priority: '1.0' },
    ...applications.map((app) => {
      const newest = newestBuild(app.builds ?? []);
      return {
        path: applicationPath(app.appId),
        lastmod: new Date((newest?.publishedAt ?? app.createdAt) * 1000).toISOString(),
        changefreq: 'weekly',
        priority: '0.8'
      };
    })
  ];

  const body = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">',
    ...entries.map((entry) =>
      [
        '  <url>',
        `    <loc>${escapeXml(absolute(entry.path, origin))}</loc>`,
        ...(entry.lastmod ? [`    <lastmod>${entry.lastmod}</lastmod>`] : []),
        `    <changefreq>${entry.changefreq}</changefreq>`,
        `    <priority>${entry.priority}</priority>`,
        '  </url>'
      ].join('\n')
    ),
    '</urlset>',
    ''
  ].join('\n');

  return new Response(body, {
    headers: {
      'Content-Type': 'application/xml; charset=utf-8',
      'Cache-Control': 'public, max-age=3600'
    }
  });
};

function escapeXml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&apos;');
}
