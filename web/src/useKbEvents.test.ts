import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createInvalidationCoalescer } from "./useKbEvents";

describe("a burst of events buys one refetch", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("the same key pushed many times inside the settle window invalidates once", () => {
    const seen: unknown[] = [];
    const { push } = createInvalidationCoalescer((key) => seen.push(key), 300);
    for (let i = 0; i < 12; i++) push(["graph"]);
    expect(seen).toEqual([]);
    vi.advanceTimersByTime(299);
    expect(seen).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(seen).toEqual([["graph"]]);
  });

  it("different keys in one burst each invalidate once, in arrival order", () => {
    const seen: unknown[] = [];
    const { push } = createInvalidationCoalescer((key) => seen.push(key), 300);
    push(["documents", "kb"], ["graph"]);
    push(["graph"]);
    push(["review", "kb"]);
    vi.advanceTimersByTime(300);
    expect(seen).toEqual([["documents", "kb"], ["graph"], ["review", "kb"]]);
  });

  it("a burst after the window is a new burst", () => {
    const seen: unknown[] = [];
    const { push } = createInvalidationCoalescer((key) => seen.push(key), 300);
    push(["graph"]);
    vi.advanceTimersByTime(300);
    push(["graph"]);
    vi.advanceTimersByTime(300);
    expect(seen).toEqual([["graph"], ["graph"]]);
  });

  it("a broader key covers a narrower queued key regardless of their arrival order", () => {
    for (const keys of [
      [["graph", "kb"], ["graph"]],
      [["graph"], ["graph", "kb"]],
    ]) {
      const seen: unknown[] = [];
      const { push, flush } = createInvalidationCoalescer((key) => seen.push(key), 300);
      for (const key of keys) push(key);
      flush();
      expect(seen).toEqual([["graph"]]);
    }
  });

  it("flushing on unmount delivers what was pending instead of dropping it", () => {
    const seen: unknown[] = [];
    const { push, flush } = createInvalidationCoalescer((key) => seen.push(key), 300);
    push(["pending", "kb"]);
    flush();
    expect(seen).toEqual([["pending", "kb"]]);
    vi.advanceTimersByTime(300);
    expect(seen).toEqual([["pending", "kb"]]);
  });
});
