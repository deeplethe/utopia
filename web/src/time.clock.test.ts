import { describe, expect, it } from "vitest";
import { parseDateInput } from "./time";

describe("date input clock bounds", () => {
  it.each(["2024-01-01T24Z", "2024-01-01T24:00Z", "2024-01-01T24:00:00+08:00"])(
    "rejects a clock that rolls %s into the next date", (input) => {
      expect(parseDateInput(input)).toBeNull();
    },
  );
});
