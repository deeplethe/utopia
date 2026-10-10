import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

function chart(outName, keepPaging = false) {
  const dir = mkdtempSync(join(tmpdir(), "utopia-stars-"));
  const fixture = join(dir, "fixture.mjs");
  writeFileSync(fixture, `let cursor = 0; Date.now = () => Date.UTC(2026, 0, 1); globalThis.fetch = async () => ({ ok: true, json: async () => ({ data: { repository: { stargazerCount: 401, viewerPermission: "ADMIN", stargazers: { totalCount: 401, edges: [{ starredAt: "2026-01-01T00:00:00Z" }], pageInfo: { hasNextPage: ${keepPaging}, endCursor: String(++cursor) } } } } }) });`);
  const out = join(dir, outName);
  const result = spawnSync(process.execPath, ["--import", fixture, resolve("scripts/star-history.mjs")], {
    env: { ...process.env, REPO: "fixture/project", GITHUB_TOKEN: "fixture-not-a-credential", OUT: out },
    encoding: "utf8",
  });
  return { dir, out, result };
}

test("an unfinished timeline cannot be published as a complete chart", () => {
  const { dir, result } = chart("stars.svg", true);
  try {
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /400.*page|page.*400/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
