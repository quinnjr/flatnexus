/**
 * The signed-in session.
 *
 * Persisted to `localStorage` rather than a cookie: the token is a bearer
 * credential for a cross-origin API, so it has to be attached as a header by
 * this client anyway, and a cookie would only add CSRF surface without adding
 * protection.
 */

import { browser } from '$app/environment';
import { derived, writable } from 'svelte/store';

const STORAGE_KEY = 'flatnexus.session';

export type Session = {
  token: string;
  expiresAt: number;
};

function load(): Session | null {
  if (!browser) return null;

  const raw = localStorage.getItem(STORAGE_KEY);
  if (!raw) return null;

  try {
    const parsed = JSON.parse(raw) as Session;
    // An expired token would fail every request; dropping it here means the UI
    // shows a signed-out state instead of a wall of auth errors.
    if (!parsed.token || parsed.expiresAt * 1000 < Date.now()) {
      localStorage.removeItem(STORAGE_KEY);
      return null;
    }
    return parsed;
  } catch {
    localStorage.removeItem(STORAGE_KEY);
    return null;
  }
}

export const session = writable<Session | null>(load());

/**
 * Whether the stored session has expired.
 *
 * `load()` runs once, at module evaluation, so a session that expires while the
 * tab is open stays truthy: the layout keeps offering "Sign out", the bearer
 * token keeps being attached, and the guarded pages take their signed-in branch
 * and then render a wall of auth errors. Guards read this instead of just
 * checking that the store is non-null.
 */
export function isExpired(value: Session | null): boolean {
  return !value || value.expiresAt * 1000 <= Date.now();
}

/** A store that is null both when signed out and when the token has expired. */
export const liveSession = derived(session, (value) => (isExpired(value) ? null : value));

session.subscribe((value) => {
  if (!browser) return;
  if (value) {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(value));
  } else {
    localStorage.removeItem(STORAGE_KEY);
  }
});

export function signIn(token: string, expiresAt: number): void {
  session.set({ token, expiresAt });
}

export function signOut(): void {
  session.set(null);
}
