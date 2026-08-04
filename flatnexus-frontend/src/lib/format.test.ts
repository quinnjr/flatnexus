import { describe, expect, it } from 'vitest';
import { formatBytes, shortHash } from './format';

describe('formatBytes', () => {
  it('keeps raw byte counts whole', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
  });

  it('scales through the units a bundle actually spans', () => {
    expect(formatBytes(1024)).toBe('1.0 KiB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MiB');
    expect(formatBytes(2.5 * 1024 * 1024 * 1024)).toBe('2.5 GiB');
  });

  it('does not invent a size for nonsense input', () => {
    expect(formatBytes(-1)).toBe('—');
    expect(formatBytes(Number.NaN)).toBe('—');
  });
});

describe('shortHash', () => {
  it('truncates to a comparable prefix', () => {
    expect(shortHash('0123456789abcdef0123')).toBe('0123456789ab');
  });
});
