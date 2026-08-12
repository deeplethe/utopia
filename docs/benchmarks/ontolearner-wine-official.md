# OntoLearner Wine Taxonomy Benchmark

This benchmark runs OntoPilot's configured verifier model through the official OntoLearner
taxonomy-discovery RAG protocol. It is intentionally separate from the real-text extraction
benchmark: no documents are ingested and OntoPilot's extraction prompt is not used here.

## Reproduction

Run from `backend/`:

```bash
python scripts/benchmark_ontolearner_official.py --candidate-mode paper
python scripts/benchmark_ontolearner_repeated.py --repeats 5
```

The single-run adapter caches embeddings and pair-level responses in its run directory. The repeated
runner creates a frozen protocol-script and dataset snapshot, gives every repetition fresh caches,
runs repetitions sequentially, resumes interrupted response sets, and writes `aggregate.json` plus a
Markdown report. A child process that exhausts request retries is restarted with backoff and continues
from its cache.

## Frozen Baseline

Run date: 2026-08-11

| Setting | Value |
|---|---|
| OntoLearner version | 1.6.0 |
| OntoLearner source revision | `da7dd03c349ab8516518c5b0dee3bfed2deb8252` |
| Ontology | Wine |
| Types | 20 |
| Raw taxonomy rows | 47 |
| Unique taxonomy pairs | 15 |
| Retriever | `qwen/qwen3-embedding-8b` |
| Retrieval | Full type space, top-k 15 |
| Candidate orientation | Strict paper parent-candidate direction |
| Candidate pairs | 300 |
| Prompt | Unmodified `StandardizedPrompting("taxonomy-discovery")` |
| Temperature / seed | 0 / 42 |
| Repetitions | 5 fresh embedding and response caches |
| Concurrent workers | 10 within each sequential repetition |

### Retrieval

| Metric | Official denominator | Deduplicated diagnostic |
|---|---:|---:|
| Recall | 29.79% | 93.33% |
| Retrieved gold pairs | 14 / 47 | 14 / 15 |

### End-to-End Taxonomy Discovery

| Verifier | Runs | Mean F1 | Std. dev. | Run-to-run 95% t-interval | Min | Max |
|---|---:|---:|---:|---:|---:|---:|
| `qwen/qwen3-8b` | 5 | **26.29%** | 2.86% | 22.74–29.83% | 21.92% | 29.73% |
| `deepseek/deepseek-chat` | 5 | **25.37%** | 2.63% | 22.11–28.63% | 22.22% | 28.95% |

| Run | Qwen3-8B F1 | DeepSeek F1 |
|---:|---:|---:|
| 1 | 29.73% | 27.03% |
| 2 | 26.67% | 24.32% |
| 3 | 27.40% | 28.95% |
| 4 | 21.92% | 24.32% |
| 5 | 25.71% | 22.22% |

The [OntoLearner paper](https://arxiv.org/abs/2607.01977) reports 18.6% F1 for Qwen3-8B and a
best listed Food & Beverage taxonomy-discovery result of 25.0%. All five hosted Qwen3-8B runs exceed
the same-model 18.6% result; the mean gain is 7.69 percentage points, or 41.3% relative. This supports
the narrow statement:

> Across five fresh-cache repetitions of OntoLearner's Wine taxonomy-discovery paper protocol, our
> hosted Qwen3-8B configuration averaged 26.29% F1, 7.69 percentage points above the paper's reported
> 18.6% result for the same model.

It does **not** support saying that OntoPilot stably beats the paper's best listed 25.0% result: one
run scored below 25.0%, and the run-to-run interval overlaps it. It is also not a byte-identical
reproduction because OpenRouter applies a hosted serving stack while the reference implementation
runs Hugging Face generation locally. These are protocol-level taxonomy-discovery results, not an
official leaderboard submission or an evaluation of OntoPilot's raw-text extraction pipeline.

## Metric Caveat

Wine's `type_taxonomies.json` contains 47 rows but only 15 unique parent-child pairs. OntoLearner's
metric converts rows to sets when calculating correct predictions, while retaining the raw list
length as the recall denominator. As a result, recall cannot exceed 15 / 47 = 31.91%, even if every
unique gold edge is recovered. This report preserves that behavior for comparison and also reports a
deduplicated diagnostic.

The official prompt accepts direct or indirect superclass relationships, while the gold file records
only its listed edges. Consequently, valid transitive statements such as `Port is-a wine` can be
counted as false positives when only `Port is-a RedWine` appears in gold. This mismatch is a benchmark
artifact and one reason to retain both official and structure-aware diagnostics.

## Prompt Decision

The prompt should not be optimized before the first official run:

1. Freeze the official prompt and record a reproducible baseline.
2. Develop prompt variants only on a separate development set.
3. Lock the selected prompt before evaluating a held-out test set.
4. Keep the untouched official-prompt score in every report.

Tuning directly against the full Wine gold after inspecting its errors would leak test information.
Prompt work should instead use other OntoLearner ontologies or a predeclared development partition.
For the real-text product pipeline, tune the extraction prompt against the real-text benchmark rather
than this pair-classification task.
