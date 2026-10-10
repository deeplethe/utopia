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

test("an extensionless output keeps separate dark and light charts", () => {
  const { dir, out, result } = chart("stars");
  try {
    assert.equal(result.status, 0, result.stderr);
    assert.match(readFileSync(out, "utf8"), /fill="#0d1117"/);
    assert.match(readFileSync(`${out}-light.svg`, "utf8"), /fill="#ffffff"/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
