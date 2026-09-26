# 0053 · A phrase decision records the inputs it considered

- **Status**: implemented 2026-09-23 in PR #878 · `phrase_bindings.basis` (migration 0072), candidates admitted through the class hierarchy and shown to the model as such, structural outcomes recorded instead of skipped, the requeue condition reads live signatures only · closes the lifecycle half of #807 and the phrase half of #795 · kind words since 2026-09-26: `type_bindings.basis` (migration 0092), read from one snapshot and compared again when a reply is accepted, see [the revision below](#revision-2026-09-26-kind-words)
- **Written**: 2026-09-23
- **Related**: [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) decision 3; [0051](0051-a-human-phrase-decision-carries-its-materialization-work.md); #807, #795, #801 (withdrawn), #773, #754

## Problem

A phrase signature is decided once and cached; the cache is only right while the inputs that
produced it hold. Until now "the inputs" were identified by timestamps: a bound signature went
stale when its property was updated after the decision, a negative one when any property in the
base was added or updated. #807 and #795 showed four things timestamps cannot see:

1. **Inheritance.** Candidates were properties whose declared domain and range contained the
   endpoint class itself. A property declared on `legal_entity` was never a candidate for an
   `organization` signature, so a correct binding was structurally impossible, and no edit to the
   property would ever make it stale, because the property was never considered.
2. **Parent edges.** Adding `organization ⊂ legal_entity` can turn "no candidate" into a
   candidate; removing it can take the support from a bound signature. `entity_type_parents`
   carries no timestamp and no decision was tied to it.
3. **Edits during the request.** A definition changed while the model was answering commits before
   the decision does, so the decision's `decided_at` is later than the edit and nothing is stale,
   although both votes read the old definition (#795, reproduced with a scripted model).
4. **Two silences.** A signature with no candidate was skipped, never recorded: a previously bound
   signature whose property stopped fitting kept its typed projection. A signature with more than
   `CANDIDATE_LIMIT` candidates was also skipped, and since `stale` kept returning it, it queued a
   run every time without ever becoming executable. Worse, a signature whose endpoint class changed
   left an orphan row behind that `stale` returned forever (three rounds, one job each, in the
   review of #801).

#801 fixed the first and part of the fourth locally and was withdrawn by its author: the
interactions between signature identity, cached decisions, ontology changes and scheduling needed a
design, not another patch.

## Decision

**A decision stores a fingerprint of what it considered, and staleness is "the fingerprint of the
current inputs differs".** The fingerprint (`basis`) covers the ancestor closure of both endpoint
classes, whether the object is a value, and the set of candidate properties admitted through that
closure with each one's `updated_at`. The worker recomputes it for every live signature on every
run and compares it with the stored one. Nothing is compared to a clock.

This answers the four gaps at once. Inheritance changes the closure. A parent edge changes the
closure. An edit during the request changes a candidate's `updated_at`, so the stored fingerprint,
computed before the model was called, no longer matches at the next run: the decision remains
detectably stale exactly as #795 asked. And the two silences become recorded outcomes with their
own reasons, so they participate in staleness like any other decision.

**Candidates are admitted through the class hierarchy, and the model is told why.** `fits` walks
the ancestor closure; when a property fits only through an ancestor, the candidate line says
`fits by inheritance: organization is a subclass of legal_entity` and the system prompt says that
this is a fit. Widening the candidates in code without showing the basis made the model answer
null (#801's finding).

**Structural outcomes are decisions.** No admissible property: `none` with
`votes.reason = "no_candidates"`, which lets materialisation retire a projection whose support is
gone. More than the limit: `undecided` with `reason = "too_many_candidates"` and the count, which
puts it in the alignment queue for a person and stops it from requeueing. Both carry the
fingerprint, so a property added or removed reopens them like any other negative.

**The requeue condition reads live signatures only.** A run queues another run when a batch failed,
when a live signature has no decision and was not attempted, or when a live agent decision's
fingerprint no longer matches. An orphaned row (its signature moved because an endpoint class
changed) has no live signature and is never consulted; its typed rows retire through the ordinary
materialisation rule that a statement's current signature must be bound. Unchanged inputs
therefore leave no queued work.

**A person's decision is not fingerprinted.** It is never re-evaluated by the agent, so it carries
no basis; the human-precedence rule in `decide_on` is unchanged. A person-bound signature whose
property stops fitting keeps its projection: the person said so.

## Not doing

- Fingerprinting the kind-word aligner. #795's reproduction is on `align_types`; the same design
  applies and is the obvious next cut, but its inputs (kind words, class definitions, the
  hierarchy) are a different set and this record does not claim them. Done since, as the
  [revision below](#revision-2026-09-26-kind-words).
- A revision table of decisions. The old decision is overwritten in place as before; the audit
  ledger keeps the person's decisions and the projection changes. #807 asked what records are
  retained: the answer here is the current decision plus its basis, nothing historical.
- A separate job per signature. One run per base, batched, as before.

## Measurement

Regression coverage, each with a scripted model and a real PostgreSQL: inheritance admits a
property declared on an ancestor and the model sees the basis; removing the parent edge retires the
bound signature's projection without a model call; adding the edge reopens a structural `none`;
overflow is recorded as `undecided` with no model call and no requeue, and shrinking the candidate
set makes it executable again; an endpoint class change orphans the old row without an endless
requeue and decides the new signature; an edit during the model request leaves the decision stale
for the next run; a person's decision made during a request is not overwritten.

The cost is one fingerprint per live signature per run, computed from data the run already loads,
plus one query for property versions. Rows decided before this record have no basis and are
re-decided once.

## Revision 2026-09-26 (kind words)

The kind-word aligner records a basis too (`type_bindings.basis`, migration 0092, #795). Its inputs
are not a phrase's: the model is shown a kind word's candidate classes with their labels and
definitions, so the fingerprint covers each candidate class's `updated_at` and ancestor closure.
Candidates come from embedding retrieval, or the whole class list when there is no embedding model
and few classes. They are retrieved once per run for every live kind word, and the same lists are
reused when the run checks for staleness at its end.

Two things go further than the phrase half, which records its basis and catches an edit during
the request at the next run:

- **One snapshot.** The classes, their versions and parent edges, the signatures and the existing
  decisions are read in one `REPEATABLE READ, READ ONLY` transaction that closes before the first
  model call, so the definitions in the prompt and the versions in the fingerprint are one state.
- **Acceptance compares.** Writing an agent decision first locks the candidate class rows
  `FOR SHARE`, the order a class delete takes before it cascades to the binding, then recomputes
  the fingerprint from the rows as they are now. A reply whose fingerprint moved is discarded and
  the run queues another. Updates and deletes of the candidates wait for that transaction; a parent
  edge or a new class committed in the same window is not blocked, and leaves the decision
  detectably stale for the next run.

A failed retrieval falls back to the whole class list, as the phrase shortlist does: the words
decided during the failure carry that list in their fingerprints and are asked once more when
retrieval recovers. Deciding nothing instead would leave a small base untyped for as long as its
embedding endpoint is broken.

A stale word the run asked about but could not settle (a failed call, an unreadable reply, a
missing vote, no candidates) takes the bounded re-ask rather than the immediate requeue, so an
endpoint that fails every time cannot requeue the job round after round. A person's kind-word
decision commits together with its `align_phrases` job, the way 0051 pairs a phrase decision with
its materialisation. Every statement that changes a class's label or definition moves `updated_at`,
parent edges are in the closure, and the vector refresh (`set_type_embeddings`) changes neither, so
no semantic change escapes the fingerprint and re-embedding does not make decisions stale. What
imports and packs still lack is a shared writers' guard, which would only narrow the window above
from "stale next run" to "rejected now".

The cost is one retrieval per live kind word per run, one embedding request per 64 words and one
nearest-class query per word, where before only the words being decided were retrieved; the phrase
shortlist likewise embeds its wide signatures on every run. Rows decided before this revision have
no basis and are decided again once.

Regression coverage, with a scripted model and a real PostgreSQL: an edit during the model request
is not accepted and the next run asks with the new definition (red before this revision); a parent
edge alone makes an agent binding stale; rows without a basis are decided again once; a stale word
whose batch fails takes the bounded re-ask; a failed retrieval falls back and is not asked again
while it keeps failing; a person's decision carries no basis, is not overwritten, and commits with
its job; an acceptance and a class delete wait for each other instead of deadlocking, and a
candidate deleted before the check turns the reply away.
