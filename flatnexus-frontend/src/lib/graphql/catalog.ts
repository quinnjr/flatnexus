/**
 * Fetching the whole application catalog.
 *
 * The server clamps `limit` to 100 and does not error when asked for more
 * (`query.rs`: `limit.clamp(1, MAX_PAGE_SIZE)`), so a single request for 5000
 * silently returns the first hundred. Every catalog consumer — the sitemap and
 * both llms endpoints — was doing exactly that, which meant a server with more
 * than 100 applications quietly published a partial catalog while claiming a
 * complete one.
 *
 * This pages until the server returns a short page, and reports whether it
 * stopped because the catalog ran out or because the cap was reached, so a
 * truncated response can say so instead of pretending.
 */

import { gql } from './client';
import { CATALOG_APPLICATIONS, type Application } from './operations';

/** The server's own page ceiling. Requesting more is silently clamped to this. */
export const SERVER_PAGE_SIZE = 100;

export interface CatalogResult {
  applications: Application[];
  /** True when `max` was reached with more applications still available. */
  truncated: boolean;
  /** True when the catalog could not be fetched at all. */
  failed: boolean;
}

/**
 * Page through the catalog up to `max` applications.
 */
export async function fetchCatalog(
  max: number,
  fetchImpl?: typeof globalThis.fetch
): Promise<CatalogResult> {
  const applications: Application[] = [];
  let offset = 0;

  try {
    while (applications.length < max) {
      const page = await gql<{ applications: Application[] }>(
        CATALOG_APPLICATIONS,
        { limit: SERVER_PAGE_SIZE, offset },
        { fetch: fetchImpl }
      );

      applications.push(...page.applications);

      // A short page means the catalog is exhausted. Checking this rather than
      // looping until an empty page saves one request per fetch and terminates
      // even if the server's ceiling changes.
      if (page.applications.length < SERVER_PAGE_SIZE) {
        return { applications, truncated: false, failed: false };
      }

      offset += SERVER_PAGE_SIZE;
    }
  } catch {
    // A partial catalog is more useful than none: an empty sitemap or llms.txt
    // can drop pages out of an index, whereas a short one merely lags.
    return { applications, truncated: false, failed: applications.length === 0 };
  }

  // Stopped on the cap. Whether more exist is exactly the question `truncated`
  // answers, so ask for one more row to find out.
  try {
    const probe = await gql<{ applications: Application[] }>(
      CATALOG_APPLICATIONS,
      { limit: 1, offset: max },
      { fetch: fetchImpl }
    );
    return {
      applications: applications.slice(0, max),
      truncated: probe.applications.length > 0,
      failed: false
    };
  } catch {
    return { applications: applications.slice(0, max), truncated: false, failed: false };
  }
}
