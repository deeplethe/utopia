import { describe, expect, it } from "vitest";
import { parseDateInput } from "./time";

describe("parseDateInput", () => {
  it.each([
    "2023-02-29T10Z",
    "2024-02-30T10:15Z",
    "2026-04-31T10:15:30Z",
    "2023-02-29T00:30+08:00",
    "2023-02-29T23:30-08:00",
  ])("rejects nonexistent calendar dates in %s", (input) => {
    expect(parseDateInput(input)).toBeNull();
  });

  it.each([
    ["2024-02-29T10Z", "2024-02-29T10:00:00.000Z", "hour"],
    ["2024-02-29T00:30+08:00", "2024-02-28T16:30:00.000Z", "minute"],
    ["2024-12-31T23:30:15-08:00", "2025-01-01T07:30:15.000Z", "second"],
    ["2024", "2024-01-01T00:00:00.000Z", "year"],
    ["2024-02", "2024-02-01T00:00:00.000Z", "month"],
    ["2024-02-29", "2024-02-29T00:00:00.000Z", "day"],
  ])("keeps the stated precision and offset of %s", (input, iso, precision) => {
    expect(parseDateInput(input)).toEqual({ iso, precision });
  });

  it.each(["2023-02-29", "2024-02-30", "2024-13", "2024-00-01", "2024-02-29T10:15"])(
    "still rejects invalid or timezone-free input %s", (input) => {
      expect(parseDateInput(input)).toBeNull();
    },
  );
});
