#!/usr/bin/env node
// 身份的测量台（0041 第 0 刀）：**同一个东西有没有归成一个实体，不同的东西有没有被并成一个。**
//
// 两类错一起量：重名（两个张伟、两个技术中心、1 号与 2 号）会被错并，多名（更名、简称、
// 中英文名）会被错拆。真值是 truth/identity.json 里的锚点：「这篇文档里、引文含这段话的
// 事实、这个类型那一侧的实体，是真实世界里的哪一个」。打分只比锚点解析到的实体 id 两两
// 是否相同——**名字不参与打分**，否则别名与重名会先把打分本身搅乱。
//
// 一轮 = 新建一个库 → 种本体（冻结：自动扩本体、类型消解、治理、物化全关）→ 按顺序一篇
// 一篇灌，每篇等这个库的任务全部跑完再灌下一篇 → 打分。**入库顺序是被量的东西之一**，
// 所以同一轮正序、倒序各建一个库，报两边判得不一样的锚点对占多少。
//
// 用法：
//   node scripts/bench/identity.mjs --label <名字>                 # 正序 + 倒序各一个新库
//   node scripts/bench/identity.mjs --label <名字> --order forward
//   node scripts/bench/identity.mjs --score <kb id>                # 只给已有库打分
//
// 环境变量：BENCH_BASE / BENCH_EMAIL / BENCH_PASSWORD / BENCH_PSQL（同 recall.mjs）。
// 账号要能在工作区里建库。

import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.env.BENCH_BASE || "http://localhost:18080";
const EMAIL = process.env.BENCH_EMAIL || "bench@test.local";
const PASSWORD = process.env.BENCH_PASSWORD || "benchbench123";
const CORPUS = path.join(HERE, "corpora", "identity");
const truth = JSON.parse(fs.readFileSync(path.join(HERE, "truth", "identity.json"), "utf8"));

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, cur, i, arr) => {
    if (cur.startsWith("--")) acc.push([cur.slice(2), arr[i + 1]?.startsWith("--") ? true : arr[i + 1] ?? true]);
    return acc;
  }, []),
);

function psql(sql) {
  const cmd =
    process.env.BENCH_PSQL ||
    "docker exec -e PGPASSWORD=utopia landscapebi-db-1 psql -U utopia -d utopia -tAc";
  const parts = cmd.split(" ");
  return execFileSync(parts[0], [...parts.slice(1), sql], { encoding: "utf8", maxBuffer: 64 << 20 }).trim();
}
const num = (sql) => Number(psql(sql) || 0);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stamp = () => new Date().toISOString().slice(11, 19);
const log = (...a) => console.error(`[${stamp()}]`, ...a);

let cookie = "";
async function api(method, url, body) {
  const init = { method, headers: {} };
  if (cookie) init.headers.cookie = cookie;
  if (body !== undefined) {
    init.headers["content-type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  const r = await fetch(BASE + "/api/v1" + url, init);
  for (const c of r.headers.getSetCookie?.() ?? []) cookie = c.split(";")[0];
  const t = await r.text();
  if (!r.ok) throw new Error(`${method} ${url} -> ${r.status} ${t.slice(0, 300)}`);
  return t ? JSON.parse(t) : null;
}

// ---------- 等：这个库还有没有活没干完 ----------

function busy(kb) {
  // 抽取任务的 payload 只带 document_id，别的带 kb_id：两种都得认
  return num(`
    SELECT count(*) FROM jobs j
     WHERE j.status IN ('queued', 'running')
       AND (j.payload->>'kb_id' = '${kb}'
            OR j.payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id = '${kb}'))`);
}

async function settle(kb, what, limitMs = 15 * 60 * 1000) {
  const start = Date.now();
  // 起码等一拍：刚入队的任务可能还没写进 jobs
  await sleep(1500);
  let quiet = 0;
  while (Date.now() - start < limitMs) {
    const pending = busy(kb);
    const extracting = num(
      `SELECT count(*) FROM documents WHERE kb_id = '${kb}' AND graph_status IN ('queued', 'extracting')`,
    );
    if (pending === 0 && extracting === 0) {
      // 连着两拍都空才算停：一个任务收尾时会排下一个（抽取 → 裁决）
      if (++quiet >= 2) return;
    } else quiet = 0;
    await sleep(3000);
  }
  log(`!! ${what}: 等了 ${Math.round(limitMs / 60000)} 分钟还没停，照样往下走`);
}

// ---------- 建库、种本体、灌语料 ----------

async function workspaceId() {
  // 与 run.mjs、govern.mjs 同一个取法：部署里第一个工作区
  return (await api("GET", "/workspaces"))[0].id;
}

async function buildBase(ws, name) {
  const kb = (await api("POST", `/workspaces/${ws}/kbs`, { name })).id;
  await api("PATCH", `/kbs/${kb}`, {
    auto_extend_ontology: false,
    auto_type_resolution: false,
    governance: false,
    materialize_inferences: false,
  });
  const types = {};
  for (const t of truth.ontology.types) {
    types[t.key] = (await api("POST", `/kbs/${kb}/ontology/entity-types`, t)).id;
  }
  for (const a of truth.ontology.attributes) {
    await api("POST", `/kbs/${kb}/ontology/relation-types`, {
      key: a.key, label: a.label, kind: "attribute", temporal: "state",
      datatype: a.datatype, unit: a.unit ?? "", description: a.description,
      domains: a.domains.map((d) => types[d]),
    });
  }
  for (const r of truth.ontology.relations) {
    await api("POST", `/kbs/${kb}/ontology/relation-types`, {
      key: r.key, label: r.label, kind: "relation", temporal: r.temporal, description: r.description,
      functional: !!r.functional, inverse_functional: !!r.inverse_functional,
      domains: r.domains.map((d) => types[d]), ranges: r.ranges.map((d) => types[d]),
    });
  }
  return kb;
}

async function ingestInOrder(kb, docs) {
  await settle(kb, "本体向量");
  for (const d of docs) {
    const content = fs.readFileSync(path.join(CORPUS, d.file), "utf8");
    await api("POST", `/kbs/${kb}/ingest`, { filename: d.file, content, doc_time: d.doc_time });
    await settle(kb, d.file);
    const st = psql(`SELECT graph_status FROM documents WHERE kb_id = '${kb}' AND filename = '${d.file}'`);
    log(`${d.file}: ${st}`);
  }
}

// ---------- 打分 ----------

function resolveAnchors(kb) {
  // 每条现行事实的证据：哪篇、引文、两侧解析到的**幸存者**（顺着 merged_into 走到头）与类型
  const rows = psql(`
    WITH RECURSIVE chain(id, cur) AS (
      SELECT id, id FROM entities WHERE kb_id = '${kb}'
      UNION ALL
      SELECT c.id, e.merged_into FROM chain c JOIN entities e ON e.id = c.cur
       WHERE e.merged_into IS NOT NULL
    ), surv AS (
      SELECT c.id, c.cur AS survivor FROM chain c JOIN entities e ON e.id = c.cur
       WHERE e.merged_into IS NULL
    )
    SELECT concat_ws(chr(31),
           d.filename,
           replace(replace(coalesce(fe.quote, ''), chr(10), ' '), chr(13), ' '),
           coalesce(ss.survivor::text, ''), coalesce(st.key, ''), coalesce(sv.canonical_name, ''),
           coalesce(os.survivor::text, ''), coalesce(ot.key, ''), coalesce(ov.canonical_name, ''),
           coalesce(f.object_value->>'value', ''))
      FROM facts f
      JOIN fact_evidence fe ON fe.fact_id = f.id
      JOIN documents d ON d.id = fe.document_id
      LEFT JOIN surv ss ON ss.id = f.subject_id
      LEFT JOIN entities sv ON sv.id = ss.survivor
      LEFT JOIN entity_types st ON st.id = sv.type_id
      LEFT JOIN surv os ON os.id = f.object_id
      LEFT JOIN entities ov ON ov.id = os.survivor
      LEFT JOIN entity_types ot ON ot.id = ov.type_id
     WHERE f.kb_id = '${kb}' AND f.invalidated_at IS NULL`)
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      const [file, quote, sId, sType, sName, oId, oType, oName, value] = l.split("");
      return { file, quote, value, sides: { subject: [sId, sType, sName], object: [oId, oType, oName] } };
    });

  const squash = (s) => (s || "").toLowerCase().replace(/\s+/g, "");
  const out = {};
  for (const a of truth.anchors) {
    const ids = new Map(); // id -> name
    for (const r of rows) {
      if (r.file !== a.doc || !squash(r.quote).includes(squash(a.quote_has))) continue;
      for (const role of a.role ? [a.role] : ["subject", "object"]) {
        const [id, type, name] = r.sides[role];
        if (!id || type !== a.type) continue;
        // 一句话里并排好几条事实时（「位于舟山，下设财务部和工程部」），用另一侧把锚点钉在
        // 那一条上：另一侧的名字或值里得有 other_has
        if (a.other_has) {
          const other = r.sides[role === "subject" ? "object" : "subject"];
          if (!squash(`${other[2]} ${r.value}`).includes(squash(a.other_has))) continue;
        }
        ids.set(id, name);
      }
    }
    out[a.id] = [...ids.entries()];
  }
  return out;
}

function score(kb) {
  const found = resolveAnchors(kb);
  const byId = Object.fromEntries(truth.anchors.map((a) => [a.id, a]));
  const missing = truth.anchors.filter((a) => found[a.id].length === 0).map((a) => a.id);
  // 一个锚点在同一篇里解析出几个实体：那一篇之内就已经拆开了
  const splitInDoc = truth.anchors.filter((a) => found[a.id].length > 1).map((a) => a.id);

  // 两两比：一个锚点取它解析到的第一个 id；拆开的锚点另外报
  const usable = truth.anchors.filter((a) => found[a.id].length >= 1);
  const verdicts = {};
  let tp = 0, fp = 0, fn = 0, tn = 0;
  const falseMerges = [], falseSplits = [];
  for (let i = 0; i < usable.length; i++) {
    for (let j = i + 1; j < usable.length; j++) {
      const a = usable[i], b = usable[j];
      const same = found[a.id][0][0] === found[b.id][0][0];
      const truthSame = a.truth === b.truth;
      verdicts[`${a.id}~${b.id}`] = same;
      if (same && truthSame) tp++;
      else if (same && !truthSame) { fp++; falseMerges.push(`${a.id}(${a.truth}) = ${b.id}(${b.truth})`); }
      else if (!same && truthSame) { fn++; falseSplits.push(`${a.id} ≠ ${b.id} (${a.truth})`); }
      else tn++;
    }
  }
  const precision = tp + fp ? tp / (tp + fp) : 1;
  const recall = tp + fn ? tp / (tp + fn) : 1;
  const f1 = precision + recall ? (2 * precision * recall) / (precision + recall) : 0;

  // 每个真实实体落成了几个系统实体、叫什么
  const perEntity = {};
  for (const [key] of Object.entries(truth.entities)) {
    const ids = new Map();
    for (const a of usable.filter((x) => x.truth === key)) for (const [id, name] of found[a.id]) ids.set(id, name);
    perEntity[key] = { entities: ids.size, as: [...ids.values()] };
  }
  const lookalikes = truth.lookalikes.map(([x, y]) => {
    const xs = new Set(usable.filter((a) => a.truth === x).flatMap((a) => found[a.id].map(([id]) => id)));
    const shared = usable.filter((a) => a.truth === y).flatMap((a) => found[a.id].map(([id]) => id)).some((id) => xs.has(id));
    return { pair: `${x}/${y}`, merged: shared };
  });

  const reviews = psql(`
    SELECT concat_ws(chr(31), stage, why, n) FROM (
      SELECT stage, split_part(coalesce(reason, ''), '|', 1) AS why, count(*) AS n
        FROM resolution_reviews WHERE kb_id = '${kb}' AND status = 'pending'
       GROUP BY 1, 2) r ORDER BY stage, why`)
    .split("\n").filter(Boolean).map((l) => l.split("")).map(([stage, reason, n]) => ({ stage, reason, n: Number(n) }));

  return {
    kb,
    anchors: truth.anchors.length,
    missing,
    split_in_doc: splitInDoc,
    pairs: { tp, fp, fn, tn },
    precision: +precision.toFixed(3),
    recall: +recall.toFixed(3),
    f1: +f1.toFixed(3),
    false_merges: falseMerges,
    false_splits: falseSplits,
    per_entity: perEntity,
    lookalikes,
    pending_reviews: reviews,
    entities_in_base: num(`SELECT count(*) FROM entities WHERE kb_id = '${kb}' AND merged_into IS NULL`),
    verdicts,
    _byId: byId,
  };
}

function orderDisagreement(a, b) {
  const keys = Object.keys(a.verdicts).filter((k) => k in b.verdicts);
  const differ = keys.filter((k) => a.verdicts[k] !== b.verdicts[k]);
  return { compared: keys.length, differ: differ.length, rate: keys.length ? +(differ.length / keys.length).toFixed(3) : 0, pairs: differ };
}

function brief(r) {
  const { verdicts, _byId, ...rest } = r;
  return rest;
}

// ---------- 主流程 ----------

await api("POST", "/auth/login", { email: EMAIL, password: PASSWORD });

if (args.score) {
  console.log(JSON.stringify(brief(score(args.score)), null, 2));
  process.exit(0);
}

const label = args.label || "run";
const orders = args.order === "forward" ? ["forward"] : args.order === "reverse" ? ["reverse"] : ["forward", "reverse"];
const ws = await workspaceId();
const results = {};
for (const order of orders) {
  const name = `identity ${label} ${order} ${new Date().toISOString().slice(0, 16)}`;
  const kb = await buildBase(ws, name);
  log(`${order}: base ${kb}`);
  const docs = order === "forward" ? truth.docs : [...truth.docs].reverse();
  await ingestInOrder(kb, docs);
  results[order] = score(kb);
  log(`${order}: precision ${results[order].precision} recall ${results[order].recall} f1 ${results[order].f1}`);
}
const summary = { label, ...Object.fromEntries(Object.entries(results).map(([k, v]) => [k, brief(v)])) };
if (results.forward && results.reverse) summary.order_disagreement = orderDisagreement(results.forward, results.reverse);
console.log(JSON.stringify(summary, null, 2));
