import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
const script = new URL("./subset-corpus.mjs", import.meta.url);
function subset(docs) {
  const dir = mkdtempSync(join(tmpdir(), "utopia-corpus-order-"));
  try {
    const input = join(dir, "corpus.json"); writeFileSync(input, JSON.stringify({name:"fixture",docs}));
    const result = spawnSync(process.execPath, [script.pathname,input,"item"], {encoding:"utf8"});
    assert.equal(result.status,0,result.stderr); return JSON.parse(result.stdout).docs;
  } finally { rmSync(dir,{recursive:true,force:true}); }
}
test("snapshots sort by actual instant when timestamps use different offsets", () => {
  const docs=[["item@late.txt","Late","2024-01-01T00:00:00Z"],["item@early.txt","Early","2024-01-01T01:00:00+08:00"]];
  assert.deepEqual(subset(docs).map(d=>d[0]),["item@early.txt","item@late.txt"]);
});
test("undated corpora keep the deterministic filename order", () => {
  assert.deepEqual(subset([["item@z.txt","Z"],["item@a.txt","A"]]).map(d=>d[0]),["item@a.txt","item@z.txt"]);
});
