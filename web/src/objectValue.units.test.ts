import { describe, expect, it } from "vitest";
import { fmtObjectValue } from "./objectValue";

describe("adopted physical quantities", () => {
  it.each(["kg", "MW", "km", "m²"])("keeps %s after the numeric quantity", (unit) => {
    expect(fmtObjectValue({ value: 42, unit })).toBe(`42 ${unit}`);
  });
  it("still places currency symbols before adopted amounts", () => {
    expect(fmtObjectValue({ value: 42, unit: "$" })).toBe("$42");
  });
});
