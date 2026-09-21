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
| `on_completion_call` | `tool_choice: required` until a tool has run; at the budget boundary, stop before provider I/O and hand evidence to the answer call |
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
omits tool fields on the wire.

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

## Budget finalization is an answer boundary (#844, revised 2026-09-21)

The endpoint can emit tool-control syntax in `delta.content` after tools are withdrawn.
In six captured failures, the raw upstream body already contained DSML; the actual
`LlmClient` parser reproduced that content without converting structured tool calls.
Explicit `tool_choice: none` did not eliminate the problem in a fixed-evidence comparison.
This establishes an upstream-content failure for those samples, not the provider's internal
root cause. See [the versioned validation](0042-evidence-finalization-validation.md).

The gathering policy still has six tool-capable logical turns, including early empty-reply
and required-tool nudges. An ordinary early answer or `no_evidence_needed` keeps its existing
short path. At turn seven, `on_completion_call` sets a per-run handoff flag and returns
`CompletionCallAction::Stop`. The locked Rig 0.42 implementation resolves this hook before
provider I/O. The route accepts the handoff only with both that flag and typed
`PromptCancelled`; an upstream error containing the same words is still an error.

The reserved seventh call is now the independent answer call, rather than an old protocol
history request that must fail before recovery. `chat_finalization` owns this call and at
most one repair of an invalid candidate. There is no second agent framework or tool server.
It sends only a dedicated answer system message and an explicitly untrusted JSON data
message: current question, conversation background, previous observations, every completed
current tool result, final source registry, and resolved entities. Tool result bytes and
identities are retained, including failures, unknown status, duplicate observations and
existing truncation markers. The no-evidence gate is not presented as retrieved evidence.
The current user message is excluded by stored identity, not text deduplication. Previous
citation numbers have a separate unmapped namespace; they cannot be reused as current IDs.

The answer policy preserves numbers, units, time precision, plan/report/verified distinctions,
and the pending-review status of a memory write. It asks for the requested facts concisely;
it does not retain the gathering preamble. Neither call contains `tools`, `tool_choice`,
`role=tool`, or protocol-level `assistant.tool_calls`, and there is no request-shape fallback.
Rejected candidate text never enters the repair input: only its error category does.

Without gathering-stage compatibility retries, the normal boundary costs six gathering
calls plus one answer call; one format repair raises that to eight. Auth, billing, rate,
input, transport, content-filter, unknown-finish, size and deadline failures do not authorize
a repair. Blank text, bare DSML, structured calls and a length finish may be repaired once.
A missing finish reason remains missing and follows the existing completed-stream contract.
The two answer attempts share one 120-second deadline. The fully serialized request and
accumulated answer text each have a 1 MiB bound; no evidence is silently cut to fit.
Physical HTTP counts must also include the pre-existing gathering compatibility retries.

DSML detection remains a last publication guard, never a parser or executor. It checks the
assembled terminal candidate and bare control line starts outside Markdown fences, while
preserving explanations, quotations and explicit example requests. Merely mentioning DSML
in a business question is not permission to output a control block.

Only the boundary answer is buffered; earlier narration and steps continue streaming.
Final sources and entities are snapshotted under the sink lock, which is released before
model or database I/O. The assistant INSERT must succeed before the buffered answer and
`done` are published. A save error emits an error, without another model call or a successful
terminal event. Already-streamed early narration cannot be retracted. Disconnect/reattach
continues through the existing background producer and persisted body/source mapping.

This boundary is not a factuality oracle. The evaluation separately records required fact
slots, citation syntax/mapping, additional unsupported statements and false insufficiency.
The historical 29/30 run belongs to an older head; it must not be reported as the result of
the current implementation. A clean result on one frozen set is not a zero-failure guarantee.

## Not done

- A per-task model (`on_model_select`, #470) is available in the runner and not wired.
- Choosing a different chat model per base is the product answer to the skip rate; it is
  configuration, not loop code.
