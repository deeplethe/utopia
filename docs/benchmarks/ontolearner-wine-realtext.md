# OntoLearner Wine Real-Text Benchmark

> This evaluates open ontology induction from public review text. For the separate OntoLearner
> taxonomy-discovery RAG protocol, see `docs/benchmarks/ontolearner-wine-official.md`.

This benchmark measures OntoPilot's ontology-learning pipeline on public, real prose while retaining
an external ontology gold standard.

## Design

- **Gold:** the MIT-licensed `wine` task in
  [`SciKnowOrg/ontolearner-food_and_beverage`](https://huggingface.co/datasets/SciKnowOrg/ontolearner-food_and_beverage).
- **Corpus:** 1,200 public Wine Reviews records, grouped into 60 auditable text documents. The mirror
  attributes Zackthoutt and identifies the source license as CC BY-NC-SA 4.0.
- **Leakage control:** OntoLearner labels may reserve matching records, but taxonomy edges are never
  verbalised or added to the corpus. Remaining reviews are selected deterministically by round-robin
  over wine varieties.
- **Primary metric:** direct taxonomy precision, recall, and F1 after projecting learned edges onto
  OntoLearner's 20 candidate types.
- **Diagnostic metrics:** class coverage and unfiltered open-ontology taxonomy precision, recall, and F1.

This is deliberately described as a **hybrid real-text benchmark**. It is not the official OntoLearner
Text2Onto task, whose documents are generated synthetically from an ontology.

## Run

From `backend/`, with OntoPilot already running:

```bash
git clone --depth 1 https://huggingface.co/datasets/spawn99/wine-reviews data/benchmarks/wine-reviews
pip install pyarrow
python scripts/benchmark_ontolearner_realtext.py prepare --reviews 1200 --reviews-per-document 20
python scripts/benchmark_ontolearner_realtext.py ingest
python scripts/benchmark_ontolearner_realtext.py extract --ks-id <id>
python scripts/benchmark_ontolearner_realtext.py score --ks-id <id>
```

Use `run` instead to execute every phase in one process. Authentication defaults to the local
development account and can be overridden with `ONTOPILOT_USERNAME` and `ONTOPILOT_PASSWORD`.

Generated corpus files, the exact source manifest, resumable state, `result.json`, and `REPORT.md`
are saved under `backend/data/benchmarks/ontolearner-wine-realtext/`, which is ignored by git.

## Interpreting Results

The projected taxonomy score is the closest comparison to OntoLearner taxonomy discovery because it
uses the benchmark's closed candidate vocabulary. The open score is intentionally harsher: every
additional relation learned from the reviews counts as a false positive against the narrow Wine gold.
Neither score should be presented as an official OntoLearner leaderboard submission.

## Reference Run

The first full run on 2026-08-09 used `deepseek/deepseek-chat` and the deterministic 1,200-review
corpus described above:

| Measure | Result |
|---|---:|
| Documents / chunks | 60 / 422 |
| Real-text characters | 584,648 |
| Chunk success rate | 421/422 (99.76%) |
| Final classes / properties / subclass edges | 1,578 / 65 / 1,679 |
| Gold class coverage | 8/20 (40.00%) |
| Projected taxonomy P / R / F1 | 0.6000 / 0.2000 / 0.3000 |
| Open taxonomy P / R / F1 | 0.0018 / 0.2000 / 0.0035 |
| Controlled terms / pending proposals | 1,630 / 5 |
| Open conflicts | 11 |
| End-to-end extraction time | 3,072.5 seconds |

The run exposed two important scale characteristics: unconstrained review extraction strongly
over-generates fine-grained classes, and semantic duplicate detection plus terminology sync dominate
post-processing time once the learned ontology reaches roughly 1,600 classes. These are product
diagnostics, not reasons to alter the benchmark score.
