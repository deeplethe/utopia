# The design has a current layer

`docs/design/` says what the system does today, one file per domain. A design file changes when
the design changes. `docs/decisions/` says why: each record is history, written once and never
rewritten; a change of mind is a dated revision note in place or a new record that supersedes the
old one. Read a design file first; open a record when you need the reasoning, the measurements or
the dead ends behind one sentence of it. Records are cited as [0022]; issues and pull requests as
#725.

## Domains

| File | What it covers |
|---|---|
| [ontology](ontology.md) | Classes, relations, attributes and their declarations; packs and OWL import; growth and adoption; the workbench and alignment over the open graph (0044) |
| [extraction](extraction.md) | What extraction reads and writes, the open contract, drop signals, benches and thresholds |
| [identity](identity.md) | Entities, names (0041), resolution, adjudication, merges and their undo |
| [time](time.md) | Two clocks, the world axis and its precision, unknown bounds, the temporal engine, events, time mentions (0045) |
| [ledger](ledger.md) | Facts and their layers, evidence, invalidation and supersedes, qualifiers, pending facts, purge, export |
| [governance](governance.md) | Review queues, the governor and its gates, agent decisions, nods, the queue redesign (#725) |
| [rules](rules.md) | Axioms, business rules, derived facts, proofs, contradictions |
| [sources](sources.md) | Ingestion, connectors, documents and versions, chunks, media origins (0040), embeddings |
| [lakehouse-and-actions](lakehouse-and-actions.md) | Query engines over mounted data, the semantic layer, declared actions |
| [access-and-audit](access-and-audit.md) | Roles, tokens, credentials, the audit ledger, the export an auditor reads |
| [interface](interface.md) | Language, theme, alerts, the design rules, what the browse pages show |
| [chat-and-mcp](chat-and-mcp.md) | The chat loop, tools, retrieval, `remember`, MCP |

## What changed on 2026-09-16

Cut 1 of 0044 landed (#731): the ledger holds open statements in the document's own words. A
statement is a `facts` row with `layer = 'open'` and the phrase the document used, with qualifiers
keyed by the document's role word, the time words it mentions kept verbatim with an offset, and
evidence located by character offsets in the chunk; a thing the document describes without naming
is an entity with a `description` and no name fact. The same day the user decided that extraction
writes only the open graph: memory documents take the open path and wait for their nod as open
statements (#735), the typed extraction path is deleted together with its per-base switch (#736),
the optional bound pass of 0044 decision 3 is withdrawn, and typed facts will come only from
alignment (0044 cut 2). Extraction writes nothing bound to an ontology any more.

That decision supersedes the prompt-related decisions of 0001, 0003, 0004, 0006, 0010, 0011 and
0037 (the ontology in the prompt, its budget and per-chunk retrieval, the description language the
model reads, the growth loop fed from the model's predicate words, qualifiers keyed by declared
attributes at extraction) and, for the same reason, the write-time direction correction of 0012 and
the input of 0007's counting loop. Their ledger decisions stand: a predicate may be null and display
falls back to the source's wording (0010), evidence on every fact and every drop a row (0001),
qualifiers live beside the edge (0037), an adoption rewrite is an append with undo (0003). The index
marks these records from this note; their own status lines predate it and were not touched.

## Status words

`current`: every decision in the record holds. `partly superseded (by NNNN)`: a later record, or
the note above, overturned some of its decisions and the rest hold; the record's own revision notes
say which. `superseded (by NNNN)`: none of its decisions hold. `proposed`: the direction is accepted
and the code is not yet on `dev` (0034 has no code; 0043's cut 1 was in PR #699, closed on 2026-09-17 to re-land on the open graph after alignment, while the record itself is on `dev`; 0044 and 0045 sit in
PRs #710 and #724, and 0044's cut 1 has landed ahead of the record). The record's own status line
is the source of truth for what is built; the index only adds whether a later record has overtaken
it.

## Every record

The records are listed by domain in the [index](../decisions/README.md#index), with these words in its last column.

[prior-work.md](prior-work.md) is not a domain: it places each layer in the literature it stands on
and lists the pitfalls taken from it, with what is still open.

0016 is a schedule and a convention rather than a design; its convention (the PR that implements a
record updates its status line) lives in the [decisions README](../decisions/README.md), and its
lines are tracked in the domain files they belong to. [pipeline.md](../pipeline.md) still describes
the typed path of 2026-09-02 and waits for a rewrite around the open graph.
