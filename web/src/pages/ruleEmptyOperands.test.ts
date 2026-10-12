import { describe, expect, it } from "vitest";
import { parseOperand } from "./RulesPanel";

describe("unfinished numeric rule conditions", () => {
  it.each(["gt", "gte", "lt", "lte"])("does not turn an empty %s operand into zero", (op) => {
    expect(parseOperand(op, "")).toBeUndefined();
    expect(parseOperand(op, "  ")).toBeUndefined();
  });
  it("still accepts a deliberately written zero", () => {
    expect(parseOperand("gt", "0")).toBe(0);
  });
});
