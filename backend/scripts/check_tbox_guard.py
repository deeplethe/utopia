"""Domain-neutral regression checks for TBox/ABox role boundaries."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.ontology import conflicts as conflict_detector  # noqa: E402
from app.ontology import entity_roles, resolution, role_evidence  # noqa: E402
from app.ontology.abox_extract import (  # noqa: E402
    _apply_abox_role_decisions,
    _database_is_locked,
    _is_non_identifying_label,
    _is_self_typed_mention,
)
from app.ontology.extract import (  # noqa: E402
    _apply_corpus_role_decisions,
    _apply_corpus_evidence_selections,
    _apply_subclass_decisions,
    _apply_tbox_role_decisions,
    _prepare_corpus_evidence,
)
from app.ontology.tbox_guard import (  # noqa: E402
    canonical_datatype_name,
    sanitize_ontology_delta,
)


def _check_structured_roles() -> None:
    source = """Asset: Orion-7
Type: Centrifugal Pump
Status: ready
类别：实验设备
编号：EQ-42
"""
    roles = role_evidence.structured_value_roles(source)
    assert roles["orion 7"] == {role_evidence.ROLE_LITERAL}
    assert roles["centrifugal pump"] == {role_evidence.ROLE_TYPE}
    assert roles["ready"] == {role_evidence.ROLE_LITERAL}
    assert roles["实验设备"] == {role_evidence.ROLE_TYPE}
    assert roles["eq 42"] == {role_evidence.ROLE_LITERAL}
    assert role_evidence.evidence_is_grounded(source, "Type: Centrifugal Pump")
    assert role_evidence.surface_is_grounded(source, "Centrifugal Pump")
    assert not role_evidence.surface_is_grounded(source, "Orion-7 Device")
    assert not role_evidence.surface_is_grounded("Southern Italy", "South")


def _check_tbox_critic_boundary() -> None:
    source = """Asset: Orion-7
Type: Centrifugal Pump
Default: Machine
Every Centrifugal Pump is a Machine.
"""
    ontology = {
        "classes": [
            {"label": "Asset", "comment": "A reusable asset type."},
            {"label": "Centrifugal Pump", "comment": "A reusable pump type."},
            {"label": "Machine", "comment": "A reusable machine type."},
            {"label": "Orion-7", "comment": "A concrete asset."},
            {"label": "Orion-7 Device", "comment": "A renamed concrete asset."},
        ],
        "object_properties": [],
        "data_properties": [],
        "subclass_of": [{"sub": "Centrifugal Pump", "super": "Machine"}],
        "disjoint_with": [],
        "equivalent_class": [],
    }
    payload = {
        "class_decisions": [
            {"label": "Asset", "role": "type", "keep": True, "confidence": 0.99,
             "evidence": "Asset: Orion-7", "reason": "field denotes a reusable role"},
            {"label": "Centrifugal Pump", "role": "type", "keep": True, "confidence": 0.99,
             "evidence": "Type: Centrifugal Pump", "reason": "explicit type declaration"},
            {"label": "Machine", "role": "type", "keep": True, "confidence": 0.99,
             "evidence": "Every Centrifugal Pump is a Machine.", "reason": "general category"},
            {"label": "Orion-7", "role": "type", "keep": True, "confidence": 0.99,
             "evidence": "Asset: Orion-7", "reason": "incorrect critic decision"},
            {"label": "Orion-7 Device", "role": "type", "keep": True, "confidence": 0.99,
             "evidence": "Asset: Orion-7", "reason": "incorrect critic rename"},
        ],
        "subclass_decisions": [
            {"sub": "Centrifugal Pump", "super": "Machine", "keep": True,
             "confidence": 0.99, "evidence": "Every Centrifugal Pump is a Machine.",
             "reason": "every centrifugal pump is a machine"},
        ],
    }
    checked = _apply_tbox_role_decisions(source, ontology, payload)
    assert [row["label"] for row in checked["classes"]] == [
        "Asset", "Centrifugal Pump", "Machine",
    ]
    assert checked["subclass_of"] == [{
        "sub": "Centrifugal Pump",
        "super": "Machine",
        "evidence": "Every Centrifugal Pump is a Machine.",
    }]
    assert {row["label"] for row in checked["_role_rejections"]} == {
        "Orion-7", "Orion-7 Device",
    }


def _check_corpus_role_recovery() -> None:
    source = (
        "The Common module defines reusable classes. System generalizes Sensor and Sampler. "
        "Individual systems are members of the System class. Asset: Orion-7. Status: ready."
    )
    candidates = {
        "system": {
            "label": "System",
            "occurrences": [{"chunk_id": 1, "text": source}],
        },
        "orion 7": {
            "label": "Orion-7",
            "occurrences": [{"chunk_id": 1, "text": source}],
        },
        "ready": {
            "label": "ready",
            "occurrences": [{"chunk_id": 1, "text": source}],
        },
    }
    payload = {
        "class_decisions": [
            {"label": "System", "role": "type", "keep": True, "confidence": 0.99,
             "evidence": "Individual systems are members of the System class.",
             "reason": "explicit reusable class"},
            {"label": "Orion-7", "role": "individual", "keep": False, "confidence": 0.99,
             "evidence": "Asset: Orion-7", "reason": "named asset"},
            {"label": "ready", "role": "type", "keep": True, "confidence": 0.99,
             "evidence": "Status: ready", "reason": "incorrect type decision"},
        ],
    }
    accepted = _apply_corpus_role_decisions(candidates, payload, {"system", "ready"})
    assert accepted == [{
        "label": "System",
        "comment": "",
        "evidence": "Individual systems are members of the System class.",
        "_role_verified": True,
        "chunk_id": 1,
        "source_text": source,
    }]


def _check_corpus_evidence_selection() -> None:
    padding = "Routine reference text. " * 35
    source = (
        "Cooling Unit appears in the index. " + padding
        + "Cooling Unit is a reusable equipment class whose members remove heat from a process."
    )
    candidates = {
        "cooling unit": {
            "label": "Cooling Unit",
            "occurrences": [{
                "chunk_id": 9,
                "text": source,
                "earlier_reason": "ambiguous local mention",
                "extractor_evidence": "Cooling Unit appears in the index.",
            }],
        },
    }
    prepared = _prepare_corpus_evidence(candidates)
    passages = prepared["cooling unit"]["passages"]
    assert len(passages) == 2
    defining = next(
        row for row in passages if "reusable equipment class" in row["text"]
    )
    selected = _apply_corpus_evidence_selections(
        prepared,
        {"evidence_selections": [{
            "label": "Cooling Unit",
            "passage_ids": [defining["passage_id"], "invented"],
            "reason": "direct definition",
        }]},
    )
    assert selected["cooling unit"] == [defining]


def _check_subclass_edge_critic() -> None:
    source = "System generalizes Sensor and Sampler. A Sensor is necessarily a System."
    proposed = [
        {"sub": "Sensor", "super": "System", "evidence": "System generalizes Sensor"},
        {"sub": "System", "super": "Sensor", "evidence": "System generalizes Sensor"},
    ]
    payload = {
        "subclass_decisions": [
            {"sub": "Sensor", "super": "System", "keep": True, "confidence": 0.99,
             "evidence": "A Sensor is necessarily a System.", "reason": "valid is-a"},
            {"sub": "System", "super": "Sensor", "keep": False, "confidence": 0.99,
             "evidence": "System generalizes Sensor", "reason": "reversed"},
        ],
    }
    assert _apply_subclass_decisions(
        source, proposed, payload, {"sensor", "system"},
    ) == [{
        "sub": "Sensor",
        "super": "System",
        "evidence": "A Sensor is necessarily a System.",
    }]


def _check_abox_critic_boundary() -> None:
    source = (
        "Asset: Pump P-101\nType: Pump\nPump P-101 maintains pressure. Its status is ready. Pump is a reusable equipment "
        "type. The phrase Unit Alpha may denote a particular rig, but the text is ambiguous."
    )
    mentions = [
        {"label": "Pump P-101", "class": "Pump?Equipment", "attributes": [], "relations": []},
        {"label": "ready", "class": "Status", "attributes": [], "relations": []},
        {"label": "Pump", "class": "Pump", "attributes": [], "relations": []},
        {"label": "Pump P-101 Device", "class": "Pump", "attributes": [], "relations": []},
        {"label": "Unit Alpha", "class": "Rig", "attributes": [], "relations": []},
    ]
    payload = {
        "decisions": [
            {"label": "Pump P-101", "candidate_class": "Pump?Equipment",
             "selected_class": "Rig", "role": "individual", "keep": True,
             "confidence": 0.99, "evidence": "Pump P-101 maintains pressure.",
             "reason": "explicitly named equipment"},
            {"label": "ready", "class": "Status", "role": "literal", "keep": False,
             "confidence": 0.99, "evidence": "status is ready", "reason": "status value"},
            {"label": "Pump", "class": "Pump", "role": "type", "keep": False,
             "confidence": 0.99, "evidence": "Pump is a reusable equipment type.",
             "reason": "reusable type"},
            {"label": "Pump P-101 Device", "class": "Pump", "role": "individual", "keep": True,
             "confidence": 0.99, "evidence": "Pump P-101 maintains pressure.",
             "reason": "incorrect rewritten label"},
            {"label": "Unit Alpha", "class": "Rig", "role": "uncertain", "keep": True,
             "confidence": 0.75,
             "evidence": "Unit Alpha may denote a particular rig, but the text is ambiguous.",
             "reason": "identity is not established"},
        ],
    }
    accepted, rejected = _apply_abox_role_decisions(
        source, mentions, payload, ["Pump", "Status", "Rig"],
    )
    assert rejected == 3
    assert [row["label"] for row in accepted] == ["Pump P-101", "Unit Alpha"]
    assert accepted[0]["class"] == "Pump"
    assert accepted[0]["_role_verified"] is True
    assert accepted[1]["_force_review"] == "identity is not established"
    assert accepted[1]["_role_confidence"] == 0.75
    assert _is_self_typed_mention({"label": "Pump", "class": "Pump"})
    assert _is_self_typed_mention({"label": "Temperature Sensor", "class": "Temperature_Sensor"})
    assert not _is_self_typed_mention({"label": "Pump P-101", "class": "Pump"})
    assert _database_is_locked(RuntimeError("database is locked"))
    assert not _database_is_locked(RuntimeError("different failure"))


def _check_structural_sanitizer() -> None:
    source = """Asset: Orion-7
Type: Centrifugal Pump
Status: ready
Default: Machine
Every Centrifugal Pump is a Machine.
"""
    cleaned, rejected = sanitize_ontology_delta(
        {
            "classes": [
                {"label": "Asset"},
                {"label": "Orion-7"},
                {"label": "Orion-7 Device"},
                {"label": "Centrifugal Pump"},
                {"label": "Machine", "_role_verified": True},
                {"label": "xsd:string"},
            ],
            "object_properties": [
                {"label": "assigned to", "domain": "Orion-7", "range": "Machine"},
                {"label": "serial code", "domain": "Centrifugal Pump", "range": "xsd:string"},
                {"label": "has", "domain": "Asset", "range": "Machine"},
            ],
            "data_properties": [
                {"label": "status label", "domain": "Asset", "range": "string"},
            ],
            "subclass_of": [
                {"sub": "Centrifugal Pump", "super": "Machine"},
                {"sub": "Orion-7", "super": "Asset"},
                {"sub": "xsd:string", "super": "Asset"},
            ],
            "disjoint_with": [],
            "equivalent_class": [],
        },
        source,
    )
    assert [row["label"] for row in cleaned["classes"]] == [
        "Asset", "Centrifugal Pump", "Machine",
    ]
    assert cleaned["object_properties"] == [{"label": "assigned to", "range": "Machine"}]
    assert cleaned["data_properties"] == [
        {"label": "status label", "domain": "Asset", "range": "string"},
        {"label": "serial code", "domain": "Centrifugal Pump", "range": "string"},
    ]
    assert cleaned["subclass_of"] == [{"sub": "Centrifugal Pump", "super": "Machine"}]
    assert {row["label"] for row in rejected} == {"Orion-7", "Orion-7 Device", "xsd:string"}

    existing, rejected = sanitize_ontology_delta(
        {
            "classes": [],
            "object_properties": [
                {"label": "connected to", "domain": "Existing Component", "range": "Existing Component"},
            ],
        },
        "The components are connected.",
        existing_class_norms={"Existing Component"},
    )
    assert rejected == []
    assert existing["object_properties"] == [{
        "label": "connected to",
        "domain": "Existing Component",
        "range": "Existing Component",
    }]


def _check_generic_helpers() -> None:
    assert canonical_datatype_name("xsd:string") == "string"
    assert canonical_datatype_name("http://www.w3.org/2001/XMLSchema#double") == "decimal"
    assert canonical_datatype_name("Device") is None

    roles = entity_roles.class_role_map({
        "classes": [
            {"iri": "urn:class:asset", "label": "Asset", "superclasses": []},
            {"iri": "urn:class:pump", "label": "Pump", "superclasses": ["urn:class:asset"]},
        ],
    })
    assert roles == {"urn:class:asset": frozenset(), "urn:class:pump": frozenset()}
    assert entity_roles.roles_for_types(set(roles), roles) == frozenset()

    candidates = [
        ("urn:ind:101", "Pump P-101"),
        ("urn:ind:102", "Pump P-102"),
        ("urn:ind:tower", "Cooling Tower"),
    ]
    assert resolution._lexical_candidate_pool("Pump P-101", candidates) == [
        ("urn:ind:101", "Pump P-101"),
    ]
    plugin_candidates = [
        ("urn:ind:falcon", "FalconGuard"),
        ("urn:ind:eagle", "EagleGate"),
    ]
    assert resolution._lexical_candidate_pool(
        "FalconGuard admission plugin",
        plugin_candidates,
        class_label="admission plugin",
    ) == [("urn:ind:falcon", "FalconGuard")]
    assert resolution._identity_name_key(
        "FalconGuard admission plugin", "admission plugin",
    ) == "falconguard"
    assert resolution._identity_name_key("蓝盾准入插件", "准入插件") == "蓝盾"
    graph_model = conflict_detector._GraphModel(
        labels={"urn:controller": "Controller", "urn:resource": "Workload Resource"},
        unions={"anonymous-union": ("urn:controller", "urn:resource")},
    )
    assert conflict_detector._lbl(graph_model, "anonymous-union") == "Controller ∪ Workload Resource"
    assert conflict_detector._concrete_values(
        graph_model, ["anonymous-union", "urn:kubelet"],
    ) == ["urn:controller", "urn:resource", "urn:kubelet"]
    assert _is_non_identifying_label("Untitled")
    assert _is_non_identifying_label("${RESOURCE_NAME}")
    assert not _is_non_identifying_label("Pump P-101")


def _check_forced_role_review() -> None:
    recorded: list[dict] = []
    original_prior = resolution._prior
    original_record = resolution._record
    resolution._prior = lambda *args, **kwargs: None
    resolution._record = lambda *args, **kwargs: recorded.append(kwargs)
    try:
        individual_iri, status = resolution.resolve_mention(
            object(),
            ks_id=7,
            abox_iri="urn:test:abox",
            base_iri="urn:test:",
            surface="Unit Alpha",
            class_iri="urn:class:rig",
            chunk_id=11,
            pending_payload={"pending_attributes": [{"prop": "urn:prop:code", "value": "A"}]},
            force_review_reason="identity is not established",
            force_review_confidence=0.72,
        )
    finally:
        resolution._prior = original_prior
        resolution._record = original_record

    assert individual_iri is None and status == "pending"
    assert recorded[0]["status"] == "pending"
    assert recorded[0]["confidence"] == 0.72
    assert recorded[0]["context"] == {
        "reason": "identity is not established",
        "review_kind": "entity_role",
        "pending_attributes": [{"prop": "urn:prop:code", "value": "A"}],
    }


def _check_resolution_new_reasons() -> None:
    recorded: list[dict] = []
    original_prior = resolution._prior
    original_record = resolution._record
    original_individuals = resolution.abox.individuals_of_class
    original_create = resolution.abox.create_individual
    original_similarities = resolution._similarities
    resolution._prior = lambda *args, **kwargs: None
    resolution._record = lambda *args, **kwargs: recorded.append(kwargs)
    resolution.abox.create_individual = lambda *args, **kwargs: "urn:ind:new"
    try:
        resolution.abox.individuals_of_class = lambda *args, **kwargs: []
        individual_iri, status = resolution.resolve_mention(
            object(),
            ks_id=7,
            abox_iri="urn:test:abox",
            base_iri="urn:test:",
            surface="Unit Alpha",
            class_iri="urn:class:rig",
            chunk_id=11,
        )
        assert individual_iri == "urn:ind:new" and status == "new"
        assert recorded[-1]["context"]["reason"] == (
            "no compatible existing individual met the candidate threshold"
        )

        recorded.clear()
        resolution.abox.individuals_of_class = lambda *args, **kwargs: [
            ("urn:ind:old", "Unit Alpha"),
        ]
        resolution._similarities = lambda *args, **kwargs: [
            (0.99, ("urn:ind:old", "Unit Alpha")),
        ]
        individual_iri, status = resolution.resolve_mention(
            object(),
            ks_id=7,
            abox_iri="urn:test:abox",
            base_iri="urn:test:",
            surface="Unit Alpha",
            class_iri="urn:class:rig",
            class_label="Rig",
            chunk_id=12,
            agent=lambda *args: {
                "decision": "new",
                "confidence": 0.84,
                "reason": "facts describe a separate identity",
            },
        )
        assert individual_iri == "urn:ind:new" and status == "new"
        assert recorded[-1]["confidence"] == 0.84
        assert recorded[-1]["context"]["reason"] == "facts describe a separate identity"
        assert recorded[-1]["context"]["candidates"][0]["label"] == "Unit Alpha"
    finally:
        resolution._prior = original_prior
        resolution._record = original_record
        resolution.abox.individuals_of_class = original_individuals
        resolution.abox.create_individual = original_create
        resolution._similarities = original_similarities


def main() -> None:
    _check_structured_roles()
    _check_tbox_critic_boundary()
    _check_corpus_role_recovery()
    _check_corpus_evidence_selection()
    _check_subclass_edge_critic()
    _check_abox_critic_boundary()
    _check_structural_sanitizer()
    _check_generic_helpers()
    _check_forced_role_review()
    _check_resolution_new_reasons()
    print("Domain-neutral ontology boundary regression checks passed")


if __name__ == "__main__":
    main()
