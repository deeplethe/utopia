# 0006 · Ontology scale and the extraction prompt

- **Status**: Built · the character budget (`deployment_settings.ontology_prompt_budget`,
  24,000) and per-chunk retrieval are live, values unchanged; the "built-in classes always
  present" floor is replaced by ancestor completion; answer keys are still hand-filled.
- **Written**: 2026-08-29 · condensed into English 2026-09-03
- **Related**: [0008](0008-ontology-packs-as-cold-start.md) packs are now the starting
  ontology; [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) measured the bias
  that forced ancestor completion.

## The problem

The extraction prompt inlined the whole ontology once per chunk; `extraction.rs` called
`entity_types(kb)` with no selection step. After importing schema.org (968 classes, 1034
relations, 599 attributes) each chunk cost 108,133 tokens.
[0001](0001-ontology-import-and-governance.md) P3 had guessed "800 classes blows the
prompt" and got the shares wrong: classes 38%, relations 34%, attributes 28%.

## Decisions

1. **Switch by budget.** If the ontology fits, inline it all, unchanged: a forty-class
   ontology is 2k characters and retrieval would be a wasted round trip. If not, each
   chunk retrieves its own candidates with `chunk.embedding` (already loaded for entity
   resolution), falling back to the full list when nothing is retrieved. The budget is in
   characters because description lengths differ by orders of magnitude, and it measures
   the text `build_lists` actually lays out. It is a deployment setting because tuning it
   needs paired runs per level, and nobody reruns a curve that needs a server restart per
   point.
2. **Three details that exist only once there is a choice.** Signatures may name only
   inlined classes (an absent class in `works_at (person → organization)` teaches the
   model a type that does not exist; an unselected side becomes `*`); attributes follow
   their domain class; both paths share one `build_lists`, or the prompt stops matching
   what the code accepts.
3. **Ancestor completion** (2026-09-02): whatever retrieval hits, its `subClassOf`
   ancestors are inlined too, breadth-first with a visited set. The chain is the
   ontology's own declaration of generalization; no hand-kept list of "general classes".
   The only detail forced by real data.

**The bench** (`scripts/bench/corpora/pharma.json`, 5 Chinese pharmaceutical documents,
one run each) showed no degradation up to 58,651 characters of ontology (about 15k tokens,
202 classes): entities per run swung 23–28 with no trend, so 24,000 is conservative. Full
inline has a ceiling unrelated to quality: at 394 classes (169,380 characters) it hit the
vendor's per-minute token limit (429) and never finished; retrieval did. With schema.org,
correctly typed entities went 2 of 19 with seeds only, 7 with the large ontology in
post-hoc resolution, 12 with per-chunk retrieval. Caveats: single runs (the same input has
produced 25 and 18 entities) and alphabetical subsets (`subset.mjs` takes the first N
classes), hence no hit rate.

## Dead ends

- **A fixed floor of built-in classes.** Its premise, that seed classes are the general
  ones, died when #128 stopped seeding.
  [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) measured the symptom:
  retrieval favors leaf classes that appear literally in the text (`researcher` ranks 4th
  of 976, `person` 359th), so `employee (organization → person)` degraded to `(* → *)`.
- **`BATCH = 16` in the ontology index**, chosen from synthetic short texts; 64 doubled
  throughput. **`join_all` over 31 embedding batches** held the shared `model_concurrency`
  gate for fifteen minutes; now `EMBED_JOBS = 4`.

## Revisions

- Withdrawn: "the 108k prompt ate 5 entities". The bench ingested first and imported
  later, so both runs extracted with 9 seed classes and `prompt_tokens_est` measured an
  ontology extraction never saw; 25 vs 18 was run variance. The bench now reports
  `ontology_at_extraction` and `ontology_at_resolution` separately and has
  `--ontology-first`.
- 2026-09-02: per-chunk retrieval sends one vector query per chunk with concurrency up to
  `worker_concurrency` (cap 256 since #133) against a pool of 32; migration `0011` records
  it.

## Open questions

- **Legitimacy of the corpus.** The set grew from two to eight, with generation scripts
  committed, but `scripts/bench/truth/` still holds only the hand-filled `pharma` and
  `tech` keys. The intended fix is Chinese Wikipedia text with Wikidata `P31` as the type
  answer, so the answer stops being our opinion; no `P31` code exists yet.
- **Budget and per-chunk candidate counts** (24,000; 40 classes / 30 relations / 30
  attributes) are guesses; tuning before the corpus question is settled only tightens the
  overfit. `run.mjs --packs schema-org,prov-o` walks the real cold-start path, but that
  comparison has not been run.
