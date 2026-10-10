import { describe, expect, it } from "vitest";
import { fmtTime } from "./time";

describe("world-time formatting", () => {
  it.each(["year", "month", "day", "hour", "minute", "second", null])(
    "does not display NaN fields at %s precision", (precision) => {
      expect(fmtTime("not-a-date", precision)).toBeNull();
    },
  );
});
