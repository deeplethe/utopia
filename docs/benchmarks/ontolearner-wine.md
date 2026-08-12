# OntoPilot Wine Taxonomy Benchmark

Wine is evaluated as a closed-vocabulary hierarchy task over 20 supplied type labels. This report
uses only **Unique-edge F1**: duplicate gold rows are removed before precision, recall, and F1 are
calculated. It complements the [six-dataset benchmark](ontolearner-multidomain.md).

## Result

Run date: 2026-08-12

| Prompt profile | Runs | Precision | Recall | **Unique-edge F1** | Invalid responses |
|---|---:|---:|---:|---:|---:|
| OntoLearner baseline | 5 | 36.96% | 64.00% | 46.81% | 0 / 1,500 |
| **OntoPilot taxonomy critic** | **5** | **37.93%** | **73.33%** | **50.00%** | **0 / 1,500** |

With the verifier, retriever, candidate direction, temperature, seed, and scorer fixed, the
OntoPilot prompt improves mean Unique-edge F1 from 46.81% to **50.00%**: **+3.19 percentage points,
or +6.8% relative**.

Every fresh-cache OntoPilot repetition produced the same result:

| Run | Unique-edge F1 |
|---:|---:|
| 1 | **50.00%** |
| 2 | **50.00%** |
| 3 | **50.00%** |
| 4 | **50.00%** |
| 5 | **50.00%** |
| **Mean** | **50.00%** |

## Frozen OntoPilot Profile

| Setting | Value |
|---|---|
| Profile | `OntoPilot closed-vocabulary taxonomy critic v1` |
| Prompt SHA-256 | `cca6fc094ab6cf2cef33bc7d1902b7211a11129b487e8a53bed4ba50da474d35` |
| Model / retriever | `qwen/qwen3-8b` / `qwen/qwen3-embedding-8b` |
| Candidate orientation | Paper parent-candidate direction |
| Candidate pairs per run | 300 |
| Temperature / seed | 0 / 42 |
| Acceptance threshold | `0.85` |
| Repetitions | 5 independent response and embedding caches |

The profile derives from OntoPilot's production TBox boundary and subclass semantics, but is an
explicit closed-label task adapter. Production extraction requires source text and exact evidence,
which the benchmark does not distribute. Result snapshots record the complete system and user
prompts, their source mapping, and the content hash. Strict parsing fails closed on malformed JSON,
missing booleans, or renamed and reversed endpoints.

## Candidate-Direction Ablation

| Prompt | Paper direction | Upstream source direction | Gain |
|---|---:|---:|---:|
| OntoLearner baseline · 5-run mean | **46.81%** | 42.73% | **+4.08 pp / +9.5%** |
| OntoPilot profile · paired run | **50.00%** | 46.15% | **+3.85 pp / +8.3%** |

The paired source-direction run reuses the same embeddings and shared candidate responses, changing
only which directed candidate pairs enter verification.

## Data and Metric

| Item | Value |
|---|---:|
| Types | 20 |
| Raw hierarchy rows | 47 |
| Unique directed hierarchy edges | 15 |
| Retrieved unique gold edges | 14 / 15 |
| Dataset SHA-256 | `b71612525de75ccbcad83e731d2ea353216e886a7b2d140ec423f547d16bfae6` |

The 47 source rows contain repeated parent-child relations. Public results therefore use the 15
unique directed edges as the gold set. Predictions are deduplicated in the same way. The prompt can
accept a valid indirect superclass relation even when the gold file lists only a direct edge, so
some semantically defensible transitive relations can still count as false positives.

## Reproduction

Run from `backend/` after configuring the model endpoint:

```bash
python scripts/benchmark_ontolearner_repeated.py \
  --run-root data/benchmarks/ontopilot-prompt-wine-repeats-20260812 \
  --repeats 5 --models qwen/qwen3-8b --prompt-profile ontopilot
```

The runner freezes the protocol script and dataset, isolates caches by prompt profile and run,
persists raw provider responses, resumes interrupted work, and regenerates `aggregate.json` plus a
Markdown report. Existing complete caches are re-scored without another model request.

This is a taxonomy-discovery result over supplied labels. It does not evaluate source ingestion,
evidence grounding, human review, release governance, or end-to-end ontology extraction.
