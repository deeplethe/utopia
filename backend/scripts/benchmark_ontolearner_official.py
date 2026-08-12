"""Run the OntoLearner taxonomy-discovery protocol through OpenRouter.

This adapter follows OntoLearner's end-to-end RAG formulation and source-code
candidate generation:
embed every ontology type with Qwen3-Embedding-8B, retrieve the top-k potential
neighbors for every type, verify both source-code orientations with the official
standardized yes/no prompt, and score with OntoLearner's taxonomy metric. Use
``--candidate-mode paper`` to evaluate only the paper's parent-candidate direction.

The run also reports a deduplicated diagnostic because some published datasets
contain repeated parent-child rows while OntoLearner's metric deduplicates the
intersection but uses the raw row count as the recall denominator.

Run from ``backend``:

    python scripts/benchmark_ontolearner_official.py
"""
from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import http.client
import json
import math
import os
import re
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections import Counter
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
BACKEND_DIR = SCRIPT_DIR.parent
DEFAULT_GOLD = BACKEND_DIR / "data" / "benchmarks" / "ontolearner-food_and_beverage" / "wine" / "type_taxonomies.json"
DEFAULT_RUN_DIR = BACKEND_DIR / "data" / "benchmarks" / "ontolearner-wine-official"
DEFAULT_RETRIEVER = "qwen/qwen3-embedding-8b"
DEFAULT_MODELS = ("qwen/qwen3-8b", "deepseek/deepseek-chat")
DEFAULT_BASE_URL = "https://openrouter.ai/api/v1"
OFFICIAL_SOURCE_REVISION = "da7dd03c349ab8516518c5b0dee3bfed2deb8252"
OFFICIAL_PROMPT = """You are identifying taxonomic (is-a) relationships.

Question:
Is "{parent}" a superclass (direct or indirect) of "{child}" in a standard conceptual or ontological hierarchy?

Rules:
- A superclass means: "{child}" is a type or instance of "{parent}".
- Answer "yes" only if the relationship is a true is-a relationship.
- Answer "no" for part-of, related-to, or associative relationships.
- Use general world knowledge.
- Do not explain.

Parent: {parent}
Child: {child}
Answer (yes or no):"""
_ANSWER = re.compile(r"\b(yes|no|true|false)\b", re.IGNORECASE)
_CACHE_LOCK = threading.Lock()


def now_iso() -> str:
    return datetime.now(UTC).isoformat()


def load_env(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    if not path.exists():
        return values
    for raw_line in path.read_text(encoding="utf-8-sig").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip('"').strip("'")
    return values


def atomic_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as handle:
        json.dump(value, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
        temp_path = Path(handle.name)
    temp_path.replace(path)


def read_json(path: Path, default: Any = None) -> Any:
    if not path.exists():
        return default
    with path.open("r", encoding="utf-8-sig") as handle:
        return json.load(handle)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def post_json(url: str, api_key: str, payload: dict, timeout: float, retries: int = 7) -> dict:
    body = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://github.com/deeplethe/ontopilot",
            "X-Title": "OntoPilot OntoLearner Benchmark",
        },
        method="POST",
    )
    for attempt in range(retries):
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")[:1000]
            if error.code not in {408, 409, 429, 500, 502, 503, 504} or attempt == retries - 1:
                raise RuntimeError(f"OpenRouter HTTP {error.code}: {detail}") from error
        except (TimeoutError, urllib.error.URLError, http.client.RemoteDisconnected) as error:
            if attempt == retries - 1:
                raise RuntimeError(f"OpenRouter request failed: {error}") from error
        time.sleep(min(30.0, 1.5 * (2**attempt)))
    raise RuntimeError("OpenRouter request exhausted retries")


def embeddings(base_url: str, api_key: str, model: str, inputs: list[str], timeout: float) -> list[list[float]]:
    payload = post_json(
        f"{base_url.rstrip('/')}/embeddings",
        api_key,
        {"model": model, "input": inputs, "encoding_format": "float"},
        timeout,
    )
    rows = sorted(payload.get("data", []), key=lambda row: row.get("index", 0))
    vectors = [row.get("embedding") for row in rows]
    if len(vectors) != len(inputs) or any(not isinstance(vector, list) for vector in vectors):
        raise RuntimeError(f"Unexpected embeddings response: expected {len(inputs)} vectors, got {len(vectors)}")
    return vectors


def cosine(left: list[float], right: list[float]) -> float:
    dot = sum(a * b for a, b in zip(left, right))
    left_norm = math.sqrt(sum(value * value for value in left))
    right_norm = math.sqrt(sum(value * value for value in right))
    return dot / (left_norm * right_norm) if left_norm and right_norm else 0.0


def retrieve_candidates(
    types: list[str],
    vectors: list[list[float]],
    top_k: int,
    candidate_mode: str,
) -> list[dict]:
    candidates: list[dict] = []
    seen: set[tuple[str, str]] = set()
    for child_index, child in enumerate(types):
        ranked = sorted(
            (
                (cosine(vectors[child_index], vectors[parent_index]), parent_index, parent)
                for parent_index, parent in enumerate(types)
                if parent_index != child_index
            ),
            key=lambda row: (-row[0], row[1]),
        )[:top_k]
        for rank, (score, _, parent) in enumerate(ranked, start=1):
            orientations = [(parent, child)]
            if candidate_mode == "source":
                orientations.append((child, parent))
            for candidate_parent, candidate_child in orientations:
                key = candidate_parent.lower(), candidate_child.lower()
                if key in seen:
                    continue
                seen.add(key)
                candidates.append(
                    {
                        "parent": candidate_parent,
                        "child": candidate_child,
                        "similarity": round(score, 8),
                        "rank": rank,
                    }
                )
    return candidates


def cache_key(model: str, parent: str, child: str) -> str:
    value = json.dumps([model, OFFICIAL_PROMPT, parent, child], ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def map_answer(content: str) -> str:
    match = _ANSWER.search(content or "")
    if not match:
        return "invalid"
    return "yes" if match.group(1).lower() in {"yes", "true"} else "no"


def classify_pair(
    base_url: str,
    api_key: str,
    model: str,
    candidate: dict,
    timeout: float,
) -> dict:
    prompt = OFFICIAL_PROMPT.format(parent=candidate["parent"], child=candidate["child"])
    payload = post_json(
        f"{base_url.rstrip('/')}/chat/completions",
        api_key,
        {
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0,
            "max_tokens": 8,
            "seed": 42,
        },
        timeout,
    )
    choices = payload.get("choices") or []
    content = choices[0].get("message", {}).get("content", "") if choices else ""
    return {
        "parent": candidate["parent"],
        "child": candidate["child"],
        "answer": map_answer(content),
        "raw_answer": content.strip()[:500],
        "similarity": candidate["similarity"],
        "rank": candidate["rank"],
    }


def run_model(
    run_dir: Path,
    base_url: str,
    api_key: str,
    model: str,
    candidates: list[dict],
    workers: int,
    timeout: float,
) -> list[dict]:
    cache_path = run_dir / f"responses-{model.replace('/', '--')}.json"
    cached = read_json(cache_path, {}) or {}
    pending = [candidate for candidate in candidates if cache_key(model, candidate["parent"], candidate["child"]) not in cached]
    print(f"[{model}] cached={len(candidates) - len(pending)} pending={len(pending)}")

    def task(candidate: dict) -> tuple[str, dict]:
        key = cache_key(model, candidate["parent"], candidate["child"])
        return key, classify_pair(base_url, api_key, model, candidate, timeout)

    completed = 0
    if pending:
        with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as executor:
            future_map = {executor.submit(task, candidate): candidate for candidate in pending}
            for future in concurrent.futures.as_completed(future_map):
                candidate = future_map[future]
                try:
                    key, result = future.result()
                except Exception as error:
                    for other in future_map:
                        other.cancel()
                    raise RuntimeError(
                        f"Classification failed for {model}: {candidate['parent']} -> {candidate['child']}: {error}"
                    ) from error
                with _CACHE_LOCK:
                    cached[key] = result
                    atomic_json(cache_path, cached)
                completed += 1
                if completed % 25 == 0 or completed == len(pending):
                    print(f"[{model}] completed {completed}/{len(pending)} new classifications")

    return [cached[cache_key(model, candidate["parent"], candidate["child"])] for candidate in candidates]


def pair(row: dict) -> tuple[str, str]:
    return row["parent"].strip().lower(), row["child"].strip().lower()


def metrics(gold_rows: list[dict], predicted_rows: list[dict], deduplicate_gold: bool) -> dict:
    gold_set = {pair(row) for row in gold_rows}
    predicted_set = {pair(row) for row in predicted_rows}
    correct = gold_set & predicted_set
    total_gold = len(gold_set) if deduplicate_gold else len(gold_rows)
    total_predicted = len(predicted_rows)
    precision = len(correct) / total_predicted if total_predicted else 0.0
    recall = len(correct) / total_gold if total_gold else 0.0
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    return {
        "precision": precision,
        "recall": recall,
        "f1_score": f1,
        "total_correct": len(correct),
        "total_predicted": total_predicted,
        "total_ground_truth": total_gold,
    }


def rounded(value: dict) -> dict:
    return {
        key: round(item, 6) if isinstance(item, float) else item
        for key, item in value.items()
    }


def report_markdown(result: dict) -> str:
    dataset_name = result["dataset"]["name"]
    lines = [
        f"# OntoLearner {dataset_name} Official-Protocol Baseline",
        "",
        f"Generated: `{result['generated_at']}`",
        "",
        "## Protocol",
        "",
        f"- OntoLearner source revision: `{result['protocol']['source_revision']}`",
        f"- Dataset SHA-256: `{result['dataset']['sha256']}`",
        f"- Types: {result['dataset']['types']}",
        f"- Raw taxonomy rows: {result['dataset']['raw_taxonomy_rows']}",
        f"- Unique taxonomy pairs: {result['dataset']['unique_taxonomy_pairs']}",
        f"- Retriever: `{result['protocol']['retriever_model']}`",
        f"- Candidate search: full type space, top-k `{result['protocol']['top_k']}` per query",
        f"- Candidate orientation: `{result['protocol']['candidate_mode']}`",
        f"- Candidate pairs: {result['protocol']['candidate_pairs']}",
        "- Verifier prompt: OntoLearner `StandardizedPrompting('taxonomy-discovery')`, unchanged",
        "",
        "## Retrieval",
        "",
        "| Metric | Official raw-row denominator | Deduplicated diagnostic |",
        "|---|---:|---:|",
        f"| Recall | {result['retrieval']['official']['recall']:.4f} | {result['retrieval']['deduplicated']['recall']:.4f} |",
        f"| Gold pairs retrieved | {result['retrieval']['official']['total_correct']} | {result['retrieval']['deduplicated']['total_correct']} |",
        "",
        "## End-to-End Results",
        "",
        "| Verifier | Official P | Official R | Official F1 | Dedup P | Dedup R | Dedup F1 | Yes | Invalid |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for model, model_result in result["models"].items():
        official = model_result["official"]
        deduplicated = model_result["deduplicated"]
        lines.append(
            f"| `{model}` | {official['precision']:.4f} | {official['recall']:.4f} | {official['f1_score']:.4f} "
            f"| {deduplicated['precision']:.4f} | {deduplicated['recall']:.4f} | {deduplicated['f1_score']:.4f} "
            f"| {model_result['answers'].get('yes', 0)} | {model_result['answers'].get('invalid', 0)} |"
        )
    lines.extend(
        [
            "",
            "## Metric Note",
            "",
            "The official OntoLearner taxonomy metric converts gold rows to a set for matching, but uses the raw",
            "gold row count as the recall denominator. This report preserves that value for protocol comparability",
            "and separately reports a deduplicated diagnostic.",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description="Run the official OntoLearner taxonomy protocol.")
    parser.add_argument("--gold", type=Path, default=DEFAULT_GOLD)
    parser.add_argument("--run-dir", type=Path, default=DEFAULT_RUN_DIR)
    parser.add_argument("--dataset-name", help="Display name; defaults to the gold file's parent directory")
    parser.add_argument("--retriever", default=DEFAULT_RETRIEVER)
    parser.add_argument("--models", default=",".join(DEFAULT_MODELS))
    parser.add_argument("--top-k", type=int, default=15)
    parser.add_argument("--candidate-mode", choices=("source", "paper"), default="source")
    parser.add_argument("--workers", type=int, default=8)
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()

    env = {**load_env(BACKEND_DIR / ".env"), **os.environ}
    api_key = env.get("OPENROUTER_API_KEY", "")
    if not api_key:
        raise SystemExit("OPENROUTER_API_KEY is not configured")
    base_url = env.get("OPENROUTER_BASE_URL", DEFAULT_BASE_URL)
    models = [model.strip() for model in args.models.split(",") if model.strip()]
    if not models:
        raise SystemExit("At least one verifier model is required")

    payload = read_json(args.gold)
    types = list(dict.fromkeys(payload.get("types", [])))
    gold_rows = payload.get("taxonomies", [])
    if len(types) < 2 or not gold_rows:
        raise SystemExit(f"Invalid OntoLearner taxonomy dataset: {args.gold}")
    top_k = min(args.top_k, len(types) - 1)
    args.run_dir.mkdir(parents=True, exist_ok=True)

    embedding_cache = args.run_dir / f"embeddings-{args.retriever.replace('/', '--')}.json"
    embedding_data = read_json(embedding_cache)
    if not embedding_data or embedding_data.get("types") != types:
        print(f"[{args.retriever}] embedding {len(types)} ontology types")
        vectors = embeddings(base_url, api_key, args.retriever, types, args.timeout)
        embedding_data = {"model": args.retriever, "types": types, "vectors": vectors}
        atomic_json(embedding_cache, embedding_data)
    else:
        vectors = embedding_data["vectors"]
        print(f"[{args.retriever}] loaded cached embeddings for {len(types)} ontology types")

    candidates = retrieve_candidates(types, vectors, top_k, args.candidate_mode)
    candidate_rows = [{"parent": row["parent"], "child": row["child"]} for row in candidates]
    retrieval = {
        "official": rounded(metrics(gold_rows, candidate_rows, deduplicate_gold=False)),
        "deduplicated": rounded(metrics(gold_rows, candidate_rows, deduplicate_gold=True)),
    }
    print(
        f"retrieval candidates={len(candidates)} "
        f"official_recall={retrieval['official']['recall']:.4f} "
        f"dedup_recall={retrieval['deduplicated']['recall']:.4f}"
    )

    model_results: dict[str, dict] = {}
    for model in models:
        responses = run_model(args.run_dir, base_url, api_key, model, candidates, args.workers, args.timeout)
        predictions = [
            {"parent": row["parent"], "child": row["child"]}
            for row in responses
            if row["answer"] == "yes"
        ]
        model_results[model] = {
            "official": rounded(metrics(gold_rows, predictions, deduplicate_gold=False)),
            "deduplicated": rounded(metrics(gold_rows, predictions, deduplicate_gold=True)),
            "answers": dict(Counter(row["answer"] for row in responses)),
            "predictions": predictions,
        }
        score = model_results[model]["official"]
        print(
            f"[{model}] P={score['precision']:.4f} R={score['recall']:.4f} "
            f"F1={score['f1_score']:.4f} ({score['total_correct']}/{score['total_ground_truth']} gold rows)"
        )

    result = {
        "generated_at": now_iso(),
        "protocol": {
            "name": "OntoLearner taxonomy-discovery end-to-end RAG",
            "source_revision": OFFICIAL_SOURCE_REVISION,
            "retriever_model": args.retriever,
            "top_k": top_k,
            "candidate_mode": args.candidate_mode,
            "candidate_pairs": len(candidates),
            "prompt": OFFICIAL_PROMPT,
            "temperature": 0,
            "seed": 42,
        },
        "dataset": {
            "name": args.dataset_name or args.gold.parent.name,
            "path": str(args.gold.resolve()),
            "sha256": sha256(args.gold),
            "types": len(types),
            "raw_taxonomy_rows": len(gold_rows),
            "unique_taxonomy_pairs": len({pair(row) for row in gold_rows}),
        },
        "retrieval": retrieval,
        "models": model_results,
    }
    atomic_json(args.run_dir / "result.json", result)
    (args.run_dir / "REPORT.md").write_text(report_markdown(result), encoding="utf-8")
    print(f"wrote {args.run_dir / 'result.json'}")
    print(f"wrote {args.run_dir / 'REPORT.md'}")


if __name__ == "__main__":
    main()
