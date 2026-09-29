# Decision records

The current design, condensed per domain, is in [../design/](../design/README.md); this directory holds the reasons behind it.

Code records what was built and git records when it changed. Neither records why it was built this way and which roads turned out to be dead ends. This directory does.

The test for writing one: if someone (including us) looks at a piece of code in six months and asks "why not simply…", and the answer is not in the code, there should be a record.

## Conventions

**File names** are `NNNN-short-english-title.md`, four digits, increasing in creation order, no gaps. The number is a stable anchor for references and says nothing about priority.

**The directory is flat.** No subdirectories until a second kind of document appears.

**Revisions stay in place.** When a conclusion changes, especially because checking the code overturned it, keep a dated revision note where the original claim stood: what it said, why it was wrong, what changed it. The ledger this product keeps never updates a fact in place; a correction inserts a new row that supersedes the old one, because the change of mind is information. Records follow the same rule: knowing "we assumed a range would take effect immediately, checked, and found it never did" tells the next reader what to check first.

**When too much has changed**, write a new record and mark the old one `superseded by NNNN` at the top instead of rewriting it.

**The PR that implements a record updates its status line.** Three status lines once lagged behind the code in the same PR, written by the same person. So the PR description answers one question: which record does this change implement or overturn, and is its status line updated. (Added 2026-09-02, from [0016](0016-close-the-open-seams-before-cutting-new-ones.md).)

**A status line is one line.** It starts with one of five words, `Proposed`, `Accepted`, `In progress`, `Implemented` or `Superseded by NNNN`, then the date and the PR or migration, then `open:` and what is not built. What a cut contains belongs in the record's body or in the design file, not in the status line. The index below repeats only the first word, so a PR touches this README only when that word changes. (Added 2026-09-27; the longer lines that stood before are kept at the end of each record under Status history.)

**Line numbers drift and file names change.** Prefer function, table and constant names over `file.rs:123`. Migrations were consolidated from 53 files into one per domain (#130, #131); older references to migration files are by domain.

**Language: English.** The first sixteen records were written in Chinese and condensed into English on 2026-09-03; the Chinese originals remain in git history. Code comments are still Chinese; UI, README and records are English.

## Index

By domain; the domains are the files of [../design/](../design/README.md). **Status** is the first word of the record's own status line. **Overtaken** says whether a later record overturned it: `partly (by NNNN)` means some decisions fell and the record's revision notes say which. Open work: `grep 'open:' docs/decisions/0*.md`.

### [ontology](../design/ontology.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0001 | [Ontology import and governance](0001-ontology-import-and-governance.md) | In progress | partly (by 0009, 0010, 0012, 0044) |
| 0003 | [The ontology grows out of the corpus](0003-ontology-growth-loop.md) | Implemented | partly (by 0007, 0010, 0044) |
| 0007 | [Counting decides what becomes a relation](0007-who-decides-what-becomes-a-relation.md) | Implemented | partly (by 0044) |
| 0008 | [Ontology packs as the cold start](0008-ontology-packs-as-cold-start.md) | Implemented |  |
| 0009 | [An undecided type stays empty](0009-no-type-is-a-type.md) | Implemented |  |
| 0012 | [The ontology is a contract](0012-the-ontology-is-a-contract-not-a-suggestion.md) | Implemented | partly (by 0044) |
| 0044 | [The ontology is a view over what documents say](0044-the-ontology-is-a-view-over-what-documents-say.md) | In progress |  |
| 0051 | [A human phrase decision carries its materialization work](0051-a-human-phrase-decision-carries-its-materialization-work.md) | Implemented |  |
| 0053 | [A phrase decision records the inputs it considered](0053-a-phrase-decision-records-the-inputs-it-considered.md) | Implemented |  |
| 0061 | [The ontology is proposed from the open graph and judged by its questions](0061-the-ontology-is-proposed-from-the-open-graph-and-judged-by-its-questions.md) | In progress |  |
| 0062 | [The export says what is contested](0062-the-export-says-what-is-contested.md) | Implemented |  |

### [extraction](../design/extraction.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0006 | [Ontology scale and the extraction prompt](0006-ontology-scale-and-the-prompt.md) | Superseded | fully (by 0044) |

### [identity](../design/identity.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0041 | [A name is a claim about an entity](0041-a-name-is-a-claim-about-an-entity.md) | In progress |  |

### [time](../design/time.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0019 | [The second clock can be rewound](0019-the-second-clock-can-be-rewound.md) | Implemented |  |
| 0022 | [An unknown date is not an open one](0022-an-unknown-date-is-not-an-open-one.md) | Implemented | partly (by 0045) |
| 0024 | [The world axis reaches the second](0024-the-world-axis-reaches-the-second.md) | Implemented |  |
| 0031 | [An event holds at the moment it names](0031-an-event-holds-at-the-moment-it-names.md) | Implemented |  |
| 0045 | [A time mention is resolved against its document](0045-a-time-mention-is-resolved-against-its-document.md) | In progress |  |

### [ledger](../design/ledger.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0010 | [An unnamed relation stays empty](0010-no-relation-is-no-relation.md) | Implemented | partly (by 0044) |
| 0037 | [A relation carries its own attributes](0037-a-relation-carries-its-own-attributes.md) | In progress | partly (by 0044) |
| 0048 | [Provenance references stay inside the knowledge base](0048-provenance-references-stay-inside-the-knowledge-base.md) | Implemented |  |

### [governance](../design/governance.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0015 | [A recorded sentence waits for a nod](0015-recording-a-sentence-is-not-asserting-a-fact.md) | Implemented |  |
| 0017 | [A contradiction points at an error upstream](0017-a-contradiction-points-upstream.md) | Implemented |  |
| 0025 | [Governance reads the ledger before it decides](0025-governance-reads-the-ledger-before-it-decides.md) | Implemented |  |
| 0026 | [A decision records why](0026-a-decision-records-why.md) | Implemented |  |
| 0027 | [An automatic merge is gated by what it can undo](0027-an-automatic-merge-is-gated-by-what-it-can-undo.md) | Implemented |  |
| 0028 | [The adjudicator looks before it asks](0028-the-adjudicator-looks-before-it-asks.md) | Implemented |  |
| 0043 | [Every review queue is governed](0043-every-review-queue-is-governed.md) | Accepted |  |

### [rules](../design/rules.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0002 | [Reasoning engine](0002-reasoning-engine.md) | In progress |  |
| 0021 | [A rule reads attributes and concludes a type](0021-a-rule-reads-attributes-and-concludes-a-type.md) | Implemented |  |
| 0029 | [A rule may say "or", once](0029-a-rule-may-say-or-once.md) | Implemented |  |
| 0030 | [A rule may read what a rule concluded](0030-a-rule-may-read-what-a-rule-concluded.md) | In progress |  |
| 0032 | [A rule computes what it concludes](0032-a-rule-computes-what-it-concludes.md) | In progress |  |
| 0047 | [A rule may conclude a relation](0047-a-rule-may-conclude-a-relation.md) | Implemented |  |
| 0049 | [Expression declarations are checked when a rule is written](0049-expression-declarations-are-checked-when-a-rule-is-written.md) | Proposed |  |
| 0060 | [A rule's definition has a history](0060-a-rule-definition-has-a-history.md) | Implemented |  |

### [sources](../design/sources.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0013 | [A source hands over its history](0013-a-source-should-hand-over-its-history.md) | Implemented |  |
| 0023 | [RSS observations are not documents](0023-rss-observations-are-not-documents.md) | Implemented |  |
| 0033 | [RSS summaries are scoped to the source being listed](0033-rss-source-summaries-are-source-scoped.md) | Implemented |  |
| 0035 | [A vector index is built by a job](0035-a-vector-index-is-built-by-a-job.md) | Implemented |  |
| 0039 | [A chunk is what extraction sees](0039-a-chunk-is-what-extraction-sees.md) | In progress |  |
| 0040 | [A chunk says where its words came from](0040-a-chunk-says-where-its-words-came-from.md) | Implemented |  |
| 0052 | [Document content is a read contract over the retained ledger](0052-document-content-is-a-read-contract.md) | Proposed |  |
| 0054 | [A source may push statements in the open contract](0054-a-source-may-push-statements-in-the-open-contract.md) | Implemented |  |

### [lakehouse-and-actions](../design/lakehouse-and-actions.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0011 | [A mapping is configuration](0011-a-mapping-is-not-a-fact.md) | Implemented | partly (by 0036, 0044) |
| 0018 | [The lakehouse is one protocol away](0018-the-lakehouse-is-one-protocol-away.md) | Implemented |  |
| 0034 | [An action is a declared call](0034-an-action-is-a-declared-call.md) | Proposed |  |
| 0036 | [Exploration aligns a schema to the ontology](0036-exploration-aligns-a-schema-to-the-ontology.md) | In progress |  |
| 0050 | [An action attempt keeps its identity and uncertain outcome](0050-an-action-attempt-keeps-its-identity-and-uncertain-outcome.md) | Proposed |  |

### [access-and-audit](../design/access-and-audit.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0014 | [Identity from the person, scope from the token](0014-identity-from-the-person-scope-from-the-token.md) | Implemented |  |
| 0020 | [An auditor reads it without us](0020-an-auditor-reads-it-without-us.md) | Implemented |  |

### [interface](../design/interface.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0004 | [Language follows the reader of each text](0004-language-and-localization.md) | Implemented | partly (by 0044) |
| 0005 | [The alert center](0005-alert-center.md) | Implemented |  |
| 0038 | [The interface has a light side](0038-the-interface-has-a-light-side.md) | Implemented |  |

### [chat-and-mcp](../design/chat-and-mcp.md)

| | Record | Status | Overtaken |
|---|---|---|---|
| 0042 | [The chat loop is a runner with hooks](0042-the-chat-loop-is-a-runner-with-hooks.md) | Implemented |  |
| 0046 | [The app surface is MCP](0046-the-app-surface-is-mcp.md) | Accepted |  |
| 0063 | [Stopping a chat ends the generation](0063-stopping-a-chat-ends-the-generation.md) | Implemented |  |
| 0064 | [Extraction reads time as it goes](0064-extraction-reads-time-as-it-goes.md) | In progress |  |

### process

| | Record | Status | Overtaken |
|---|---|---|---|
| 0016 | [Close the open seams before cutting new ones](0016-close-the-open-seams-before-cutting-new-ones.md) | In progress | partly (by 0036) |

## Not a decision record

**[../pipeline.md](../pipeline.md), how a document becomes a graph.** Records explain why; that page explains how things flow and where they get dropped, with five mermaid diagrams. Newcomers read it first, then come back here for the reasons. It is the "second kind of document" the conventions mention, kept at the `docs/` root beside `decisions/`.

## What does not belong here

The `docs/` root is a local scratch area (`/docs/*` is git-ignored except `/docs/decisions/`, `/docs/design/` and `/docs/pipeline.md`). Research notes, temporary checklists and test output live there and stay out of the repository. When a draft settles into a judgment worth keeping, it moves here as a record.
