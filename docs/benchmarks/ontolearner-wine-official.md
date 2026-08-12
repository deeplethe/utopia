# OntoLearner Wine Taxonomy Benchmark

This benchmark reports two deliberately separate prompt profiles: OntoPilot's frozen
closed-vocabulary taxonomy critic as the primary capability result, and OntoLearner's unchanged
prompt as a compatibility baseline. It is separate from the real-text extraction benchmark: no
documents are ingested, so the evidence-grounded production prompt cannot be used byte-for-byte.

Wine is also part of the [six-dataset, three-domain benchmark](ontolearner-multidomain.md).

## Reproduction

Run from `backend/`:

```bash
python scripts/benchmark_ontolearner_official.py --candidate-mode paper --prompt-profile ontopilot
python scripts/benchmark_ontolearner_repeated.py \
  --repeats 5 --models qwen/qwen3-8b --prompt-profile ontopilot
```

The single-run adapter caches embeddings and pair-level responses in its run directory. The repeated
runner creates a frozen protocol-script and dataset snapshot, gives every repetition fresh caches,
runs repetitions sequentially, resumes interrupted response sets, and writes `aggregate.json` plus a
Markdown report. A child process that exhausts request retries is restarted with backoff and continues
from its cache.

## Primary OntoPilot Profile

Run date: 2026-08-12

| Setting | Value |
|---|---|
| Prompt profile | `OntoPilot closed-vocabulary taxonomy critic v1` |
| Prompt SHA-256 | `cca6fc094ab6cf2cef33bc7d1902b7211a11129b487e8a53bed4ba50da474d35` |
| Model / retriever | `qwen/qwen3-8b` / `qwen/qwen3-embedding-8b` |
| Candidate orientation | Paper parent-candidate direction |
| Repetitions | 5 fresh response and embedding caches |
| Invalid responses | 0 / 1,500 |

| Metric | OntoLearner prompt baseline | OntoPilot profile | Gain |
|---|---:|---:|---:|
| Official F1 · 5-run mean | 26.29% | **28.95%** | **+2.66 pp / +10.1%** |
| Deduplicated structure F1 | 46.81% | **50.00%** | **+3.19 pp / +6.8%** |
| Versus paper Qwen3-8B F1 | 18.60% | **28.95%** | **+10.35 pp / +55.6%** |

Every OntoPilot run produced 37.93% precision, 23.40% recall, 28.9474% official F1, and 50.00%
structure F1. A paired candidate ablation scored 28.95% with the paper direction and 28.57% with
upstream bidirectional source candidates, a +0.38 pp direction contribution in that run.

The profile is derived from OntoPilot's production TBox boundary and subclass semantics but is
explicitly a closed-label task adapter. Production requires exact source evidence; OntoLearner does
not distribute a source extraction corpus. The result snapshot records the exact system/user prompt,
source mapping, and content hash. Strict JSON parsing rejects malformed output and endpoint reversal.

## Official-Prompt Compatibility Baseline

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

The compatibility baseline alone does **not** support a stable lead over the paper's best listed
25.0% result: one run scored below 25.0%, and the run-to-run interval overlaps it. The primary
OntoPilot profile does clear 25.0% in all five runs, but neither profile is a byte-identical
reproduction because OpenRouter applies a hosted serving stack while the reference implementation
runs Hugging Face generation locally. These are protocol-level taxonomy-discovery results, not an
official leaderboard submission or an evaluation of OntoPilot's raw-text extraction pipeline.

As a separate paired source-control experiment, the same five caches were extended with the reverse
candidates generated by OntoLearner revision `da7dd03c349ab8516518c5b0dee3bfed2deb8252`'s
`AutoRetrieverLearner._taxonomy_discovery`. The source-control mean was **25.97% F1**, versus
**26.29%** for the strict paper parent-candidate direction: **+0.32 percentage points or +1.2%
relative**. This comparison fixes the hosted model service and isolates candidate orientation; it is
not the source of the larger 41.3% paper comparison.

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

## Prompt Governance

The official prompt was frozen before its compatibility run. The OntoPilot profile follows the same
governance:

1. Retain the official prompt as an untouched compatibility baseline.
2. Derive task adapters from production rules rather than gold-edge examples.
3. Freeze exact text and hash before the full run.
4. Keep prompt and candidate contributions in separate ablation cells.

The only pre-full-run smoke test used top-k 2 to validate structured-output stability. The output
budget was raised after one truncated response; no semantic rule changed. Future tuning should use a
predeclared development partition, while the real-text production prompt belongs in the separate
real-text benchmark.
