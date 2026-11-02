# Source-mention references: a proposal for conflated namesake subjects

Status: for discussion; no ADR or change to the open extraction/push contract yet. Related: #1136 and #1193. Cross-script recall remains tracked by #709.

The subsequent [internal protocol draft](source-mention-reference-contract-draft.md) specifies fields, failure handling, compatibility routing and experimental results. Its recommendations still require review and an ADR; they are not in production.

## Reproduced information loss

The awards passage in the identity corpus reads:

> 财务部总监张伟获得年度管理奖。工程部工程师张伟获得年度技术奖。

In English: finance director Zhang Wei receives the annual management award; engineer Zhang Wei receives the annual technical award. The Chinese quotation remains unchanged because it is the actual regression input.

All four recorded model replies from the frozen 2026-10-22 experiment list only one named Zhang Wei, with the same subject string for both award statements. On 2026-10-28, calling `utopia_extract::open::parse_open_response` from then-current dev `1142792` directly on those replies retained one Zhang Wei subject in every case. A separate control put two same-name entries in e; the parser retained the first and counted the second as skipped. A control where one person held both roles also retained one subject. The representation cannot distinguish these cases from that result.

The full corpus gold assigns different identities to the two award recipients; two job titles alone do not prove two people. This reproduction made no new model calls and does not measure the current dev model error rate.

Three parts of the path must be addressed together:

- `crates/utopia-extract/src/open.rs`: e deduplicates by name_key; s/n reference names.
- `crates/utopia-server/src/extraction_open.rs`: local and known_by_name map a name to one entity.
- `crates/utopia-server/src/extraction.rs`: resolve_handle's response_claims excludes entities already claimed in the response. Adding two mentions while retaining this rule can force a false split of one person holding two roles.

The proposal therefore validates both which mention a fact belongs to and which entity that mention identifies. Different mentions do not imply cannot-link; a shared name does not prove same.

## Alternatives and experimental findings

| Representation | Benefit | Known limitation |
|---|---|---|
| Keep using the full source description as a name | Uses the existing format | Cannot express different occurrences of identical text; roles may be mistaken for names or identity. |
| Copy the quote and mention text; locate them on the server | Does not trust model-supplied offsets or entity IDs | Repeated sentences or two namesakes in one sentence may not locate uniquely. |
| Supply server fragment indexes; copy text and reference an index | Constrains the reply to the current chunk and source fragment | One fragment may still contain namesakes; fragment boundaries, input cost and version validity need measurement. |

The independent copied-quote prototype passed 13 expected checks: separately located awards, Markdown/emoji, PDF whitespace, repeated sentences, two same-name mentions in one sentence, fabricated/missing/wrong-chunk references, overlapping matches, empty quotes and one person changing roles. Passing includes correctly rejecting ambiguous references; it does not mean identity resolution is fixed.

**Recommendation:** do not implement a final contract based only on copied quotes. First add server source-fragment references to address ambiguity, while still requiring the mention to locate uniquely inside its current fragment. Preserve an explicit failure reason when it cannot. Agree on the fields, fragment granularity and input-token cost before recording the ADR. Models must not invent database entity IDs, and the server must not silently choose the first match.

## Contract decisions to agree on

1. Bound mention locations to the current chunk/version and compute character coordinates on the server. Keep the existing character units; do not mix in byte offsets. Whitespace normalization must map back to the original text.
2. Use one reference mechanism for subjects and entity objects in s, owners in n, and qualifiers. The known-entity list must represent multiple same-name records.
3. Give new model replies an explicit format version while preserving legacy reply semantics. A broken new-format reference must not silently fall back to the first name match.
4. Statements pushes under ADR 0054 keep accepting their existing format and null quotes. Do not impose the model's copied-source rule on clients; discuss any public extension separately.
5. Quote existence, valid location, subject ownership and identity are separate checks. A shared sentence must not automatically become exclusive identity evidence for every subject.
6. With two clear candidates, resolve each mention using only its own valid evidence. Repeated mentions of one person may resolve to the same entity. Preserve genuinely incompatible types and prior different decisions.
7. With no candidates or insufficient evidence, do not force two entities merely because roles or refs differ. First establish whether existing drop/unfinished signals preserve the source and report missing work accurately. If a new state is necessary, discuss its contract first rather than adding an entire unresolved-entity lifecycle.
8. Re-extraction and later chunks must not let known_by_name collapse distinct mentions again. Repeated mentions must not repeatedly add entities or human reviews.

These decisions precede implementation. CONTRIBUTING requires discussion and an ADR for data models, ontology contracts and public APIs. Statements pushes reuse the open extraction parser, so this PR supplies design and evidence without changing that public protocol.

## Implementation and acceptance scope

A minimal implementation must connect parser → mention validation → resolver context → fact evidence. Fixed tests cover both core controls in both orders:

- Two existing candidates with identities established by the complete sources, and a new reply correctly expressing two Zhang Wei mentions: each award belongs to the correct entity.
- An explicit same-person dual-role or role-change case: both mentions may resolve to one entity without response_claims forcing a split.

Also cover same-name subject/object pairs, repeated fragments, cross-sentence/shared/wrong-chunk/stale-version references, truncation, unclassified candidates, legacy format, explicit aliases, known lists, memory logs, identity sources and repeated imports. Silently dropping facts or reducing coverage does not resolve ownership.

The four old replies did not represent two subjects. A new parser cannot be required to reconstruct gold from missing information. Whether a new prompt improves raw output requires a separate same-session multi-KB comparison with a fixed model. Preserve the original identity corpus and scorer; report false merges/splits, coverage, entities, human work and call costs. Keep a candidate draft when new regressions cannot be excluded.

## Constraints from cross-script measurements

This is a separate problem, outside the subject-ownership implementation. Across eight frozen first cross-script states, the current 0.60 threshold recalled 李娜/Li Na in 0/8 cases; an offline control with deduplication and unique mutual nearest neighbours recalled 8/8; an adjacency control using only exact neighbour canonical names recalled 0/8.

This is not a production-ready recall design: with only two same-type candidates, any pair are mutual nearest neighbours. With fixed BGE vectors, the finance director Zhang Wei and engineer Zhang Wei (different people) have cosine≈0.7720; the finance director Zhang Wei and the Lanhai Institute finance director Zhang Wei (the same person) have cosine≈0.8196. Both pairs are mutual nearest neighbours and exceed the current threshold. This counterexample shows that mutual neighbours still need identity adjudication and candidate budgets; it does not establish acceptable noise for a new channel.

All eight states share one corpus and provide no independent generalization evidence. The adjacency control tested only exact name matching, not complete alias, relation-semantic or source-validity checks. The next step under #709 is to measure verifiable adjacency bridges. Do not add a production channel without reliable noise and safety evidence.

A subsequent read-only prototype required the same relation, direction and neighbour entity, supported by currently valid sources. It resolved neighbours using current name facts, proposed at most two candidates and abstained when a tied group exceeded that budget. Target recall remained 0/8. Further inspection found:

| Historical states | Structure missing at the first cross-script query |
|---|---|
| Four forward runs (0043, 0174, 0300, 0437) | Stored 李娜 has no relation edge to another entity. The new reply gives a named=0 object, `sea trials of Haitan-2`, without a bound shared edge between Li Na and the equipment. |
| Four reverse runs (0086, 0218, 0353, 0483) | Stored Li Na has only an open `lead → sea trials of Haitan-2` statement, with no bound relation. The new reply's object is “耐压舱技术规格” (pressure-hull technical specifications) or “理事会成员” (council member); equipment appears only in some qualifiers. |

These numbers refer to frozen `candidate-NNNN` snapshots, not new model runs. Equipment in a qualifier cannot be rewritten as a direct responsibility relation without semantic justification, nor can a co-occurring institute become an employment relation. This negative result concerns existing valid, bound graph edges; it does not rule out every open-fact or qualifier design. Lowering the name threshold cannot repair this missing structure.

After freezing the prototype, four development controls and 28 synthetic held-out controls were added, covering explicit neighbour aliases, shared organizations, parent organizations/centres, unknown/incompatible types, identifiers, prior different decisions, identity sources, stale/fabricated quotes and repeated mentions. Both input orders produced 64 rows and 448 prototype checks matching expectations. New held-out proposals included seven true and two false candidates; one further target was recalled by the existing explicit-name channel. Shared organizations still proposed a different colleague or a subordinate centre. These deliberately constructed graphs do not estimate real precision or validate the production queue. Source location, a common neighbour and shared identity remain separate questions.

This evidence rejects production integration of the one-hop bound-common-neighbour prototype. #709 remains open. The next experiment should first test whether open facts/qualifiers provide owned, explainable adjacency, then prepare independent real-corpus and adjudication-safety measurements.

## Scope of the recorded validation

- Direct production Rust parser calls: six groups (four real failure replies, duplicate e declarations and a same-person dual-role control).
- Source-location prototype: 13 checks, including ambiguous inputs that must be rejected.
- Original requests and prior states: four, with request and snapshot hashes retained.
- Offline cross-script comparison: eight frozen states, plus two adversarial candidate pools with fixed vectors.
- New real-model/embedding calls: zero. No merge-policy, prompt, database or push-API changes.

This document requests review of the source-reference contract. It does not present experimental prototypes or draft fields as accepted decisions.
