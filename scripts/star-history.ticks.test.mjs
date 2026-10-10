import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

function chart(outName, keepPaging = false) {
  const dir = mkdtempSync(join(tmpdir(), "utopia-stars-"));
  const fixture = join(dir, "fixture.mjs");
  writeFileSync(fixture, `Date.now = () => Date.UTC(2026, 0, 1); globalThis.fetch = async () => ({ ok: true, json: async () => ({ data: { repository: { stargazerCount: 1, viewerPermission: "ADMIN", stargazers: { totalCount: 1, edges: [{ starredAt: "2026-01-01T00:00:00Z" }], pageInfo: { hasNextPage: ${keepPaging}, endCursor: "cursor" } } } } }) });`);
  const out = join(dir, outName);
  const result = spawnSync(process.execPath, ["--import", fixture, resolve("scripts/star-history.mjs")], {
    env: { ...process.env, REPO: "fixture/project", GITHUB_TOKEN: "fixture-not-a-credential", OUT: out },
    encoding: "utf8",
  });
  return { dir, out, result };
}

test("one star does not produce repeated integer tick labels", () => {
  const { dir, out, result } = chart("stars.svg");
  try {
    assert.equal(result.status, 0, result.stderr);
    const svg = readFileSync(out, "utf8");
    assert.equal((svg.match(/>0<\/text>/g) ?? []).length, 1);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
