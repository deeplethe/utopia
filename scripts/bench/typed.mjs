#!/usr/bin/env node
// 类型图的测量台（0044 §Measurement「Typed graph」）：**Re-DocRED 的属性当已批准的本体，
// 量对齐把开放陈述算成类型化事实之后，金标召回了多少、裁判认多少是对的。**
//
// 一组 = 新建库 → 装 6 个粗类型的类与 95 条属性（定义域/值域来自训练集，见 fetch-redocred.mjs）
//        → 灌 100 篇 → 抽取（开放图谱）→ 类别词对齐 → 短语对齐 → 物化 → 打分。
// 每一组一个新库（bench/README 的第一条规则）。跑两遍比一遍：0044 的门槛按两轮报。
//
// 报三个数，各自的含义：
//   gold recall     金标三元组里被类型化事实覆盖的比例，**同句与跨句分开**。跨句的等派生规则，
//                   记录说它先报数不设门槛；门槛只在同句那一半。匹配按名字：主语、宾语各自的
//                   任一提及名（金标给的）对上实体的任一名字（canonical_name 或 known_as），
//                   属性按键；宾语是值时按数字与年份的宽松包含。
//   judged precision  裁判模型读原文判类型化事实（抽样）：stated / misworded / not_stated。
//                   金标严重漏标（0044 §3：严格精度 37.8%，裁判 76.1%），所以精度只信裁判。
//   entity-pair recall  开放陈述连上的金标实体对，量的是抽取那一层，不是对齐。
//
// 用法：
//   node scripts/bench/typed.mjs --label run1                 # 完整一组
//   node scripts/bench/typed.mjs --label run1 --judge 200     # 加裁判抽样 200 条
//   node scripts/bench/typed.mjs --kb <id> --score            # 只对已有的库重新打分
//   node scripts/bench/typed.mjs --label dry --dry-run        # 建库、装本体、灌语料、等解析，不抽取：验管线
//   node scripts/bench/typed.mjs --label run1 --judge 200 --errata   # 对齐之后再跑勘误 agent，报前后两份分与撤错多少
//   node scripts/bench/typed.mjs --kb <id> --score --errata --judge 200   # 已有的库：跑勘误、打分
//   node scripts/bench/typed.mjs --kb <id> --score --approve-rules         # 替审核人批下全部蕴含规则、读数、物化，再打分
// 环境：BENCH_BASE（默认 http://localhost:1516）、BENCH_EMAIL / BENCH_PASSWORD（lib.mjs）、
//       BENCH_PSQL（指向应用库的 psql 命令行）、BENCH_JUDGE_BASE / _KEY / _MODEL（裁判端点）

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { api, login, parseArgs, sleep, until, log, EMAIL, PASSWORD } from "./lib.mjs";
import { execFileSync } from "node:child_process";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv);
const CORPUS = args.corpus || "redocred-100";
const LABEL = args.label || "typed";
const OUT = args.out || path.join(process.env.TMPDIR || "/tmp", `typed-${LABEL}.json`);
const PSQL = process.env.BENCH_PSQL || "docker exec -e PGPASSWORD=utopia utopia-db-1 psql -U utopia -d utopia -tAc";
const psql = (sql) => {
  const parts = PSQL.split(" ");
  return execFileSync(parts[0], [...parts.slice(1), sql], { encoding: "utf8", maxBuffer: 256 << 20, stdio: ["ignore", "pipe", "pipe"] }).trim();
};
const num = (sql) => Number(psql(sql) || 0);
const rows = (sql) => psql(sql).split("\n").filter(Boolean).map((l) => l.split("|"));
const q = (s) => String(s).replace(/'/g, "''");

const corpus = JSON.parse(fs.readFileSync(path.join(HERE, "corpora", `${CORPUS}.json`), "utf8"));
const truth = JSON.parse(fs.readFileSync(path.join(HERE, "truth", `${CORPUS}.json`), "utf8"));
const ontology = JSON.parse(fs.readFileSync(path.join(HERE, "truth", "redocred-ontology.json"), "utf8"));

// Re-DocRED 的 6 个粗类型 → 库里的类。类别词对齐把文档的类别词绑到这几个类上；
// 类的定义写给模型看，所以要说人话，不能只写 PER
const CLASSES = {
  PER: ["person", "Person", "a human being"],
  ORG: ["organization", "Organization", "a company, institution, team, band, party, government body or other organised group"],
  LOC: ["location", "Location", "a country, city, region, river, building or any other place"],
  TIME: ["time", "Time", "a date, year or period"],
  NUM: ["number", "Number", "a quantity, count, amount or measurement"],
  MISC: ["miscellaneous", "Miscellaneous", "a work, product, event, language, award, nationality or other named thing that is not a person, organisation or place"],
};

// 数值与时间在开放陈述里是字面值：值域只有 TIME / NUM 的属性建成 attribute（datatype 相应），
// 否则建成 relation 并把 TIME / NUM 从值域里去掉——对齐器只拿 attribute 配字面值签名
function shape(p) {
  const valueOnly = p.ranges.length > 0 && p.ranges.every((r) => r === "TIME" || r === "NUM");
  if (valueOnly) return { kind: "attribute", datatype: p.ranges.includes("TIME") ? "date" : "number", ranges: [] };
  return { kind: "relation", datatype: null, ranges: p.ranges.filter((r) => r !== "TIME" && r !== "NUM") };
}

const stamp = () => new Date().toISOString().slice(11, 19);

async function setup() {
  await login();
  const ws = (await api("GET", "/api/v1/workspaces"))[0];
  if (!ws) throw new Error("没有工作区");
  // 建库是管理员动作。开发库里测量台的账号往往不是首用户（首用户才是 admin），
  // 这里直接把它提成管理员再试——台子本来就靠 psql 做前置，不装作没有这个依赖
  let kb;
  try {
    kb = await api("POST", `/api/v1/workspaces/${ws.id}/kbs`, { name: `typed-${LABEL}-${Date.now()}`, ontology_packs: [] });
  } catch (e) {
    if (!String(e).includes("403")) throw e;
    psql(`UPDATE users SET is_admin=TRUE WHERE email='${q(EMAIL)}'`);
    log(`${EMAIL} 不是管理员，已通过 psql 提升后重试建库`);
    kb = await api("POST", `/api/v1/workspaces/${ws.id}/kbs`, { name: `typed-${LABEL}-${Date.now()}`, ontology_packs: [] });
  }
  const KB = kb.id;
  log(`新库 ${KB}（工作区 ${ws.id}）`);
  // 本体固定：不让抽取长本体、不跑老的类型消解、不做公理派生；治理按产品默认
  // 治理 agent（0025 的裁决）也关掉：它在第三轮里花了约 500 次调用，量的不是类型图
  psql(`UPDATE knowledge_bases SET auto_extend_ontology=FALSE, auto_type_resolution=FALSE, materialize_inferences=FALSE, governance=FALSE WHERE id='${KB}'`);
  const classId = {};
  for (const [t, [key, label, description]] of Object.entries(CLASSES)) {
    const r = await api("POST", `/api/v1/kbs/${KB}/ontology/entity-types`, { key, label, description, parents: [] });
    classId[t] = r.id;
  }
  let attributes = 0;
  for (const p of ontology.properties) {
    const s = shape(p);
    if (s.kind === "attribute") attributes += 1;
    await api("POST", `/api/v1/kbs/${KB}/ontology/relation-types`, {
      key: p.key.toLowerCase(),
      label: p.label,
      description: p.description,
      kind: s.kind,
      temporal: "state",
      domains: p.domains.map((d) => classId[d]),
      ranges: s.ranges.map((r) => classId[r]),
      datatype: s.datatype,
    });
  }
  log(`本体：${Object.keys(CLASSES).length} 类，${ontology.properties.length} 属性（${attributes} 条是 attribute）`);
  // 建本体排下的对齐任务在语料到之前没意义，清掉，抽完再排
  psql(`DELETE FROM jobs WHERE kind IN ('align_types','align_phrases') AND payload->>'kb_id'='${KB}' AND status='queued'`);
  return KB;
}

async function ingest(KB) {
  for (const d of corpus.docs) {
    const fd = new FormData();
    fd.append("files", new Blob([d.text], { type: "text/plain" }), d.filename);
    const r = await fetch(`${process.env.BENCH_BASE || "http://localhost:1516"}/api/v1/kbs/${KB}/documents`, {
      method: "POST", headers: { cookie: cookieOf() }, body: fd,
    });
    if (!r.ok) throw new Error(`上传 ${d.filename} -> ${r.status} ${(await r.text()).slice(0, 200)}`);
  }
  log(`上传 ${corpus.docs.length} 篇`);
  await until(() => {
    const left = num(`SELECT count(*) FROM documents WHERE kb_id='${KB}' AND status <> 'ready'`);
    log(`解析中：还剩 ${left}`);
    return left === 0 ? true : left;
  }, 5000, 10 * 60000);
}
// lib.mjs 的 cookie 不导出给 fetch 用；上传是 multipart，走不了它的 api()
import { cookieHeader } from "./lib.mjs";
const cookieOf = () => cookieHeader();

async function extract(KB) {
  const docs = (await api("GET", `/api/v1/kbs/${KB}/documents?limit=500`)).docs;
  for (const d of docs) await api("POST", `/api/v1/documents/${d.id}/extract`, {});
  log(`排队抽取 ${docs.length} 篇`);
  const live = `SELECT count(*) FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.kb_id='${KB}' AND c.superseded_at IS NULL`;
  await until(() => {
    const done = num(live.replace("count(*)", "count(c.extracted_at)"));
    const total = num(live);
    // 抽取任务的 payload 只带 document_id（没有 kb_id）：按文档所属的库数，否则这里永远是 0，
    // 台子会在第一篇抽完时就去排对齐，对齐跑在半截语料上
    const left = num(`SELECT count(*) FROM jobs j WHERE j.kind IN ('extract_document','process_document') AND j.status IN ('queued','running')
      AND (j.payload->>'kb_id'='${KB}' OR j.payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id='${KB}'))`);
    log(`抽取：${done}/${total} 块，${left} 个任务`);
    return left === 0 && done > 0 ? true : done;
  }, 15000, 15 * 60000);
  log(`开放陈述 ${num(`SELECT count(*) FROM facts WHERE kb_id='${KB}' AND layer='open' AND invalidated_at IS NULL`)} 条，实体 ${num(`SELECT count(*) FROM entities WHERE kb_id='${KB}' AND merged_into IS NULL`)} 个`);
}

async function align(KB) {
  // 类别词先绑到类，短语签名才定型；类别词对齐结束时自己会排短语对齐，短语对齐结束时物化
  psql(`INSERT INTO jobs (kind, payload) VALUES ('align_types', '{"kb_id":"${KB}"}')`);
  await until(() => {
    const left = num(`SELECT count(*) FROM jobs WHERE kind IN ('align_types','align_phrases','materialize_typed','read_phrases') AND status IN ('queued','running') AND payload->>'kb_id'='${KB}'`);
    const bound = num(`SELECT count(*) FROM phrase_bindings WHERE kb_id='${KB}' AND status='bound'`);
    const decided = num(`SELECT count(*) FROM phrase_bindings WHERE kb_id='${KB}'`) + num(`SELECT count(*) FROM type_bindings WHERE kb_id='${KB}'`);
    // 签名判完之后还有一段提规则（0044 决定 3 第五片）：每条判定过的签名问一次，规则行慢慢增加，
    // 绑定数却不再动——进展得把它也算上，否则这一段会被当成卡死
    const rules = num(`SELECT count(*) FROM implication_rules WHERE kb_id='${KB}'`);
    const typed = num(`SELECT count(*) FROM facts WHERE kb_id='${KB}' AND layer='typed' AND from_statement_id IS NOT NULL AND invalidated_at IS NULL`);
    log(`对齐：${left} 个任务在跑，判定 ${decided}，绑定 ${bound}，规则 ${rules}，类型化 ${typed}`);
    // `until` 只把数字当进展：四个数拼成一个单调的数
    return left === 0 && num(`SELECT count(*) FROM phrase_bindings WHERE kb_id='${KB}'`) > 0 ? true : decided * 1e9 + bound * 1e6 + rules * 1e3 + typed;
  }, 15000, 30 * 60000);
  const failed = num(`SELECT count(*) FROM jobs WHERE kind IN ('align_types','align_phrases','materialize_typed') AND status='failed' AND payload->>'kb_id'='${KB}'`);
  if (failed) log(`注意：${failed} 个对齐任务失败（看 jobs.last_error）`);
}

// --approve-rules：替审核人把对齐器提的蕴含规则全批了（0044 决定 3 第五片的上限：真人会驳回一部分），
// 排读数任务（读数完了自己排物化），等隐含行算出来。不带这个开关时提议只是躺在队列里，隐含行为 0
async function approveRules(KB) {
  const n = num(`SELECT count(*) FROM implication_rules WHERE kb_id='${KB}' AND status='proposed'`);
  psql(`UPDATE implication_rules SET status='approved', decided_by='person', decided_at=now() WHERE kb_id='${KB}' AND status='proposed'`);
  psql(`INSERT INTO jobs (kind, payload) VALUES ('read_phrases', '{"kb_id":"${KB}"}')`);
  log(`批了 ${n} 条规则，排读数与物化`);
  await until(() => {
    const left = num(`SELECT count(*) FROM jobs WHERE kind IN ('read_phrases','materialize_typed') AND status IN ('queued','running') AND payload->>'kb_id'='${KB}'`);
    const read = num(`SELECT count(*) FROM phrase_readings WHERE kb_id='${KB}'`);
    const implied = num(`SELECT count(*) FROM facts WHERE kb_id='${KB}' AND implied AND invalidated_at IS NULL`);
    log(`读数：${left} 个任务在跑，缓存 ${read} 条，隐含 ${implied} 条`);
    return left === 0 ? true : read * 100000 + implied;
  }, 15000, 30 * 60000);
  return { approved: n, readings: num(`SELECT count(*) FROM phrase_readings WHERE kb_id='${KB}'`), implied: num(`SELECT count(*) FROM facts WHERE kb_id='${KB}' AND implied AND invalidated_at IS NULL`) };
}

// 勘误（0044 决定 7）：物化之后排一次 errata_review，等它把每篇文档看完。度量按 errata_runs 与
// errata_actions 报：撤了几条、改了几条、加了几条、留给人几条、拒了几条、花了多少 token
async function errata(KB) {
  psql(`INSERT INTO jobs (kind, payload) VALUES ('errata_review', '{"kb_id":"${KB}"}')`);
  await until(() => {
    const left = num(`SELECT count(*) FROM jobs WHERE kind='errata_review' AND status IN ('queued','running') AND payload->>'kb_id'='${KB}'`);
    const seen = num(`SELECT count(*) FROM errata_actions WHERE kb_id='${KB}'`);
    log(`勘误：${left} 个任务在跑，记了 ${seen} 笔`);
    return left === 0 ? true : seen;
  }, 15000, 30 * 60000);
  const failed = num(`SELECT count(*) FROM jobs WHERE kind='errata_review' AND status='failed' AND payload->>'kb_id'='${KB}'`);
  if (failed) log(`注意：${failed} 个勘误任务失败（看 jobs.last_error）`);
  const count = (where) => num(`SELECT count(*) FROM errata_actions WHERE kb_id='${KB}' AND ${where}`);
  const out = {
    documents: num(`SELECT count(DISTINCT document_id) FROM errata_runs WHERE kb_id='${KB}'`),
    reviewed: count(`fact_id IS NOT NULL`),
    flagged: count(`flag IS NOT NULL`),
    retracted: count(`action='retract' AND status='applied'`),
    revised: count(`action='revise' AND status='applied'`),
    added: count(`action='add' AND status='applied'`),
    held: count(`status='held'`),
    refused: count(`status='refused'`),
    requests: num(`SELECT coalesce(sum(requests),0) FROM errata_runs WHERE kb_id='${KB}'`),
    prompt_tokens: num(`SELECT coalesce(sum(prompt_tokens),0) FROM errata_runs WHERE kb_id='${KB}'`),
    completion_tokens: num(`SELECT coalesce(sum(completion_tokens),0) FROM errata_runs WHERE kb_id='${KB}'`),
  };
  console.log(`勘误看了 ${out.documents} 篇 ${out.reviewed} 条（结构报的 ${out.flagged}）：撤 ${out.retracted}，改 ${out.revised}，加 ${out.added}，留给人 ${out.held}，拒 ${out.refused}；${out.requests} 次请求，token ${out.prompt_tokens}+${out.completion_tokens}`);
  return out;
}

// 撤掉的行里有多少是对的（0044 §7 的另一半：precision gained against correct facts removed）：
// 把勘误撤掉的类型化行交给裁判，按原文判 stated 的就是撤错的
async function judgeRetracted(KB) {
  const ep = judgeEndpoint(KB);
  const all = rows(`
    SELECT f.id, d.filename, s.canonical_name, r.label, coalesce(o.canonical_name, f.object_value->>'value', f.object_value#>>'{}', '')
      FROM errata_actions ea JOIN facts f ON f.id=ea.fact_id
      JOIN relation_types r ON r.id=f.predicate_id JOIN entities s ON s.id=f.subject_id LEFT JOIN entities o ON o.id=f.object_id
      JOIN documents d ON d.id=ea.document_id
     WHERE ea.kb_id='${KB}' AND ea.action IN ('retract','revise') AND ea.status='applied' ORDER BY f.id`);
  const text = Object.fromEntries(corpus.docs.map((d) => [d.filename, d.text]));
  const byFile = new Map();
  for (const r of all) (byFile.get(r[1]) || byFile.set(r[1], []).get(r[1])).push(r);
  const counts = { stated: 0, misworded: 0, not_stated: 0, unjudged: 0 };
  for (const [file, items] of byFile) {
    const list = items.map((r, i) => `${i}. ${r[2]} — ${r[3]} — ${r[4]}`).join("\n");
    let verdicts = {};
    try {
      const reply = await chat(ep, [{ role: "system", content: JUDGE }, { role: "user", content: `Document:\n${text[file]}\n\nFacts:\n${list}` }]);
      const m = reply.match(/\{[\s\S]*\}/);
      for (const r of (m ? JSON.parse(m[0]).results : [])) verdicts[r.i] = r.verdict;
    } catch (e) { log(`裁判失败 ${file}: ${String(e).slice(0, 120)}`); }
    items.forEach((_, i) => { counts[["stated", "misworded", "not_stated"].includes(verdicts[i]) ? verdicts[i] : "unjudged"] += 1; });
  }
  console.log(`撤改掉的 ${all.length} 条里裁判判 stated ${counts.stated}（撤错的），misworded ${counts.misworded}，not_stated ${counts.not_stated}（原型撤了 278 条，约四分之一是对的）`);
  return { removed: all.length, ...counts };
}

// ---- 打分 ----
const norm = (s) => String(s).toLowerCase().replace(/[\s ]+/g, " ").replace(/[.,;:!?"'()\[\]]/g, "").trim();
const years = (s) => new Set(String(s).match(/\b1\d{3}\b|\b20\d{2}\b/g) || []);
const nums = (s) => new Set((String(s).match(/\d[\d,]*\.?\d*/g) || []).map((x) => x.replace(/,/g, "")));
function valueMatches(gold, got) {
  const g = norm(gold), v = norm(got);
  if (!g || !v) return false;
  if (g === v || v.includes(g) || g.includes(v)) return true;
  const gy = years(g), vy = years(v);
  if (gy.size && vy.size && [...gy].some((y) => vy.has(y))) return true;
  const gn = nums(g), vn = nums(v);
  return gn.size > 0 && vn.size > 0 && [...gn].some((n) => vn.has(n));
}

// scope：'after' 是库现在的样子（勘误撤掉的不算、勘误加上的算），'before' 是勘误之前
// （撤掉的算回来、加上的不算）。0044 的门槛看勘误前，勘误的度量看两者之差
function score(KB, scope = "after") {
  // 每篇文档里的类型化事实：主语名集合、属性键、宾语名集合或值。
  // 名字之间用 ¦ 接：psql -A 的列分隔符是 |，名字里再用 | 会把列切错（第一轮就是这么错到 0 的）
  const rowsOf = rows(`
    SELECT d.filename, r.key,
           s.canonical_name || '¦' || coalesce((SELECT string_agg(n.object_value->>'value','¦') FROM facts n JOIN relation_types nr ON nr.id=n.predicate_id
                 WHERE n.subject_id=s.id AND nr.key='known_as' AND n.invalidated_at IS NULL AND n.object_value IS NOT NULL),''),
           coalesce(o.canonical_name || '¦' || coalesce((SELECT string_agg(n.object_value->>'value','¦') FROM facts n JOIN relation_types nr ON nr.id=n.predicate_id
                 WHERE n.subject_id=o.id AND nr.key='known_as' AND n.invalidated_at IS NULL AND n.object_value IS NOT NULL),''), ''),
           coalesce(f.object_value->>'value', f.object_value#>>'{}', ''),
           EXISTS (SELECT 1 FROM errata_actions ea WHERE ea.fact_id=f.id AND ea.action IN ('retract','revise') AND ea.status='applied') AS removed,
           -- 只算「只有勘误这一个来源」的行：加的事实撞上已有的行时 insert_fact_on 复用那一行，它仍是陈述算出来的
           (f.from_statement_id IS NULL AND NOT f.implied
              AND EXISTS (SELECT 1 FROM errata_actions ea WHERE ea.new_fact_id=f.id AND ea.status='applied')) AS added
      FROM facts f
      JOIN relation_types r ON r.id=f.predicate_id
      JOIN entities s ON s.id=f.subject_id
 LEFT JOIN entities o ON o.id=f.object_id
      JOIN LATERAL (SELECT document_id FROM fact_evidence WHERE fact_id=f.id LIMIT 1) ev ON true
      JOIN documents d ON d.id=ev.document_id
     WHERE f.kb_id='${KB}' AND f.layer='typed' AND NOT r.builtin
       AND (f.from_statement_id IS NOT NULL OR f.implied
            OR EXISTS (SELECT 1 FROM errata_actions ea WHERE ea.new_fact_id=f.id AND ea.status='applied'))
       AND (f.invalidated_at IS NULL
            OR EXISTS (SELECT 1 FROM errata_actions ea WHERE ea.fact_id=f.id AND ea.action IN ('retract','revise') AND ea.status='applied'))`
  );
  const typed = rowsOf
    .map(([file, key, subj, obj, val, removed, added]) => ({ file, key: key.toUpperCase(), subj: subj.split("¦").map(norm).filter(Boolean), obj: obj.split("¦").map(norm).filter(Boolean), val, removed: removed === "t", added: added === "t" }))
    .filter((t) => (scope === "before" ? !t.added : !t.removed));
  // 开放陈述连上的实体对（抽取那一层）
  const open = rows(`
    SELECT d.filename, s.canonical_name, coalesce(o.canonical_name, f.object_value->>'value', '')
      FROM facts f JOIN entities s ON s.id=f.subject_id LEFT JOIN entities o ON o.id=f.object_id
      JOIN LATERAL (SELECT document_id FROM fact_evidence WHERE fact_id=f.id LIMIT 1) ev ON true
      JOIN documents d ON d.id=ev.document_id
     WHERE f.kb_id='${KB}' AND f.layer='open' AND f.invalidated_at IS NULL`
  ).map(([file, s, o]) => ({ file, s: norm(s), o: norm(o) }));

  const byFile = new Map();
  for (const t of typed) (byFile.get(t.file) || byFile.set(t.file, []).get(t.file)).push(t);
  const openByFile = new Map();
  for (const t of open) (openByFile.get(t.file) || openByFile.set(t.file, []).get(t.file)).push(t);

  const perProp = {};
  let gold = 0, hit = 0, goldSame = 0, hitSame = 0, pairs = 0, pairHit = 0;
  const nameHit = (names, got) => names.some((n) => got.includes(norm(n)));
  for (const doc of truth.docs) {
    const facts = byFile.get(doc.filename) || [];
    const opens = openByFile.get(doc.filename) || [];
    for (const g of doc.facts) {
      gold += 1; if (g.same_sentence) goldSame += 1;
      const H = doc.entities[g.h], T = doc.entities[g.t];
      const pp = (perProp[g.r] ??= { gold: 0, hit: 0 }); pp.gold += 1;
      const ok = facts.some((f) => f.key === g.r && nameHit(H.names, f.subj)
        && (f.obj.length ? nameHit(T.names, f.obj) : T.names.some((n) => valueMatches(n, f.val))));
      if (ok) { hit += 1; pp.hit += 1; if (g.same_sentence) hitSame += 1; }
      pairs += 1;
      if (opens.some((o) => (nameHit(H.names, [o.s]) && nameHit(T.names, [o.o])) || (nameHit(T.names, [o.s]) && nameHit(H.names, [o.o])))) pairHit += 1;
    }
  }
  const pct = (a, b) => (b ? (100 * a / b).toFixed(1) + "%" : "-");
  const result = {
    label: LABEL, kb: KB, corpus: CORPUS, scope, at: new Date().toISOString(),
    typed_facts: typed.length, open_statements: open.length,
    gold, gold_recall: hit / (gold || 1), gold_recall_same_sentence: hitSame / (goldSame || 1),
    gold_recall_cross_sentence: (hit - hitSame) / ((gold - goldSame) || 1),
    entity_pair_recall: pairHit / (pairs || 1),
    bindings: { bound: num(`SELECT count(*) FROM phrase_bindings WHERE kb_id='${KB}' AND status='bound'`), none: num(`SELECT count(*) FROM phrase_bindings WHERE kb_id='${KB}' AND status='none'`), undecided: num(`SELECT count(*) FROM phrase_bindings WHERE kb_id='${KB}' AND status='undecided'`) },
    per_property: Object.fromEntries(Object.entries(perProp).sort((a, b) => b[1].gold - a[1].gold).map(([k, v]) => [k, { ...v, recall: v.hit / v.gold }])),
  };
  console.log(`\n=== ${LABEL} · 库 ${KB} ===`);
  console.log(`类型化事实 ${typed.length}，开放陈述 ${open.length}，绑定 bound ${result.bindings.bound} / none ${result.bindings.none} / undecided ${result.bindings.undecided}`);
  console.log(`gold recall ${pct(hit, gold)}（同句 ${pct(hitSame, goldSame)}，跨句 ${pct(hit - hitSame, gold - goldSame)}）· entity-pair recall ${pct(pairHit, pairs)}`);
  console.log(`0044 的门槛只看同句那一半：完整原型的同句召回是它的对照，两轮各报一次`);
  return result;
}

// ---- 裁判：抽样判类型化事实是不是原文说的 ----
function judgeEndpoint(KB) {
  if (process.env.BENCH_JUDGE_BASE) return { base: process.env.BENCH_JUDGE_BASE, key: process.env.BENCH_JUDGE_KEY || "", model: process.env.BENCH_JUDGE_MODEL || "" };
  const [base, key, model] = psql(`SELECT s.chat_base_url, s.chat_api_key, s.chat_model FROM llm_settings s JOIN knowledge_bases k ON k.workspace_id=s.workspace_id WHERE k.id='${KB}'`).split("|");
  if (!base || !model) throw new Error("工作区没配对话模型，也没给 BENCH_JUDGE_*");
  log("裁判与抽取是同一个模型，数字要打折看（judge_open.mjs 同一条提醒）");
  return { base, key, model };
}
async function chat(ep, messages) {
  const r = await fetch(`${ep.base.replace(/\/$/, "")}/chat/completions`, {
    method: "POST", headers: { "content-type": "application/json", ...(ep.key ? { authorization: `Bearer ${ep.key}` } : {}) },
    body: JSON.stringify({ model: ep.model, temperature: 0, messages }),
  });
  if (!r.ok) throw new Error(`judge -> ${r.status} ${(await r.text()).slice(0, 200)}`);
  const j = await r.json();
  return j.choices?.[0]?.message?.content ?? "";
}
const JUDGE = `You check facts extracted from a document. Each numbered fact says that a subject stands in a named relation to an object (a thing or a value). Judge only from the document text given.
- "stated": the document states this, or a careful reader takes it directly from the document, and the relation is the right one;
- "misworded": the document does relate this subject and object, but not by this relation (wrong relation, wrong direction);
- "not_stated": the document does not relate this subject and this object at all.
Output one JSON object: {"results":[{"i":0,"verdict":"stated|misworded|not_stated"}]}`;

async function judge(KB, n, seed) {
  const ep = judgeEndpoint(KB);
  const all = rows(`
    SELECT f.id, d.filename, s.canonical_name, r.label, coalesce(o.canonical_name, f.object_value->>'value', f.object_value#>>'{}', '')
      FROM facts f JOIN relation_types r ON r.id=f.predicate_id JOIN entities s ON s.id=f.subject_id LEFT JOIN entities o ON o.id=f.object_id
      JOIN LATERAL (SELECT document_id FROM fact_evidence WHERE fact_id=f.id LIMIT 1) ev ON true JOIN documents d ON d.id=ev.document_id
     WHERE f.kb_id='${KB}' AND f.layer='typed' AND NOT r.builtin AND f.invalidated_at IS NULL
       AND (f.from_statement_id IS NOT NULL OR f.implied
            OR EXISTS (SELECT 1 FROM errata_actions ea WHERE ea.new_fact_id=f.id AND ea.status='applied')) ORDER BY f.id`);
  // 可复现抽样
  let a = (seed >>> 0) || 1; const rnd = () => { a = (a * 1103515245 + 12345) >>> 0; return a / 4294967296; };
  const sample = all.map((r) => [rnd(), r]).sort((x, y) => x[0] - y[0]).slice(0, n).map(([, r]) => r);
  const text = Object.fromEntries(corpus.docs.map((d) => [d.filename, d.text]));
  const byFile = new Map();
  for (const r of sample) (byFile.get(r[1]) || byFile.set(r[1], []).get(r[1])).push(r);
  const counts = { stated: 0, misworded: 0, not_stated: 0, unjudged: 0 };
  for (const [file, items] of byFile) {
    const list = items.map((r, i) => `${i}. ${r[2]} — ${r[3]} — ${r[4]}`).join("\n");
    let verdicts = {};
    try {
      const reply = await chat(ep, [{ role: "system", content: JUDGE }, { role: "user", content: `Document:\n${text[file]}\n\nFacts:\n${list}` }]);
      const m = reply.match(/\{[\s\S]*\}/);
      for (const r of (m ? JSON.parse(m[0]).results : [])) verdicts[r.i] = r.verdict;
    } catch (e) { log(`裁判失败 ${file}: ${String(e).slice(0, 120)}`); }
    items.forEach((_, i) => { counts[["stated", "misworded", "not_stated"].includes(verdicts[i]) ? verdicts[i] : "unjudged"] += 1; });
  }
  const judged = counts.stated + counts.misworded + counts.not_stated;
  console.log(`裁判 ${sample.length} 条（判了 ${judged}）：stated ${counts.stated}，misworded ${counts.misworded}，not_stated ${counts.not_stated} → judged precision ${judged ? (100 * counts.stated / judged).toFixed(1) + "%" : "-"}（原型勘误前 75.3% 是门槛）`);
  return { sampled: sample.length, ...counts, judged_precision: judged ? counts.stated / judged : null };
}

// ---- 主流程 ----
const started = Date.now();
let KB = args.kb;
if (!KB) {
  KB = await setup();
  await ingest(KB);
  if (args["dry-run"]) { log("干跑到此为止：库、本体、语料都在，没抽取"); console.log(JSON.stringify({ kb: KB, dry_run: true })); process.exit(0); }
  await extract(KB);
  await align(KB);
} else {
  await login();
}
let rules = null;
if (args["approve-rules"]) rules = await approveRules(KB);
// 勘误前的分：已经跑过勘误的库也能按 scope 算回来（撤掉的算回来、加上的不算）
const result = score(KB, "before");
if (rules) result.rules = rules;
if (args.judge) result.judge = await judge(KB, Number(args.judge) || 200, Number(args.seed || 1));
if (args.errata) {
  // 勘误前的分留着，勘误后再打一次：0044 §7 的度量是两份分的差，与撤错了多少
  result.before_errata = { gold_recall: result.gold_recall, gold_recall_same_sentence: result.gold_recall_same_sentence, typed_facts: result.typed_facts, judge: result.judge };
  result.errata = await errata(KB);
  const after = score(KB, "after");
  result.after_errata = { gold_recall: after.gold_recall, gold_recall_same_sentence: after.gold_recall_same_sentence, typed_facts: after.typed_facts };
  if (args.judge) {
    result.after_errata.judge = await judge(KB, Number(args.judge) || 200, Number(args.seed || 1));
    result.errata.removed = await judgeRetracted(KB);
  }
}
result.minutes = Math.round((Date.now() - started) / 60000);
fs.writeFileSync(OUT, JSON.stringify(result, null, 1));
console.log(`结果写到 ${OUT}（${result.minutes} 分钟）`);
