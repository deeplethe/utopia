import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import test from "node:test";
import { firstRow, value, same, roughly } from "./lib.mjs";

// Stub only psql's stdout: the production command wrapper, parsers and comparison
// functions still run, without needing a database for these boundary cases.
function stdout(t, text) {
  t.mock.method(childProcess, "execFileSync", () => text);
  syncBuiltinESMExports();
  t.after(() => {
    t.mock.restoreAll();
    syncBuiltinESMExports();
  });
}

for (const [label, output] of [
  ["trailing NULL or empty string", "SET\n7|\n"],
  ["leading NULL or empty string", "SET\n|7\n"],
  ["interior empty field", "SET\n7||9\n"],
  ["whitespace-only field", "SET\n7| \t \n"],
]) {
  test(`firstRow does not let ${label} match a zero answer`, (t) => {
    stdout(t, output);
    const ns = firstRow("fixture", "SELECT fixture").ns;
    assert.deepEqual(ns, label === "interior empty field" ? [7, 9] : [7]);
    // ask.mjs searches every returned number using roughly; mapping scores use same.
    assert.equal(ns.some((n) => roughly(n, 0)), false);
    assert.equal(ns.some((n) => same(n, 0)), false);
  });
}

test("firstRow keeps real zero, negatives, decimals and numbers in later columns", (t) => {
  stdout(t, "SET\ncontext|0|-7|0.25|-2.5|Infinity|NaN\n999\n");
  const { ns } = firstRow("fixture", "SELECT fixture");
  assert.deepEqual(ns, [0, -7, 0.25, -2.5]);
  assert.equal(ns.some((n) => roughly(n, 0)), true);
});

test("a first row of empty fields contains no numbers", (t) => {
  stdout(t, "SET\n| |\n");
  assert.deepEqual(firstRow("fixture", "SELECT fixture"), { ns: [] });
});

for (const output of ["SET\n", "SET\n\n"]) {
  test(`no rows or a single blank value stays empty (${JSON.stringify(output)})`, (t) => {
    stdout(t, output);
    assert.deepEqual(firstRow("fixture", "SELECT fixture"), { empty: true });
    assert.deepEqual(value("fixture", "SELECT fixture"), { empty: true });
  });
}

for (const [label, output] of [
  ["NULL or empty", "SET\n|7\n"],
  ["whitespace-only", "SET\n \t |7\n"],
]) {
  test(`value keeps a ${label} first column empty instead of selecting a later number`, (t) => {
    stdout(t, output);
    const got = value("fixture", "SELECT fixture");
    assert.deepEqual(got, { empty: true });
    assert.equal(same(got.n, 0), false);
    assert.equal(roughly(got.n, 0), false);
  });
}

for (const n of [0, -7, 0.25]) {
  test(`value preserves a numeric first column (${n})`, (t) => {
    stdout(t, `SET\n${n}|999\n`);
    assert.deepEqual(value("fixture", "SELECT fixture"), { n });
  });
}

test("a nonnumeric first column does not become a later numeric value", (t) => {
  stdout(t, "SET\ntext|0\n");
  assert.ok(Number.isNaN(value("fixture", "SELECT fixture").n));
});

test("SQL errors retain their diagnostic instead of supplying a number", (t) => {
  stdout(t, "");
  childProcess.execFileSync.mock.mockImplementation(() => {
    throw Object.assign(new Error("command failed"), { stderr: "ERROR: bad SQL\n" });
  });
  assert.deepEqual(firstRow("fixture", "SELECT fixture"), { error: "ERROR: bad SQL" });
  assert.deepEqual(value("fixture", "SELECT fixture"), { error: "ERROR: bad SQL" });
});
