import { describe, expect, it } from "vitest";
import { citeHref, citeNumbers } from "./citations";

describe("citation identifiers", () => {
  it.each(["0x2", "1e2", "+2", "9007199254740993"])(
    "does not interpret %s as a source number", (value) => {
      expect(citeNumbers(value)).toEqual([]);
      expect(citeHref(`#cite-${value}`)).toBeNull();
    },
  );
  it("preserves decimal lists", () => {
    expect(citeHref("#cite-1,2")).toEqual([1, 2]);
  });
});
