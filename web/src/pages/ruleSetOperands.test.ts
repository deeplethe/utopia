import { describe, expect, it } from "vitest";
import { parseOperand } from "./RulesPanel";

describe("rule set operands", () => {
  it.each(["in", "not_in"])("splits %s members on the comma a Chinese input method types", (op) => {
    expect(parseOperand(op, "气测异常，气测异常后效")).toEqual(["气测异常", "气测异常后效"]);
    expect(parseOperand(op, "气测异常，气测异常后效, 气层")).toEqual(["气测异常", "气测异常后效", "气层"]);
  });
});
