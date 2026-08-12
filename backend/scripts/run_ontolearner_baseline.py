"""End-to-end OntoLearner baseline for OntoPilot's TBox extraction.

Text2Onto-style: take an OntoLearner benchmark ontology (gold taxonomy shipped as JSON), verbalise it
into an English corpus, run it through OntoPilot's real extraction pipeline in a throwaway KS, then
score the extracted subclass hierarchy against the gold with scripts/eval_ontology.py. Cleans up the
temp KS afterwards.

Usage:  python scripts/run_ontolearner_baseline.py [domain_repo] [ontology_name]
        (defaults: SciKnowOrg/ontolearner-food_and_beverage  wine)
"""
from __future__ import annotations

import json
import os
import sys
import time
from collections import defaultdict

import requests
from huggingface_hub import hf_hub_download

sys.path.insert(0, os.path.dirname(__file__))
import eval_ontology as E  # noqa: E402

BASE = "http://127.0.0.1:8000"
os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")


def load_gold(repo: str, name: str):
    tax = json.load(open(hf_hub_download(repo, f"{name}/type_taxonomies.json", repo_type="dataset"), encoding="utf-8"))
    tts = json.load(open(hf_hub_download(repo, f"{name}/term_typings.json", repo_type="dataset"), encoding="utf-8"))
    return tax, tts


def verbalize(tax: dict, tts: list) -> str:
    """Turn the gold taxonomy + term typings into plain English the extractor can read."""
    kids = defaultdict(set)
    for t in tax["taxonomies"]:
        kids[t["parent"]].add(t["child"])
    lines = ["This document describes a domain ontology as prose.\n"]
    for parent, cs in kids.items():
        cs = sorted(cs)
        lines.append(f"Types of {parent} include {', '.join(cs)}. Each of {', '.join(cs)} is a kind of {parent}.")
    # Term typings are a SEPARATE task (Task A); their "X is a Y" prose would otherwise be
    # extracted as extra subclass links and unfairly hurt taxonomy precision. Set BENCH_TYPINGS=1
    # to include them (e.g. to score against the taxonomy∪typing union instead).
    if os.environ.get("BENCH_TYPINGS") == "1":
        for tt in tts:
            for ty in tt.get("types", []):
                lines.append(f"{tt['term']} is a {ty}.")
    return "\n".join(lines)


def poll_job(s: requests.Session, ks: int, job_id: int, label: str, timeout=420):
    t0 = time.time()
    while time.time() - t0 < timeout:
        j = s.get(f"{BASE}/api/knowledge/{ks}/jobs/{job_id}").json()
        if j["status"] in ("completed", "failed"):
            return j
        time.sleep(2)
    raise TimeoutError(f"{label} job timed out")


def main():
    repo = sys.argv[1] if len(sys.argv) > 1 else "SciKnowOrg/ontolearner-food_and_beverage"
    name = sys.argv[2] if len(sys.argv) > 2 else "wine"

    tax, tts = load_gold(repo, name)
    cap = int(os.environ.get("BENCH_MAX_PAIRS", "0"))  # cap huge ontologies (e.g. cco) to a subset
    if cap:
        tax = {**tax, "taxonomies": tax["taxonomies"][:cap]}
    gold = E.tasks_from_ontolearner_gold(taxonomies=tax, term_typings=tts)
    corpus = verbalize(tax, tts)
    print(f"[{name}] gold taxonomy pairs={len(gold['taxonomy'])}  term-typings={len(gold['term_typing'])}  corpus={len(corpus)} chars")

    s = requests.Session()
    s.post(f"{BASE}/api/auth/login", json={"username": "admin", "password": "admin"}).raise_for_status()
    ks = s.post(f"{BASE}/api/knowledge", json={"name": f"__bench_{name}__", "description": "OntoLearner baseline (temp)"}).json()
    ks_id, graph_iri = ks["id"], ks["graph_iri"]
    print(f"temp KS id={ks_id}  graph={graph_iri}")
    try:
        doc = s.post(f"{BASE}/api/knowledge/{ks_id}/documents/upload",
                     files={"file": (f"{name}_corpus.txt", corpus, "text/plain")}, data={"folder": "/"}).json()
        s.post(f"{BASE}/api/knowledge/{ks_id}/documents/{doc['id']}/parse").raise_for_status()
        chunks = s.get(f"{BASE}/api/knowledge/{ks_id}/documents/{doc['id']}/chunks").json()
        chunk_ids = [c["id"] for c in chunks]
        print(f"parsed into {len(chunk_ids)} chunk(s); extracting TBox…")
        job = s.post(f"{BASE}/api/knowledge/{ks_id}/extract", json={"chunk_ids": chunk_ids}).json()
        j = poll_job(s, ks_id, job["id"], "extract")
        print(f"extraction: {j['status']}  +{j.get('classes_added')} classes / +{j.get('axioms_added')} axioms")

        view = s.get(f"{BASE}/api/knowledge/{ks_id}/ontology").json()
        pred = E.tasks_from_view(view)
        print(f"predicted: classes={len(view['classes'])}  taxonomy pairs={len(pred['taxonomy'])}\n")
        rep = E.evaluate({"taxonomy": pred["taxonomy"]}, {"taxonomy": gold["taxonomy"]})
        E._print_report(rep)
        if os.environ.get("BENCH_DUMP") == "1":
            fp = sorted(pred["taxonomy"] - gold["taxonomy"])
            fn = sorted(gold["taxonomy"] - pred["taxonomy"])
            print(f"\n-- FALSE POSITIVES (predicted, not in gold) [{len(fp)}] --")
            for a, b in fp[:80]:
                print(f"  {a}  <=  {b}")
            print(f"\n-- FALSE NEGATIVES (gold, missed) [{len(fn)}] --")
            for a, b in fn[:50]:
                print(f"  {a}  <=  {b}")
    finally:
        s.delete(f"{BASE}/api/knowledge/{ks_id}")
        print(f"\ncleaned up temp KS {ks_id}")


if __name__ == "__main__":
    main()
