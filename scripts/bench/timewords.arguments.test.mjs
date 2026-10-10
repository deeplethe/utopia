import assert from 'node:assert/strict';
import { mkdtempSync, copyFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

for (const value of ['0', '-1', '1.5', 'NaN', 'Infinity']) {
  test(`reject invalid --runs ${value} before login or corpus loading`, () => {
    const root = mkdtempSync(path.join(tmpdir(), 'utopia-timewords-'));
    try {
      copyFileSync(new URL('./timewords.mjs', import.meta.url), path.join(root, 'timewords.mjs'));
      copyFileSync(new URL('./lib.mjs', import.meta.url), path.join(root, 'lib.mjs'));
      writeFileSync(path.join(root, 'offline.mjs'), 'globalThis.fetch = () => { throw new Error("unexpected network call"); };');
      const result = spawnSync(process.execPath, ['--import', path.join(root, 'offline.mjs'), path.join(root, 'timewords.mjs'), '--db', 'bench', '--runs', value], { encoding: 'utf8' });
      assert.equal(result.status, 2, result.stderr);
      assert.match(result.stderr, /--runs/);
      assert.doesNotMatch(result.stderr, /unexpected network call|ENOENT/);
    } finally { rmSync(root, { recursive: true, force: true }); }
  });
}
