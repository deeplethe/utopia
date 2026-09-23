import { afterEach, describe, expect, it, vi } from "vitest";
import { streamChat } from "./api";
import { liveAnswer, type Turn } from "./liveAnswer";

const turns = (): Turn[] => [{ role: "assistant", content: "" }];

afterEach(() => {
  vi.unstubAllGlobals();
  for (const entry of liveAnswer.get()) {
    liveAnswer.stop(entry.kbId, entry.conversationId);
  }
});

describe("live answer generation ownership", () => {
  it("keeps a follow-up streaming when the previous SSE cleanup finishes", async () => {
    let wire!: ReadableStreamDefaultController<Uint8Array>;
    let finishCleanup!: () => void;
    const cancel = vi.fn(() => new Promise<void>((resolve) => { finishCleanup = resolve; }));
    const body = new ReadableStream<Uint8Array>({ start(c) { wire = c; }, cancel });
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(body)));
    const previous = liveAnswer.begin("kb", "conversation", turns(), () => {});
    const done = vi.fn(() => previous.finish());
    streamChat("kb", { conversation_id: "conversation", message: "first" }, {
      onConversation: (id) => previous.identify(id),
      onSources: () => {},
      onStep: () => {},
      onDelta: (text) => previous.patchLast((t) => ({ ...t, content: t.content + text })),
      onDone: done,
      onError: (message) => { throw new Error(message); },
    });
    wire.enqueue(new TextEncoder().encode('event: done\ndata: {}\n\n'));
    await vi.waitFor(() => expect(done).toHaveBeenCalledTimes(1));
    const followUp = liveAnswer.begin("kb", "conversation", turns(), () => {});
    await vi.waitFor(() => expect(cancel).toHaveBeenCalledTimes(1));
    finishCleanup();
    await vi.waitFor(() => expect(body.locked).toBe(false));
    expect(done).toHaveBeenCalledTimes(1);
    expect(liveAnswer.entry("kb", "conversation")?.streaming).toBe(true);
    followUp.finish();
  });

  it("ignores every stale handle operation after the conversation slot is replaced", () => {
    const old = liveAnswer.begin("kb", "same", turns(), () => {});
    old.finish();
    const abort = vi.fn();
    const current = liveAnswer.begin("kb", "same", turns(), abort);
    const staleAbort = vi.fn();
    old.patchLast((t) => ({ ...t, content: "old response" }));
    old.setAbort(staleAbort);
    old.identify("wrong");
    old.finish();
    current.patchLast((t) => ({ ...t, content: t.content + "new response" }));
    current.finish();
    expect(liveAnswer.entry("kb", "same")?.turns[0].content).toBe("new response");
    expect(liveAnswer.entry("kb", "wrong")).toBeNull();
    liveAnswer.stop("kb", "same");
    expect(abort).toHaveBeenCalledTimes(1);
    expect(staleAbort).not.toHaveBeenCalled();
  });

  it("keeps pending identification and other conversations independent", () => {
    const a = liveAnswer.begin("kb", null, turns(), () => {});
    const b = liveAnswer.begin("other-kb", null, turns(), () => {});
    a.identify("a");
    b.identify("b");
    a.patchLast((t) => ({ ...t, content: "A" }));
    b.patchLast((t) => ({ ...t, content: "B" }));
    a.finish();
    expect(liveAnswer.entry("kb", "a")?.turns[0].content).toBe("A");
    expect(liveAnswer.entry("other-kb", "b")?.turns[0].content).toBe("B");
    expect(liveAnswer.entry("other-kb", "b")?.streaming).toBe(true);
    b.finish();
  });
});
