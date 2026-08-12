from __future__ import annotations

import importlib.util
from pathlib import Path
import sys


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "benchmark_public_industrial.py"
SPEC = importlib.util.spec_from_file_location("benchmark_public_industrial", SCRIPT)
assert SPEC and SPEC.loader
benchmark = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = benchmark
SPEC.loader.exec_module(benchmark)


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
