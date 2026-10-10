import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, copyFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

for (const [flag, value] of [['n', '-1'], ['n', '0'], ['n', '1.5'], ['n', 'NaN'], ['n', 'Infinity'], ['seed', '1.5'], ['seed', '-1'], ['seed', '4294967296'], ['seed', 'NaN']]) {
  test(`reject invalid --${flag} ${value} before fetching or sampling`, () => {
    const root = mkdtempSync(path.join(tmpdir(), 'utopia-redocred-'));
    try {
      copyFileSync(new URL('./fetch-redocred.mjs', import.meta.url), path.join(root, 'fetch-redocred.mjs'));
      copyFileSync(new URL('./lib.mjs', import.meta.url), path.join(root, 'lib.mjs'));
      writeFileSync(path.join(root, 'offline.mjs'), 'globalThis.fetch = () => { throw new Error("unexpected network call"); };');
      mkdirSync(path.join(root, 'raw'));
      const result = spawnSync(process.execPath, ['--import', path.join(root, 'offline.mjs'), path.join(root, 'fetch-redocred.mjs'), `--${flag}`, value, '--dir', path.join(root, 'raw')], { encoding: 'utf8' });
      assert.equal(result.status, 2, result.stderr);
      assert.match(result.stderr, new RegExp(`--${flag}`));
      assert.doesNotMatch(result.stderr, /unexpected network call/);
    } finally { rmSync(root, { recursive: true, force: true }); }
  });
}
