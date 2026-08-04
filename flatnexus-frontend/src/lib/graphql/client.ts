/**
 * GraphQL transport.
 *
 * Two entry points, because the API has two transports:
 *
 * - `gql` for ordinary JSON operations.
 * - `gqlUpload` for the multipart request spec, which is how a `.flatpak`
 *   bundle reaches `publishBuild`.
 *
 * Errors are normalized into `GraphQLRequestError`, which carries the server's
 * `extensions.code`. Callers branch on that code rather than on message text,
 * so wording changes on the server never break a UI check.
 */

import { PUBLIC_API_URL } from '$env/static/public';
import { session, signOut } from '$lib/stores/session';
import { get } from 'svelte/store';

export const API_URL = PUBLIC_API_URL || 'http://localhost:4000';
const ENDPOINT = `${API_URL}/graphql`;

export type ErrorCode =
  | 'UNAUTHENTICATED'
  | 'FORBIDDEN'
  | 'OTP_REJECTED'
  | 'VALIDATION_ERROR'
  | 'NOT_FOUND'
  | 'CONFLICT'
  | 'UPLOAD_REJECTED'
  | 'REPO_ERROR'
  | 'INTERNAL_ERROR'
  | 'UNKNOWN';

export class GraphQLRequestError extends Error {
  readonly code: ErrorCode;

  constructor(message: string, code: ErrorCode) {
    super(message);
    this.name = 'GraphQLRequestError';
    this.code = code;
  }

  /** True when the failure is specifically a bad or spent one-time password. */
  get isOtpFailure(): boolean {
    return this.code === 'OTP_REJECTED';
  }
}

type GraphQLResponse<T> = {
  data?: T;
  errors?: Array<{ message: string; extensions?: { code?: string } }>;
};

function authHeaders(token?: string | null): Record<string, string> {
  const active = token ?? get(session)?.token;
  return active ? { Authorization: `Bearer ${active}` } : {};
}

function unwrap<T>(payload: GraphQLResponse<T>): T {
  const failure = payload.errors?.[0];
  if (failure) {
    const code = (failure.extensions?.code as ErrorCode) ?? 'UNKNOWN';
    // A dead token would otherwise fail every subsequent request while the UI
    // kept showing a signed-in state, because the session store only checks
    // expiry once, at module load.
    if (code === 'UNAUTHENTICATED') {
      signOut();
    }
    throw new GraphQLRequestError(failure.message, code);
  }
  if (!payload.data) {
    throw new GraphQLRequestError('The server returned an empty response.', 'INTERNAL_ERROR');
  }
  return payload.data;
}

/** Run a JSON GraphQL operation. */
export async function gql<T>(
  query: string,
  variables: Record<string, unknown> = {},
  options: { token?: string | null; fetch?: typeof globalThis.fetch } = {}
): Promise<T> {
  const doFetch = options.fetch ?? fetch;

  const response = await doFetch(ENDPOINT, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...authHeaders(options.token)
    },
    body: JSON.stringify({ query, variables })
  });

  return unwrap<T>(await readPayload(response));
}

/**
 * Parse a response body that is supposed to be a GraphQL payload.
 *
 * A non-JSON body is expected on any hop that is not the API itself — a proxy
 * error page, adapter-node's own 413 on an oversized upload, an auth gateway's
 * HTML. Previously only `status >= 500` was intercepted, so those cases threw a
 * bare `SyntaxError` from `response.json()`, which every caller then reported as
 * "Could not reach the server" — replacing a specific cause with a wrong one.
 */
async function readPayload<T>(response: Response): Promise<GraphQLResponse<T>> {
  const text = await response.text();

  try {
    return JSON.parse(text) as GraphQLResponse<T>;
  } catch {
    throw new GraphQLRequestError(
      statusMessage(response.status),
      codeForStatus(response.status)
    );
  }
}

function statusMessage(status: number): string {
  switch (status) {
    case 401:
      return 'Your session has expired. Sign in again.';
    case 413:
      return 'That upload is larger than the server accepts.';
    default:
      return `The server responded with ${status}.`;
  }
}

function codeForStatus(status: number): ErrorCode {
  switch (status) {
    case 401:
      return 'UNAUTHENTICATED';
    case 403:
      return 'FORBIDDEN';
    case 404:
      return 'NOT_FOUND';
    case 413:
      return 'UPLOAD_REJECTED';
    default:
      return status >= 500 ? 'INTERNAL_ERROR' : 'UNKNOWN';
  }
}

/**
 * Run a GraphQL operation carrying a file, per the multipart request spec.
 *
 * `filePaths` maps each file to the variable path it fills, e.g.
 * `{ bundle: 'variables.input.bundle' }`. The corresponding variable must be
 * `null` in `variables` — that null is the placeholder the server replaces.
 *
 * `onProgress` reports upload percentage, which is why this uses
 * `XMLHttpRequest`: `fetch` still has no portable request-progress event, and a
 * multi-gigabyte bundle upload with no feedback reads as a hung page.
 */
/**
 * Build the `files` and `filePaths` arguments for an ordered list of uploads.
 *
 * The multipart spec addresses a list slot by index — `variables.input.screenshots.0` —
 * and the JSON operations part must carry a `null` in each of those slots. This
 * returns both halves so a caller cannot get the two out of step.
 */
export function uploadList(
  files: File[],
  basePath: string
): { files: Record<string, File>; filePaths: Record<string, string>; placeholders: null[] } {
  const named: Record<string, File> = {};
  const paths: Record<string, string> = {};

  files.forEach((file, index) => {
    const key = `${basePath}.${index}`;
    named[key] = file;
    paths[key] = `variables.${basePath}.${index}`;
  });

  return { files: named, filePaths: paths, placeholders: files.map(() => null) };
}

export async function gqlUpload<T>(
  query: string,
  variables: Record<string, unknown>,
  files: Record<string, File>,
  filePaths: Record<string, string>,
  options: { token?: string | null; onProgress?: (percent: number) => void } = {}
): Promise<T> {
  // `async` so the argument validation below rejects rather than throwing
  // synchronously. A Promise-returning function that sometimes throws before
  // returning forces callers to write both a `try` and a `.catch` to be safe.
  const form = new FormData();
  form.append('operations', JSON.stringify({ query, variables }));

  const map: Record<string, string[]> = {};
  const names = Object.keys(files);
  names.forEach((name, index) => {
    const path = filePaths[name];
    if (!path) {
      // Left undefined this serialized to `{"0":[null]}`, which the server
      // rejects with a low-level message that never mentions the client built a
      // malformed request.
      throw new GraphQLRequestError(
        `No variable path was given for the file "${name}".`,
        'VALIDATION_ERROR'
      );
    }
    map[String(index)] = [path];
  });
  form.append('map', JSON.stringify(map));
  names.forEach((name, index) => {
    form.append(String(index), files[name]);
  });

  return new Promise<T>((resolve, reject) => {
    const request = new XMLHttpRequest();
    request.open('POST', ENDPOINT);

    const headers = authHeaders(options.token);
    for (const [key, value] of Object.entries(headers)) {
      request.setRequestHeader(key, value);
    }

    if (options.onProgress) {
      request.upload.addEventListener('progress', (event) => {
        if (event.lengthComputable) {
          options.onProgress?.(Math.round((event.loaded / event.total) * 100));
        }
      });
    }

    request.addEventListener('load', () => {
      let payload: GraphQLResponse<T>;
      try {
        payload = JSON.parse(request.responseText) as GraphQLResponse<T>;
      } catch {
        // Same reasoning as `readPayload`: an upload is the likeliest request to
        // be answered by a proxy rather than the API.
        reject(
          new GraphQLRequestError(
            statusMessage(request.status),
            codeForStatus(request.status)
          )
        );
        return;
      }

      try {
        resolve(unwrap<T>(payload));
      } catch (error) {
        reject(
          error instanceof GraphQLRequestError
            ? error
            : new GraphQLRequestError('The server returned an unreadable response.', 'UNKNOWN')
        );
      }
    });

    request.addEventListener('error', () => {
      reject(new GraphQLRequestError('The upload could not reach the server.', 'UNKNOWN'));
    });

    request.addEventListener('abort', () => {
      reject(new GraphQLRequestError('The upload was cancelled.', 'UNKNOWN'));
    });

    request.send(form);
  });
}
