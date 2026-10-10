import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, copyFileSync, writeFileSync, existsSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

for (const failure of [true, false]) {
  test(`SEC corpus download ${failure ? 'rejects an HTTP error without caching it' : 'preserves a successful response'}`, () => {
    const root = mkdtempSync(path.join(tmpdir(), 'utopia-sec-'));
    try {
      const bin = path.join(root, 'bin');
      mkdirSync(bin);
      copyFileSync(new URL('./fetch-sec-filings.mjs', import.meta.url), path.join(root, 'fetch-sec-filings.mjs'));
      // curl is the external HTTP boundary; emulate its documented --fail behavior.
      writeFileSync(path.join(bin, 'curl'), '#!/bin/sh\nif [ "$HTTP_FAILURE" = 1 ]; then\n  for arg in "$@"; do [ "$arg" = "--fail" ] && exit 22; done\n  printf "Forbidden"\nelse\n  printf "<html>Filing</html>"\nfi\n', { mode: 0o755 });
      const result = spawnSync(process.execPath, [path.join(root, 'fetch-sec-filings.mjs')], {
        encoding: 'utf8', env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, HTTP_FAILURE: failure ? '1' : '0' },
      });
      const dest = path.join(root, 'corpora/nvda-public-docs/nvda-8k-2026-09-02.html');
      if (failure) {
        assert.notEqual(result.status, 0, 'an HTTP failure must stop corpus generation');
        assert.equal(existsSync(dest), false, 'error pages must not become cached filings');
      } else {
        assert.equal(result.status, 0, result.stderr);
        assert.equal(readFileSync(dest, 'utf8'), '<html>Filing</html>');
      }
    } finally { rmSync(root, { recursive: true, force: true }); }
  });
}
