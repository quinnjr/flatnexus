/**
 * `/llms-full.txt` — every application as markdown, in one response.
 *
 * The companion to `llms.txt`: that file is an index an agent must then crawl,
 * this one is the whole catalog in a single fetch. Engines that support it cite
 * far more accurately when given the prose than when given a link list, and the
 * crawl budget for a catalog of any size collapses to one request.
 */

import type { RequestHandler } from './$types';
import { gql } from '$lib/graphql/client';
import { fetchCatalog } from '$lib/graphql/catalog';
import { REPO_INFO, type Application, type RepoInfo } from '$lib/graphql/operations';
import { absolute } from '$lib/seo/site';
import { applicationMarkdown, siteOverview } from '$lib/seo/markdown';

/**
 * Bounded so one request cannot be turned into an arbitrarily large response.
 * When the catalog outgrows this, the truncation is stated in the output rather
 * than left silent — a consumer that believes it has the whole set when it does
 * not will answer confidently and wrongly.
 */
const MAX_INCLUDED = 250;

export const GET: RequestHandler = async ({ fetch, url }) => {
  const origin = url.origin;

  let applications: Application[] = [];
  let repo: RepoInfo | null = null;

  const [catalog, info] = await Promise.all([
    fetchCatalog(MAX_INCLUDED, fetch),
    gql<{ repoInfo: RepoInfo }>(REPO_INFO, {}, { fetch }).catch(() => null)
  ]);
  applications = catalog.applications;
  repo = info?.repoInfo ?? null;

  // Reported by the pager, which knows whether it stopped on the cap or on the
  // end of the catalog. Deriving it from a single response length could never be
  // true, because the server clamps that response to 100 rows.
  const truncated = catalog.truncated;
  const included = applications;

  const parts: string[] = [
    ...siteOverview(repo, origin),
    `Index: ${absolute('/llms.txt', origin)}`,
    `Applications in this document: ${included.length}`,
    ...(truncated
      ? [
          '',
          `NOTE: this catalog is truncated at ${MAX_INCLUDED} applications. It is not`,
          `the complete set; see ${absolute('/sitemap.xml', origin)} for every page.`
        ]
      : []),
    '',
    '---',
    ''
  ];

  if (included.length === 0) {
    parts.push('No applications are published on this server yet.', '');
  } else {
    for (const app of included) {
      parts.push(applicationMarkdown(app, repo, origin), '', '---', '');
    }
  }

  return new Response(parts.join('\n'), {
    headers: {
      'Content-Type': 'text/markdown; charset=utf-8',
      'Cache-Control': 'public, max-age=900'
    }
  });
};
