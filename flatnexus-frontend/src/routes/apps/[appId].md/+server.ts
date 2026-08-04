/**
 * `/apps/<app-id>.md` — one application as plain markdown.
 *
 * Linked from the HTML page via `rel="alternate"` and named in `llms.txt`, so an
 * agent that lands on the page can fetch the prose rather than scrape the DOM.
 */

import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { gql, GraphQLRequestError } from '$lib/graphql/client';
import { APPLICATION_MARKDOWN, type Application, type RepoInfo } from '$lib/graphql/operations';
import { applicationMarkdown } from '$lib/seo/markdown';

export const GET: RequestHandler = async ({ fetch, params, url }) => {
  let app: Application | null = null;
  let repo: RepoInfo | null = null;

  try {
    const data = await gql<{ application: Application | null; repoInfo: RepoInfo }>(
      APPLICATION_MARKDOWN,
      { appId: params.appId },
      { fetch }
    );
    app = data.application;
    repo = data.repoInfo;
  } catch (e) {
    error(502, e instanceof GraphQLRequestError ? e.message : 'Could not reach the server.');
  }

  // Matches the HTML route's behaviour: a genuine 404, so a consumer does not
  // cache a page describing an application that does not exist.
  if (!app) {
    error(404, `No application '${params.appId}' on this server.`);
  }

  return new Response(applicationMarkdown(app, repo, url.origin), {
    headers: {
      'Content-Type': 'text/markdown; charset=utf-8',
      'Cache-Control': 'public, max-age=900'
    }
  });
};
