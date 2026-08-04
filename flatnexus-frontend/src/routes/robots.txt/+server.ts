/**
 * `/robots.txt`
 *
 * Answer-engine crawlers are allowed explicitly rather than by omission. Several
 * of them (GPTBot, ClaudeBot, PerplexityBot) are increasingly blocked by
 * default at the CDN or template level, and a repository whose entire purpose is
 * to be discovered wants to be quotable. Anything behind authentication is
 * disallowed — those paths render an empty shell to a crawler anyway.
 */

import type { RequestHandler } from './$types';
import { absolute } from '$lib/seo/site';

/** Paths that require a session. Nothing here is useful to a crawler. */
const PRIVATE_PATHS = ['/dashboard', '/settings', '/login', '/register', '/auth'];

/**
 * Crawlers named individually so their access survives a future default-deny
 * rule, and so the intent is legible to whoever reads this file next.
 */
const ANSWER_ENGINES = [
  'GPTBot',
  'OAI-SearchBot',
  'ChatGPT-User',
  'ClaudeBot',
  'Claude-Web',
  'anthropic-ai',
  'PerplexityBot',
  'Google-Extended',
  'Applebot-Extended',
  'CCBot',
  'Bingbot',
  'DuckDuckBot'
];

export const GET: RequestHandler = ({ url }) => {
  const origin = url.origin;
  const disallow = PRIVATE_PATHS.map((path) => `Disallow: ${path}`);

  const lines = [
    '# Public application pages are open to every crawler.',
    'User-agent: *',
    ...disallow,
    'Allow: /',
    '',
    '# Answer engines, allowed explicitly.',
    ...ANSWER_ENGINES.flatMap((agent) => [`User-agent: ${agent}`, ...disallow, 'Allow: /', '']),
    `Sitemap: ${absolute('/sitemap.xml', origin)}`,
    '',
    '# AI-readable index and full corpus (llmstxt.org).',
    `# ${absolute('/llms.txt', origin)}`,
    `# ${absolute('/llms-full.txt', origin)}`,
    ''
  ];

  return new Response(lines.join('\n'), {
    headers: {
      'Content-Type': 'text/plain; charset=utf-8',
      'Cache-Control': 'public, max-age=3600'
    }
  });
};
