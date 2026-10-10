import test from "node:test";
import assert from "node:assert/strict";
import { askChat } from "./lib.mjs";

test("chat SSE accepts CRLF frames split between transport chunks", async () => {
  const original = globalThis.fetch;
  const bytes = new TextEncoder().encode('event: conversation\r\ndata: {"id":"conversation-id"}\r\n\r\nevent: delta\r\ndata: {"text":"hello"}\r\n\r\n');
  globalThis.fetch = async () => new Response(new ReadableStream({ start(controller) { for (let i = 0; i < bytes.length; i += 3) controller.enqueue(bytes.slice(i, i + 3)); controller.close(); } }));
  try { assert.deepEqual(await askChat("kb", "query"), {conversation:"conversation-id", text:"hello", steps:[], error:null}); }
  finally { globalThis.fetch = original; }
});

test("terminal SSE events complete the result without waiting for transport closure", async () => {
  const original = globalThis.fetch;
  let cancelled = false;
  globalThis.fetch = async () => new Response(new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode('event: delta\ndata: {"text":"done text"}\n\nevent: done\ndata: {}\n\n')); }, cancel() { cancelled = true; } }));
  try {
    const result = await Promise.race([askChat("kb", "query"), new Promise(resolve => setTimeout(() => resolve("timed out"), 80))]);
    assert.notEqual(result, "timed out"); assert.equal(result.text, "done text"); assert.equal(cancelled, true);
  } finally { globalThis.fetch = original; }
});
