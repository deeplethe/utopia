# #845 — evidence-only finalization validation (2026-09-21)

## Tested versions

| Run | PR implementation | Stable-main backport used on Linux |
|---|---|---|
| H: current passive recovery | `86f7ba547ebc0ba40860d13b2bfd09962dac0181` | `560f0b752d100538a2f72aacc713061f9e58c539` |
| P: proactive answer handoff | `9114f9c` | `7b67133565870c10af84628e7e557be50544531e` |
| P2: same handoff, focused answer policy | `0ce696f9238462d98695ef241741fa63da6b5070` | `4d7e8bc6cc06e281e771b151091376b497b2319f` |

P2 is the selected implementation. The dev branch includes upstream
`ea0557ba466979449a93b7060ca42a2cf46e2b96`; the release backport keeps stable main's
migration set. This change introduces no migration. Subsequent documentation-only commits
must not be confused with a new real-model run.

Tests ran on an isolated Linux copy, with the original 30 questions and existing gold
(88 required fact slots), unchanged corpus and model configuration. Gold was never provided
to the answering model. The configured upstream was `api.deepseek.com`; both request and
response used the name `deepseek-flash`. The provider's internal model revision is unknown.
A capture proxy forwarded requests and upstream SSE without retaining authorization headers.
Raw business evidence, credentials and model answers are deliberately not committed here.

## First failing boundary

Six H responses already contained DSML in upstream `choices[].delta.content`, with no
structured tool calls and a `stop` finish. Replaying those six raw responses through the
actual `utopia-llm` parser produced exactly the same content, zero tool calls and the original
finish reason. The adapter did not turn a valid tool call into prose in these samples.

Withdrawing tool definitions while retaining protocol-role tool history is insufficient for
this endpoint. This identifies an application-side trigger and the upstream-content boundary;
it does not establish the provider's internal reason for generating that text.

## Fixed-evidence ablation

Three captured failures, three repeats each. W0 uses the captured original final request;
W1 adds the original tools and explicit `tool_choice: none`; W2 uses H's captured recovery;
W3 keeps W2's evidence data but substitutes the dedicated answer system. W3c appends the
focused-answer instruction now used by P2. Assertions checked that every original tool
result remained in the evidence payload. No new retrieval or gold was added.

| Variant | DSML responses | Required slots answered | Median latency | Reported completion tokens, total |
|---|---:|---:|---:|---:|
| W0: tool protocol history | 5/9 | 16/42 | 2.452 s | 5,399 |
| W1: same + explicit none | 5/9 | 18/42 | 2.029 s | 5,776 |
| W2: existing recovery | 0/9 | 42/42 | 7.696 s | 13,747 |
| W3: dedicated answer policy | 0/9 | 42/42 | 18.710 s | 35,801 |
| W3c: focused answer policy | 0/9 | 42/42 | 13.979 s | 23,069 |

All returned HTTP 200. W0/W1's shorter times include fast invalid outputs. W2 and W3's zero
failures do not establish a reliability difference. Cache hits and stochastic generation
also differ; latency is not a controlled estimate of prompt-only cost.

## Fresh end-to-end runs

Each column is a new run of all 30 questions. It is not the fixed-evidence experiment above.
The historical 29/30 result belongs to an older PR head (`58d67dd` / release `341044f`),
not H. The previously paused partial run is not counted as a completed evaluation.

| Measure | H | P | P2 selected |
|---|---:|---:|---:|
| First answer structurally clean | 24/30 | 30/30 | 30/30 |
| Final answer clean after permitted repair | 30/30 | 30/30 | 30/30 |
| Raw upstream DSML responses | 6 | 0 | 0 |
| DSML published / stored | 0 / 0 | 0 / 0 | 0 / 0 |
| Required fact slots answered | 88/88 | 88/88 | 88/88 |
| Original eight failures: required slots | 28/28 | 28/28 | 28/28 |
| Original fifteen capped controls: required slots | 38/38 | 38/38 | 38/38 |
| Citation syntax and numbers resolve | 29/30 | 30/30 | 30/30 |
| SSE body equals saved body | 30/30 | 30/30 | 30/30 |
| Final SSE sources equal saved sources | 30/30 | 30/30 | 30/30 |
| All chat HTTP requests | 239 | 233 | 235 |
| Included gathering shape retries | 30 | 30 | 30 |
| Tool-bearing model rounds | 173 | 173 | 175 |
| Executed tool calls | 394 | 379 | 384 |
| Cases reaching six tool rounds | 27 | 24 | 27 |
| Median end-to-end seconds | 21.30 | 32.70 | 24.15 |
| Nearest-rank p95 seconds | 25.8 | 41.4 | 33.9 |
| Reported input tokens | 2,885,842 | 2,964,421 | 2,967,814 |
| Reported completion tokens | 88,212 | 159,206 | 112,815 |
| Included reasoning tokens | 42,819 | 120,225 | 84,237 |

The existing compatibility retry received `Thinking mode does not support this tool_choice`
on the first gathering request in each conversation. Those 30 physical requests are counted,
not hidden behind the logical budget. P2 made 27 dedicated answer calls, with no candidate
repair; three conversations answered before the boundary. H made 27 old boundary calls and
six recovery calls; three answered early. Tool counts differ because retrieval was fresh.

Core facts were checked against the original gold and the actual evidence returned in each
run, including numbers, units and plan/report/verified distinctions. Core-slot completeness
and resolvable citation numbers are not a claim that every additional sentence is correct.
Separate factual/attribution findings are tracked privately, outside this DSML change.
One H answer used full-width citation brackets that the existing frontend does not recognize;
this PR does not change citation parsing.

The reason to select P2 is the provider-before-I/O answer handoff: it avoids sending a known
failure-prone final request and retains all required evidence, without more tool execution.
It is **not** a latency improvement over H: median latency is 2.85 seconds higher and reported
completion tokens increase. P2 reduces the initial P prototype's excess output and latency.
No dollar-cost claim is made from token counts. One frozen set is not a universal guarantee.

## Deterministic and combined checks

On dev implementation `0ce696f`, with real PostgreSQL fixtures and PDF extraction enabled:

- `cargo fmt --all --check`, strict workspace/all-target clippy, locked workspace build: pass.
- `cargo test --locked --workspace`: **997 passed, 0 failed, 1 ignored**. The ignored test is
  the pre-existing live public-HTTPS RSS acceptance test requiring httpbingo.org.
- Frontend: **116 passed**; typecheck/style guard/production build pass.
- Exact stable backport: **29 chat tests**, **36 LLM tests**, and **82 frontend tests** pass;
  its release image starts against migration 69 and passes the original-admin login.

Representative production-path assertions:

| Contract | Tests / evidence |
|---|---|
| Old seventh request never goes out | `budget_finalization_accepts_an_answer_and_protocol_explanations`; outgoing request has two messages, no protocol roles/tools/routing preamble |
| Early empty/narration retries consume budget; greeting stays short | `early_retry_budget_and_no_evidence_short_path_remain_bounded`, existing empty-reply regressions |
| Multiple calls per turn retained; UTF-16 offsets stable | `parallel_tool_results_and_utf16_step_positions_survive_handoff` (12 results) |
| No tools execute after handoff | `budget_finalization_refuses_structured_calls`; bounded-recovery request/step counts |
| At most one repair with unchanged evidence | `finalization_recovers_once_from_existing_evidence_without_tools`, `unsuccessful_recovery_never_loops_or_reopens_tools` |
| Real errors cannot spoof handoff | `gathering_errors_cannot_spoof_the_private_handoff` |
| Failures, prior citations, identity, injection text remain data | `evidence_and_citations_are_data_not_protocol_messages`, `prior_citations_are_separate_and_failed_unknown_results_are_not_absence`, `failed_tool_observation_is_not_reported_as_empty_knowledge` |
| Full serialized input / output / total deadline bounded | oversized-context/text tests, `the_total_deadline_covers_candidate_repair` |
| Nonrepairable statuses and finishes do not retry | HTTP 400/401/402/403/422/429, content-filter and unknown-finish matrix |
| Split DSML and narration prefixes rejected; explanations preserved | budget-finalization split/prose/fence/explicit-example controls |
| EOF, UTF-8, missing/unknown finish semantics preserved | `utopia-llm` bytewise/unfinished-stream/finish-reason tests and adapter tests |
| Save before publication; no done or model retry on DB error | `save_failure_is_an_error_without_publishing_the_buffered_answer_or_retrying` uses an actual PostgreSQL trigger failure |
| Source mapping, disconnect, concurrency | final-sources, disconnect/reattach, concurrent-chat tests plus all 30 SSE/DB comparisons |

Five mutations were actually executed in an isolated copy at `9114f9c`, each compiling and
then failing a runtime assertion (not merely failing compilation): restore the old seventh
request; add the routing preamble back; drop the last evidence result; permit a third answer;
ignore assistant persistence failure. Restoring the source passed the targeted suite.
The final focused-policy and early-budget changes subsequently passed the full checks above.

## Reproduction and privacy

The private archive retains per-run questions, source/gold review, outgoing JSON, upstream
SSE and chunk offsets, parsed-turn comparison, delivered SSE, persisted records, usage,
image identities and mutation logs. The corpus hash was identical between staging and
production before/after the runs. Runtime account/model configuration was unchanged except
for the isolated capture proxy URL. This report publishes aggregate findings only.
