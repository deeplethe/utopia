"""Stress-test OntoPilot: generate a large, deliberately-messy Chinese corpus, extract a full
ontology (TBox + ABox) into a fresh KS, and leave it for inspection. Pair with probe_bugs.py.

Planted edge cases (to flush bugs): a synonym class pair (电机 vs 电动机), the same entity mentioned
across chunks (multi-source provenance + dedup), unit-in-value attributes (datatype traps), a
whitespaced name variant, and a deep-ish class hierarchy.
"""
from __future__ import annotations

import sys
import time

import requests

BASE = "http://127.0.0.1:8000"

TOP = {
    "设备": ["抽油机", "注水泵", "分离器", "加热炉", "变压器", "电动机"],
    "井": ["油井", "注水井", "气井", "观察井"],
    "人员": ["采油工", "维修工", "技术员", "班长"],
    "场所": ["井场", "站库", "联合站", "计量站"],
    "备件": ["密封件", "轴承", "皮带", "阀芯"],
    "记录": ["巡检记录", "维修单", "设备台账", "生产报表"],
}
PLACES = ["1号井场", "2号井场", "3号井场", "东部站库", "西部联合站"]
WELLS = ["1号油井", "2号油井", "3号油井", "5号注水井", "8号气井"]
WORKERS = ["张建国", "李卫东", "王海涛", "赵春生"]


def generate() -> str:
    out: list[str] = ["华北采油厂设备与生产综合台账。本文档登记全厂设备、井、人员、场所、备件及相关记录。\n"]
    # TBox: hierarchy sentences
    for top, subs in TOP.items():
        out.append(f"{'、'.join(subs)}都属于{top}。{top}是本体系的一大类。")
    out.append("")
    # ABox: instances with attributes + relations, plus planted edge cases
    n = 0
    for top, subs in TOP.items():
        for s in subs:
            for i in range(1, 4):  # 3 instances per subclass
                n += 1
                name = f"{i}号{s}"
                place = PLACES[n % len(PLACES)]
                well = WELLS[n % len(WELLS)]
                worker = WORKERS[n % len(WORKERS)]
                # edge case: 电动机 sometimes written 电机 (synonym class)
                if s == "电动机" and i % 2 == 0:
                    name = f"{i}号电机"
                # edge case: a whitespaced variant
                if s == "抽油机" and i == 1:
                    name = "1号 抽油机"
                fact = f"{name}，编号SB{1000 + n}，{2015 + (n % 8)}年投产，安装于{place}。"
                if top == "设备":
                    fact += f"额定功率{20 + (n % 5) * 5}千瓦，负责{well}的生产，由{worker}负责维护。"
                elif top == "井":
                    # edge case: unit-in-value attribute (should be number only → datatype trap)
                    fact += f"位于{place}，日产量{1000 + n * 10}吨，井深{2000 + n}米。"
                elif top == "人员":
                    fact += f"隶属{place}，工号{2000 + n}，负责{well}的巡检。"
                elif top == "备件":
                    fact += f"库存于{place}，用于{well}的设备维修。"
                out.append(fact)
        out.append("")
    # extra cross-references so entities recur across chunks (multi-source provenance)
    out.append("补充说明：1号抽油机与1号 抽油机为同一台设备的不同写法记录。3号油井由张建国长期负责。")
    out.append(f"全厂共登记设备、井、人员等实例约 {n} 项。")
    return "\n".join(out)


def poll(s, ks, job_id, timeout=1200):
    t0 = time.time()
    last = None
    while time.time() - t0 < timeout:
        j = s.get(f"{BASE}/api/knowledge/{ks}/jobs/{job_id}").json()
        if j["status"] in ("completed", "failed"):
            return j
        if j.get("processed_chunks") != last:
            last = j.get("processed_chunks")
            print(f"  … {j['status']} {j.get('processed_chunks')}/{j.get('total_chunks')} chunks")
        time.sleep(3)
    raise TimeoutError("extraction timed out")


def main():
    corpus = generate()
    print(f"corpus: {len(corpus)} chars, {corpus.count(chr(10))} lines")

    s = requests.Session()
    s.post(f"{BASE}/api/auth/login", json={"username": "admin", "password": "admin"}).raise_for_status()
    ks = s.post(f"{BASE}/api/knowledge", json={"name": "压力测试库", "description": "large-scale extraction stress test"}).json()
    ks_id = ks["id"]
    print(f"KS id={ks_id} name={ks['name']}")

    doc = s.post(f"{BASE}/api/knowledge/{ks_id}/documents/upload",
                 files={"file": ("stress_corpus.txt", corpus, "text/plain")}, data={"folder": "/"}).json()
    s.post(f"{BASE}/api/knowledge/{ks_id}/documents/{doc['id']}/parse").raise_for_status()
    chunks = s.get(f"{BASE}/api/knowledge/{ks_id}/documents/{doc['id']}/chunks").json()
    chunk_ids = [c["id"] for c in chunks]
    print(f"parsed into {len(chunk_ids)} chunks; running extract-all (TBox + ABox)…")

    job = s.post(f"{BASE}/api/knowledge/{ks_id}/extract-all", json={"chunk_ids": chunk_ids}).json()
    j = poll(s, ks_id, job["id"])
    print(f"\nextraction {j['status']}: +{j.get('classes_added')} classes / +{j.get('properties_added')} props / "
          f"+{j.get('axioms_added')} axioms · +{j.get('individuals_added')} individuals / {j.get('pending_added')} queued / "
          f"+{j.get('assertions_added')} assertions")
    if j.get("error"):
        print("ERROR:", j["error"])
    # surface any ERROR lines from the per-chunk log
    errs = [ln for ln in (j.get("log") or "").splitlines() if "ERROR" in ln.upper()]
    if errs:
        print("\nlog errors:")
        for e in errs[:20]:
            print("  ", e)
    print(f"\nKS {ks_id} left in place for inspection + probe_bugs.py {ks_id}")


if __name__ == "__main__":
    sys.exit(main())
