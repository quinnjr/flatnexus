/**
 * Plain-markdown renderings of the site's content, for answer engines.
 *
 * These are not a fallback for browsers — they are the primary artifact for
 * consumers that do not run JavaScript and do not want to parse a page's
 * chrome. An engine given prose cites it accurately; an engine given HTML
 * paraphrases it, and paraphrasing an install command produces a command that
 * does not work.
 */

import type { Application, RepoInfo } from '$lib/graphql/operations';
import { formatBytes } from '$lib/format';
import { SITE_NAME, absolute, applicationPath } from './site';
import { newestBuild, uniqueArches } from './schema';

/** One application as a self-contained markdown document. */
export function applicationMarkdown(
  app: Application,
  repo: RepoInfo | null,
  origin?: string | URL
): string {
  const canonical = absolute(applicationPath(app.appId), origin);
  const builds = app.builds ?? [];
  const newest = newestBuild(builds);

  const lines: string[] = [
    `# ${app.name}`,
    '',
    `> ${app.summary}`,
    '',
    `Application ID: \`${app.appId}\``,
    `Canonical page: ${canonical}`
  ];

  if (newest?.version) lines.push(`Latest version: ${newest.version}`);
  if (builds.length > 0) lines.push(`Architectures: ${uniqueArches(builds).join(', ')}`);
  if (app.license) lines.push(`License: ${app.license}`);
  if (app.homepage) lines.push(`Homepage: ${app.homepage}`);
  if (app.publisher) lines.push(`Published by: ${app.publisher.displayName}`);
  if (newest) {
    lines.push(`Last published: ${isoDate(newest.publishedAt)}`);
  }

  lines.push('', '## Install', '');

  if (repo) {
    lines.push(
      'Add the remote once, then install:',
      '',
      '```sh',
      repo.addRemoteCommand,
      app.installCommand,
      '```',
      ''
    );
    lines.push(
      repo.gpgSigned
        ? 'Commits in this repository are GPG-signed; flatpak verifies signatures on install.'
        : 'This repository does not sign its commits, so flatpak cannot verify what it installs.'
    );
  } else {
    lines.push('```sh', app.installCommand, '```');
  }

  lines.push(
    '',
    `Single-click install file: ${app.flatpakrefUrl}`,
    '',
    'Updates arrive through `flatpak update` like any other remote — this is an',
    'ordinary OSTree repository, not a bespoke distribution mechanism.'
  );

  if (app.description) {
    lines.push('', '## About', '', app.description.trim());
  }

  const stills = (app.screenshots ?? []).filter((item) => item.isStillImage);
  if (stills.length > 0) {
    lines.push('', '## Screenshots', '');
    stills.forEach((item, index) => {
      // Markdown image syntax, so a consumer that renders this shows the picture
      // rather than a bare link. The URLs are signed and short-lived, which is
      // stated so nothing caches them as permanent.
      lines.push(`![${app.name} screenshot ${index + 1}](${item.url})`);
    });
    lines.push('', '_Screenshot links are signed and expire; re-fetch this page for fresh ones._');
  }

  if (builds.length > 0) {
    lines.push('', '## Builds', '', '| Ref | Version | Arch | Size | Commit | Published |', '| --- | --- | --- | --- | --- | --- |');
    for (const build of builds) {
      lines.push(
        `| \`${build.flatpakRef}\` | ${build.version || '—'} | ${build.arch} | ` +
          `${formatBytes(build.sizeBytes)} | \`${build.commit.slice(0, 12)}\` | ${isoDate(build.publishedAt)} |`
      );
    }
  } else {
    lines.push('', '## Builds', '', 'No builds have been published yet, so there is nothing to install.');
  }

  lines.push('');
  return lines.join('\n');
}

/** Shared preamble describing what this site is. */
export function siteOverview(repo: RepoInfo | null, origin?: string | URL): string[] {
  return [
    `# ${SITE_NAME}`,
    '',
    '> A self-hosted Flatpak distribution point. Applications published here are',
    '> real Flatpak refs in an OSTree repository, installed and updated by the',
    '> standard `flatpak` CLI with no client-side changes.',
    '',
    `Site: ${absolute('/', origin)}`,
    ...(repo
      ? [
          `Repository: ${repo.url}`,
          `Add the remote: \`${repo.addRemoteCommand}\``,
          `GPG-signed: ${repo.gpgSigned ? 'yes' : 'no'}`
        ]
      : []),
    ''
  ];
}

function isoDate(epochSeconds: number): string {
  return new Date(epochSeconds * 1000).toISOString().slice(0, 10);
}
