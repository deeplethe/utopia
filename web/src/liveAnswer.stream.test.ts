import { afterEach, describe, expect, it, vi } from "vitest";
import { streamChat } from "./api";
import { liveAnswer, type LiveHandle, type Turn } from "./liveAnswer";

const turns = (): Turn[] => [{ role: "assistant", content: "" }];
const handles: LiveHandle[] = [];
const begin = (...args: Parameters<typeof liveAnswer.begin>) => {
  const handle = liveAnswer.begin(...args);
  handles.push(handle);
  return handle;
};

afterEach(() => {
  for (const handle of handles.splice(0)) handle.discard();
  vi.unstubAllGlobals();
});

describe("live answer generation ownership", () => {
  it("keeps a follow-up streaming when the previous SSE cleanup finishes", async () => {
    let wire!: ReadableStreamDefaultController<Uint8Array>;
    let finishCleanup!: () => void;
    const cancel = vi.fn(() => new Promise<void>((resolve) => { finishCleanup = resolve; }));
    const body = new ReadableStream<Uint8Array>({ start(c) { wire = c; }, cancel });
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(body)));
    const previous = begin("kb", "conversation", turns(), () => {});
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
    const followUp = begin("kb", "conversation", turns(), () => {});
    await vi.waitFor(() => expect(cancel).toHaveBeenCalledTimes(1));
    finishCleanup();
    await vi.waitFor(() => expect(body.locked).toBe(false));
    expect(done).toHaveBeenCalledTimes(1);
    expect(liveAnswer.entry("kb", "conversation")?.streaming).toBe(true);
    followUp.finish();
  });

  it("ignores every stale handle operation after the conversation slot is replaced", () => {
    const old = begin("kb", "same", turns(), () => {});
    old.finish();
    const abort = vi.fn();
    const current = begin("kb", "same", turns(), abort);
    const staleAbort = vi.fn();
    old.patchLast((t) => ({ ...t, content: "old response" }));
    old.setAbort(staleAbort);
    old.identify("wrong");
    old.finish();
    old.discard();
    current.patchLast((t) => ({ ...t, content: t.content + "new response" }));
    current.finish();
    expect(liveAnswer.entry("kb", "same")?.turns[0].content).toBe("new response");
    expect(liveAnswer.entry("kb", "wrong")).toBeNull();
    current.discard();
    expect(abort).toHaveBeenCalledTimes(1);
    expect(staleAbort).not.toHaveBeenCalled();
  });

  it("keeps pending identification and other conversations independent", () => {
    const a = begin("kb", null, turns(), () => {});
    const b = begin("other-kb", null, turns(), () => {});
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

describe("explicit server stop", () => {
  it("queues Stop before identity and waits for done after the HTTP acknowledgement", async () => {
    const fetch = vi.fn().mockResolvedValue(Response.json({ ok: true }));
    vi.stubGlobal("fetch", fetch);
    const abort = vi.fn();
    const handle = begin("kb", null, [{ role: "assistant", content: "partial", steps: [], sources: [] }], abort);
    liveAnswer.stop("kb", null);
    expect(fetch).not.toHaveBeenCalled();
    expect(liveAnswer.entry("kb", null)).toMatchObject({ streaming: true, stopping: true });

    handle.identify("conversation", undefined, "generation");
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
    expect(fetch).toHaveBeenCalledWith("/api/v1/kbs/kb/chat/conversation/stop", expect.objectContaining({
      method: "POST", body: JSON.stringify({ generation_id: "generation" }),
    }));
    expect(liveAnswer.entry("kb", "conversation")).toMatchObject({ streaming: true, stopping: true });
    expect(abort).not.toHaveBeenCalled();
    liveAnswer.stop("kb", "conversation");
    expect(fetch).toHaveBeenCalledTimes(1);

    handle.finish(true);
    expect(liveAnswer.entry("kb", "conversation")).toMatchObject({
      streaming: false, stopping: false, turns: [{ content: "partial", stopped: true, steps: [], sources: [] }],
    });
    handle.patchLast((turn) => ({ ...turn, content: "late" }));
    liveAnswer.stop("kb", "conversation");
    expect(liveAnswer.entry("kb", "conversation")?.turns[0].content).toBe("partial");
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("keeps the stream open when Stop fails and lets the user retry", async () => {
    const fetch = vi.fn()
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValue(Response.json({ ok: true }));
    vi.stubGlobal("fetch", fetch);
    const handle = begin("kb", "conversation", turns(), vi.fn());
    const other = begin("kb", "other", turns(), vi.fn());
    handle.identify("conversation", undefined, "generation");
    liveAnswer.stop("kb", "conversation");
    await vi.waitFor(() => expect(liveAnswer.entry("kb", "conversation")?.stopError).toBe("offline"));
    expect(liveAnswer.entry("kb", "conversation")).toMatchObject({ streaming: true, stopping: false });
    expect(liveAnswer.entry("kb", "conversation")?.turns[0].error).toBeUndefined();
    expect(liveAnswer.entry("kb", "other")).toMatchObject({ streaming: true, stopping: false });

    liveAnswer.stop("kb", "conversation");
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
    expect(liveAnswer.entry("kb", "conversation")).toMatchObject({ streaming: true, stopping: true, stopError: undefined });
    handle.finish(true);
    other.finish();
  });

  it("ignores a late stop failure after another generation owns the conversation", async () => {
    let rejectStop!: (error: Error) => void;
    vi.stubGlobal("fetch", vi.fn(() => new Promise<Response>((_, reject) => { rejectStop = reject; })));
    const old = begin("kb", "same", turns(), () => {});
    old.identify("same", undefined, "old-generation");
    liveAnswer.stop("kb", "same");
    old.finish(true);
    const current = begin("kb", "same", turns(), () => {});
    current.identify("same", undefined, "new-generation");
    rejectStop(new Error("late failure"));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(liveAnswer.entry("kb", "same")).toMatchObject({ generationId: "new-generation", streaming: true, stopping: false });
    expect(liveAnswer.entry("kb", "same")?.stopError).toBeUndefined();
  });
});
