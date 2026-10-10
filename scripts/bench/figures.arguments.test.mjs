import assert from 'node:assert/strict';
import { mkdtempSync, copyFileSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

for (const args of [[], ['--other', 'value'], ['--kb'], ['--kb', '--other']]) {
  test(`missing knowledge-base value is rejected: ${JSON.stringify(args)}`, () => {
    const root = mkdtempSync(path.join(tmpdir(), 'utopia-figures-'));
    try {
      copyFileSync(new URL('./figures.mjs', import.meta.url), path.join(root, 'figures.mjs'));
      const called = path.join(root, 'called');
      const fake = path.join(root, 'psql');
      writeFileSync(fake, `#!/bin/sh\ntouch '${called}'\n`, { mode: 0o755 });
      const result = spawnSync(process.execPath, [path.join(root, 'figures.mjs'), ...args], { encoding: 'utf8', env: { ...process.env, BENCH_PSQL: fake } });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /--kb/);
      assert.equal(existsSync(called), false, 'invalid invocation must not reach the database');
    } finally { rmSync(root, { recursive: true, force: true }); }
  });
}
