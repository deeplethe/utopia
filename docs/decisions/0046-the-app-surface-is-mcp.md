# 0046 · The app surface is MCP

- **Status**: Decided 2026-09-19 · no app center, no component runtime, no sandbox. The design that
  was refused is kept below so the question is not reopened from nothing
- **Written**: 2026-09-19 (conventions in the [README](README.md))
- **Related**: [0014](0014-identity-from-the-person-scope-from-the-token.md) and
  [0020](0020-an-auditor-reads-it-without-us.md) are the surface this record points at;
  [0016](0016-close-the-open-seams-before-cutting-new-ones.md) is the rule it obeys;
  [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) is what is being rebuilt while the
  question was asked; [0034](0034-an-action-is-a-declared-call.md) is the thing worth building
  before any of this; [0042](0042-the-chat-loop-is-a-runner-with-hooks.md) is the runner an app
  would have instantiated

> The question, asked on 2026-09-19: let people build applications on the knowledge this base
> holds, mount them in an app center, run them in a sandbox, and hand them to a team. It is a fair
> question — a base that answers whoever is typing has no way to keep what one person worked out
> about asking it well.

## What the product already answers

A personal token carries the person's identity and a scope, and effective permission is that scope
intersected with their role in the base (0014). Ten read tools serve chat and MCP from one place,
`as_of` reaches every graph read, and since #601 a read returns `structuredContent` with stable
ledger identities (0020). Streamable HTTP at `POST /api/v1/kbs/{kb_id}/mcp`.

So **someone who wants a coding agent to build an application on this knowledge can do it today**,
in their own agent platform, in their own language, inside their own sandbox, with their own
review and their own CI. The application reads this base through a contract built to be read by
someone who is not us — which is what 0020 set out to do.

An app center would re-host that: an execution boundary, a catalog, quotas, versioning, a review
path for code, and the support burden of all of it. None of those are things this product would be
good at, and two of them are products in their own right.

## Why not here, and not now

**1. The layer an app would read is in motion.** Extraction writes only the open graph (#736), and
typed facts come from alignment, whose cuts are landing as this is written (#751, #752, #753,
#754). The first applications would be written against a model being replaced underneath them, and
the cost of that lands on whoever wrote them.

**2. 0016 is a rule this project made for itself.** Seams left open are closed before new ones are
cut. An app center is the largest new seam available.

**3. The differentiator is not on this axis.** Time is: a fact that carries its evidence, an
interval, and a reading that can be replayed at any moment. Every hour spent on a catalog is an
hour not spent making that true of more of the corpus.

## The shape it would take, if it is ever built

Kept so the next person starts here rather than at the beginning. An app is **data** — instructions
in prose, a tool list, pinned definitions, a clock, an output shape, an optional action, scalar
parameters shaped like `action_params` — where everything deciding what it may touch is structured
and only what it is asked to do is prose, so a prompt can never widen reach. That is the answer
owed to 0034, which refused scripting because the page should show what runs.

Four gates it would have to keep. They are prerequisites, not features:

- **Identity.** An app runs as the person who runs it: the app's tools ∩ their role in the base ∩
  the token's scope. An app carrying its author's reach makes `require_kb` decorative — a Viewer
  would read a restricted base through an app an Editor published. A schedule runs as whoever armed
  it and disarms when they lose the role.
- **Egress.** No URL, no fetch tool; the world is reached only through an action granted to that
  base (0034), so the pinned addresses, the sealed credentials and the outbound ledger stay on the
  path.
- **Time.** A run resolves `as_of` from the clock or a parameter and passes it to every graph read,
  so a number someone acted on can be got again.
- **Execution.** Runs go through the existing `jobs` queue, deduped on `(app_id, scheduled_for)`.
  No second executor, no second concurrency knob.

Writes still wait for a nod (0015). Every run is a row with the resolved `as_of`, the steps, the
citations and the token counts, read by the base's log and the deployment's (0020).

## Dead ends

**A container as the runtime.** Argued first from WeKnora, then withdrawn. Their skills are written
by people, installed from a catalog and assume a shell, so they need an operating system — and they
pay for it: the host-process backend removed outright, the Docker backend made opt-in because a
mounted `docker.sock` is host root, every exec moved off root to uid 1000, symlink escapes out of
the workspace closed, plus TTLs, idle sweeping and snapshot storage. Code written by this product's
own agent against a typed interface has no such requirement.

**A WebAssembly component runtime instead.** Better on every axis that matters here — in-process
(no daemon beside the binary, which keeps the one-binary-and-a-Postgres promise), microsecond
instantiation, no capability at all unless the host imports one, fuel-metered and interruptible,
and **deterministic**, which is the one this product actually needs: re-parsing claims existing
chunks by matching their text, and a transform that can read a clock or a socket can produce
different blocks on a second run and break the claim. It is still not built, because the reason
above is about priority rather than about which runtime is better. If it is ever built, the app's
source is JavaScript run by a JS engine compiled to Wasm, so no build toolchain is required in a
deployment, and the agent's iteration loop is the production runtime itself.

**A service identity per app.** The confused deputy, wearing a different word.

**An app as a saved conversation.** A rerun would replay a resolved question — the entities already
identified, the month it was asked in.

## What would reopen this

- A customer who needs a button **inside** this product rather than an agent outside it, named,
  with the thing they would click.
- The type layer settled: 0044 cut 2 complete, so an app has something stable to read.
- 0034 built first. A base that can conclude but cannot act is the older and larger gap, and an
  action is the smaller piece of work.

The smallest step that is not a platform, if one is ever wanted, is a saved question: a stored
prompt with its pinned definitions and its `as_of`, run by whoever opens it, with no new runtime,
no grants matrix and no schedules.
