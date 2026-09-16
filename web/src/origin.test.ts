import { describe, expect, it } from "vitest";
import { clock, originHint, originLabel } from "./origin";

describe("originLabel", () => {
  it("says nothing for words the file itself holds", () => {
    expect(originLabel("stated", null)).toBeNull();
    expect(originLabel(undefined, null)).toBeNull();
  });

  it("names the page a scan was read from", () => {
    expect(originLabel("ocr", { page: 2, bbox: [1, 2, 3, 4] })).toBe("OCR · p. 2");
    expect(originLabel("ocr", {})).toBe("OCR");
  });

  it("names the stretch of the recording and who spoke", () => {
    expect(
      originLabel("transcribed", { start_ms: 6400, end_ms: 9750, speaker: ["A", "B"] }),
    ).toBe("Transcribed · 0:06–0:09 · A, B");
    expect(originLabel("transcribed", { start_ms: 3_725_000, end_ms: 3_731_000, speaker: "A" })).toBe(
      "Transcribed · 1:02:05–1:02:11 · A",
    );
  });

  it("marks a description as one", () => {
    expect(originLabel("described", {})).toBe("Described by a model");
  });
});

describe("originHint", () => {
  it("adds the model that read it", () => {
    expect(originHint("ocr", "mineru 2.5.4 vlm-auto-engine")).toContain("mineru 2.5.4 vlm-auto-engine");
    expect(originHint("stated", "x")).toBeUndefined();
  });
});

describe("clock", () => {
  it("pads minutes and seconds", () => {
    expect(clock(0)).toBe("0:00");
    expect(clock(65_000)).toBe("1:05");
  });
});
