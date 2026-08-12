from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import sys


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "benchmark_public_industrial.py"
SPEC = importlib.util.spec_from_file_location("benchmark_public_industrial", SCRIPT)
assert SPEC and SPEC.loader
benchmark = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = benchmark
SPEC.loader.exec_module(benchmark)

ONTOLEARNER_SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "benchmark_ontolearner_official.py"
ONTOLEARNER_SPEC = importlib.util.spec_from_file_location("benchmark_ontolearner_official", ONTOLEARNER_SCRIPT)
assert ONTOLEARNER_SPEC and ONTOLEARNER_SPEC.loader
ontolearner = importlib.util.module_from_spec(ONTOLEARNER_SPEC)
sys.modules[ONTOLEARNER_SPEC.name] = ontolearner
ONTOLEARNER_SPEC.loader.exec_module(ontolearner)


class FakeClient:
    def __init__(self) -> None:
        self.uploaded: list[str] = []

    def get(self, path: str):
        assert path == "/api/knowledge/7"
        return {"id": 7, "public_id": "public", "name": "Bench"}

    def post(self, path: str, **kwargs):
        if path.endswith("/documents/upload"):
            filename = kwargs["files"]["file"][0]
            self.uploaded.append(filename)
            return {"id": 9}
        if path.endswith("/parse"):
            return {"parse_status": "parsed", "text_char_count": 12, "chunk_count": 2}
        raise AssertionError(path)

    def close(self) -> None:
        return None


def test_ingest_resumes_after_partial_document_upload(tmp_path, monkeypatch) -> None:
    prepared = tmp_path / "prepared"
    dataset = prepared / "sample"
    corpus = dataset / "corpus"
    corpus.mkdir(parents=True)
    (corpus / "first.md").write_text("already uploaded", encoding="utf-8")
    (corpus / "second.md").write_text("resume this one", encoding="utf-8")
    benchmark.write_json(dataset / "manifest.json", {
        "name": "Sample",
        "domain": "tests",
        "documents": [{"filename": "first.md"}, {"filename": "second.md"}],
    })
    monkeypatch.setattr(benchmark, "PREPARED_DIR", prepared)
    run_dir = tmp_path / "run"
    benchmark.write_json(run_dir / "run-state.json", {
        "ks_id": 7,
        "documents": [{"document_id": 8, "filename": "first.md", "chars": 10, "chunks": 1}],
    })
    client = FakeClient()

    state = benchmark.ingest_dataset(client, "sample", "round", run_dir, None)

    assert client.uploaded == ["second.md"]
    assert [row["filename"] for row in state["documents"]] == ["first.md", "second.md"]
    assert state["document_count"] == 2
    assert state["chunk_count"] == 3


def test_role_collision_normalization_preserves_instance_prefix() -> None:
    assert benchmark.normalise_role_label(":hvac_system") == ":hvac system"
    assert benchmark.normalise_role_label("ex:Lazor_Series") == "ex:lazor series"
    assert benchmark.normalise_role_label("HVAC_System") == "hvac system"


def test_structural_metrics_do_not_confuse_named_resource_with_class_label() -> None:
    view = {
        "classes": [{"iri": "urn:class:hvac", "label": "HVAC_System"}],
        "axioms": {"subclass_of": []},
    }
    individuals = [{"iri": "urn:instance:hvac", "label": ":hvac_system"}]
    metrics = benchmark.structural_metrics(view, individuals)
    assert metrics["tbox_abox_label_collisions"] == []


def test_ontolearner_report_uses_dataset_name_and_generic_metric_note() -> None:
    result = {
        "generated_at": "2026-08-12T00:00:00Z",
        "protocol": {
            "source_revision": "revision",
            "retriever_model": "retriever",
            "top_k": 10,
            "candidate_mode": "paper",
            "candidate_pairs": 20,
            "prompt_profile": {
                "profile": "official",
                "name": "OntoLearner official",
                "source": "upstream",
                "sha256": "prompt-digest",
            },
        },
        "dataset": {
            "name": "OWL-Time",
            "sha256": "digest",
            "types": 3,
            "raw_taxonomy_rows": 2,
            "unique_taxonomy_pairs": 2,
        },
        "retrieval": {
            "official": {"recall": 1.0, "total_correct": 2},
            "deduplicated": {"recall": 1.0, "total_correct": 2},
        },
        "models": {
            "model": {
                "official": {"precision": 1.0, "recall": 1.0, "f1_score": 1.0},
                "deduplicated": {"precision": 1.0, "recall": 1.0, "f1_score": 1.0},
                "answers": {"yes": 2, "invalid": 0},
            }
        },
    }

    report = ontolearner.report_markdown(result)

    assert report.startswith("# OntoLearner OWL-Time Official-Protocol Baseline")
    assert "Official-paper Wine comparison" not in report
    assert "protocol comparability" in report


def test_ontolearner_prompt_profiles_have_isolated_caches() -> None:
    types = ["Wine", "Beverage"]
    official = ontolearner.cache_key("model", "Beverage", "Wine", "official", types)
    ontopilot = ontolearner.cache_key("model", "Beverage", "Wine", "ontopilot", types)

    assert official != ontopilot
    assert ontolearner.prompt_snapshot("official")["sha256"] != ontolearner.prompt_snapshot("ontopilot")["sha256"]


def test_ontopilot_prompt_parser_fails_closed_and_checks_direction() -> None:
    valid = '{"sub":"Wine","super":"Beverage","keep":true,"confidence":0.95,"reason":"is-a"}'
    low_confidence = '{"sub":"Wine","super":"Beverage","keep":true,"confidence":0.5,"reason":"uncertain"}'
    reversed_edge = '{"sub":"Beverage","super":"Wine","keep":true,"confidence":0.95,"reason":"is-a"}'

    assert ontolearner.map_ontopilot_answer(valid, "Beverage", "Wine")[0] == "yes"
    assert ontolearner.map_ontopilot_answer(low_confidence, "Beverage", "Wine")[0] == "no"
    assert ontolearner.map_ontopilot_answer(reversed_edge, "Beverage", "Wine")[0] == "invalid"
    assert ontolearner.map_ontopilot_answer("yes", "Beverage", "Wine") == ("invalid", None)


def test_ontolearner_run_lock_rejects_second_process(tmp_path) -> None:
    holder = subprocess.Popen(
        [
            sys.executable,
            "-c",
            (
                "import sys,time; sys.path.insert(0, sys.argv[1]); "
                "import benchmark_ontolearner_official as b; "
                "b.acquire_run_lock(b.Path(sys.argv[2])); print('locked', flush=True); time.sleep(10)"
            ),
            str(ONTOLEARNER_SCRIPT.parent),
            str(tmp_path),
        ],
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert holder.stdout is not None
        assert holder.stdout.readline().strip() == "locked"
        try:
            ontolearner.acquire_run_lock(tmp_path)
        except RuntimeError as error:
            assert "already active" in str(error)
        else:
            raise AssertionError("second benchmark process unexpectedly acquired the run lock")
    finally:
        holder.terminate()
        holder.wait(timeout=5)


def test_ontolearner_source_mode_matches_upstream_bidirectional_expansion() -> None:
    types = ["A", "B", "C"]
    vectors = [[1.0, 0.0], [0.9, 0.1], [0.0, 1.0]]

    paper = ontolearner.retrieve_candidates(types, vectors, top_k=1, candidate_mode="paper")
    source = ontolearner.retrieve_candidates(types, vectors, top_k=1, candidate_mode="source")

    paper_pairs = {(row["parent"], row["child"]) for row in paper}
    source_pairs = {(row["parent"], row["child"]) for row in source}
    assert paper_pairs == {("B", "A"), ("A", "B"), ("B", "C")}
    assert source_pairs == {
        ("B", "A"),
        ("A", "B"),
        ("B", "C"),
        ("C", "B"),
    }


def test_score_round_skips_dataset_without_run_state(tmp_path, monkeypatch) -> None:
    monkeypatch.setattr(benchmark, "RUNS_DIR", tmp_path)
    monkeypatch.setattr(benchmark, "OntoPilotClient", lambda *args: FakeClient())
    args = type("Args", (), {
        "base_url": "http://example.invalid",
        "username": "user",
        "password": "pass",
        "round": "partial-round",
        "datasets": ["ssn-sosa"],
    })()

    benchmark.score_round(args)

    summary = benchmark.read_json(tmp_path / "partial-round" / "summary.json")
    assert summary["results"] == []
    assert summary["failures"] == {}
    assert summary["skipped"] == {
        "ssn-sosa": "no completed run state for this dataset in the selected round",
    }
