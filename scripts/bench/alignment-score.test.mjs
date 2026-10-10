import test from "node:test";
import assert from "node:assert/strict";
import { scoreAlignments } from "./alignment-score.mjs";

test("a key misread as a number and a flattened field on the wrong owner lose separate checks", () => {
  const truth = [{ table: "dw.orders", class: "order", columns: [
    { column: "buyer_id", class: "order", property: "buyer", target_class: "customer" },
    { column: "tier", class: "customer", property: "tier" },
    { column: "etl", omitted: true },
  ] }];
  const proposals = [{ payload: { draft: { table: "dw.orders", class: "order", columns: [
    { column: "buyer_id", class: "order", property: "buyer", expression: {const: 1} },
    { column: "tier", class: "order", property: "tier", expression: {const: 1} },
    { column: "etl", class: "order", property: "etl", expression: {const: 1} },
  ], omitted: [] } } }];
  assert.equal(scoreAlignments(proposals, truth).right, 1);
  proposals[0].payload.draft.columns = [
    { column: "buyer_id", class: "order", property: "buyer", target_class: "customer" },
    { column: "tier", class: "customer", property: "tier", expression: {const: 1} },
  ];
  proposals[0].payload.draft.omitted = [{column: "etl", reason: "bookkeeping"}];
  assert.equal(scoreAlignments(proposals, truth).right, 4);
  assert.equal(scoreAlignments([], truth).right, 0);
});
