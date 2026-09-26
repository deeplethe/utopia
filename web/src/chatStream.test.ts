import { afterEach, describe, expect, it, vi } from "vitest";
import { reattachChat, streamChat, type ChatHandlers } from "./api";
vi.mock("./i18n", () => ({
  S: {
    ask: { streamInterrupted: "Stream interrupted" },
    err: {
      no_chat_model: "Worded: configure a chat model",
      model_out_of_credit: "Worded: the model account cannot pay",
    },
    errDetail: (message: string, detail: string) => `${message} (${detail})`,
  },
  lang: "en",
}));
afterEach(() => vi.unstubAllGlobals());

const encoder = new TextEncoder();
async function replay(text: string, attach = false, bytewise = false) {
  const events: [string, unknown][] = [];
  const h: ChatHandlers = {
    onConversation: (v) => events.push(["conversation", v]), onSources: (v) => events.push(["sources", v]),
    onStep: (v) => events.push(["step", v]), onDelta: (v) => events.push(["delta", v]),
    onSnapshot: (v) => events.push(["snapshot", v]), onDone: () => events.push(["done", null]),
    onError: (v) => events.push(["error", v]), onIdle: () => events.push(["idle", null]),
  };
  const bytes = encoder.encode(text);
  const body = new ReadableStream<Uint8Array>({ start(c) {
    if (bytewise) for (const byte of bytes) c.enqueue(new Uint8Array([byte]));
    else c.enqueue(bytes);
    c.close();
  } });
  const fetch = vi.fn().mockResolvedValue(new Response(body));
  vi.stubGlobal("fetch", fetch);
  if (attach) reattachChat("kb", "c", h); else streamChat("kb", {message:"hello"}, h);
  await vi.waitFor(() => expect(events.some(([e]) => ["done","error","idle"].includes(e))).toBe(true));
  expect(fetch).toHaveBeenCalledTimes(1);
  return events;
}

/** The chat request is refused before any stream opens */
async function refuse(status: number, body: unknown) {
  const events: [string, unknown][] = [];
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify(body), { status })));
  streamChat("kb", { message: "hello" }, {
    onConversation: vi.fn(), onSources: vi.fn(), onStep: vi.fn(), onDelta: vi.fn(),
    onDone: () => events.push(["done", null]), onError: (v) => events.push(["error", v]),
  });
  await vi.waitFor(() => expect(events).toHaveLength(1));
  return events;
}

describe("a refused chat request", () => {
  it("is worded from its stable code, like every other request", async () => {
    expect(await refuse(422, { error: "Chat model not configured. Go to Settings → Models.", code: "no_chat_model" }))
      .toEqual([["error", "Worded: configure a chat model"]]);
  });
  it("keeps the server's sentence when there is no code to word it by", async () => {
    expect(await refuse(404, { error: "Not found" })).toEqual([["error", "Not found"]]);
    expect(await refuse(422, { error: "Not in the table yet", code: "unlisted_code" }))
      .toEqual([["error", "Not in the table yet"]]);
  });
  it("keeps the server's detail beside the wording", async () => {
    expect(await refuse(422, { error: "x", code: "no_chat_model", detail: "no endpoint" }))
      .toEqual([["error", "Worded: configure a chat model (no endpoint)"]]);
  });
});

describe("a chat stream that fails", () => {
  const failed = (body: unknown) => `event: error\ndata: ${JSON.stringify(body)}\n\n`;
  it("is worded from its stable code, as a refused request is", async () => {
    const text = failed({ error: "LLM account cannot pay for this request (402): x", code: "model_out_of_credit" });
    expect(await replay(text)).toEqual([["error", "Worded: the model account cannot pay"]]);
    expect(await replay(text, true)).toEqual([["error", "Worded: the model account cannot pay"]]);
  });
  it("keeps the server's sentence for a code the table does not have", async () => {
    expect(await replay(failed({ error: "Some new failure", code: "unlisted_code" })))
      .toEqual([["error", "Some new failure"]]);
  });
  it("shows a plain-text error frame as it came", async () => {
    expect(await replay("event: error\ndata: Model returned an empty answer\n\n"))
      .toEqual([["error", "Model returned an empty answer"]]);
  });
});

describe("application chat terminal outcomes", () => {
  it.each(["\n", "\r\n", "\r"])("reads %j line endings once, including split UTF-8", async (nl) => {
    const text = 'event: delta\ndata: {"text":"中文🙂"}\n\nevent: done\ndata: {}\n\n'.replaceAll("\n", nl);
    expect(await replay(text, false, true)).toEqual([["delta","中文🙂"],["done",null]]);
  });
  it.each(["\n", "\r\n", "\r"])("preserves multiline error data with %j and ignores later events", async (nl) => {
    const text = "event: error\ndata: first\ndata:  second\n\nevent: done\ndata: {}\n\nevent: delta\ndata: not-json\n\n".replaceAll("\n", nl);
    expect(await replay(text, false, true))
      .toEqual([["error","first\n second"]]);
  });
  it("does not turn partial output plus EOF into success", async () => {
    expect(await replay('event: delta\ndata: {"text":"partial"}\n\n')).toEqual([["delta","partial"],["error","Stream interrupted"]]);
  });
  it("idle only terminates a reattachment", async () => {
    expect(await replay("event: idle\ndata: {}\n\n", true)).toEqual([["idle",null]]);
    expect(await replay("event: idle\ndata: {}\n\n")).toEqual([["error","Stream interrupted"]]);
  });
  it("ignores frames after the first done", async () => {
    expect(await replay('event: done\ndata: {}\n\nevent: delta\ndata: {"text":"late"}\n\nevent: error\ndata: late error\n\n')).toEqual([["done",null]]);
  });
  it.each(["event: done\ndata: {}\n", "event: done\ndata: {}\r", "event: done\ndata: {}", "", "event: delta\ndata: {broken}\n\n"])("requires a complete terminal frame: %s", async (s) => {
    const result = await replay(s);
    expect(result).toHaveLength(1); expect(result[0][0]).toBe("error");
  });
  it("cancels an open stream after done even if cancellation rejects", async () => {
    const cancel = vi.fn(() => Promise.reject(new Error("cancel failed")));
    const body = new ReadableStream<Uint8Array>({
      start(c) { c.enqueue(encoder.encode("event: done\ndata: {}\n\n")); }, cancel,
    });
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(body)));
    const h = {onConversation:vi.fn(),onSources:vi.fn(),onStep:vi.fn(),onDelta:vi.fn(),onDone:vi.fn(),onError:vi.fn()};
    streamChat("kb", {message:"hello"}, h);
    await vi.waitFor(() => expect(body.locked).toBe(false));
    await vi.waitFor(() => expect(cancel).toHaveBeenCalledTimes(1));
    expect(h.onDone).toHaveBeenCalledTimes(1); expect(h.onError).not.toHaveBeenCalled();
  });
  // 这一问存下的 id 随 conversation 帧一起到：答到一半失败时，页面凭它重答（#936）
  it("hands the stored id of the question to the page with the conversation", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(
      'event: conversation\ndata: {"id":"c1","message_id":"m1"}\n\nevent: done\ndata: {}\n\n',
    )));
    const h = {onConversation:vi.fn(),onSources:vi.fn(),onStep:vi.fn(),onDelta:vi.fn(),onDone:vi.fn(),onError:vi.fn()};
    streamChat("kb", {message:"hello"}, h);
    await vi.waitFor(() => expect(h.onDone).toHaveBeenCalledTimes(1));
    expect(h.onConversation).toHaveBeenCalledWith("c1", "m1");
    expect(h.onError).not.toHaveBeenCalled();
  });
  it("active abort is silent and cancels the reader", async () => {
    const cancelled = vi.fn();
    const body = new ReadableStream<Uint8Array>({ cancel: cancelled });
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(body)));
    const h = {onConversation:vi.fn(),onSources:vi.fn(),onStep:vi.fn(),onDelta:vi.fn(),onDone:vi.fn(),onError:vi.fn()};
    const abort = streamChat("kb",{message:"hello"},h);
    await vi.waitFor(() => expect(body.locked).toBe(true));
    abort();
    await vi.waitFor(() => expect(cancelled).toHaveBeenCalledTimes(1));
    expect(h.onDone).not.toHaveBeenCalled(); expect(h.onError).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(body.locked).toBe(false));
  });
});
