import { describe, expect, it } from "vitest";
import { fmtObjectValue } from "./objectValue";

describe("adopted physical quantities", () => {
  it.each(["kg", "MW", "km", "m²"])("keeps %s after the numeric quantity", (unit) => {
    expect(fmtObjectValue({ value: 42, unit })).toBe(`42 ${unit}`);
  });
  it("keeps the digits of a small quantity read into a larger declared unit", () => {
    // 「350万元」读进单位亿元的属性是 0.035：只给两位小数时显示成 0.04，「3万元」成了 0
    expect(fmtObjectValue({ value: 0.035, unit: "亿元" })).toBe("0.035 亿元");
    expect(fmtObjectValue({ value: 0.0003, unit: "亿元" })).toBe("0.0003 亿元");
  });
  it("does not expose floating-point noise from rule arithmetic", () => {
    expect(fmtObjectValue({ value: 0.1 + 0.2, unit: "kg" })).toBe("0.3 kg");
  });
  it("still places currency symbols before adopted amounts", () => {
    expect(fmtObjectValue({ value: 42, unit: "$" })).toBe("$42");
  });
});
