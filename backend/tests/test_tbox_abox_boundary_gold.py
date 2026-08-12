from __future__ import annotations

import json
from pathlib import Path

from app.ontology.tbox_guard import sanitize_ontology_delta


GOLD = Path(__file__).parent / "gold" / "tbox_abox_boundary.json"


def test_project_boundary_gold_set() -> None:
    cases = json.loads(GOLD.read_text(encoding="utf-8"))
    for case in cases:
        ontology = {
            "classes": [{"label": label} for label in case["classes"]],
            "object_properties": [],
            "data_properties": [],
            "subclass_of": [],
            "disjoint_with": [],
            "equivalent_class": [],
        }
        cleaned, rejected = sanitize_ontology_delta(ontology, case["source"])
        assert [row["label"] for row in cleaned["classes"]] == case["expected_classes"], case["name"]
        assert [row["label"] for row in rejected] == case["expected_rejected"], case["name"]
