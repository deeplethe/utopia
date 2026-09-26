# The chat loop is a runner with hooks

Records: [0042] (the loop), [0014] (MCP tools and scope), [0015] (`remember` and the nod), [0020]
(the read contract), [0021] (rule tools), [0019] and [0022] (timed reads), [0035] (retrieval),
[0011] and [0036] (mappings in the prompt), [0040] (origin in results), [0046] (where an app gets built).

## What it does today

**The loop** is rig's multi-turn runner (`rig-core` / `rig-agent`, no default features); every
policy is one `AgentHook` in `api/agent.rs`: `tool_choice: required` until a tool has run, then the
budget withdraws the tools and orders an answer; a malformed call is refused with the same message
as before; a tool's UI step goes to the stream, with fields the client words (#942); an empty turn is
asked again once; a text-only first
turn from an endpoint that ignored `required` is sent back once; a turn that writes its tool call as
text is held back from the stream and sent back once, and a second one is an error [0042]. A turn
cannot end before a tool has run; `no_evidence_needed` is the exit for a greeting or "make it
shorter"; `STALL_NUDGE` and `DONE` are gone [0042 d3]. The wire stays `LlmClient` behind
`RigModel`: the read timeout,
error bodies, the classification of a failure as out of credit, rate limited, unavailable or nothing
of the kind, and cache logging; earlier
entities become a `system` message right before the question; degradation to one-shot RAG happens
only on a 400 or 422 to the first request with tools [0042 d2]. A question whose answer failed stays
stored without one; a retry names it (`retry_message_id`) and answers it in place, only while it is
the conversation's last message and no answer is being written there (#936).

**Tools** live once in `tools.rs` and serve chat and MCP alike: `search_chunks`, `search_docs`,
`find_entities`, `entity_facts` (names marked as names, derived rows with their rule, `as_of` and
`at` respected), `changes`, `list_rules`, `rule_matches`, `remember`; MCP results carry
`structuredContent` with ledger UUIDs and each evidence row's origin [0014, 0021, 0041, 0019,
0022, 0040]. The previous turn's tool calls are replayed so the model knows what it did, not only
what it said [0015]; a turn that gathers nothing, such as a restatement, keeps the previous answer's
sources for the citation numbers it repeats (#943).

**Retrieval.** Hybrid: vector recall on `chunks.embedding` through the per-dimension HNSW index with
`relaxed_order` iterative scan, plus full text in embedded Tantivy; both take `as_of`; full text is
"now" only; neither takes `at` [0035, 0019, 0022]. Confirmed mappings and a schema document reach
the prompt through retrieval; a conventions document works today where a rule would be exact
[0011, 0036].

**`remember`** writes a memory document at once; its statements go through open extraction and wait
in `pending_facts`; the assistant says the sentence is recorded and its statements will be shown
for confirmation; the card grows into the conversation on the SSE `pending` event, showing the
sentence above the statements with phrase, time words and qualifiers; a nod writes an open
statement through the extraction path [0015, #735]. Over MCP an external agent can therefore only
propose [0015 d6].

**MCP.** Streamable HTTP at `POST /api/v1/kbs/{kb_id}/mcp`, JSON responses, a personal token per
person with scope read or write and a base list, every call re-authenticated and audited [0014].
The external read contract is the RDF export; MCP fields documented in `web/src/docs/mcp.md` are
removed or renamed only with a record, and clients tolerate additions [0020].

**Measured.** DeepSeek-V3 skips the tool on 1 to 3 of 12 data questions and Qwen2.5-72B on 0;
prompt wording did not move it; the miss is recorded in `tool_exchange` and `sources` is empty,
which #547 marks on the answer [0042].

## Why

- **A heuristic judged by the model it corrects does not converge**: the "are you done" guard fired
  four times in fifteen turns and the model said DONE every time [0042].
- **The loop belongs to a library and the policy is ours**, so a policy is a typed hook and not a
  branch in an 1,100-line body [0042].
- **Termination is structural** and guarantees a decision, not a correct one [0042].
- **A first-round error is not "this endpoint cannot call tools"**; only a 400 or 422 is [0042].
- **Earlier entities as a `user` message were answered as if the user wrote them**, hence `system`
  [0042].
- **What the assistant says and what the graph gets must not differ**; the card shows the sentence so
  the person judges from something [0015].
- **The nod gate removes the confused-deputy objection to writes over MCP** [0014, 0015].
- **One tool implementation for chat and MCP**, or the two drift [0014].
- **MCP is where an application on this knowledge gets built** — in the customer's own agent
  platform, language and sandbox, against the read contract. This product hosts no app center,
  no catalog and no sandbox of its own, and the refused design is kept in the record [0046].

## Proposed and not built

- A per-task model (`on_model_select`) and a chat model chosen per base [0042].
- Retrieval that takes `at`; a versioned full-text index [0022, 0019].
- The read-contract gaps: conflict and review state, chunk identity behind a quote [0020].
- Which protocol renders an agent's interface in an external client (A2UI or MCP's own extension)
  when the nod card is needed there [0016].
- `query_data` over MCP: what evidence an external agent's fact carries, how SQL runs are audited
  [0014].

## Open questions

- Whether the nod gate holds every single-item interactive write, such as a hand-added edge [0015].
- `search_chunks` still says results can be cited as `[n]`, which MCP clients cannot see [0014].
- Whether "on 2024-03-15" reads better than "from 2024-03-15 to 2024-03-15" in the tools' wording
  of an event [0031].
