import assert from "node:assert/strict";
import { test, mock } from "node:test";
import { until } from "./lib.mjs";

test("NaN progress cannot keep renewing a stalled benchmark", async () => {
  let now = 0;
  let polls = 0;
  mock.method(Date, "now", () => ++now);
  try {
    await assert.rejects(until(() => {
      if (++polls > 10) throw new Error("poll guard reached");
      return NaN;
    }, 0, 3), /等超时/);
    assert.ok(polls <= 10);
  } finally {
    mock.restoreAll();
  }
});
