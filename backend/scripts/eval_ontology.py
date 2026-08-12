"""Ontology-learning evaluation for OntoPilot, using the LLMs4OL / OntoLearner metric definitions.

Three standard tasks (LLMs4OL Task A/B/C ≈ OntoLearner term-typing / taxonomy-discovery / relation):
  - term_typing : (term, type) pairs                    -> pair-level P/R/F1 (single-answer ⇒ MAP@1 == accuracy)
  - taxonomy    : (sub, super) subClassOf pairs          -> pair-level P/R/F1
  - relation    : (domain_type, property, range_type)    -> triple-level P/R/F1

Predictions come from an OntoPilot TBox (schema.build_view); the gold comes from a reference
ontology (any RDF/OWL/TTL file — e.g. an OntoLearner/HuggingFace benchmark ontology). Matching is
on NORMALISED labels (case / whitespace / punctuation folded) so surface differences aren't counted
as misses — the role OntoLearner's LabelMapper plays. This is a faithful re-implementation of the
standard set-based P/R/F1, kept dependency-light so it never touches the backend runtime; cross-check
against the official `ontolearner` library if you need leaderboard-identical numbers.

NOTE on language: comparison is label-based, so predictions and gold must be in the SAME language.
A Chinese OntoPilot KS evaluated against an English gold will score ~0 — extract from an
English corpus (or build a same-language gold) for a meaningful number.
"""
from __future__ import annotations

import re
import sys
from dataclasses import dataclass

_PUNCT = re.compile(r"[\s_\-/·、,，.。:：;；()（）\"'`]+")
_CAMEL = re.compile(r"(?<=[a-z0-9])(?=[A-Z])")


def _norm(s: str) -> str:
    """Fold a label to a comparison key: split camelCase (so `ItalianWine` == `Italian wine`),
    lowercase, collapse whitespace/punctuation — the role OntoLearner's LabelMapper plays."""
    return _PUNCT.sub(" ", _CAMEL.sub(" ", s or "").strip().lower()).strip()


def _local(iri: str) -> str:
    return re.split(r"[#/]", iri.rstrip("#/"))[-1] if iri else iri


@dataclass
class Score:
    tp: int
    fp: int
    fn: int

    @property
    def precision(self) -> float:
        return self.tp / (self.tp + self.fp) if self.tp + self.fp else 0.0

    @property
    def recall(self) -> float:
        return self.tp / (self.tp + self.fn) if self.tp + self.fn else 0.0

    @property
    def f1(self) -> float:
        p, r = self.precision, self.recall
        return 2 * p * r / (p + r) if p + r else 0.0

    def as_dict(self) -> dict:
        return {"precision": round(self.precision, 4), "recall": round(self.recall, 4),
                "f1": round(self.f1, 4), "tp": self.tp, "fp": self.fp, "fn": self.fn}


def score(pred: set, gold: set) -> Score:
    """Set-based P/R/F1 (works for pairs and triples alike)."""
    tp = len(pred & gold)
    return Score(tp=tp, fp=len(pred) - tp, fn=len(gold) - tp)


# --------------------------------------------------------------------------- #
# Task tuples from an OntoPilot TBox view (schema.build_view output).
# --------------------------------------------------------------------------- #
def tasks_from_view(view: dict) -> dict[str, set]:
    labels = view.get("labels", {})

    def L(iri: str) -> str:
        return labels.get(iri) or _local(iri)

    taxonomy = {(_norm(L(r["sub"])), _norm(L(r["super"]))) for r in view["axioms"]["subclass_of"]}
    relation = set()
    for p in view["object_properties"]:
        if p.get("domain") and p.get("range"):
            relation.add((
                _norm(p.get("domain_label") or L(p["domain"])),
                _norm(p["label"]),
                _norm(p.get("range_label") or L(p["range"])),
            ))
    return {"taxonomy": taxonomy, "relation": relation}


# --------------------------------------------------------------------------- #
# Gold task tuples from a reference RDF ontology (rdflib).
# --------------------------------------------------------------------------- #
def tasks_from_rdf(path_or_graph) -> dict[str, set]:
    import rdflib
    from rdflib.namespace import OWL, RDF, RDFS

    g = path_or_graph if isinstance(path_or_graph, rdflib.Graph) else rdflib.Graph().parse(path_or_graph)

    def L(node) -> str:
        lbl = g.value(node, RDFS.label)
        return _norm(str(lbl) if lbl is not None else _local(str(node)))

    taxonomy = set()
    for s, o in g.subject_objects(RDFS.subClassOf):
        if isinstance(s, rdflib.URIRef) and isinstance(o, rdflib.URIRef):
            taxonomy.add((L(s), L(o)))

    relation = set()
    for p in g.subjects(RDF.type, OWL.ObjectProperty):
        dom, rng = g.value(p, RDFS.domain), g.value(p, RDFS.range)
        if isinstance(dom, rdflib.URIRef) and isinstance(rng, rdflib.URIRef):
            relation.add((L(dom), L(p), L(rng)))
    return {"taxonomy": taxonomy, "relation": relation}


def tasks_from_ontolearner_gold(taxonomies: dict | None = None, non_taxonomies: dict | None = None,
                                term_typings: list | None = None) -> dict[str, set]:
    """Gold task tuples from OntoLearner's shipped JSON (type_taxonomies.json etc.)."""
    out: dict[str, set] = {}
    if taxonomies:
        out["taxonomy"] = {(_norm(t["child"]), _norm(t["parent"])) for t in taxonomies.get("taxonomies", [])}
    if non_taxonomies:
        out["relation"] = {(_norm(r["head"]), _norm(r["relation"]), _norm(r["tail"]))
                           for r in non_taxonomies.get("non_taxonomies", [])}
    if term_typings:
        out["term_typing"] = {(_norm(tt["term"]), _norm(ty)) for tt in term_typings for ty in tt.get("types", [])}
    return out


def evaluate(pred: dict[str, set], gold: dict[str, set]) -> dict:
    """Per-task P/R/F1 + a micro-average over all tasks."""
    out, all_pred, all_gold = {}, set(), set()
    for task in ("term_typing", "taxonomy", "relation"):
        if task not in pred and task not in gold:
            continue
        p, gd = pred.get(task, set()), gold.get(task, set())
        out[task] = score(p, gd).as_dict()
        all_pred |= {(task, *t) if isinstance(t, tuple) else (task, t) for t in p}
        all_gold |= {(task, *t) if isinstance(t, tuple) else (task, t) for t in gd}
    out["overall_micro"] = score(all_pred, all_gold).as_dict()
    return out


def _print_report(rep: dict) -> None:
    print(f"{'task':<16}{'P':>8}{'R':>8}{'F1':>8}{'tp':>6}{'fp':>6}{'fn':>6}")
    for task, m in rep.items():
        print(f"{task:<16}{m['precision']:>8.3f}{m['recall']:>8.3f}{m['f1']:>8.3f}{m['tp']:>6}{m['fp']:>6}{m['fn']:>6}")


# --------------------------------------------------------------------------- #
def _selftest() -> None:
    # Predicted TBox (OntoPilot-style view): pump ⊑ device, well ⊑ device; pump --installed_at--> well
    view = {
        "labels": {"c1": "Pump", "c2": "Device", "c3": "Well"},
        "axioms": {"subclass_of": [{"sub": "c1", "super": "c2"}, {"sub": "c3", "super": "c2"}]},
        "object_properties": [
            {"iri": "p1", "label": "installed at", "domain": "c1", "domain_label": "Pump", "range": "c3", "range_label": "Well"},
        ],
    }
    pred = tasks_from_view(view)
    # Gold: pump⊑device (hit), well⊑equipment (miss vs pred's well⊑device), plus a relation pred missed.
    gold = {
        "taxonomy": {("pump", "device"), ("well", "equipment")},
        "relation": {("pump", "installed at", "well"), ("device", "has part", "pump")},
    }
    rep = evaluate(pred, gold)
    _print_report(rep)
    tax = rep["taxonomy"]
    assert tax["tp"] == 1 and tax["fp"] == 1 and tax["fn"] == 1, tax           # pump⊑device matches
    rel = rep["relation"]
    assert rel["tp"] == 1 and rel["fp"] == 0 and rel["fn"] == 1, rel           # 1 matched, 1 gold missed
    assert tax["f1"] == round(0.5, 4) and rel["f1"] == round(2 / 3, 4)
    print("\nself-test OK — metrics match hand computation (taxonomy F1=0.5, relation F1=0.667)")


def _cli() -> None:
    import argparse

    ap = argparse.ArgumentParser(description="Evaluate an OntoPilot KS TBox against a gold ontology.")
    ap.add_argument("--selftest", action="store_true", help="run the metric self-test and exit")
    ap.add_argument("--gold", help="path to a gold RDF/OWL/TTL ontology")
    ap.add_argument("--ks-graph", help="OntoPilot TBox graph IRI (e.g. http://ontopilot.local/ks/1)")
    args = ap.parse_args()

    if args.selftest or not (args.gold and args.ks_graph):
        _selftest()
        if not (args.gold and args.ks_graph):
            print("\n(pass --gold <file> --ks-graph <iri> to score a real KS)")
        return

    from app.ontology import schema  # imported lazily so --selftest needs no app/store

    pred = tasks_from_view(schema.build_view(args.ks_graph))
    gold = tasks_from_rdf(args.gold)
    _print_report(evaluate(pred, gold))


if __name__ == "__main__":
    sys.exit(_cli())
