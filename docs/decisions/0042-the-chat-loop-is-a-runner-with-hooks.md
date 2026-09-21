# 0042 · The chat loop is a runner with hooks

- **Status**: Implemented (#548) · the loop is rig's runner (`rig-core` / `rig-agent` 0.42, no
  default features) · policy is one `AgentHook` in `api/agent.rs` · the wire stays `LlmClient`
  behind `api/rig_model.rs`
- **Written**: 2026-09-13 (conventions in the [README](README.md))
- **Related**: #546 (the issue and its findings), #509 / #543 (the stall and the guard this
  replaces), #547 (the mark on answers that cite nothing), #631 (the empty-reply retry, moved
  into a hook here), [0014](0014-identity-from-the-person-scope-from-the-token.md) (MCP sees the
  same tool list)

## Why a decision is needed

`chat.rs` used to hold a hand-written agent loop of about 1,100 lines. Every policy question
became another branch inside it: budget exhaustion, the first-round fallback to one-shot RAG,
the replay of earlier tool exchanges, and finally #543's guard for #509 — the model says "let me
look that up" and the turn ends. The guard asked the model once whether it was done; in fifteen
real turns it fired four times and the model answered `DONE` every time, including right after
promising a search. A heuristic judged by the model it is meant to correct does not converge.

Three loop defects were found while reading it (#546): any first-round error was treated as
"this endpoint cannot call tools" and degraded to RAG; the block of entities already identified
in the conversation was injected as a `user` message, which the model answered as if the user
had written it; and an empty reply became an error frame with no retry.

## Decisions

### 1. The loop belongs to a library; the policy is ours

The loop is rig's multi-turn runner. Every decision we make is a hook with a typed result, not
a branch in a loop body:

| hook | decision |
|---|---|
| `on_completion_call` | `tool_choice: required` until a tool has run; after the budget, withdraw the tools and order an answer |
| `on_tool_call` | `check_call` refuses a malformed call, and the model gets the same message as before |
| `on_tool_result` | the tool's UI step goes to the stream |
| `on_model_turn_finished` | an empty turn is asked again once (#631); a text-only first turn from an endpoint that ignored `required` is sent back once |

rig was chosen over swiftide-agents because it is maintained and does not bring its own context
model. Neither has a provider we want: see decision 2.

### 2. The wire stays `LlmClient`

`RigModel` implements rig's `CompletionModel` on top of `LlmClient`. The read timeout, error
bodies (#538), the out-of-credit versus rate-limit classification and the cache-hit logging all
predate this and are not re-earned in another client. Two request-shape decisions live there:
earlier entities become a `system` message right before the question, and `ToolChoice::None`
sends no tools field at all, which every endpoint accepts.

Degradation to one-shot RAG happens only when the first request that carries tools comes back
400 or 422 (`utopia_llm::Rejected`). A network failure is an error frame.

### 3. A turn cannot end before a tool has run

Termination is structural. The first request of a turn requires a tool call, and
`no_evidence_needed` is the honest exit for a greeting or "make it shorter". "Please wait, I will
call the tool" is no longer a possible final state; `STALL_NUDGE`, `DONE` and the no-evidence
note are deleted.

This guarantees a decision, not a correct one. DeepSeek-V3 still calls `no_evidence_needed` on
some data questions (1–3 of 12 fresh questions across runs; Qwen2.5-72B, same prompt: 0 of 12).
Neither prompt wording nor the terminal's result moved the rate (measured in #548, 36 turns).
What changed is that the miss is recorded: the call and the model's reason are in
`tool_exchange`, and `sources` is empty, which is what #547 marks.

## Budget finalization is an answer boundary (#844)

Withdrawing tools does not prevent an endpoint from emitting tool-control syntax in
`delta.content`. A nonempty accumulator may also contain only narration from earlier tool
turns. The terminal candidate must therefore be checked separately: at the budget boundary,
empty text, structured tool calls, unexpected bare DSML control output, or a reported
non-natural finish stop the tool runner. The pre-tool hook independently refuses execution
during finalization. DSML text is never interpreted as a tool call; ordinary explanations,
fenced quotations, and explicit
DSML requests remain allowed.

Only the budget-finalization text is buffered, up to 1 MiB, before publication. Earlier
narration and tool steps still stream normally. The chat route checks again before emitting
the final text and persisting the assistant message, so a rejected candidate does not enter
the live snapshot or normal conversation history. This uses the existing background producer
and error event; disconnecting the browser does not cancel generation.

The tool runner retains its six tool-capable turns and seven logical model-call limit.
After a rejected final candidate, the route permits exactly one additional physical request,
with no tools or request-shape fallback and a 120-second deadline. It copies existing tool
results and source IDs into an explicitly untrusted evidence payload, retaining conversation
context but omitting the rejected candidate and protocol-role messages. It never performs
another search or summarizes away evidence. Input and output are each bounded at 1 MiB;
oversize input fails explicitly instead of silently dropping evidence. Thus recovery cannot
execute tools or retry itself, and a failed recovery emits an error without persistence.

Tool turns preserve the provider's finish reason through the adapter: missing stays missing,
unknown stays unknown, and an explicit length/tool-call/filter finish cannot pass this final
answer boundary. Normal early answers keep their existing behavior. This does not establish
why the upstream endpoint generated markup, or assess factual answer quality.

## Not done

- A per-task model (`on_model_select`, #470) is available in the runner and not wired.
- Choosing a different chat model per base is the product answer to the skip rate; it is
  configuration, not loop code.

A rerun of the original 30 real-model questions exposed a same-turn narration
prefix before a bare DSML block (29 clean, one leaked). Finalization therefore
also checks bare line starts outside Markdown fences. Inline mentions, block
quotes, fenced examples, and explicit DSML questions remain allowed. This is
still a finalization guard, never a parser that executes text as tools.
