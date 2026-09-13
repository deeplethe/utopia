import { describe, expect, it } from "vitest";
import type { Source } from "./api";
import { answeredWithoutSources, citedSources, type Turn } from "./liveAnswer";

const answer = (over: Partial<Turn> = {}): Turn => ({
  role: "assistant",
  content: "以下是我找到的内容",
  ...over,
});
const source = (n: number) => ({ n, kind: "chunk" }) as unknown as Source;
const six = [1, 2, 3, 4, 5, 6].map(source);

describe("citedSources", () => {
  it("lists only the sources the text points at, numbers unchanged", () => {
    const t = answer({ content: "营收增长 [2]，毛利下降 [5]。", sources: six });
    expect(citedSources(t).map((s) => s.n)).toEqual([2, 5]);
  });

  it("reads adjacent and comma-separated markers", () => {
    const t = answer({ content: "见 [1][3] 与 [4, 6]，另见 [2，5]", sources: six });
    expect(citedSources(t).map((s) => s.n)).toEqual([1, 2, 3, 4, 5, 6]);
  });

  it("lists nothing when a greeting cites nothing it retrieved", () => {
    const t = answer({ content: "你好！我是 Utopia 的知识助手。", sources: six });
    expect(citedSources(t)).toEqual([]);
  });

  it("ignores a number that matches no source", () => {
    const t = answer({ content: "见 [9]", sources: six });
    expect(citedSources(t)).toEqual([]);
  });
});

describe("answeredWithoutSources (#547)", () => {
  it("marks a finished answer with no sources", () => {
    expect(answeredWithoutSources(answer({ sources: [] }), false)).toBe(true);
  });

  it("marks an answer that retrieved sources but cites none", () => {
    expect(answeredWithoutSources(answer({ sources: six }), false)).toBe(true);
  });

  it("does not mark an answer that cites something", () => {
    const t = answer({ content: "见 [1]", sources: [source(1)] });
    expect(answeredWithoutSources(t, false)).toBe(false);
  });

  it("waits for the stream to finish before deciding", () => {
    expect(answeredWithoutSources(answer(), true)).toBe(false);
    expect(answeredWithoutSources(answer(), false)).toBe(true);
  });

  it("marks a replayed turn, whose empty sources are stored as undefined", () => {
    expect(answeredWithoutSources(answer({ sources: undefined }), false)).toBe(true);
  });

  it("leaves user turns and bare errors alone", () => {
    expect(answeredWithoutSources({ role: "user", content: "hi" }, false)).toBe(false);
    expect(answeredWithoutSources(answer({ content: "", error: "boom" }), false)).toBe(false);
    expect(answeredWithoutSources(answer({ error: "stopped" }), false)).toBe(true);
  });
});
