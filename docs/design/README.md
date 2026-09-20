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
qualifiers live beside the edge (0037), an adoption rewrite is an append with undo (0003). The table
below marks these records from this note; their own status lines predate it and were not touched.

## Status words

`current`: every decision in the record holds. `partly superseded (by NNNN)`: a later record, or
the note above, overturned some of its decisions and the rest hold; the record's own revision notes
say which. `superseded (by NNNN)`: none of its decisions hold. `proposed`: the direction is accepted
and the code is not yet on `dev` (0034 has no code; 0043's cut 1 was in PR #699, closed on 2026-09-17 to re-land on the open graph after alignment, while the record itself is on `dev`; 0044 and 0045 sit in
PRs #710 and #724, and 0044's cut 1 has landed ahead of the record). The record's own status line
is the source of truth for what is built; this table only adds whether a later record has overtaken
it.

## Every record

| | Date | Record | Domain | Status |
|---|---|---|---|---|
| 0001 | 2026-08-27 | [Ontology import and governance](../decisions/0001-ontology-import-and-governance.md) | ontology | partly superseded (by 0009, 0010, 0012, 0044) |
| 0002 | 2026-08-28 | [Reasoning engine](../decisions/0002-reasoning-engine.md) | rules | current |
| 0003 | 2026-08-28 | [The ontology grows out of the corpus](../decisions/0003-ontology-growth-loop.md) | ontology | partly superseded (by 0007, 0010, 0044) |
| 0004 | 2026-08-29 | [Language follows the reader of each text](../decisions/0004-language-and-localization.md) | interface | partly superseded (by 0044) |
| 0005 | 2026-08-29 | [The alert center](../decisions/0005-alert-center.md) | interface | current |
| 0006 | 2026-08-29 | [Ontology scale and the extraction prompt](../decisions/0006-ontology-scale-and-the-prompt.md) | extraction | superseded (by 0044) |
| 0007 | 2026-08-30 | [Counting decides what becomes a relation](../decisions/0007-who-decides-what-becomes-a-relation.md) | ontology | partly superseded (by 0044) |
| 0008 | 2026-08-30 | [Ontology packs as the cold start](../decisions/0008-ontology-packs-as-cold-start.md) | ontology | current |
| 0009 | 2026-08-30 | [An undecided type stays empty](../decisions/0009-no-type-is-a-type.md) | ontology | current |
| 0010 | 2026-08-30 | [An unnamed relation stays empty](../decisions/0010-no-relation-is-no-relation.md) | ledger | partly superseded (by 0044) |
| 0011 | 2026-08-31 | [A mapping is configuration](../decisions/0011-a-mapping-is-not-a-fact.md) | lakehouse-and-actions | partly superseded (by 0036, 0044) |
| 0012 | 2026-08-31 | [The ontology is a contract](../decisions/0012-the-ontology-is-a-contract-not-a-suggestion.md) | ontology | partly superseded (by 0044) |
| 0013 | 2026-08-31 | [A source hands over its history](../decisions/0013-a-source-should-hand-over-its-history.md) | sources | current |
| 0014 | 2026-09-01 | [Identity from the person, scope from the token](../decisions/0014-identity-from-the-person-scope-from-the-token.md) | access-and-audit | current |
| 0015 | 2026-09-01 | [A recorded sentence waits for a nod](../decisions/0015-recording-a-sentence-is-not-asserting-a-fact.md) | governance | current |
| 0016 | 2026-09-02 | [Close the open seams before cutting new ones](../decisions/0016-close-the-open-seams-before-cutting-new-ones.md) | process | partly superseded (by 0036) |
| 0017 | 2026-09-03 | [A contradiction points at an error upstream](../decisions/0017-a-contradiction-points-upstream.md) | governance | current |
| 0018 | 2026-09-03 | [The lakehouse is one protocol away](../decisions/0018-the-lakehouse-is-one-protocol-away.md) | lakehouse-and-actions | current |
| 0019 | 2026-09-04 | [The second clock can be rewound](../decisions/0019-the-second-clock-can-be-rewound.md) | time | current |
| 0020 | 2026-09-05 | [An auditor reads it without us](../decisions/0020-an-auditor-reads-it-without-us.md) | access-and-audit | current |
| 0021 | 2026-09-05 | [A rule reads attributes and concludes a type](../decisions/0021-a-rule-reads-attributes-and-concludes-a-type.md) | rules | current |
| 0022 | 2026-09-06 | [An unknown date is not an open one](../decisions/0022-an-unknown-date-is-not-an-open-one.md) | time | partly superseded (by 0045) |
| 0023 | 2026-09-05 | [RSS observations are not documents](../decisions/0023-rss-observations-are-not-documents.md) | sources | current |
| 0024 | 2026-09-06 | [The world axis reaches the second](../decisions/0024-the-world-axis-reaches-the-second.md) | time | current |
| 0025 | 2026-09-06 | [Governance reads the ledger before it decides](../decisions/0025-governance-reads-the-ledger-before-it-decides.md) | governance | current |
| 0026 | 2026-09-08 | [A decision records why](../decisions/0026-a-decision-records-why.md) | governance | current |
| 0027 | 2026-09-08 | [An automatic merge is gated by what it can undo](../decisions/0027-an-automatic-merge-is-gated-by-what-it-can-undo.md) | governance | current |
| 0028 | 2026-09-08 | [The adjudicator looks before it asks](../decisions/0028-the-adjudicator-looks-before-it-asks.md) | governance | current |
| 0029 | 2026-09-08 | [A rule may say "or", once](../decisions/0029-a-rule-may-say-or-once.md) | rules | current |
| 0030 | 2026-09-08 | [A rule may read what a rule concluded](../decisions/0030-a-rule-may-read-what-a-rule-concluded.md) | rules | current |
| 0031 | 2026-09-08 | [An event holds at the moment it names](../decisions/0031-an-event-holds-at-the-moment-it-names.md) | time | current |
| 0032 | 2026-09-08 | [A rule computes what it concludes](../decisions/0032-a-rule-computes-what-it-concludes.md) | rules | current |
| 0033 | 2026-09-06 | [RSS summaries are scoped to the source being listed](../decisions/0033-rss-source-summaries-are-source-scoped.md) | sources | current |
| 0034 | 2026-09-08 | [An action is a declared call](../decisions/0034-an-action-is-a-declared-call.md) | lakehouse-and-actions | proposed |
| 0035 | 2026-09-09 | [A vector index is built by a job](../decisions/0035-a-vector-index-is-built-by-a-job.md) | sources | current |
| 0036 | 2026-09-09 | [Exploration aligns a schema to the ontology](../decisions/0036-exploration-aligns-a-schema-to-the-ontology.md) | lakehouse-and-actions | current |
| 0037 | 2026-09-10 | [A relation carries its own attributes](../decisions/0037-a-relation-carries-its-own-attributes.md) | ledger | partly superseded (by 0044) |
| 0038 | 2026-09-11 | [The interface has a light side](../decisions/0038-the-interface-has-a-light-side.md) | interface | current |
| 0039 | 2026-09-13 | [A chunk is what extraction sees](../decisions/0039-a-chunk-is-what-extraction-sees.md) | sources | current |
| 0040 | 2026-09-13 | [A chunk says where its words came from](../decisions/0040-a-chunk-says-where-its-words-came-from.md) | sources | current |
| 0041 | 2026-09-13 | [A name is a claim about an entity](../decisions/0041-a-name-is-a-claim-about-an-entity.md) | identity | current |
| 0042 | 2026-09-13 | [The chat loop is a runner with hooks](../decisions/0042-the-chat-loop-is-a-runner-with-hooks.md) | chat-and-mcp | current |
| 0043 | 2026-09-14 | Every review queue is governed (was PR #699, closed 2026-09-17; re-lands after 0044 cut 2) | governance | proposed |
| 0044 | 2026-09-16 | [The ontology is a view over what documents say](../decisions/0044-the-ontology-is-a-view-over-what-documents-say.md) | ontology | proposed |
| 0045 | 2026-09-16 | [A time mention is resolved against its document](../decisions/0045-a-time-mention-is-resolved-against-its-document.md) | time | proposed |

[prior-work.md](prior-work.md) is not a domain: it places each layer in the literature it stands on
and lists the pitfalls taken from it, with what is still open.

0016 is a schedule and a convention rather than a design; its convention (the PR that implements a
record updates its status line) lives in the [decisions README](../decisions/README.md), and its
lines are tracked in the domain files they belong to. [pipeline.md](../pipeline.md) still describes
the typed path of 2026-09-02 and waits for a rewrite around the open graph.
