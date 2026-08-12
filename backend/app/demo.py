"""Deterministic demo data for first-run evaluation (no model calls required)."""
from __future__ import annotations

from sqlmodel import Session, select

from app import audit
from app.db.models import AboxProvenance, AxiomProvenance, KnowledgeSystem, User
from app.ontology import (
    abox, abox_provenance, schema, statement_provenance, store, terminology_sync,
)


def seed(session: Session, owner: User, *, force: bool = False) -> KnowledgeSystem:
    existing = session.exec(select(KnowledgeSystem).where(KnowledgeSystem.name == "Pump Operations Demo")).first()
    if existing:
        return existing
    if not force and session.exec(select(KnowledgeSystem)).first():
        raise ValueError("Demo data is only seeded automatically into an empty installation")

    ks = KnowledgeSystem(
        name="Pump Operations Demo",
        description="Deterministic TBox, terminology and ABox sample for the 10-minute product tour.",
        owner_id=owner.id,
    )
    session.add(ks)
    session.commit()
    session.refresh(ks)
    ks.graph_iri = f"http://ontopilot.local/ks/{ks.id}"
    ks.base_iri = f"http://ontopilot.local/ks/{ks.id}/onto#"
    session.add(ks)
    session.commit()

    ontology = {
        "classes": [
            {"label": "Asset", "comment": "A managed physical or logical resource."},
            {"label": "Equipment", "comment": "An asset that performs an operational function."},
            {"label": "Pump", "comment": "Equipment that moves fluid."},
            {"label": "Site", "comment": "A managed operational location."},
        ],
        "object_properties": [
            {"label": "installed at", "domain": "Equipment", "range": "Site"},
        ],
        "data_properties": [
            {"label": "serial number", "domain": "Equipment", "range": "string"},
            {"label": "flow rate", "domain": "Pump", "range": "decimal"},
        ],
        "subclass_of": [
            {"sub": "Equipment", "super": "Asset"},
            {"sub": "Pump", "super": "Equipment"},
        ],
        "disjoint_with": [],
        "equivalent_class": [],
    }
    mutation = schema.build_mutation(ks.base_iri, ontology, schema.read_index(ks.graph_iri))
    store.add_triples(ks.graph_iri, mutation.triples)
    for triple in mutation.triples:
        session.add(AxiomProvenance(
            knowledge_system_id=ks.id,
            axiom_key=statement_provenance.triple_key(*triple),
            method="demo",
            actor_name="system",
            review_record={"statement": store.dump_triples([triple]).decode("utf-8").strip()},
        ))
    session.commit()
    view = schema.build_view(ks.graph_iri)
    labels = {item["label"]: item["iri"] for item in view["classes"]}
    properties = {
        item["label"]: item["iri"]
        for item in view["object_properties"] + view["data_properties"]
    }
    abox_iri = f"{ks.graph_iri}/abox"
    site = abox.create_individual(abox_iri, ks.base_iri, "North Plant", labels["Site"])
    pump = abox.create_individual(abox_iri, ks.base_iri, "Pump P-101", labels["Pump"])
    abox.add_object_assertion(abox_iri, pump, properties["installed at"], site)
    abox.add_data_assertion(
        abox_iri, pump, properties["serial number"], "P101-2026",
        "http://www.w3.org/2001/XMLSchema#string",
    )
    abox.add_data_assertion(
        abox_iri, pump, properties["flow rate"], "125.5",
        "http://www.w3.org/2001/XMLSchema#decimal",
    )
    fact_keys = (
        abox_provenance.ind_key(site),
        abox_provenance.ind_key(pump),
        abox_provenance.obj_key(pump, properties["installed at"], site),
        abox_provenance.data_key(pump, properties["serial number"], "P101-2026"),
        abox_provenance.data_key(pump, properties["flow rate"], "125.5"),
    )
    for fact_key in fact_keys:
        session.add(AboxProvenance(
            knowledge_system_id=ks.id,
            fact_key=fact_key,
            method="demo",
            actor_name="system",
        ))
    session.commit()
    terminology_sync.sync_from_ontology(ks)
    view = schema.build_view(ks.graph_iri)
    ks.class_count = view["stats"]["class_count"]
    ks.property_count = view["stats"]["property_count"]
    ks.axiom_count = view["stats"]["axiom_count"]
    session.add(ks)
    session.commit()
    audit.record(
        session,
        ks_id=ks.id,
        action="demo.seed",
        summary="Created deterministic pump operations demo",
        actor_id=owner.id,
        actor_name=owner.username,
        detail={"no_model_calls": True},
    )
    return ks
