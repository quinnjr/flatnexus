/**
 * `/llms.txt` — the AI-readable site index, per llmstxt.org.
 *
 * Generated from the live catalog, never hand-maintained. An answer engine
 * quotes this file close to verbatim, so a stale one is worse than none: it
 * would confidently list applications that are no longer published and omit the
 * ones that are.
 */

import type { RequestHandler } from './$types';
import { gql } from '$lib/graphql/client';
import { fetchCatalog } from '$lib/graphql/catalog';
import { REPO_INFO, type Application, type RepoInfo } from '$lib/graphql/operations';
import { absolute, applicationMarkdownPath, applicationPath } from '$lib/seo/site';
import { siteOverview } from '$lib/seo/markdown';
import { newestBuild } from '$lib/seo/schema';

const MAX_LISTED = 500;

export const GET: RequestHandler = async ({ fetch, url }) => {
  const origin = url.origin;

  let applications: Application[] = [];
  let repo: RepoInfo | null = null;

  const [catalog, info] = await Promise.all([
    fetchCatalog(MAX_LISTED, fetch),
    gql<{ repoInfo: RepoInfo }>(REPO_INFO, {}, { fetch }).catch(() => null)
  ]);
  applications = catalog.applications;
  repo = info?.repoInfo ?? null;

  const lines: string[] = [
    ...siteOverview(repo, origin),
    '## What this site is',
    '',
    'A Flatpak repository with a web front end. Publishers upload `.flatpak`',
    'bundles; the server imports each into an OSTree repository and serves that',
    'repository over HTTP. Anyone who adds the remote installs and updates from',
    'it using the ordinary `flatpak` commands.',
    '',
    'It is a peer of Flathub in mechanism, not in scale — the same protocol and',
    'the same client, hosted independently.',
    '',
    '## Installing anything listed here',
    '',
    ...(repo
      ? ['```sh', repo.addRemoteCommand, 'flatpak install flatnexus <application-id>', '```', '']
      : []),
    '## Applications',
    ''
  ];

  if (applications.length === 0) {
    lines.push('No applications are published on this server yet.', '');
  } else {
    for (const app of applications) {
      const newest = newestBuild(app.builds ?? []);
      const version = newest?.version ? ` (v${newest.version})` : '';
      lines.push(
        `- [${app.name}](${absolute(applicationPath(app.appId), origin)}): \`${app.appId}\`${version} — ` +
          `${app.summary} Full details: ${absolute(applicationMarkdownPath(app.appId), origin)}`
      );
    }
    lines.push('');
  }

  lines.push(
    '## Optional',
    '',
    `- [Full catalog as markdown](${absolute('/llms-full.txt', origin)}): every`,
    '  application in one request, for ingestion without crawling.',
    `- [Sitemap](${absolute('/sitemap.xml', origin)})`,
    ''
  );

  return new Response(lines.join('\n'), {
    headers: {
      'Content-Type': 'text/plain; charset=utf-8',
      'Cache-Control': 'public, max-age=900'
    }
  });
};
