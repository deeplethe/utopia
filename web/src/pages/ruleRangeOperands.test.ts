import { describe, expect, it } from "vitest";
import { parseOperand } from "./RulesPanel";

describe("rule range operands", () => {
  it.each([
    ["-5 - -1", [-5, -1]],
    ["-5 ~ 1", [-5, 1]],
    ["1e-3 - 2e-3", [0.001, 0.002]],
  ])("reads signed bounds in %s", (text, expected) => {
    expect(parseOperand("between", text as string)).toEqual(expected);
  });
  it.each(["1 -", "-", "~ 1"])("keeps %s incomplete", (text) => {
    expect(parseOperand("between", text)).toBeUndefined();
  });
});
