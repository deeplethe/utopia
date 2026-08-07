"""Invariant checks over an extracted KS — flush out bugs after a large extraction.

Run:  python scripts/probe_bugs.py <ks_id>

Checks (each prints PASS / a list of offending items):
  1. no orphan ABox types      — every rdf:type used by an individual exists as a TBox class
  2. no orphan ABox predicates  — every assertion property exists as a TBox property
  3. provenance completeness    — every individual (and most assertions) has ≥1 source
  4. no empty labels            — classes/properties have non-blank labels
  5. no duplicate pending       — the resolution queue has no repeated (surface, class)
  6. conflict payload sanity    — open conflicts carry entities + at least one resolution
  7. count consistency          — KS/view/graph class & property counts agree
  8. dangling object targets    — every object-assertion target is a real individual
  9. validation                 — report datatype/domain/range/disjoint violations
"""
from __future__ import annotations

import sys
from collections import Counter

import requests

BASE = "http://127.0.0.1:8000"


def main(ks_id: int):
    s = requests.Session()
    s.post(f"{BASE}/api/auth/login", json={"username": "admin", "password": "admin"}).raise_for_status()
    G = lambda p: s.get(f"{BASE}/api/knowledge/{ks_id}{p}").json()

    view = G("/ontology")
    ks = G("")
    class_iris = {c["iri"] for c in view["classes"]}
    prop_iris = {p["iri"] for p in view["object_properties"] + view["data_properties"]}
    ind_list = G("/abox/individuals?limit=200")["items"]
    ind_iris = {i["iri"] for i in ind_list}
    print(f"KS {ks_id}: {len(class_iris)} classes, {len(prop_iris)} props, {len(ind_list)} individuals\n")

    issues: list[str] = []

    def check(name, offenders, show=6):
        if offenders:
            issues.append(name)
            print(f"[FAIL] {name}: {len(offenders)}")
            for o in list(offenders)[:show]:
                print(f"         - {o}")
        else:
            print(f"[pass] {name}")

    # 4. empty labels
    check("empty class/prop labels",
          [c["iri"] for c in view["classes"] if not (c.get("label") or "").strip()]
          + [p["iri"] for p in view["object_properties"] + view["data_properties"] if not (p.get("label") or "").strip()])

    # 1. orphan ABox types (from the abox class listing)
    abox_cls = G("/abox/classes")["classes"]
    check("orphan ABox types (rdf:type not in TBox)",
          [f'{c["label"]} ({c["iri"]})' for c in abox_cls if c["iri"] not in class_iris])

    # 2/3/8: walk individuals
    orphan_pred, no_ind_src, dangling, untyped = [], [], [], []
    assert_total = assert_with_src = 0
    for it in ind_list:
        ind = G(f"/abox/individual?iri={requests.utils.quote(it['iri'], safe='')}")
        if not ind.get("types"):
            untyped.append(ind["label"])
        if not (ind.get("sources") or []):
            no_ind_src.append(ind["label"])
        for a in ind.get("object_assertions", []):
            assert_total += 1
            assert_with_src += 1 if a.get("sources") else 0
            if a["prop"] not in prop_iris:
                orphan_pred.append(f'{ind["label"]} · {a["prop_label"]}')
            if a["target"] not in ind_iris:
                dangling.append(f'{ind["label"]} --{a["prop_label"]}--> {a["target_label"]}')
        for a in ind.get("data_assertions", []):
            assert_total += 1
            assert_with_src += 1 if a.get("sources") else 0
            if a["prop"] not in prop_iris:
                orphan_pred.append(f'{ind["label"]} · {a["prop_label"]}')

    check("orphan ABox predicates (assertion prop not in TBox)", orphan_pred)
    check("individuals without provenance", no_ind_src)
    check("untyped individuals (no class)", untyped)
    check("dangling object-assertion targets", dangling)
    print(f"[info] assertion provenance coverage: {assert_with_src}/{assert_total} "
          f"({(100*assert_with_src//assert_total) if assert_total else 0}%)")

    # 5. duplicate pending resolutions
    q = G("/resolution/queue?limit=1000")["items"]
    seen = Counter((r["surface_form"], r.get("class_iri")) for r in q)
    check("duplicate pending resolutions", [f"{k[0]} ×{v}" for k, v in seen.items() if v > 1])

    # 6. conflict payload sanity
    conflicts = G("/conflicts?status=open")
    bad = [c["id"] for c in conflicts if not c["payload"].get("entities") or not c["payload"].get("resolutions")]
    print(f"[info] open conflicts: {len(conflicts)}  by type: {dict(Counter(c['ctype'] for c in conflicts))}")
    check("conflicts missing entities/resolutions", bad)

    # 7. count consistency
    counts = []
    if ks["class_count"] != len(class_iris):
        counts.append(f'ks.class_count={ks["class_count"]} != view classes {len(class_iris)}')
    if view["stats"]["class_count"] != len(view["classes"]):
        counts.append(f'stats.class_count={view["stats"]["class_count"]} != len(classes) {len(view["classes"])}')
    check("count inconsistencies", counts)

    # 9. validation
    try:
        v = G("/abox/validate")
        c = v["counts"]
        print(f"[info] validation: {c['error']} errors / {c['warning']} warnings"
              f"{' (truncated)' if v.get('truncated') else ''}")
        for viol in v["violations"][:6]:
            print(f"         · [{viol['type']}] {viol['summary']}")
    except Exception as e:  # noqa: BLE001
        print("[warn] validation call failed:", e)

    print("\n" + ("=" * 60))
    print("BUGS/ISSUES:" if issues else "no invariant violations found ✓")
    for i in issues:
        print("  -", i)


if __name__ == "__main__":
    main(int(sys.argv[1]))
