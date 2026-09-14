"""租约链测量台：Blackbaud 总部租约与它的补充协议，按时点问 18 道题。

    py -3 lease_bench.py setup --label <名字> [--with-base]
    py -3 lease_bench.py score <kb id>

**setup** 建一个新库，声明一套通用的租约与物业买卖本体，把 `edgar/blackbaud-hq-lease/`
里的文件导进去（先跑 `fetch.py`）。默认不导 2016 年的原租约——它有 145 块，一轮要两个
小时，而 18 道题问的全在补充协议里；`--with-base` 才导。库 id 写进 `lease-kb-<名字>.json`。
等 `documents.graph_status` 全部 done 再打分。

**score** 用产品自己的读法答题：取每个租约、买卖协议实体的事实，留下服务端算出的成立区间
（`holds_from` / `holds_to`）盖住那个时刻的，比那一刻的值。那一刻所有同类实体上的值
**恰好**是真值算对；真值在、但还有别的值算部分对。真值是逐份读原文定的，写在下面 QUESTIONS 里。

本体是通用的：它说任何租约、买卖协议都有的东西（房东、租户、起租日、选择权截止日、买价），
单值的标 functional——后一份补充协议才关得上前一份写的值。里面没有一个字是这份合同特有的。

**每轮一个新库，单轮波动约 3 题**，一个版本至少跑两轮再下结论（2026-09-13 的记录见 README）。

环境变量：
    BENCH_BASE      服务地址，缺省 http://127.0.0.1:1516
    BENCH_TOKEN     登录令牌；没有就用下面两个登录
    BENCH_EMAIL / BENCH_PASSWORD   同 recall.mjs，缺省 bench@test.local / benchbench123
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
BASE = os.environ.get("BENCH_BASE", "http://127.0.0.1:1516")
# 本机的 HTTP(S)_PROXY 指向本地代理，连 localhost 不该经过它
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))
TOKEN = None


def api(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(BASE + "/api/v1" + path, data=data, method=method)
    if TOKEN:
        req.add_header("Authorization", f"Bearer {TOKEN}")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with OPENER.open(req, timeout=120) as res:
            raw = res.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raise SystemExit(f"{method} {path} -> {e.code}: {e.read()[:400]!r}")


def login():
    global TOKEN
    TOKEN = os.environ.get("BENCH_TOKEN")
    if TOKEN:
        return
    email = os.environ.get("BENCH_EMAIL", "bench@test.local")
    password = os.environ.get("BENCH_PASSWORD", "benchbench123")
    TOKEN = api("POST", "/auth/login", {"email": email, "password": password})["token"]


# ---------------------------------------------------------------- setup

CLASSES = [
    ("organization", "Organization", "A company, LLC, fund or public body that can be a party to an agreement."),
    ("lease", "Lease", "A lease agreement together with all its amendments; one lease is one entity however many amendments change it."),
    ("amendment", "Amendment", "A document that changes an existing agreement. The terms it changes belong to the agreement it amends, not to the amendment."),
    ("property", "Property", "A parcel of real property or a building."),
    ("purchase_agreement", "Purchase agreement", "An agreement to buy and sell real property, together with its amendments."),
]

# (key, label, kind, domain, range 或 datatype, 单位, functional, 描述)
AXIOMS = [
    ("landlord", "landlord", "relation", "lease", "organization", None, True,
     "The party leasing the premises out. A lease has one landlord at a time; a new owner of the property becomes the new landlord."),
    ("tenant", "tenant", "relation", "lease", "organization", None, True,
     "The party leasing the premises."),
    ("leased_property", "leased property", "relation", "lease", "property", None, False,
     "Real property covered by the lease."),
    ("seller", "seller", "relation", "purchase_agreement", "organization", None, True,
     "The party selling the property under a purchase agreement."),
    ("purchaser", "purchaser", "relation", "purchase_agreement", "organization", None, True,
     "The party buying the property under a purchase agreement."),
    ("property_sold", "property sold", "relation", "purchase_agreement", "property", None, False,
     "The property a purchase agreement sells."),
    ("affiliate_of", "affiliate of", "relation", "organization", "organization", None, False,
     "An organization controlled by, or under common control with, another."),
    ("amends", "amends", "relation", "amendment", "lease|purchase_agreement", None, False,
     "The agreement an amendment changes."),
    ("effective_date", "effective date", "attribute", "amendment", "date", None, True,
     "The date an amendment takes effect."),
    ("premises_acres", "premises area", "attribute", "lease", "number", "acres", True,
     "Land area of the leased premises, in acres."),
    ("lease_term_years", "initial term", "attribute", "lease", "number", "years", True,
     "Length of the initial term, in years."),
    ("commencement_date", "commencement date", "attribute", "lease", "date", None, True,
     "The date the lease term begins. When the date is defined by a condition, record the fixed date the text gives."),
    ("expansion_option_deadline", "expansion option exercise deadline", "attribute", "lease", "date", None, True,
     "The last day the tenant may exercise an option to expand the premises, such as a Phase 2 exercise deadline."),
    ("expansion_lease_deadline", "expansion lease signing deadline", "attribute", "lease", "date", None, True,
     "The last day for the parties to sign the lease for the expansion phase."),
    ("expansion_budget_deadline", "expansion budget delivery deadline", "attribute", "lease", "date", None, True,
     "The last day for the landlord to deliver the preliminary budget for the expansion phase."),
    ("purchase_price", "purchase price", "attribute", "purchase_agreement", "number", "USD", True,
     "The price payable for the property, in US dollars."),
    ("inspection_period_end", "inspection period end", "attribute", "purchase_agreement", "date", None, True,
     "The date the purchaser's inspection period ends."),
    ("outside_closing_date", "outside closing date", "attribute", "purchase_agreement", "date", None, True,
     "The latest date by which the sale must close."),
]

# (edgar/blackbaud-hq-lease 里的文件, 开头读出的文件日期, 上传名)。第二份买卖协议修订只写了
# 「July 2020」没写哪天，不给日期
DOCS = [
    ("00-base-2016-08-04.html", "2016-05-16", "hq-lease-2016-05-16-lease-agreement.html"),
    ("01-amendment-2016-11-04.html", "2016-08-22", "hq-lease-2016-08-22-first-amendment.html"),
    ("02-amendment-2018-02-20.html", "2017-05-18", "hq-lease-2017-05-18-second-amendment.html"),
    ("03-amendment-2018-02-20.html", "2017-12-11", "hq-lease-2017-12-11-third-amendment.html"),
    ("04-amendment-2018-05-04.html", "2018-02-28", "hq-lease-2018-02-28-fourth-amendment.html"),
    ("05-amendment-2020-08-04.html", "2020-02-18", "hq-lease-2020-02-18-fifth-amendment.html"),
    ("06-amendment-2020-08-04.html", "2020-03-17", "hq-lease-2020-03-17-sixth-amendment.html"),
    ("07-amendment-2020-08-04.html", "2020-04-14", "hq-lease-2020-04-14-seventh-amendment.html"),
    ("08-amendment-2020-08-04.html", "2020-05-26", "hq-lease-2020-05-26-eighth-amendment.html"),
    ("09-amendment-2020-08-04.html", "2020-06-08", "hq-lease-2020-06-08-ninth-amendment.html"),
    ("11-amendment-2020-08-04.html", "2020-06-26", "hq-lease-2020-06-26-tenth-amendment.html"),
    ("12-sale-amendment-2020-11-03.html", "2020-07-08", "hq-sale-2020-07-08-psa-first-amendment.html"),
    ("13-sale-amendment-2020-11-03.html", None, "hq-sale-2020-07-psa-second-amendment.html"),
    ("14-amendment-2020-11-03.html", "2020-08-13", "hq-lease-2020-08-13-eleventh-amendment.html"),
]


def setup(label, with_base):
    folder = HERE / "edgar" / "blackbaud-hq-lease"
    if not folder.exists():
        raise SystemExit("先跑 fetch.py")
    ws = api("GET", "/workspaces")[0]["id"]
    kb = api("POST", f"/workspaces/{ws}/kbs", {"name": f"Blackbaud HQ lease ({label})", "ontology_packs": []})["id"]
    print("kb", kb)
    time.sleep(4)
    # 与召回测量台同一个理由：抽取之后会改图的开关一律关掉，量的是抽取本身
    api("PATCH", f"/kbs/{kb}", {"auto_extend_ontology": False, "materialize_inferences": False, "governance": False})
    ids = {}
    for key, name, desc in CLASSES:
        ids[key] = api("POST", f"/kbs/{kb}/ontology/entity-types", {"key": key, "label": name, "description": desc})["id"]
    for key, name, kind, domain, rng, unit, functional, desc in AXIOMS:
        body = {"key": key, "label": name, "kind": kind, "temporal": "state", "functional": functional,
                "description": desc, "domains": [ids[domain]]}
        if kind == "attribute":
            body["datatype"] = rng
            if unit:
                body["unit"] = unit
        else:
            body["ranges"] = [ids[r] for r in rng.split("|")]
        api("POST", f"/kbs/{kb}/ontology/relation-types", body)
    print(f"声明 {len(CLASSES)} 个类、{len(AXIOMS)} 条关系与属性")
    for src, date, name in DOCS:
        if src.startswith("00-") and not with_base:
            continue
        body = {"filename": name, "content": (folder / src).read_text(encoding="utf-8", errors="replace")}
        if date:
            body["doc_time"] = f"{date}T00:00:00Z"
        api("POST", f"/kbs/{kb}/ingest", body)
        print("导入", name)
    (HERE / f"lease-kb-{label}.json").write_text(json.dumps({"kb": kb, "base": BASE}, indent=1), encoding="utf-8")


# ---------------------------------------------------------------- score

# (题目, 类, 谓词, 时刻, 真值)
QUESTIONS = [
    ("expansion option deadline, 2020-03-01", "lease", "expansion_option_deadline", "2020-03-01", "2020-03-17"),
    ("expansion option deadline, 2020-04-01", "lease", "expansion_option_deadline", "2020-04-01", "2020-04-14"),
    ("expansion option deadline, 2020-05-01", "lease", "expansion_option_deadline", "2020-05-01", "2020-05-26"),
    ("expansion option deadline, 2020-06-01", "lease", "expansion_option_deadline", "2020-06-01", "2020-06-09"),
    ("expansion option deadline, 2020-06-15", "lease", "expansion_option_deadline", "2020-06-15", "2020-06-23"),
    ("expansion budget deadline, 2020-03-01", "lease", "expansion_budget_deadline", "2020-03-01", "2020-05-07"),
    ("expansion budget deadline, 2020-04-01", "lease", "expansion_budget_deadline", "2020-04-01", "2020-06-04"),
    ("expansion budget deadline, 2020-05-01", "lease", "expansion_budget_deadline", "2020-05-01", "2020-07-16"),
    ("expansion budget deadline, 2020-06-01", "lease", "expansion_budget_deadline", "2020-06-01", "2020-07-30"),
    ("expansion budget deadline, 2020-06-15", "lease", "expansion_budget_deadline", "2020-06-15", "2020-08-13"),
    ("landlord, 2019-01-01", "lease", "landlord", "2019-01-01", "HPBB1"),
    ("landlord, 2020-07-01", "lease", "landlord", "2020-07-01", "HPBB1"),
    ("landlord, 2020-09-01", "lease", "landlord", "2020-09-01", "BBHQ1"),
    ("tenant, 2020-09-01", "lease", "tenant", "2020-09-01", "BLACKBAUD"),
    ("premises acres, 2019-01-01", "lease", "premises_acres", "2019-01-01", "12.98"),
    ("purchase price, 2020-08-01", "purchase_agreement", "purchase_price", "2020-08-01", "76272484.66"),
    ("purchaser, 2020-08-01", "purchase_agreement", "purchaser", "2020-08-01", "BBHQ1"),
    ("outside closing date, 2020-08-01", "purchase_agreement", "outside_closing_date", "2020-08-01", "2020-12-14"),
]


def norm(v):
    s = str(v).strip().upper()
    s = s.split(",")[0].strip() if not s.replace(",", "").replace(".", "").isdigit() else s.replace(",", "")
    return s[:10] if len(s) >= 10 and s[4:5] == "-" else s


def holds(f, t):
    start, end = f.get("holds_from"), f.get("holds_to")
    return (start is None or start[:10] <= t) and (end is None or end[:10] > t)


def score(kb):
    by_class = defaultdict(list)
    type_ids = {t["key"]: t["id"] for t in api("GET", f"/kbs/{kb}/ontology")["entity_types"]}
    for cls in {q[1] for q in QUESTIONS}:
        # 这个接口分页，默认一页 12 个：不翻页就只看到前 12 个实体
        page_no = 0
        while True:
            page = api("GET", f"/kbs/{kb}/ontology/entity-types/{type_ids[cls]}/entities?page={page_no}&per=100")
            by_class[cls].extend(page["entities"])
            if not page["entities"] or len(by_class[cls]) >= page.get("total", 0):
                break
            page_no += 1
    facts = {}
    for ents in by_class.values():
        for e in ents:
            facts[e["id"]] = [f for f in api("GET", f"/kbs/{kb}/entities/{e['id']}")["facts"] if f.get("direction") == "out"]

    carriers = [e["name"] for e in by_class["lease"] if any(f["predicate_key"] in ("landlord", "tenant") for f in facts[e["id"]])]
    right = partial = 0
    for label, cls, pred, t, truth in QUESTIONS:
        values = set()
        for e in by_class[cls]:
            for f in facts[e["id"]]:
                if f["predicate_key"] == pred and holds(f, t):
                    v = f["other_name"] if f.get("other_name") else (f.get("object_value") or {}).get("value")
                    values.add(norm(v))
        want = norm(truth)
        hit = any(v.startswith(want) for v in values)
        if hit and len(values) == 1:
            right += 1
            mark = "right"
        elif hit:
            partial += 1
            mark = "partial"
        else:
            mark = "wrong"
        print(f"{mark:8} {label:40} want {truth:14} got {sorted(values)}")
    print(f"\nright {right}/{len(QUESTIONS)}, partial {partial}")
    print(f"lease entities carrying a landlord or tenant: {len(carriers)}")
    for name in carriers:
        print("  ", name[:90])


def main():
    if len(sys.argv) < 3 or sys.argv[1] not in ("setup", "score"):
        raise SystemExit(__doc__)
    login()
    if sys.argv[1] == "setup":
        label = next((a.split("=", 1)[1] for a in sys.argv if a.startswith("--label=")), None)
        if not label and "--label" in sys.argv:
            label = sys.argv[sys.argv.index("--label") + 1]
        setup(label or "run", "--with-base" in sys.argv)
    else:
        score(sys.argv[2])


if __name__ == "__main__":
    main()
