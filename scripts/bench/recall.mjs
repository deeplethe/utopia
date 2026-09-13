#!/usr/bin/env node
// 召回的测量台：**这篇文档里该有的东西，进图了没有。**
//
// 与 run.mjs 的分工：那个量「类型消解把实体归到哪个类」，这个量「文档里的信息丢没丢」。
// 判据是人读原文列出来的一张表（truth/nvda-public-docs.json，52 条），不是模型自己的说法，
// 也不是 drops/misses 那两张信号表——**它们只看得见「抽出来了没落地」，看不见「根本没抽」**，
// 而后者才是大头：一份 8-K 的收购价、留任池、交割时点全都不在图里的时候，那两张表是空的。
//
// 打分只问「在不在」，不问「落没落上本体」：谓词取 coalesce(关系 key, proposed_predicate)，
// 空谓词的事实照样算命中。本体接不接得住是另一个问题（run.mjs 那条线）。
//
// 一轮 = 清空事实与实体 → 本体退回装包那一刻 → 重抽 → 打分。**每轮条件相同，数字才可比**；
// 自动扩本体与类型消解在轮内关掉，否则上一轮长出来的关系会进下一轮的提示词。
//
// 用法：
//   node scripts/bench/recall.mjs --kb <id>            # 已有库（本体向量已就绪）
//   node scripts/bench/recall.mjs --kb <id> --score    # 只打分，不重抽
//   node scripts/bench/recall.mjs --kb <id> --reprocess # 改了解析器：连分块一起重来
//
// 环境变量：BENCH_BASE / BENCH_EMAIL / BENCH_PASSWORD / BENCH_PSQL（同 run.mjs）。
//
// **为什么要 `--kb` 而不是每轮建新库**：装一次 schema.org 要嵌 2500 条向量、二十分钟，
// 而这个台子量的不是本体。库里除了本体没有别的跨轮状态——事实、实体、信号每轮都清空。

import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.env.BENCH_BASE || "http://localhost:18080";
const EMAIL = process.env.BENCH_EMAIL || "bench@test.local";
const PASSWORD = process.env.BENCH_PASSWORD || "benchbench123";
const CORPUS = path.join(HERE, "corpora", "nvda-public-docs");
const TRUTH = path.join(HERE, "truth", "nvda-public-docs.json");

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, cur, i, arr) => {
    if (cur.startsWith("--")) acc.push([cur.slice(2), arr[i + 1]?.startsWith("--") ? true : arr[i + 1] ?? true]);
    return acc;
  }, []),
);
const KB = args.kb;
if (!KB) {
  console.error("要一个 --kb <id>：建一个装了本体包的库，等它的本体向量补齐，再把 id 给这里。");
  process.exit(1);
}

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

let cookie = "";
async function api(method, url, body, isForm) {
  const init = { method, headers: {} };
  if (cookie) init.headers.cookie = cookie;
  if (isForm) init.body = body;
  else if (body !== undefined) {
    init.headers["content-type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  const r = await fetch(BASE + url, init);
  for (const c of r.headers.getSetCookie?.() ?? []) cookie = c.split(";")[0];
  const t = await r.text();
  if (!r.ok) throw new Error(`${method} ${url} -> ${r.status} ${t.slice(0, 200)}`);
  return t ? JSON.parse(t) : null;
}

const truth = JSON.parse(fs.readFileSync(TRUTH, "utf8"));
const DOCS = [...new Set(truth.map((t) => t.doc))];

function score() {
  // 一篇文档的全部现行事实，摊平成可搜的一行
  // 用单元分隔符拼成一列再取回：BENCH_PSQL 是一条固定的命令串（`-tAc` 结尾），
  // 加不了 `-F`，而默认的 `|` 会被名字里的竖线撞上
  const rows = psql(`
    SELECT concat_ws(chr(31),
           d.filename,
           coalesce(s.canonical_name,''),
           coalesce(rt.key, coalesce(fe.proposed_predicate,'')),
           coalesce(o.canonical_name, f.object_value #>> '{}', f.object_value::text, ''),
           coalesce(to_char(f.valid_from,'YYYY-MM-DD'),''),
           coalesce((SELECT string_agg(q.key || '=' || coalesce(fq.value #>> '{value}', '') || ' ' || coalesce(fq.value #>> '{unit}', ''), ' ')
                       FROM fact_qualifiers fq JOIN relation_types q ON q.id = fq.qualifier_type_id
                      WHERE fq.fact_id = f.id), ''))
    FROM facts f
    JOIN fact_evidence fe ON fe.fact_id = f.id
    JOIN documents d ON d.id = fe.document_id
    JOIN entities s ON s.id = f.subject_id
    LEFT JOIN relation_types rt ON rt.id = f.predicate_id
    LEFT JOIN entities o ON o.id = f.object_id
    WHERE f.kb_id = '${KB}' AND f.invalidated_at IS NULL`)
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      // 边上的属性（0037）也在这一行里：金额挂在边上时，值不在 object 那一格
      const [file, subj, pred, obj, from, quals] = l.split("");
      return { file, subj, pred, obj, from, quals };
    });

  const byDoc = new Map();
  for (const r of rows) {
    const key = r.file.replace(/\.(html|md|pdf)$/i, "");
    if (!byDoc.has(key)) byDoc.set(key, []);
    byDoc.get(key).push(r);
  }

  const norm = (s) => (s || "").toLowerCase().replace(/[’‘]/g, "'");
  // 值只比字符，不比排版："8 IT-GW" 与 "8-IT GW" 是同一个数；不抹平就会把命中判成漏抽
  const squash = (s) => norm(s).replace(/[\s\-–—]/g, "");

  let pass = 0;
  const misses = [];
  const perDoc = new Map();
  const perKind = new Map();

  for (const t of truth) {
    const facts = byDoc.get(t.doc) || [];
    let ok = false;
    if (t.kind === "value") {
      ok = facts.some((f) => {
        const raw = `${f.subj} | ${f.pred} | ${f.obj} | ${f.from} | ${f.quals}`;
        return t.value_any.some((v) => norm(raw).includes(norm(v)) || squash(raw).includes(squash(v)));
      });
    } else if (t.kind === "edge") {
      ok = facts.some((f) => {
        const s = norm(f.subj), o = norm(f.obj);
        return (
          (t.subject.some((x) => s.includes(norm(x))) && t.object.some((x) => o.includes(norm(x)))) ||
          (t.subject.some((x) => o.includes(norm(x))) && t.object.some((x) => s.includes(norm(x))))
        );
      });
    } else if (t.kind === "predicate") {
      ok = facts.some((f) => t.predicate_any.some((p) => norm(f.pred).includes(norm(p))));
    }
    for (const [map, key] of [[perDoc, t.doc], [perKind, t.kind]]) {
      const cur = map.get(key) || { pass: 0, total: 0 };
      cur.total++;
      if (ok) cur.pass++;
      map.set(key, cur);
    }
    if (ok) pass++;
    else misses.push(t);
  }

  console.log(`\n总分 ${pass}/${truth.length}  (${((100 * pass) / truth.length).toFixed(1)}%)\n`);
  for (const [doc, d] of [...perDoc].sort()) {
    console.log(`  ${String(d.pass).padStart(2)}/${String(d.total).padEnd(2)}  ${doc}`);
  }
  const label = { value: "字面值（金额/日期/职务/票数）", edge: "关系边", predicate: "谓词区分度" };
  console.log("\n按类别：");
  for (const [k, v] of perKind) {
    console.log(`  ${String(v.pass).padStart(2)}/${String(v.total).padEnd(2)}  ${label[k] || k}`);
  }
  console.log("\n没进图的：");
  for (const m of misses) console.log(`  [${m.doc}] ${m.id} — ${m.what}`);
  // **52 个条目、九成命中率，一个标准差 ≈ 2.7 条。** 单轮差一两条不是证据，
  // 同一份代码重跑一遍再说；结构性的改善看 drops 计数与某一类是否整类进来了
  console.log("\n（52 条真值，1σ ≈ 2.7 条：单轮小幅波动不作数）");
}

if (args.score) {
  score();
  process.exit(0);
}

await api("POST", "/api/v1/auth/login", { email: EMAIL, password: PASSWORD });

// 已经在库里的就不重传；第一次跑要先 fetch-sec-filings.mjs
const existing = ((await api("GET", `/api/v1/kbs/${KB}/documents?limit=200`)).docs ?? []).map((d) => d.filename);
const want = DOCS.map((d) => `${d}.html`);
const missing = want.filter((f) => !existing.includes(f));
for (const f of missing) {
  const p = path.join(CORPUS, f);
  if (!fs.existsSync(p)) throw new Error(`语料缺 ${f}，先跑 node scripts/bench/fetch-sec-filings.mjs`);
  const fd = new FormData();
  fd.append("files", new Blob([fs.readFileSync(p)]), f);
  await api("POST", `/api/v1/kbs/${KB}/documents`, fd, true);
  console.log(`${stamp()} 上传 ${f}`);
}
if (missing.length) {
  // 新传的要先解析入库，才谈得上重抽
  while (num(`SELECT count(*) FROM documents WHERE kb_id='${KB}' AND status <> 'ready'`) > 0) await sleep(5000);
}

const names = want.map((f) => `'${f}'`).join(",");
const packTs = psql(
  `SELECT coalesce(min(created_at)::text,'') FROM relation_types WHERE kb_id='${KB}'`,
);
console.log(`=== ${stamp()} 清空（本体退回 ${packTs}）===`);
psql(`DELETE FROM jobs WHERE kind IN ('adjudicate_entities','bootstrap_ontology','resolve_types','govern')`);
psql(`DELETE FROM facts WHERE kb_id='${KB}'`);
psql(`DELETE FROM entities WHERE kb_id='${KB}'`);
psql(`DELETE FROM extraction_drops WHERE kb_id='${KB}'`);
psql(`DELETE FROM ontology_misses WHERE kb_id='${KB}'`);
// 自动扩本体上一轮长出来的关系：留着就进下一轮的提示词，两轮条件不同
psql(`DELETE FROM relation_types WHERE kb_id='${KB}' AND created_at > '${packTs}'::timestamptz + interval '1 second'`);
// 四个会在抽取之后改图的开关一律关掉：测的是抽取本身，推理物化和治理闸门会在打分前增删事实
psql(`UPDATE knowledge_bases SET auto_extend_ontology=FALSE, auto_type_resolution=FALSE, materialize_inferences=FALSE, governance=FALSE WHERE id='${KB}'`);
psql(`UPDATE chunks SET extracted_at=NULL WHERE document_id IN (SELECT id FROM documents WHERE kb_id='${KB}' AND filename IN (${names}))`);
console.log(`本体 ${num(`SELECT count(*) FROM relation_types WHERE kb_id='${KB}'`)} 个关系 / ${num(`SELECT count(*) FROM entity_types WHERE kb_id='${KB}'`)} 个类`);

const docs = (await api("GET", `/api/v1/kbs/${KB}/documents?limit=200`)).docs.filter((d) => want.includes(d.filename));
const endpoint = args.reprocess ? "reprocess" : "extract";
for (const d of docs) {
  await api("POST", `/api/v1/documents/${d.id}/${endpoint}`, {});
  console.log(`${stamp()} 排队 ${endpoint} ${d.filename}`);
}

// **看进展，不看总时长**：慢不算超时，卡住才算（与 run.mjs 同一条规矩）
const live = `SELECT count(*) FROM chunks c JOIN documents d ON d.id=c.document_id
              WHERE d.kb_id='${KB}' AND d.filename IN (${names}) AND c.superseded_at IS NULL`;
let last = -1, stall = 0;
for (;;) {
  const done = num(`${live.replace("count(*)", "count(c.extracted_at)")}`);
  const total = num(live);
  const left = num(
    `SELECT count(*) FROM jobs WHERE kind IN ('extract_document','process_document') AND status IN ('queued','running')`,
  );
  console.log(`${stamp()} ${done}/${total} 块，还有 ${left} 个任务`);
  if (left === 0 && done > 0) break;
  if (done === last) {
    if (++stall > 30) { console.log("十五分钟没有进展，停"); break; }
  } else stall = 0;
  last = done;
  await sleep(30000);
}

score();
