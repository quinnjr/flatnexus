/**
 * Application detail data.
 *
 * Server-loaded for the same reason as the browse page: this is the page that
 * should rank for an application's name, and it cannot if its content only
 * exists after hydration.
 */

import { error } from '@sveltejs/kit';
import type { PageLoad } from './$types';
import { gql, GraphQLRequestError } from '$lib/graphql/client';
import { APPLICATION_DETAIL, type Application, type RepoInfo } from '$lib/graphql/operations';
import { applicationMarkdownPath, applicationPath } from '$lib/seo/site';
import {
  applicationSchema,
  breadcrumbSchema,
  installHowToSchema,
  newestBuild
} from '$lib/seo/schema';
import type { PageMeta } from '$lib/seo/meta';

export const load: PageLoad = async ({ fetch, params, url }) => {
  const origin = url.origin;

  let app: Application | null = null;
  let repo: RepoInfo | null = null;

  try {
    const data = await gql<{ application: Application | null; repoInfo: RepoInfo }>(
      APPLICATION_DETAIL,
      { appId: params.appId },
      { fetch }
    );
    app = data.application;
    repo = data.repoInfo;
  } catch (e) {
    const message = e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.';
    error(502, message);
  }

  // A real 404 rather than a rendered "not found": an unknown application must
  // not be indexed, and a soft 404 is how a site accumulates thousands of
  // indexed empty pages.
  if (!app) {
    error(404, `No application '${params.appId}' on this server.`);
  }

  const newest = newestBuild(app.builds ?? []);

  const schemas = [
    applicationSchema(app, repo, origin),
    breadcrumbSchema(
      [
        { name: 'Applications', path: '/' },
        { name: app.name, path: applicationPath(app.appId) }
      ],
      applicationPath(app.appId),
      origin
    )
  ];

  if (repo) schemas.push(installHowToSchema(app, repo, origin));

  const meta: PageMeta = {
    title: app.name,
    // The summary is the publisher's own one-line description, which is a better
    // snippet than anything generated from the page would be.
    description: `${app.summary} Install ${app.appId} with Flatpak from this repository.`,
    path: applicationPath(app.appId),
    schemas,
    // The markdown view of *this* application, so an agent reading only the head
    // is pointed at the right document rather than the site index.
    alternates: [
      {
        href: applicationMarkdownPath(app.appId),
        type: 'text/markdown',
        title: `${app.name} as markdown`
      }
    ],
    dateModified: newest ? new Date(newest.publishedAt * 1000).toISOString() : undefined
  };

  return { app, repo, meta };
};
