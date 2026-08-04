/**
 * Account settings are behind a session that only exists in the browser.
 *
 * The session lives in `localStorage`, so `load()` in the session store returns
 * null during SSR and every guard here renders its signed-out branch. The server
 * HTML therefore told a signed-in publisher to sign in, and only hydration
 * corrected it. These routes are `noindex` anyway, so there is nothing to lose
 * by rendering them on the client alone — and a brief empty frame beats a wrong
 * one.
 */
export const ssr = false;
