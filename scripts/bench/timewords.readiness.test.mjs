import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, copyFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

for (const phase of ['documents', 'extraction', 'complete']) {
  test(`time-word scoring requires completed ${phase} work`, () => {
    const root = mkdtempSync(path.join(tmpdir(), 'utopia-timewords-ready-'));
    try {
      copyFileSync(new URL('./timewords.mjs', import.meta.url), path.join(root, 'timewords.mjs'));
      copyFileSync(new URL('./lib.mjs', import.meta.url), path.join(root, 'lib.mjs'));
      mkdirSync(path.join(root, 'truth'));
      writeFileSync(path.join(root, 'truth/timewords.json'), JSON.stringify({ docs: [] }));
      const fake = path.join(root, 'psql');
      writeFileSync(fake, '#!/bin/sh\ncase "$*" in\n  *"status<>"*) [ "$PHASE" = documents ] && printf 1 || printf 0;;\n  *"FROM jobs"*) [ "$PHASE" = extraction ] && printf 1 || printf 0;;\n  *) printf 0;;\nesac\n', { mode: 0o755 });
      writeFileSync(path.join(root, 'offline.mjs'), `
        globalThis.setTimeout = (fn) => { queueMicrotask(fn); return 0; };
        globalThis.fetch = async (url) => {
          const pathname = new URL(url).pathname;
          const body = pathname.endsWith('/auth/login') ? {} : pathname.endsWith('/workspaces') ? [{ id: 'ws' }] : pathname.includes('/documents') ? { docs: [] } : { id: 'kb' };
          return { ok: true, headers: { getSetCookie: () => [] }, text: async () => JSON.stringify(body) };
        };
      `);
      const result = spawnSync(process.execPath, ['--import', path.join(root, 'offline.mjs'), path.join(root, 'timewords.mjs'), '--db', 'bench', '--runs', '1'], { encoding: 'utf8', env: { ...process.env, BENCH_PSQL: `${fake} -d unused`, PHASE: phase }, timeout: 30000 });
      if (phase === 'complete') {
        assert.equal(result.status, 0, result.stderr);
        assert.match(result.stdout, /statements in the base/);
      } else {
        assert.equal(result.status, 1, result.stderr);
        assert.match(result.stderr, /not finish|not ready|still pending|timed out/i);
        assert.doesNotMatch(result.stdout, /statements in the base|sentences with the same outcome/);
      }
    } finally { rmSync(root, { recursive: true, force: true }); }
  });
}
