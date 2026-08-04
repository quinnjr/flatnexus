/**
 * Tests for the GraphQL transport's error handling.
 *
 * The cases here are the ones that previously produced a wrong message rather
 * than no message: a non-JSON body from a proxy became a bare `SyntaxError` that
 * every caller reported as "Could not reach the server", and a file with no
 * mapped variable path serialized to `{"0":[null]}` and failed server-side with
 * a message that never mentioned the client had built it wrong.
 */

import { describe, expect, it } from 'vitest';
import { GraphQLRequestError, gql, gqlUpload } from './client';

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' }
  });
}

function textResponse(body: string, status: number): Response {
  return new Response(body, { status, headers: { 'Content-Type': 'text/html' } });
}

const stubFetch = (response: Response) => async () => response;

describe('gql', () => {
  it('returns data on success', async () => {
    const data = await gql<{ ok: boolean }>('query { ok }', {}, {
      fetch: stubFetch(jsonResponse({ data: { ok: true } })) as unknown as typeof fetch
    });
    expect(data).toEqual({ ok: true });
  });

  it('surfaces the server error code so callers can branch on it', async () => {
    const failing = jsonResponse({
      errors: [{ message: 'That one-time code is not valid.', extensions: { code: 'OTP_REJECTED' } }]
    });

    await expect(
      gql('mutation { publishBuild }', {}, { fetch: stubFetch(failing) as unknown as typeof fetch })
    ).rejects.toMatchObject({ code: 'OTP_REJECTED' });
  });

  it('flags an OTP failure distinctly', async () => {
    const failing = jsonResponse({
      errors: [{ message: 'nope', extensions: { code: 'OTP_REJECTED' } }]
    });

    const error = await gql('mutation { x }', {}, {
      fetch: stubFetch(failing) as unknown as typeof fetch
    }).catch((e) => e as GraphQLRequestError);

    expect(error).toBeInstanceOf(GraphQLRequestError);
    expect((error as GraphQLRequestError).isOtpFailure).toBe(true);
  });

  it('turns a non-JSON body into a typed error rather than a SyntaxError', async () => {
    // A proxy error page, adapter-node's own 413, an auth gateway's HTML.
    const proxyError = textResponse('<html>413 Request Entity Too Large</html>', 413);

    const error = await gql('mutation { x }', {}, {
      fetch: stubFetch(proxyError) as unknown as typeof fetch
    }).catch((e) => e as GraphQLRequestError);

    expect(error).toBeInstanceOf(GraphQLRequestError);
    expect((error as GraphQLRequestError).code).toBe('UPLOAD_REJECTED');
    expect((error as GraphQLRequestError).message).toContain('larger than');
  });

  it('maps an unauthenticated non-JSON response to UNAUTHENTICATED', async () => {
    const error = await gql('query { me }', {}, {
      fetch: stubFetch(textResponse('nope', 401)) as unknown as typeof fetch
    }).catch((e) => e as GraphQLRequestError);

    expect((error as GraphQLRequestError).code).toBe('UNAUTHENTICATED');
  });

  it('rejects a response carrying neither data nor errors', async () => {
    const error = await gql('query { x }', {}, {
      fetch: stubFetch(jsonResponse({})) as unknown as typeof fetch
    }).catch((e) => e as GraphQLRequestError);

    expect((error as GraphQLRequestError).code).toBe('INTERNAL_ERROR');
  });
});

describe('gqlUpload', () => {
  it('refuses to build a request with an unmapped file', async () => {
    // Left undefined this produced `{"0":[null]}` — a malformed request the
    // server rejected with a message that blamed the wrong thing.
    const file = new File(['bytes'], 'app.flatpak');

    await expect(
      gqlUpload('mutation { publishBuild }', { input: {} }, { bundle: file }, {})
    ).rejects.toMatchObject({ code: 'VALIDATION_ERROR' });
  });
});
