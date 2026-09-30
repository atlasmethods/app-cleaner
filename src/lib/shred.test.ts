import { describe, expect, it } from 'vitest';
import { isAbsolutePath, parsePaths, pathProblem, summarize } from './shred';

describe('shred helpers', () => {
  it('recognises absolute paths on every platform', () => {
    for (const p of ['/', '/home/a', 'C:\\Users\\a', 'd:/x', '\\\\host\\share\\f']) expect(isAbsolutePath(p)).toBe(true);
    for (const p of ['a/b', './a', '~/a', '..', 'C:', '']) expect(isAbsolutePath(p)).toBe(false);
  });

  it('explains what is wrong with a path', () => {
    expect(pathProblem('')).toMatch(/Enter/);
    expect(pathProblem('notes.txt')).toMatch(/absolute/);
    expect(pathProblem('/ok/file')).toBeNull();
    expect(pathProblem('/bad\u0000')).toMatch(/control/);
  });

  it('splits pasted text into unique paths, dropping quotes and blank lines', () => {
    expect(parsePaths('  /a/b  \r\n\n"/c d/e"\n\'/f\'\n/a/b\n')).toEqual(['/a/b', '/c d/e', '/f']);
    expect(parsePaths('   \n ')).toEqual([]);
  });

  it('summarises results', () => {
    expect(
      summarize([
        { path: '/a', ok: true, bytes: 10 },
        { path: '/b', ok: false, error: 'x', bytes: 0 },
        { path: '/c', ok: true, bytes: 5 },
      ]),
    ).toEqual({ done: 2, failed: 1, bytes: 15 });
  });
});
