"""Fail when an OntoLearner result drops below the reviewed project baseline."""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("result", type=Path)
    parser.add_argument(
        "--baseline",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "tests" / "gold" / "ontolearner_baseline.json",
    )
    parser.add_argument("--model", help="Model key in result.models; defaults to the best deduplicated F1")
    args = parser.parse_args()
    result = json.loads(args.result.read_text(encoding="utf-8"))
    baseline = json.loads(args.baseline.read_text(encoding="utf-8"))
    models = result.get("models", {})
    if not models:
        raise SystemExit("result has no model scores")
    model = args.model or max(models, key=lambda key: models[key]["deduplicated"]["f1_score"])
    score = models[model]["deduplicated"]
    failures = []
    if score["f1_score"] < baseline["minimum_deduplicated_f1"]:
        failures.append(f"F1 {score['f1_score']:.4f} < {baseline['minimum_deduplicated_f1']:.4f}")
    if score["recall"] < baseline["minimum_deduplicated_recall"]:
        failures.append(f"recall {score['recall']:.4f} < {baseline['minimum_deduplicated_recall']:.4f}")
    if failures:
        raise SystemExit(f"OntoLearner regression for {model}: " + "; ".join(failures))
    print(f"OntoLearner regression passed for {model}: F1={score['f1_score']:.4f}, R={score['recall']:.4f}")


if __name__ == "__main__":
    main()
