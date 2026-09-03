# 0007 · Counting decides what becomes a relation

- **Status**: Built · adoption by counting (`MIN_DOCS = 2`; an LLM run needs `MIN_SIGNALS = 3`); six defects fixed; proposals persist in `ontology_proposals` (#112); open: narrative verbs in the ontology, `merge_key` not folding `_by`; the starting point (10 seeds, the `related_to` share) no longer exists.
- **Written**: 2026-08-30 · condensed into English 2026-09-03
- **Related**: [0006](0006-ontology-scale-and-the-prompt.md) for the bench and its caveats; [0010](0010-no-relation-is-no-relation.md) and [0011](0011-a-mapping-is-not-a-fact.md) removed the seed relations this record started from; [0008](0008-ontology-packs-as-cold-start.md) packs are now the start; [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) has the successor metric.

## The problem

A new knowledge base started with 10 seed relations. Most relations the model produced were not among them and degraded to `related_to`, a predicate that says nothing: 49.8% of facts on the ai-timeline corpus (15 Wikipedia articles on AI companies, 348 chunks). `bootstrap_ontology` existed for exactly this and worked badly, for six independent reasons:

1. Exact-string matching discarded vocabulary already in the ontology: `produced_by | ChatGPT → OpenAI` missed `produces`. Fix: `predicate_match` (spelling, inflection, and a trailing `by` that swaps subject and object).
2. If the last document failed, the KB stuck forever: auto-extension was enqueued on the success path only. Fix: enqueue on both paths.
3. The ≥ 2 documents threshold only gated the LLM run; `build_proposals` re-queried the unfiltered set, so 456 of 526 phrasings (86.7%) came from one document and five were adopted.
4. `doc_count` counted leftovers still on the fallback predicate, so a KB fed one document at a time never reached two. Fix: prevalence counts all evidence.
5. "Ignore" was a one-way door: `record_miss` skipped dismissed phrasings, so counts froze at 1 while twenty later documents used them. Fix: count everything; list dismissed apart, undoable.
6. Undated facts claimed day precision (`valid_precision NOT NULL DEFAULT 'day'`; 728 and 843 live rows). Fix: nullable, CHECK-tied to a date (`facts.valid_from_precision`).

## Decisions

1. **Counting decides adoption; the model is not asked.** The data answers "which phrasings deserve to become relations", and the model got it wrong: it skipped `runs_on` (8 documents, 13 facts) and adopted a single-document `pledged_capital`. Three deterministic steps: group by inflectional base (`predicate_match::merge_key`: `sued` and `sues` are one), take the union of documents per group (never a sum), pass `MIN_DOCS = 2`; the group is named by its most frequent phrasing. From 10 seeds, `related_to` went 55.5% with auto-extension off, 39.1% with LLM adoption (17 relations), 25.8% with counted adoption (104).
2. **The model keeps the question that needs meaning**: synonym merging (is `collaborates_with` the same as `partnered_with`), reversible with `unadopt`.
3. **Deterministic adoption is the most valuable side effect**: the same corpus produces the same ontology, so the bench can compare this stage at all.
4. **`_by` is the only inverse marker worth handling** (#109): 31 forms (`founded_by` 42 facts), nearly all coexisting with an active form (`produces` 265). Before adopting, `PredicateIndex` is consulted; an existing equivalent, `_by` inverse included, is rewritten to with subject and object swapped. `has_X`/`X_of` had two pairs, not enough evidence.

Caveats: differences between LLM groups are within run variance (25 vs 18 entities on the same input); only the deterministic part is certain (single-document adoptions 5 → 0). The `related_to` share is no quality metric, since adopting everything lowers it fastest, and the corpus is dense in training data.

## Dead ends

- **Snowball stemming** (`rust-stemmers`) went the wrong way: 49 matches with a 10-word vocabulary, 18 with schema.org's 629, because it strips derivational suffixes too; `producer` and `produces` both become `produc` and the collision rule refuses to match. Inflectional suffixes only: 49 → 59.
- **Subject diversity instead of document count**: stricter in practice (6 gained, 33 lost), and most of the 6 were coordinated subjects ("A, B and C proposed X" split into three facts). `docs >= 2 OR subjects >= 3` fell to the same problem.
- **Auto-extension after every document**: "≥ 2 documents" becomes a so-far property, and merging degrades, since the LLM merges `acquired`/`acquires`/`acquisition_of` only when it sees them together.
- **A fact-count threshold (≥ 3) for new single-document relations**: the top two it admits are the worst two (`has_property` 11, `intends_to` 5); about 5 of 20 groups look like relations, 1.2% of the KB. Growing KBs solve this themselves; static corpora have the manual panel (`min_docs = 0`).

## Revisions

- 2026-09-02: both premises are gone. Seed relations left in three steps (`related_to` in [0010](0010-no-relation-is-no-relation.md), eight more in #125, `mapped_to` in [0011](0011-a-mapping-is-not-a-fact.md)) and the seeding function with them (#128); a new KB starts from a pack ([0008](0008-ontology-packs-as-cold-start.md)). The numbers above are not comparable with anything measured today: a fact may now have no predicate (the original word goes to `fact_evidence.proposed_predicate`), and the successor metric, empty-predicate share, measured 25.6% in [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) under a different start and definition. The decisions are unaffected and all live.
- 2026-09-02: the ontology can declare inverses (#177/#179) and direction is corrected on write (#138, leaving a `direction_corrected` trace). That does not close the `merge_key` gap below.

## Open questions

- **Narrative verbs enter the ontology.** Among the 104 relations: `reported/report` 11, `states/stated` 6, `describes/described` 5, `criticizes/criticized` 6: the article citing sources, no structure between companies. They recur across documents, so counting cannot stop them, and the ontology feeds back into the prompt. Candidate: ask the synonym-merging LLM call "which of these are the article's voice", small and reversible; a verb list would break with the next corpus.
- **`merge_key` does not fold `_by`.** When neither direction is in the ontology (`founded_by` 42 vs `founded` 4, both qualifying), two opposite relations are still created. The original reason, "the adoption path cannot swap", no longer holds; fold `_by` into the group and mark it for swapping.
- **What a 1,500-term start does to the adoption loop** has not been measured; no threshold has moved.
