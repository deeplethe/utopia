import assert from "node:assert/strict";
import test from "node:test";

// BENCH_TEST_PSQL is a psql command with -tAc and connection options; BENCH_TEST_DB
// names an existing test database. These probes only SELECT, with no fixture writes.
test("real psql keeps blank SQL fields out of numeric benchmark scores", {
  skip: !process.env.BENCH_TEST_PSQL,
}, async () => {
  assert.ok(process.env.BENCH_TEST_DB, "BENCH_TEST_DB must name the test database");
  process.env.BENCH_PSQL = process.env.BENCH_TEST_PSQL;
  const { firstRow, value, roughly } = await import("./lib.mjs");
  const db = process.env.BENCH_TEST_DB;
  for (const field of ["NULL::numeric", "''::text", "'   '::text"]) {
    const sql = `SELECT 7, ${field}`;
    const { ns } = firstRow(db, sql);
    assert.deepEqual(ns, [7], sql);
    assert.equal(ns.some((n) => roughly(n, 0)), false, sql);
    assert.deepEqual(value(db, `SELECT ${field}, 7`), { empty: true }, field);
  }
  assert.deepEqual(firstRow(db, "SELECT 7, 0, -2.5, 0.25"), { ns: [7, 0, -2.5, 0.25] });
  assert.deepEqual(value(db, "SELECT 0"), { n: 0 });
  assert.deepEqual(firstRow(db, "SELECT NULL::numeric"), { empty: true });
  assert.deepEqual(firstRow(db, "SELECT 0 WHERE FALSE"), { empty: true });
});
