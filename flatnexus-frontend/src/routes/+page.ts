/**
 * Browse page data.
 *
 * Loaded here rather than in `onMount` so the application list is present in the
 * server-rendered HTML. A crawler — or an answer engine, which generally does
 * not execute JavaScript at all — otherwise sees an empty shell, and no amount
 * of metadata makes an empty page rank or get cited.
 */

import type { PageLoad } from './$types';
import { gql } from '$lib/graphql/client';
import {
  BROWSE_APPLICATIONS,
  REPO_INFO,
  type Application,
  type RepoInfo
} from '$lib/graphql/operations';
import { SITE_DESCRIPTION, SITE_NAME, applicationPath } from '$lib/seo/site';
import { collectionSchema, repositorySchema, websiteSchema, faqSchema } from '$lib/seo/schema';
import type { PageMeta } from '$lib/seo/meta';
import { faqEntries } from '$lib/seo/faq';

export const load: PageLoad = async ({ fetch, url }) => {
  const search = url.searchParams.get('search') ?? '';
  const origin = url.origin;

  // A failure here must not blank the page: the site still has a purpose to
  // explain even when the API is down, and returning a 500 would drop it out of
  // the index over a transient outage.
  let applications: Application[] = [];
  let repo: RepoInfo | null = null;
  let error = '';

  try {
    const [browse, info] = await Promise.all([
      gql<{ applications: Application[] }>(
        BROWSE_APPLICATIONS,
        { search: search || null, limit: 50 },
        { fetch }
      ),
      gql<{ repoInfo: RepoInfo }>(REPO_INFO, {}, { fetch })
    ]);
    applications = browse.applications;
    repo = info.repoInfo;
  } catch {
    error = 'Could not reach the server.';
  }

  const schemas = [
    websiteSchema(origin),
    collectionSchema(
      {
        name: `Applications on ${SITE_NAME}`,
        description: SITE_DESCRIPTION,
        path: '/',
        items: applications.map((app) => ({
          name: app.name,
          path: applicationPath(app.appId)
        }))
      },
      origin
    ),
  ];

  if (repo) {
    schemas.push(repositorySchema(repo, origin));
    // Emitted only alongside the visible section, which is itself gated on
    // `repo`. A `FAQPage` with an empty `mainEntity` is invalid structured data,
    // and it would have been emitted on the site's most important page on
    // exactly the day the API was flaky.
    schemas.push(faqSchema(faqEntries(repo), '/', origin));
  }

  const meta: PageMeta = {
    title: SITE_NAME,
    description: SITE_DESCRIPTION,
    path: '/',
    schemas
  };

  return { applications, repo, search, error, meta };
};
