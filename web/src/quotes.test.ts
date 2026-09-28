import { describe, expect, it } from "vitest";
import { QUOTE_PARAM_MAX, docSearch, quoteParam, quotePieces, sourceLead } from "./quotes";

const text = "Project Aurora started on 2023-01-10.\n\nZhang San leads Project Aurora from 2023-01-10.";

describe("the sentences a source quotes", () => {
  it("are marked where they stand in the passage", () => {
    expect(quotePieces(text, ["Zhang San leads Project Aurora from 2023-01-10."])).toEqual([
      { text: "Project Aurora started on 2023-01-10.\n\n", quoted: false },
      { text: "Zhang San leads Project Aurora from 2023-01-10.", quoted: true },
    ]);
  });

  it("match across the passage's own line breaks", () => {
    const pieces = quotePieces("Li Si leads\nProject Aurora.", ["Li Si leads Project Aurora."]);
    expect(pieces).toEqual([{ text: "Li Si leads\nProject Aurora.", quoted: true }]);
  });

  it("leave the passage as it is when a quote is not in it", () => {
    expect(quotePieces(text, ["Wang Wu advises Project Aurora."])).toEqual([
      { text, quoted: false },
    ]);
    expect(quotePieces(text, undefined)).toEqual([{ text, quoted: false }]);
    expect(quotePieces(text, ["   "])).toEqual([{ text, quoted: false }]);
  });

  it("merge sentences that overlap, and keep two apart ones apart", () => {
    const merged = quotePieces(text, ["Project Aurora started on", "started on 2023-01-10."]);
    expect(merged[0]).toEqual({ text: "Project Aurora started on 2023-01-10.", quoted: true });
    const both = quotePieces(text, [
      "Zhang San leads Project Aurora from 2023-01-10.",
      "Project Aurora started on 2023-01-10.",
    ]);
    expect(both.filter((p) => p.quoted).map((p) => p.text)).toEqual([
      "Project Aurora started on 2023-01-10.",
      "Zhang San leads Project Aurora from 2023-01-10.",
    ]);
    expect(both.map((p) => p.text).join("")).toBe(text);
  });

  it("read a quote with the characters a pattern would take as syntax", () => {
    const pieces = quotePieces("Revenue (USD) rose 3.5% [est.]", ["(USD) rose 3.5% [est.]"]);
    expect(pieces.filter((p) => p.quoted).map((p) => p.text)).toEqual(["(USD) rose 3.5% [est.]"]);
  });
});

describe("a source's lead in the list", () => {
  it("is its first quoted sentence, else the passage's opening", () => {
    expect(sourceLead({ excerpt: "Project Aurora started", quotes: ["Zhang San leads"] })).toBe(
      "Zhang San leads",
    );
    expect(sourceLead({ excerpt: "Project Aurora started" })).toBe("Project Aurora started");
  });
});

describe("the document page's search", () => {
  it("keeps the chunk and a non-blank quote", () => {
    expect(docSearch({ chunk: "c1", quote: "Li Si leads" })).toEqual({
      chunk: "c1",
      quote: "Li Si leads",
    });
    expect(docSearch({ chunk: "c1", quote: "  " })).toEqual({ chunk: "c1", quote: undefined });
    expect(docSearch({ chunk: 3, quote: ["x"] })).toEqual({ chunk: undefined, quote: undefined });
  });
});

describe("quoteParam", () => {
  it("keeps a sentence short enough for the address bar and drops a longer one whole", () => {
    expect(quoteParam("Li Si leads Project Aurora.")).toBe("Li Si leads Project Aurora.");
    expect(quoteParam("界".repeat(QUOTE_PARAM_MAX))).toBe("界".repeat(QUOTE_PARAM_MAX));
    expect(quoteParam("界".repeat(QUOTE_PARAM_MAX + 1))).toBeUndefined();
    expect(quoteParam("   ")).toBeUndefined();
    expect(docSearch({ chunk: "c", quote: "x".repeat(QUOTE_PARAM_MAX + 1) })).toEqual({
      chunk: "c",
      quote: undefined,
    });
  });
});
