# OntoPilot vs. OntoLearner: Methodology and Full Taxonomy Results

## Headline Comparison

| Benchmark | OntoLearner | **OntoPilot** | Improvement |
|---|---:|---:|---:|
| Wine hierarchy discovery · deduplicated F1 | 46.81% | **50.00%** | **+3.19 pp / +6.8%** |

OntoPilot establishes a new SOTA result in this unique directed hierarchy-edge setting. This is a
scoped claim about the Wine closed-vocabulary taxonomy-discovery task and the metric defined below,
not a claim of universal superiority across every ontology task.

## Comparison Method

The comparison holds the Qwen3-8B verifier, Qwen3-Embedding-8B retriever, paper-direction candidate
generation, temperature 0, seed 42, and unique-edge scorer constant. OntoLearner uses its unchanged
`StandardizedPrompting("taxonomy-discovery")`; OntoPilot uses its frozen taxonomy-critic prompt and
strict JSON response contract. Wine is reported as the mean of five independent fresh-cache runs.

This report uses one public taxonomy metric throughout: **Unique-edge F1**. Gold and predicted
parent-child relations are converted to unique directed edges before precision, recall, and F1 are
calculated. Duplicate source rows never increase the denominator.

Prompts are part of OntoPilot's learning kernel. Wine and OWL-Time were evaluated with the frozen
OntoPilot taxonomy-critic profile. The other four completed datasets still use the unchanged
OntoLearner prompt and are clearly marked as baselines; they are not presented as OntoPilot-prompt
results.

Neither profile ingests source documents. The distributed task contains type labels and gold edges,
so this is a closed-vocabulary hierarchy test rather than an end-to-end document extraction test.

## Six-Dataset Results

Run dates: 2026-08-11 to 2026-08-13

| Domain | Dataset | Runs | Precision | Recall | **Unique-edge F1** | Prompt profile |
|---|---|---:|---:|---:|---:|---|
| Food and beverage | Wine | 5 | 37.93% | 73.33% | **50.00%** | **OntoPilot** |
| Units and measurements | QUDV | 1 | 25.00% | 100.00% | **40.00%** | OntoLearner baseline |
| Geography | GeoNames | 1 | 26.32% | 71.43% | **38.46%** | OntoLearner baseline |
| Units and measurements | OWL-Time | 1 | 21.43% | 64.29% | **32.14%** | **OntoPilot** |
| Geography | GTS | 1 | 19.15% | 64.29% | **29.51%** | OntoLearner baseline |
| Geography | JUSO | 1 | 17.27% | 63.16% | **27.12%** | OntoLearner baseline |
| **Macro average** | **6 datasets** | — | **24.52%** | **72.75%** | **36.21%** | Mixed; see each row |

All five fresh-cache Wine runs reached 50.00% Unique-edge F1 with zero invalid responses. OWL-Time
also completed with zero invalid responses. Every result above covers the complete candidate set
generated for that dataset; no sampled or partial run is promoted.

## Prompt Contribution

These controlled comparisons hold the hosted Qwen3-8B verifier, Qwen3-Embedding-8B retriever, candidate
direction, temperature, seed, and scorer constant. Only the prompt and response contract change.

| Dataset | OntoLearner baseline | OntoPilot profile | Absolute gain | Relative gain |
|---|---:|---:|---:|---:|
| Wine · 5-run mean | 46.81% | **50.00%** | **+3.19 pp** | **+6.8%** |
| OWL-Time | 22.22% | **32.14%** | **+9.92 pp** | **+44.6%** |

The OntoPilot profile has not yet been run on QUDV, GeoNames, GTS, or JUSO. Their rows remain useful
completed baselines, but no prompt-kernel gain is claimed for them.

## Frozen OntoPilot Prompt Profile

| Setting | Value |
|---|---|
| Profile | `OntoPilot closed-vocabulary taxonomy critic v1` |
| Prompt SHA-256 | `cca6fc094ab6cf2cef33bc7d1902b7211a11129b487e8a53bed4ba50da474d35` |
| Source mapping | Production TBox boundary and subclass semantics in `backend/app/ontology/extract.py` |
| Output contract | Exact directed endpoints, boolean `keep`, confidence, and reason in strict JSON |
| Parsing | Fail closed on malformed JSON, missing boolean, or renamed/reversed endpoints |
| Acceptance threshold | `0.85` |
| Max output tokens | `768` |

The profile is a task adapter derived from OntoPilot's production rules. It is not byte-identical to
the production extraction prompt because production requires source text and exact evidence, inputs
that this closed-label dataset does not provide. The adapter preserves the directed subclass test,
class boundary, non-taxonomic exclusions, ambiguity handling, structured output, and fail-closed
parser. Exact text and its hash are frozen by `backend/scripts/benchmark_ontolearner_official.py`
and embedded in each result snapshot.

## Candidate-Direction Ablation

The reference source expands both directions for each retrieved neighbor pair. Our paper-direction
adapter asks only whether the retrieved parent candidate subsumes the query child. The following
paired results use the OntoLearner baseline prompt and report Unique-edge F1 only.

| Dataset | Paper direction | Upstream source direction | Absolute gain | Relative gain |
|---|---:|---:|---:|---:|
| Wine · 5-run mean | **46.81%** | 42.73% | **+4.08 pp** | **+9.5%** |
| OWL-Time | **22.22%** | 21.74% | **+0.48 pp** | **+2.2%** |
| QUDV | 40.00% | 40.00% | 0.00 pp | 0.0% |
| GeoNames | 38.46% | 38.46% | 0.00 pp | 0.0% |
| GTS | **29.51%** | 26.47% | **+3.04 pp** | **+11.5%** |
| JUSO | **27.12%** | 24.22% | **+2.90 pp** | **+12.0%** |
| **Macro average** | **34.02%** | 32.27% | **+1.75 pp** | **+5.4%** |

QUDV and GeoNames each contain 11 types, so top-k 15 is bounded to 10 and already covers every
possible directed pair; both candidate modes are therefore identical. On Wine with the OntoPilot
profile, paper direction reached **50.00%**, versus **46.15%** for the paired upstream source
direction: **+3.85 pp / +8.3% relative**.

## Evaluation Protocol

| Setting | Value |
|---|---|
| OntoLearner source revision | `da7dd03c349ab8516518c5b0dee3bfed2deb8252` |
| Retriever | `qwen/qwen3-embedding-8b` |
| Verifier | `qwen/qwen3-8b` |
| Candidate search | Full ontology type space, top-k 15 per child |
| Primary candidate orientation | Paper parent-candidate direction |
| Baseline prompt | Unmodified `StandardizedPrompting("taxonomy-discovery")` |
| OntoPilot prompt | Frozen closed-vocabulary taxonomy critic v1 |
| Temperature / seed | 0 / 42 |
| Serving stack | OpenRouter hosted APIs |
| Public metric | Precision, recall, and F1 over unique directed hierarchy edges |

The current six-dataset table covers 112 type entries and 1,570 verifier decisions. Hosted-provider
behavior can affect exact scores even at temperature zero.

## Dataset Integrity

| Dataset | Types | Raw rows | Unique edges | SHA-256 |
|---|---:|---:|---:|---|
| Wine | 20 | 47 | 15 | `b71612525de75ccbcad83e731d2ea353216e886a7b2d140ec423f547d16bfae6` |
| OWL-Time | 17 | 66 | 14 | `91961ab3f709b49aaaec126686f1c2695581e66eb4bce1fe9a71cf5653f1b774` |
| QUDV | 11 | 9 | 9 | `0e0f41d6ad60864aa75d1e915066132666a1ebe041507f0ded4bdca56e498081` |
| GeoNames | 11 | 18 | 7 | `d6bf4e5f1f4d8704793eadf48b8a6210be075e0e1f9606eea02817c80f0ac0ba` |
| GTS | 18 | 77 | 14 | `f9a7143b667e20cfa30bb3bc2aebdb56645d1616d71aa5a502ba6cd35e55cd27` |
| JUSO | 35 | 61 | 38 | `5fe26744838f8c920c8737b5907083b8b5a966b8ba740a2a0630c928c6611d63` |

The source datasets are published by SciKnowOrg on Hugging Face:

- [`SciKnowOrg/ontolearner-food_and_beverage`](https://huggingface.co/datasets/SciKnowOrg/ontolearner-food_and_beverage)
- [`SciKnowOrg/ontolearner-units_and_measurements`](https://huggingface.co/datasets/SciKnowOrg/ontolearner-units_and_measurements)
- [`SciKnowOrg/ontolearner-geography`](https://huggingface.co/datasets/SciKnowOrg/ontolearner-geography)

## Reproduction

Run from `backend/` after configuring the model endpoint. Each run directory stores prompt
snapshots, embeddings, raw model responses, parsed decisions, and result JSON. Existing complete
caches can be re-scored without another model request.

```bash
python scripts/benchmark_ontolearner_official.py \
  --gold data/benchmarks/ontolearner-units_and_measurements/owltime/type_taxonomies.json \
  --run-dir data/benchmarks/ontopilot-prompt-owltime-paper-20260813 \
  --dataset-name OWL-Time --models qwen/qwen3-8b \
  --candidate-mode paper --prompt-profile ontopilot --top-k 15
```

For Wine's repeated OntoPilot-profile result:

```bash
python scripts/benchmark_ontolearner_repeated.py \
  --run-root data/benchmarks/ontopilot-prompt-wine-repeats-20260812 \
  --repeats 5 --models qwen/qwen3-8b --prompt-profile ontopilot
```

Use the corresponding dataset path, an isolated run directory, and `--prompt-profile official` to
reproduce an OntoLearner prompt baseline. The profile name is retained in the machine interface for
backward-compatible caches; public reports still score and display only unique hierarchy edges.

## Interpretation and Limits

- Precision, recall, and F1 use sets of directed parent-child edges; repeated gold rows are removed.
- The prompt accepts direct and indirect superclass relations, while a gold file may list only a
  subset. A valid transitive relation can therefore count as a false positive.
- The benchmark supplies labels rather than source passages, so it does not measure evidence
  grounding, ingestion, review, release, or the rest of OntoPilot's governed workflow.
- QUDT, GEO, UO, and OM are not included. Full paper-direction runs would require 1,260, 4,920,
  8,430, and 11,970 verifier decisions respectively; no partial result is presented as complete.
