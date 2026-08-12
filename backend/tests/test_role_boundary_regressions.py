from __future__ import annotations

from app.ontology import abox_extract, extract, role_evidence, tbox_guard


def test_explicit_instance_declaration_is_subject_specific() -> None:
    source = (
        "The ex:Lazor_Series_8030_ProdEquipCategory instance is the general category "
        "of a specific production equipment."
    )
    assert role_evidence.has_explicit_individual_declaration(
        source, "Lazor_Series_8030_ProdEquipCategory",
    )
    assert not role_evidence.has_explicit_individual_declaration(
        source, "production equipment",
    )


def test_generic_class_instance_phrase_is_not_a_named_identity() -> None:
    source = "Consider a simple Brick model with a single AHU instance."
    assert not role_evidence.has_explicit_individual_declaration(source, "AHU")


def test_corpus_recovery_cannot_promote_an_explicit_instance() -> None:
    label = "Lazor_Series_8030_ProdEquipCategory"
    instance_source = f"The ex:{label} instance is the general category of one machine."
    category_source = (
        f"The {label} category represents a model and may categorize multiple machines."
    )
    candidates = {
        "lazor series 8030 prod equip category": {
            "label": label,
            "occurrences": [
                {"chunk_id": 1, "text": instance_source},
                {"chunk_id": 2, "text": category_source},
            ],
        },
    }
    payload = {
        "class_decisions": [{
            "label": label,
            "role": "type",
            "keep": True,
            "confidence": 0.99,
            "evidence": f"The {label} category represents a model",
        }],
    }
    assert extract._apply_corpus_role_decisions(candidates, payload) == []


def test_local_verified_class_is_blocked_by_corpus_instance_evidence() -> None:
    label = "Lazor_Series_8030_ProdEquipCategory"
    local_source = f"The {label} category can describe multiple machines."
    corpus_source = (
        local_source
        + f"\n\nThe ex:{label} instance is a ProductionEquipmentCategory."
    )
    cleaned, rejected = tbox_guard.sanitize_ontology_delta(
        {
            "classes": [{"label": label, "_role_verified": True}],
            "object_properties": [],
            "data_properties": [],
            "subclass_of": [],
            "disjoint_with": [],
            "equivalent_class": [],
        },
        local_source,
        corpus_role_source_text=corpus_source,
    )
    assert cleaned["classes"] == []
    assert [row["label"] for row in rejected] == [label]


def test_abox_class_named_surface_requires_strict_adjudication() -> None:
    mention = {"label": "AHU", "class": "Air_Handling_Unit"}
    assert not abox_extract._is_self_typed_mention(mention)
    assert abox_extract._is_self_typed_mention(
        mention, ["AHU", "Air_Handling_Unit", "Equipment"],
    )


def test_named_instance_with_class_surface_still_reaches_adjudicator() -> None:
    mention = {"label": ":hvac_system", "class": "HVAC_System"}
    assert abox_extract._is_self_typed_mention(mention, ["HVAC_System"])
