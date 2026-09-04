# Decision records

Code records what was built and git records when it changed. Neither records why it was built this way and which roads turned out to be dead ends. This directory does.

The test for writing one: if someone (including us) looks at a piece of code in six months and asks "why not simply…", and the answer is not in the code, there should be a record.

## Conventions

**File names** are `NNNN-short-english-title.md`, four digits, increasing in creation order, no gaps. The number is a stable anchor for references and says nothing about priority.

**The directory is flat.** No subdirectories until a second kind of document appears.

**Revisions stay in place.** When a conclusion changes, especially because checking the code overturned it, keep a dated revision note where the original claim stood: what it said, why it was wrong, what changed it. The ledger this product keeps never updates a fact in place; a correction inserts a new row that supersedes the old one, because the change of mind is information. Records follow the same rule: knowing "we assumed a range would take effect immediately, checked, and found it never did" tells the next reader what to check first.

**When too much has changed**, write a new record and mark the old one `superseded by NNNN` at the top instead of rewriting it.

**The PR that implements a record updates its status line.** Three status lines once lagged behind the code in the same PR, written by the same person. So the PR description answers one question: which record does this change implement or overturn, and is its status line updated. (Added 2026-09-02, from [0016](0016-close-the-open-seams-before-cutting-new-ones.md).)

**Line numbers drift and file names change.** Prefer function, table and constant names over `file.rs:123`. Migrations were consolidated from 53 files into one per domain (#130, #131); older references to migration files are by domain.

**Language: English.** The first sixteen records were written in Chinese and condensed into English on 2026-09-03; the Chinese originals remain in git history. Code comments are still Chinese; UI, README and records are English.

## Index

| | Record | Status |
|---|---|---|
| 0001 | [Ontology import and governance](0001-ontology-import-and-governance.md) | In progress · P0–P2c built; the P3 budget built, P3a by hand only; P3b built in a different shape; P4b / P4c pending; P5 delivered by 0002; criterion 2 half overturned by 0012 |
| 0002 | [Reasoning engine](0002-reasoning-engine.md) | R0 checker and R1 materialization (KB switch, default off) built; R2 proof chain (#227); contradiction signals per 0017; R3 incremental maintenance not built |
| 0003 | [The ontology grows out of the corpus](0003-ontology-growth-loop.md) | Built and running, default on · starting point rewritten by 0010 and the retired seeds · dismissal redone per 0007 · the "new phrasings" reminder pending |
| 0004 | [Language follows the reader of each text](0004-language-and-localization.md) | Built · UI strings, coded server errors, ontology description language and the locale of generated text · the browser language is not guessed yet |
| 0005 | [The alert center](0005-alert-center.md) | Built · five alert kinds live, search and paging in the panel · `document.no_text_layer` still unwired |
| 0006 | [Ontology scale and the extraction prompt](0006-ontology-scale-and-the-prompt.md) | Built · the character budget (24,000) and per-chunk retrieval live, values untested · answer keys still hand-filled |
| 0007 | [Counting decides what becomes a relation](0007-who-decides-what-becomes-a-relation.md) | Built · adoption decided by counting (`MIN_DOCS = 2`, `MIN_SIGNALS = 3`), proposals persist (#112) · narrative verbs and `_by` folding still open |
| 0008 | [Ontology packs as the cold start](0008-ontology-packs-as-cold-start.md) | Built · five packs embedded, multi-select at creation, schema.org by default · three open questions stay open; Chinese labels got worse |
| 0009 | [An undecided type stays empty](0009-no-type-is-a-type.md) | Implemented · `type_id` nullable, builtin classes gone · kin classes go to Review (#226), declared `disjointWith` keeps them apart (0016 B3) · `metric` / `dimension` builtin on demand (#231) |
| 0010 | [An unnamed relation stays empty](0010-no-relation-is-no-relation.md) | Implemented · `predicate_id` nullable, `related_to` gone, wording recovered by `fact_surface_predicate` · follow-ups done with 0011 |
| 0011 | [A mapping is configuration](0011-a-mapping-is-not-a-fact.md) | Implemented (#126 / #140 / #148) · Review flow and revision history rebuilt · the evidence chain not built |
| 0012 | [The ontology is a contract](0012-the-ontology-is-a-contract-not-a-suggestion.md) | Implemented · violation rate 57% → 4%, reversals 39 → 0 · guard extended to adoption and merge (#190 / #196) · reified-shell filter at pack import open |
| 0013 | [A source hands over its history](0013-a-source-should-hand-over-its-history.md) | Implemented for GitHub, Jira and Notion (#134 / #135 / #213) · Feishu and Confluence not started · `instant` precision not triggered |
| 0014 | [Identity from the person, scope from the token](0014-identity-from-the-person-scope-from-the-token.md) | Implemented (#180) · five read-only MCP tools over Streamable HTTP · tokens page at `/account/tokens` · `can_write` still hard-coded false |
| 0015 | [A recorded sentence waits for a nod](0015-recording-a-sentence-is-not-asserting-a-fact.md) | Implemented · memory facts wait in `pending_facts`, nod queue and chat card, `remember` reopened · MCP write is the next cut |
| 0016 | [Close the open seams before cutting new ones](0016-close-the-open-seams-before-cutting-new-ones.md) | In progress · A done · B done (B4 deferred) · C1 done (#289) · C2 done (#297) · C3–C5 open · D2 worked around (#231); the lakehouse landed ahead of D4 (#239) |
| 0017 | [A contradiction points at an error upstream](0017-a-contradiction-points-upstream.md) | Implemented · B2a: engine and queue, per-item cap, aggregation by rule pair, cards with clues and repairs (#238) · B2b: contested edges in the alert colour, ghost edges for blocked derivations, the disputed chip and the "did not land" section in the panel (#243) |
| 0018 | [The lakehouse is one protocol away](0018-the-lakehouse-is-one-protocol-away.md) | Implemented: Trino (Iceberg / Delta / Hive), Databricks and Snowflake behind the same trait, scheme picks the engine (#239) · Trino verified against a real cluster (#327); Databricks and Snowflake still want one (#241, #242) · MaxCompute waits |
| 0019 | [The second clock can be rewound](0019-the-second-clock-can-be-rewound.md) | In progress · `held_at(T)` replaces the hard-coded `invalidated_at IS NULL` on the read paths, `at` and `as_of` separate to the API (#317) · the graph control stays a separate cut (#307) |
| 0020 | [A rule reads attributes and concludes a type](0020-a-rule-reads-attributes-and-concludes-a-type.md) | Planned · nothing built · `derived_facts` widens to match `facts`, a derived typing rides the builtin `is_a`, rules authored in `attribute_rules` by a person, validity is the premise intersection (#277) |

## Not a decision record

**[../pipeline.md](../pipeline.md), how a document becomes a graph.** Records explain why; that page explains how things flow and where they get dropped, with five mermaid diagrams. Newcomers read it first, then come back here for the reasons. It is the "second kind of document" the conventions mention, kept at the `docs/` root beside `decisions/`.

## What does not belong here

The `docs/` root is a local scratch area (`/docs/*` is git-ignored except `/docs/decisions/`). Research notes, temporary checklists and test output live there and stay out of the repository. When a draft settles into a judgment worth keeping, it moves here as a record.
