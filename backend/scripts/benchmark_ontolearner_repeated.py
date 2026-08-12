"""Repeat the strict OntoLearner Wine paper protocol and summarize stability."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


SCRIPT_DIR = Path(__file__).resolve().parent
BACKEND_DIR = SCRIPT_DIR.parent
DEFAULT_OFFICIAL_SCRIPT = SCRIPT_DIR / "benchmark_ontolearner_official.py"
DEFAULT_GOLD = (
    BACKEND_DIR
    / "data"
    / "benchmarks"
    / "ontolearner-food_and_beverage"
    / "wine"
    / "type_taxonomies.json"
)
DEFAULT_MODELS = ("qwen/qwen3-8b", "deepseek/deepseek-chat")
DEFAULT_RETRIEVER = "qwen/qwen3-embedding-8b"
PUBLISHED_SAME_MODEL_F1 = 0.186
PUBLISHED_BEST_LISTED_F1 = 0.250
T_CRITICAL_95 = {
    1: 12.706,
    2: 4.303,
    3: 3.182,
    4: 2.776,
    5: 2.571,
    6: 2.447,
    7: 2.365,
    8: 2.306,
    9: 2.262,
    10: 2.228,
    11: 2.201,
    12: 2.179,
    13: 2.160,
    14: 2.145,
    15: 2.131,
    16: 2.120,
    17: 2.110,
    18: 2.101,
    19: 2.093,
    20: 2.086,
    21: 2.080,
    22: 2.074,
    23: 2.069,
    24: 2.064,
    25: 2.060,
    26: 2.056,
    27: 2.052,
    28: 2.048,
    29: 2.045,
    30: 2.042,
}


def now_iso() -> str:
    return datetime.now(UTC).isoformat()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_json(path: Path, default: Any = None) -> Any:
    if not path.exists():
        return default
    with path.open("r", encoding="utf-8-sig") as handle:
        return json.load(handle)


def atomic_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as handle:
        json.dump(value, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
        temp_path = Path(handle.name)
    temp_path.replace(path)


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


def rounded(value: float | None) -> float | None:
    return round(value, 6) if value is not None else None


def summarize(values: list[float]) -> dict[str, Any]:
    if not values:
        return {
            "n": 0,
            "values": [],
            "mean": None,
            "sample_stddev": None,
            "min": None,
            "max": None,
            "ci95_low": None,
            "ci95_high": None,
            "ci95_margin": None,
        }
    mean = statistics.fmean(values)
    if len(values) == 1:
        sample_stddev = None
        margin = None
        ci_low = None
        ci_high = None
    else:
        sample_stddev = statistics.stdev(values)
        critical = T_CRITICAL_95.get(len(values) - 1, 1.96)
        margin = critical * sample_stddev / math.sqrt(len(values))
        ci_low = mean - margin
        ci_high = mean + margin
    return {
        "n": len(values),
        "values": [rounded(value) for value in values],
        "mean": rounded(mean),
        "sample_stddev": rounded(sample_stddev),
        "min": rounded(min(values)),
        "max": rounded(max(values)),
        "ci95_low": rounded(ci_low),
        "ci95_high": rounded(ci_high),
        "ci95_margin": rounded(margin),
    }


def validate_result(result: dict[str, Any], models: list[str], prompt_profile: str = "official") -> None:
    protocol = result.get("protocol", {})
    if protocol.get("candidate_mode") != "paper":
        raise RuntimeError("Result does not use the strict paper candidate orientation")
    actual_profile = protocol.get("prompt_profile", {}).get("profile", "official")
    if actual_profile != prompt_profile:
        raise RuntimeError(f"Result uses prompt profile {actual_profile!r}, expected {prompt_profile!r}")
    missing = [model for model in models if model not in result.get("models", {})]
    if missing:
        raise RuntimeError(f"Result is missing verifier models: {', '.join(missing)}")


def completed_runs(
    run_root: Path,
    repeats: int,
    models: list[str],
    prompt_profile: str = "official",
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for index in range(1, repeats + 1):
        result_path = run_root / f"run-{index:02d}" / "result.json"
        if not result_path.exists():
            continue
        result = read_json(result_path)
        validate_result(result, models, prompt_profile)
        rows.append(
            {
                "index": index,
                "generated_at": result.get("generated_at"),
                "result_path": str(result_path.resolve()),
                "models": {
                    model: {
                        "official_precision": result["models"][model]["official"]["precision"],
                        "official_recall": result["models"][model]["official"]["recall"],
                        "official_f1": result["models"][model]["official"]["f1_score"],
                        "deduplicated_f1": result["models"][model]["deduplicated"]["f1_score"],
                        "yes": result["models"][model]["answers"].get("yes", 0),
                        "invalid": result["models"][model]["answers"].get("invalid", 0),
                    }
                    for model in models
                },
            }
        )
    return rows


def build_aggregate(
    run_root: Path,
    repeats: int,
    models: list[str],
    config: dict[str, Any],
) -> dict[str, Any]:
    runs = completed_runs(run_root, repeats, models, config.get("prompt_profile", "official"))
    model_stats: dict[str, Any] = {}
    for model in models:
        official_values = [row["models"][model]["official_f1"] for row in runs]
        deduplicated_values = [row["models"][model]["deduplicated_f1"] for row in runs]
        model_stats[model] = {
            "official_f1": summarize(official_values),
            "deduplicated_f1": summarize(deduplicated_values),
        }

    complete = len(runs) == repeats
    primary_model = "qwen/qwen3-8b" if "qwen/qwen3-8b" in models else models[0]
    primary = model_stats[primary_model]["official_f1"]
    ci_low = primary["ci95_low"]
    all_above_same_model = complete and all(value > PUBLISHED_SAME_MODEL_F1 for value in primary["values"])
    ci_above_same_model = complete and ci_low is not None and ci_low > PUBLISHED_SAME_MODEL_F1
    same_model_supported = all_above_same_model and ci_above_same_model
    all_above_best = complete and all(value > PUBLISHED_BEST_LISTED_F1 for value in primary["values"])
    ci_above_best = complete and ci_low is not None and ci_low > PUBLISHED_BEST_LISTED_F1
    best_listed_supported = all_above_best and ci_above_best
    mean_gain = primary["mean"] - PUBLISHED_SAME_MODEL_F1 if primary["mean"] is not None else None
    relative_gain = mean_gain / PUBLISHED_SAME_MODEL_F1 if mean_gain is not None else None
    if complete and same_model_supported:
        wording = (
            f"Across {repeats} fresh-cache repetitions of OntoLearner's Wine taxonomy-discovery paper "
            f"protocol, our hosted {primary_model} configuration achieved mean F1 {primary['mean']:.4f} "
            f"(run-to-run 95% t-interval {primary['ci95_low']:.4f}-{primary['ci95_high']:.4f}), "
            f"{mean_gain * 100:.1f} percentage points ({relative_gain * 100:.1f}%) above the paper's "
            f"reported {PUBLISHED_SAME_MODEL_F1:.3f} result for the same model."
        )
    elif complete:
        wording = (
            f"Across {repeats} fresh-cache repetitions of OntoLearner's Wine taxonomy-discovery paper "
            f"protocol, our hosted {primary_model} configuration achieved mean F1 {primary['mean']:.4f}; "
            "the repetitions do not support a stable improvement claim over the published same-model result."
        )
    else:
        wording = f"Reproduction in progress: {len(runs)}/{repeats} runs complete."

    return {
        "generated_at": now_iso(),
        "status": "complete" if complete else "running",
        "requested_repeats": repeats,
        "completed_repeats": len(runs),
        "config": config,
        "published_baselines": {
            "same_model_qwen3_8b_official_f1": PUBLISHED_SAME_MODEL_F1,
            "best_listed_official_f1": PUBLISHED_BEST_LISTED_F1,
        },
        "runs": runs,
        "models": model_stats,
        "claim": {
            "primary_model": primary_model,
            "supported": same_model_supported,
            "same_model_improvement_supported": same_model_supported,
            "all_runs_above_same_model": all_above_same_model,
            "ci95_lower_bound_above_same_model": ci_above_same_model,
            "mean_absolute_gain_over_same_model": rounded(mean_gain),
            "mean_relative_gain_over_same_model": rounded(relative_gain),
            "best_listed_lead_supported": best_listed_supported,
            "all_runs_above_best_listed": all_above_best,
            "ci95_lower_bound_above_best_listed": ci_above_best,
            "wording": wording,
            "scope": "Wine taxonomy discovery with 20 provided types and the paper's candidate orientation",
            "not_claimed": [
                "full end-to-end ontology extraction superiority",
                "performance across domains or datasets",
                "general state of the art",
            ],
        },
    }


def display_number(value: float | None) -> str:
    return "—" if value is None else f"{value:.4f}"


def report_markdown(aggregate: dict[str, Any]) -> str:
    config = aggregate["config"]
    lines = [
        "# OntoLearner Wine Repeated Reproduction",
        "",
        f"Generated: `{aggregate['generated_at']}`",
        f"Status: **{aggregate['status']}** ({aggregate['completed_repeats']}/{aggregate['requested_repeats']} runs)",
        "",
        "## Reproduction Controls",
        "",
        "- Protocol: OntoLearner Wine taxonomy discovery, strict `paper` candidate orientation",
        f"- Prompt profile: `{config.get('prompt_profile', 'official')}`",
        f"- Verifiers: {', '.join(f'`{model}`' for model in config['models'])}",
        f"- Retriever: `{config['retriever']}`; top-k `{config['top_k']}`",
        f"- Independent response and embedding caches per run; `{config['workers']}` sequential-run workers",
        f"- Protocol script snapshot SHA-256: `{config['official_script_sha256']}`",
        f"- Dataset snapshot SHA-256: `{config['gold_sha256']}`",
        "",
        "## Runs",
        "",
        "| Run | " + " | ".join(f"{model} official F1" for model in config["models"]) + " |",
        "|---:" + "|---:" * len(config["models"]) + "|",
    ]
    for run in aggregate["runs"]:
        scores = " | ".join(f"{run['models'][model]['official_f1']:.4f}" for model in config["models"])
        lines.append(f"| {run['index']} | {scores} |")
    if not aggregate["runs"]:
        lines.append("| — | " + " | ".join("—" for _ in config["models"]) + " |")

    lines.extend(
        [
            "",
            "## Aggregate",
            "",
            "| Verifier | n | Mean F1 | Std. dev. | 95% CI | Min | Max |",
            "|---|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for model, metrics in aggregate["models"].items():
        stats = metrics["official_f1"]
        interval = (
            "—"
            if stats["ci95_low"] is None
            else f"{stats['ci95_low']:.4f}-{stats['ci95_high']:.4f}"
        )
        lines.append(
            f"| `{model}` | {stats['n']} | {display_number(stats['mean'])} | "
            f"{display_number(stats['sample_stddev'])} | {interval} | "
            f"{display_number(stats['min'])} | {display_number(stats['max'])} |"
        )

    decision = "SUPPORTED" if aggregate["claim"]["same_model_improvement_supported"] else "NOT SUPPORTED"
    if aggregate["status"] != "complete":
        decision = "PENDING"
    lines.extend(
        [
            "",
            "## Claim Decision",
            "",
            f"**{decision}**",
            "",
            aggregate["claim"]["wording"],
            "",
            "| Public claim | Guardrail | Decision |",
            "|---|---|---|",
            "| Improvement over published Qwen3-8B result (0.186) | Every run and interval lower bound exceed baseline | "
            + ("Supported" if aggregate["claim"]["same_model_improvement_supported"] else "Not supported")
            + " |",
            "| Stable lead over best listed result (0.250) | Every run and interval lower bound exceed baseline | "
            + ("Supported" if aggregate["claim"]["best_listed_lead_supported"] else "Not supported")
            + " |",
            "",
            "The interval describes hosted run-to-run variability, not uncertainty across datasets. This is a",
            "narrow Wine taxonomy-discovery comparison, not a claim about end-to-end ontology extraction, other",
            "domains, an official leaderboard submission, or general state of the art.",
            "",
        ]
    )
    return "\n".join(lines)


def write_aggregate(
    run_root: Path,
    repeats: int,
    models: list[str],
    config: dict[str, Any],
) -> dict[str, Any]:
    aggregate = build_aggregate(run_root, repeats, models, config)
    atomic_json(run_root / "aggregate.json", aggregate)
    (run_root / "REPORT.md").write_text(report_markdown(aggregate), encoding="utf-8")
    return aggregate


def run_child(command: list[str], run_log: Path, aggregate_log: Path, env: dict[str, str]) -> None:
    with run_log.open("a", encoding="utf-8") as run_handle, aggregate_log.open(
        "a", encoding="utf-8"
    ) as aggregate_handle:
        process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            env=env,
        )
        if process.stdout is None:
            raise RuntimeError("Failed to capture benchmark output")
        for line in process.stdout:
            print(line, end="", flush=True)
            run_handle.write(line)
            run_handle.flush()
            aggregate_handle.write(line)
            aggregate_handle.flush()
        return_code = process.wait()
    if return_code:
        raise subprocess.CalledProcessError(return_code, command)


def prepare_snapshots(
    run_root: Path,
    official_script: Path,
    gold: Path,
    models: list[str],
    args: argparse.Namespace,
) -> tuple[Path, Path, dict[str, Any]]:
    run_root.mkdir(parents=True, exist_ok=True)
    script_snapshot = run_root / "benchmark_ontolearner_official.snapshot.py"
    gold_snapshot = run_root / "type_taxonomies.snapshot.json"
    if not script_snapshot.exists():
        shutil.copy2(official_script, script_snapshot)
    if not gold_snapshot.exists():
        shutil.copy2(gold, gold_snapshot)
    config = {
        "created_at": now_iso(),
        "official_script_source": str(official_script.resolve()),
        "official_script_snapshot": str(script_snapshot.resolve()),
        "official_script_sha256": sha256(script_snapshot),
        "gold_source": str(gold.resolve()),
        "gold_snapshot": str(gold_snapshot.resolve()),
        "gold_sha256": sha256(gold_snapshot),
        "models": models,
        "retriever": args.retriever,
        "top_k": args.top_k,
        "candidate_mode": "paper",
        "prompt_profile": args.prompt_profile,
        "workers": args.workers,
        "timeout": args.timeout,
    }
    config_path = run_root / "run-config.json"
    existing = read_json(config_path)
    if existing:
        comparable_keys = {
            "official_script_sha256",
            "gold_sha256",
            "models",
            "retriever",
            "top_k",
            "candidate_mode",
            "prompt_profile",
            "workers",
            "timeout",
        }
        mismatches = [key for key in comparable_keys if existing.get(key) != config.get(key)]
        if mismatches:
            raise SystemExit(f"Run root configuration mismatch: {', '.join(sorted(mismatches))}")
        config = existing
    else:
        atomic_json(config_path, config)
    return script_snapshot, gold_snapshot, config


def main() -> None:
    timestamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    parser = argparse.ArgumentParser(description="Repeat the strict OntoLearner Wine paper protocol.")
    parser.add_argument(
        "--run-root",
        type=Path,
        default=BACKEND_DIR / "data" / "benchmarks" / f"ontolearner-wine-paper-repeats-{timestamp}",
    )
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--official-script", type=Path, default=DEFAULT_OFFICIAL_SCRIPT)
    parser.add_argument("--gold", type=Path, default=DEFAULT_GOLD)
    parser.add_argument("--models", default=",".join(DEFAULT_MODELS))
    parser.add_argument("--retriever", default=DEFAULT_RETRIEVER)
    parser.add_argument("--top-k", type=int, default=15)
    parser.add_argument("--prompt-profile", choices=("official", "ontopilot"), default="official")
    parser.add_argument("--workers", type=int, default=10)
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("--run-attempts", type=int, default=8)
    parser.add_argument("--retry-delay", type=float, default=45.0)
    args = parser.parse_args()

    if args.repeats < 2:
        raise SystemExit("At least two repetitions are required for a confidence interval")
    models = [model.strip() for model in args.models.split(",") if model.strip()]
    if not models:
        raise SystemExit("At least one verifier model is required")
    if not args.official_script.is_file():
        raise SystemExit(f"Official benchmark script not found: {args.official_script}")
    if not args.gold.is_file():
        raise SystemExit(f"Gold dataset not found: {args.gold}")

    run_root = args.run_root.resolve()
    script_snapshot, gold_snapshot, config = prepare_snapshots(
        run_root, args.official_script.resolve(), args.gold.resolve(), models, args
    )
    aggregate_log = run_root / "run.log"
    child_env = os.environ.copy()
    for key, value in load_env(BACKEND_DIR / ".env").items():
        child_env.setdefault(key, value)

    write_aggregate(run_root, args.repeats, models, config)
    for index in range(1, args.repeats + 1):
        run_dir = run_root / f"run-{index:02d}"
        result_path = run_dir / "result.json"
        if result_path.exists():
            validate_result(read_json(result_path), models, args.prompt_profile)
            print(f"[repeat {index}/{args.repeats}] complete; skipping", flush=True)
            continue
        run_dir.mkdir(parents=True, exist_ok=True)
        started = f"[{now_iso()}] repeat {index}/{args.repeats} started"
        print(started, flush=True)
        with aggregate_log.open("a", encoding="utf-8") as handle:
            handle.write(started + "\n")
        command = [
            sys.executable,
            "-u",
            str(script_snapshot),
            "--gold",
            str(gold_snapshot),
            "--run-dir",
            str(run_dir),
            "--retriever",
            args.retriever,
            "--models",
            ",".join(models),
            "--top-k",
            str(args.top_k),
            "--candidate-mode",
            "paper",
            "--prompt-profile",
            args.prompt_profile,
            "--workers",
            str(args.workers),
            "--timeout",
            str(args.timeout),
        ]
        for attempt in range(1, args.run_attempts + 1):
            try:
                run_child(command, run_dir / "run.log", aggregate_log, child_env)
                break
            except subprocess.CalledProcessError:
                if attempt == args.run_attempts:
                    raise
                delay = args.retry_delay * min(attempt, 4)
                message = (
                    f"[{now_iso()}] repeat {index}/{args.repeats} child attempt "
                    f"{attempt}/{args.run_attempts} failed; retrying cached remainder in {delay:.0f}s"
                )
                print(message, flush=True)
                with aggregate_log.open("a", encoding="utf-8") as handle:
                    handle.write(message + "\n")
                time.sleep(delay)
        validate_result(read_json(result_path), models, args.prompt_profile)
        aggregate = write_aggregate(run_root, args.repeats, models, config)
        print(
            f"[repeat {index}/{args.repeats}] complete; "
            f"aggregate={aggregate['completed_repeats']}/{args.repeats}",
            flush=True,
        )

    aggregate = write_aggregate(run_root, args.repeats, models, config)
    print(aggregate["claim"]["wording"], flush=True)
    print(f"wrote {run_root / 'aggregate.json'}", flush=True)
    print(f"wrote {run_root / 'REPORT.md'}", flush=True)


if __name__ == "__main__":
    main()
