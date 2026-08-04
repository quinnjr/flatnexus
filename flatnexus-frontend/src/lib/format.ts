/** Small presentation helpers shared across routes. */

const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];

/** Human-readable byte size. Bundles range from a few MiB to several GiB. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—';

  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  // Whole bytes never need a decimal; anything larger reads better with one.
  return `${unit === 0 ? value : value.toFixed(1)} ${UNITS[unit]}`;
}

/** Epoch seconds as a locale date. */
export function formatDate(epochSeconds: number): string {
  return new Date(epochSeconds * 1000).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric'
  });
}

/** First 12 characters of a checksum, which is enough to eyeball a match. */
export function shortHash(hash: string): string {
  return hash.slice(0, 12);
}
