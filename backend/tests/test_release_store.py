from __future__ import annotations

import hashlib
from pathlib import Path

from pyoxigraph import Literal, NamedNode, Quad

from app.ontology import release_store


def _quad(index: int, graph: str = "urn:graph") -> Quad:
    return Quad(
        NamedNode(f"urn:subject:{index}"),
        NamedNode("urn:predicate"),
        Literal(str(index)),
        NamedNode(graph),
    )


def test_nquads_export_is_uncompressed_and_sharded(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(release_store.store, "iter_quads", lambda _: iter(_quad(index) for index in range(5)))
    progress: list[int] = []
    files = release_store.write_graph_shards(
        "urn:graph", tmp_path, "abox", shard_size=2, progress=progress.append,
    )
    assert [item["statements"] for item in files] == [2, 2, 1]
    assert all(item["name"].endswith(".nq") for item in files)
    assert not list(tmp_path.glob("*.gz"))
    assert progress[-1] == 5
    for item in files:
        content = (tmp_path / item["name"]).read_bytes()
        assert hashlib.sha256(content).hexdigest() == item["sha256"]
        assert all(line.endswith(b" .") for line in content.splitlines())


def test_semantic_diff_ignores_file_order(tmp_path: Path) -> None:
    left_root = tmp_path / "left"
    right_root = tmp_path / "right"
    left_root.mkdir()
    right_root.mkdir()
    common = b"<urn:s1> <urn:p> \"one\" <urn:g> .\n"
    removed = b"<urn:s2> <urn:p> \"old\" <urn:g> .\n"
    added = b"<urn:s3> <urn:p> \"new\" <urn:g> .\n"
    for root, content in ((left_root, common + removed), (right_root, added + common)):
        for layer in ("tbox", "vocabulary", "abox"):
            (root / f"{layer}.nq").write_bytes(content)

    def manifest() -> dict:
        return {
            "layers": {
                layer: {"files": [{"name": f"{layer}.nq"}]}
                for layer in ("tbox", "vocabulary", "abox")
            }
        }

    diff = release_store.semantic_diff(left_root, manifest(), right_root, manifest())
    for layer in diff["layers"].values():
        assert layer["added"] == 1
        assert layer["removed"] == 1
