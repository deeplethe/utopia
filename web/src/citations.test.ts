import { describe, expect, it } from "vitest";
import { citeHref, rehypeCitations, splitCitations } from "./citations";

describe("splitCitations", () => {
  it("keeps plain text in one piece", () => {
    expect(splitCitations("no marks here")).toEqual([{ text: "no marks here" }]);
  });

  it("reads the three shapes the model writes", () => {
    expect(splitCitations("a[1]b[2][3]c[4, 5]d[6，7]")).toEqual([
      { text: "a" },
      { cite: [1] },
      { text: "b" },
      { cite: [2] },
      { cite: [3] },
      { text: "c" },
      { cite: [4, 5] },
      { text: "d" },
      { cite: [6, 7] },
    ]);
  });

  it("leaves an unfinished marker alone while the answer is still streaming", () => {
    expect(splitCitations("risk factors [2")).toEqual([
      { text: "risk factors [2" },
    ]);
  });

  it("is not fooled by brackets that hold words", () => {
    expect(splitCitations("see [note] and [3]")).toEqual([
      { text: "see [note] and " },
      { cite: [3] },
    ]);
  });

  it("starts each call from the beginning", () => {
    // 共用一个 g 正则会让第二段从上一次的 lastIndex 接着找，`[1]` 就丢了
    expect(splitCitations("x[1]")).toEqual([{ text: "x" }, { cite: [1] }]);
    expect(splitCitations("x[1]")).toEqual([{ text: "x" }, { cite: [1] }]);
  });
});

describe("rehypeCitations", () => {
  const run = (tree: unknown) => {
    rehypeCitations()(tree as never);
    return tree;
  };

  it("turns a marker into a cite link and keeps the text around it", () => {
    const tree = {
      type: "root",
      children: [
        {
          type: "element",
          tagName: "p",
          children: [{ type: "text", value: "disclosed [2][3]" }],
        },
      ],
    };
    expect(run(tree)).toEqual({
      type: "root",
      children: [
        {
          type: "element",
          tagName: "p",
          children: [
            { type: "text", value: "disclosed " },
            {
              type: "element",
              tagName: "a",
              properties: { href: "cite:2" },
              children: [{ type: "text", value: "[2]" }],
            },
            {
              type: "element",
              tagName: "a",
              properties: { href: "cite:3" },
              children: [{ type: "text", value: "[3]" }],
            },
          ],
        },
      ],
    });
  });

  it("leaves links and code alone", () => {
    const opaque = (tagName: string) => ({
      type: "root",
      children: [
        {
          type: "element",
          tagName,
          children: [{ type: "text", value: "[1]" }],
        },
      ],
    });
    for (const tag of ["a", "code", "pre"]) {
      expect(run(opaque(tag))).toEqual(opaque(tag));
    }
  });
});

describe("citeHref", () => {
  it("reads back what the plugin wrote", () => {
    expect(citeHref("cite:4")).toEqual([4]);
    expect(citeHref("cite:4,5")).toEqual([4, 5]);
  });

  it("passes an ordinary link through", () => {
    expect(citeHref("https://example.com")).toBeNull();
    expect(citeHref(undefined)).toBeNull();
    expect(citeHref("cite:")).toBeNull();
  });
});
