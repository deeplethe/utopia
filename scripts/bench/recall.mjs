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
//   node scripts/bench/recall.mjs --kb <id> --known empty --out runs/588   # 记一轮，写成 JSON
//   node scripts/bench/recall.mjs --table runs/588                        # 把记下的几轮排成表
//
// `--known shown|empty` 只是**标签**：提示词里带不带前面分块认下的实体，由服务端的
// `UTOPIA_EXTRACT_KNOWN_IN_PROMPT` 决定（#588），起服务时定死，台子改不了也查不到。
// 标签写进记录，`--table` 按它分组；与服务端对不上，那一轮的数就是错的。
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
if (args.table) {
  table(args.table);
  process.exit(0);
}
if (args.known !== undefined && !["shown", "empty"].includes(args.known)) {
  console.error("--known 只认 shown 或 empty（与服务端的 UTOPIA_EXTRACT_KNOWN_IN_PROMPT 对上）");
  process.exit(1);
}
if (args.out && !args.known) {
  console.error("--out 要带 --known shown|empty：不标清这一轮提示词里有没有 known，记下来的数没法分组");
  process.exit(1);
}
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
                      WHERE fq.fact_id = f.id), '')
           -- 开放陈述的限定（#729）按文档自己的角色词挂着：金额、职务在这里
           || ' ' || coalesce((SELECT string_agg(sq.role || '=' || coalesce(sq.value #>> '{}', qe.canonical_name, ''), ' ')
                       FROM statement_qualifiers sq LEFT JOIN entities qe ON qe.id = sq.entity_id
                      WHERE sq.fact_id = f.id), '')
           -- 开放陈述的时间是照抄的字（time_mentions），不在 valid_from 里：日期类真值靠它命中
           || ' ' || coalesce((SELECT string_agg(tm.text, ' ') FROM time_mentions tm WHERE tm.fact_id = f.id), ''))
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

  // 去重后的 (主, 谓, 宾)：两轮之间比的是「抽出来的是不是同一批」，不只是个数
  const sigs = {};
  for (const [doc, facts] of byDoc) sigs[doc] = [...new Set(facts.map((f) => `${norm(f.subj)}\u001f${norm(f.pred)}\u001f${norm(f.obj)}`))].sort();
  return {
    pass,
    total: truth.length,
    perDoc: Object.fromEntries([...perDoc].map(([k, v]) => [k, v.pass])),
    misses: misses.map((m) => m.id),
    sigs,
  };
}

// 每篇文档的形状：分块、事实、实体、跨块认回的实体、丢弃账（extraction_drops），
// 以及排队到最后一块抽完的秒数（给了 queuedAt 才有）。
// 「跨块认回」= 一个实体出现在这篇文档**两个以上分块**的事实里：前面的块认下、后面的块接着用，
// known 管的就是这一件（名字认回也算在内——那半步不受开关管，差出来的是提示词的份）
// 「排给裁决的对」= 这一轮进了 resolution_reviews 的疑似同一对，不分是哪条召回通道提的：
// 提示词里没有 known，后面的块更容易换个写法再列一遍同一个东西，这种不会静默合并（#877），
// 而是在这里多排一对——召回分不动、这个数涨，就是空 known 的代价。一对算进两边实体出现的每篇。
// **不按 `name_vector|` 筛**：一对一旦裁了（治理、升级给人、随合并作废），reason 就被裁决改写
// （`governed|…` 等），原来是哪条通道提的不留痕；按它筛，一轮跑完几乎总是 0
function stats(queuedAt) {
  const rows = psql(`
    WITH live AS (
      SELECT c.id, c.document_id, c.extracted_at FROM chunks c JOIN documents d ON d.id = c.document_id
       WHERE d.kb_id = '${KB}' AND c.superseded_at IS NULL),
    ev AS (
      SELECT l.document_id, fe.chunk_id, f.id AS fact_id, f.subject_id, f.object_id
        FROM fact_evidence fe JOIN live l ON l.id = fe.chunk_id JOIN facts f ON f.id = fe.fact_id
       WHERE f.kb_id = '${KB}' AND f.invalidated_at IS NULL),
    ents AS (
      SELECT document_id, e, count(DISTINCT chunk_id) AS n FROM (
        SELECT document_id, chunk_id, subject_id AS e FROM ev
        UNION ALL SELECT document_id, chunk_id, object_id FROM ev WHERE object_id IS NOT NULL) x
       GROUP BY 1, 2)
    SELECT concat_ws(chr(31), d.filename,
      (SELECT count(*) FROM live WHERE document_id = d.id),
      (SELECT count(*) FROM live WHERE document_id = d.id AND extracted_at IS NOT NULL),
      (SELECT count(DISTINCT fact_id) FROM ev WHERE document_id = d.id),
      (SELECT count(*) FROM ents WHERE document_id = d.id),
      (SELECT count(*) FROM ents WHERE document_id = d.id AND n > 1),
      (SELECT count(*) FROM resolution_reviews r
        WHERE r.kb_id = '${KB}'${queuedAt ? ` AND r.created_at >= '${queuedAt}'::timestamptz` : ""}
          AND EXISTS (SELECT 1 FROM ents WHERE document_id = d.id AND e IN (r.left_id, r.right_id))),
      coalesce((SELECT string_agg(reason || '=' || s, ' ' ORDER BY reason) FROM
        (SELECT reason, sum(count) AS s FROM extraction_drops WHERE document_id = d.id GROUP BY reason) r), ''),
      ${queuedAt ? `coalesce((SELECT round(extract(epoch FROM max(extracted_at) - '${queuedAt}'::timestamptz))::text FROM live WHERE document_id = d.id), '')` : "''"})
    FROM documents d WHERE d.kb_id = '${KB}' AND d.filename IN (${DOCS.map((x) => `'${x}.html'`).join(",")})
    ORDER BY d.filename`)
    .split("\n")
    .filter(Boolean);
  const out = {};
  for (const l of rows) {
    const [file, chunks, extracted, facts, entities, carried, pairs, drops, secs] = l.split("\u001f");
    out[file.replace(/\.(html|md|pdf)$/i, "")] = {
      chunks: +chunks,
      extracted: +extracted,
      facts: +facts,
      entities: +entities,
      carried: +carried,
      pairs: +pairs,
      drops: Object.fromEntries(drops.split(" ").filter(Boolean).map((kv) => [kv.slice(0, kv.lastIndexOf("=")), +kv.slice(kv.lastIndexOf("=") + 1)])),
      seconds: secs === "" ? null : +secs,
    };
  }
  console.log("\n每篇：块（已抽）/ 事实 / 实体 / 跨块认回 / 排给裁决的对 / 秒");
  for (const [doc, v] of Object.entries(out)) {
    console.log(`  ${doc}  ${v.chunks}(${v.extracted}) / ${v.facts} / ${v.entities} / ${v.carried} / ${v.pairs} / ${v.seconds ?? "-"}`);
    const d = Object.entries(v.drops);
    if (d.length) console.log(`    丢弃 ${d.map(([k, n]) => `${k}=${n}`).join(" ")}`);
  }
  return out;
}

// 全库排给裁决的对，按裁决到哪一步分开：一对可能跨两篇，按篇加起来会重，总数看这里。
// 另列现在的 reason 前缀（还待裁的仍是 `name_vector|` / `contains|` 这些通道名，裁过的是裁决写的）。
// 轮内裁决器可能已经判了一部分（merged / kept），所以三种状态都算——排过这一对就是代价。
// 只算这一轮排的（created_at 不早于排队那一刻）：每轮开头删实体会连带删掉这些对（外键 CASCADE），
// 但重抽本身不清它们，不靠那条连带也不会把上一轮的对算进来。--score 没有排队时刻，算的是库里现有的
function pairs(queuedAt) {
  const scope = `kb_id = '${KB}' ${queuedAt ? `AND created_at >= '${queuedAt}'::timestamptz` : ""}`;
  const [total, pending, merged, kept] = psql(`
    SELECT concat_ws(chr(31), count(*), count(*) FILTER (WHERE status = 'pending'),
           count(*) FILTER (WHERE status = 'merged'), count(*) FILTER (WHERE status = 'kept'))
      FROM resolution_reviews WHERE ${scope}`)
    .split("\u001f")
    .map(Number);
  const byReason = Object.fromEntries(
    psql(`SELECT k || '=' || n FROM (SELECT split_part(coalesce(reason, '-'), '|', 1) AS k, count(*) AS n
           FROM resolution_reviews WHERE ${scope} GROUP BY 1) x ORDER BY k`)
      .split("\n")
      .filter(Boolean)
      .map((kv) => [kv.slice(0, kv.lastIndexOf("=")), +kv.slice(kv.lastIndexOf("=") + 1)]),
  );
  console.log(`\n排给裁决的对 ${total}：待裁 ${pending} / 合并 ${merged} / 分开 ${kept}`);
  console.log(`  现在的 reason：${Object.entries(byReason).map(([k, n]) => `${k}=${n}`).join(" ") || "-"}`);
  return { total, pending, merged, kept, byReason };
}

function record(result, perDoc, extra) {
  if (!args.out) return;
  fs.mkdirSync(args.out, { recursive: true });
  let sha = "";
  try {
    sha = execFileSync("git", ["rev-parse", "--short", "HEAD"], { cwd: HERE, encoding: "utf8" }).trim();
  } catch {}
  const at = new Date().toISOString();
  const file = path.join(args.out, `recall-${at.replace(/[:.]/g, "-")}-${args.known}.json`);
  fs.writeFileSync(
    file,
    JSON.stringify({ at, sha, kb: KB, known: args.known, label: args.label ?? null, ...extra, score: result, docs: perDoc }, null, 1),
  );
  console.log(`\n记下 ${file}`);
}

// 把 --out 记下的几轮按 known 分组排成表：每组的均值 ± 标准差，
// 以及事实集合的重合度——组内两两比（运行间方差本身）与跨组两两比（known 的影响）。
// 跨组的重合度落在组内的范围里，就是说空 known 挪动的没超出模型自己的抖动
function table(dir) {
  const runs = fs
    .readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .map((f) => JSON.parse(fs.readFileSync(path.join(dir, f), "utf8")));
  if (!runs.length) {
    console.error(`${dir} 里没有记录`);
    process.exit(1);
  }
  const groups = ["shown", "empty"].map((k) => [k, runs.filter((r) => r.known === k)]).filter(([, rs]) => rs.length);
  const ms = (xs) => {
    const v = xs.filter((x) => x !== null && x !== undefined);
    if (!v.length) return "-";
    const m = v.reduce((a, b) => a + b, 0) / v.length;
    const sd = v.length > 1 ? Math.sqrt(v.reduce((a, b) => a + (b - m) ** 2, 0) / (v.length - 1)) : 0;
    return `${m.toFixed(1)} ± ${sd.toFixed(1)}`;
  };
  const docs = [...new Set(runs.flatMap((r) => Object.keys(r.docs)))].sort();
  const cols = groups.map(([k, rs]) => `known=${k} (n=${rs.length})`);
  const row = (name, f) => `| ${name} | ${groups.map(([, rs]) => ms(rs.map(f))).join(" | ")} |`;
  const lines = [`| | ${cols.join(" | ")} |`, `| --- |${cols.map(() => " --- |").join("")}`];
  lines.push(row("总分（/52）", (r) => r.score.pass));
  lines.push(row("抽完后等收尾（秒）", (r) => r.settleSeconds));
  lines.push(row("排给裁决的对（全库）", (r) => r.pairs?.total));
  lines.push(row("其中合并的", (r) => r.pairs?.merged));
  for (const doc of docs) {
    const d = (r) => r.docs[doc] ?? {};
    lines.push(row(`${doc} 事实`, (r) => d(r).facts));
    lines.push(row(`${doc} 实体`, (r) => d(r).entities));
    lines.push(row(`${doc} 跨块认回`, (r) => d(r).carried));
    lines.push(row(`${doc} 排给裁决的对`, (r) => d(r).pairs));
    lines.push(row(`${doc} 丢弃`, (r) => Object.values(d(r).drops ?? {}).reduce((a, b) => a + b, 0)));
    lines.push(row(`${doc} 秒`, (r) => d(r).seconds));
  }
  console.log(lines.join("\n"));
  // 没等到收尾就记下的轮次（卡住十五分钟才停）：数是快照，单独点名，别混进均值里看不出来
  const unsettled = runs.filter((r) => r.settled === false);
  if (unsettled.length) console.log(`\n没等到收尾的轮次：${unsettled.map((r) => `${r.at}（${r.known}）`).join("、")}`);

  // 事实集合的 Jaccard：每篇文档各算，再平均
  const jac = (a, b) => {
    const xs = docs.map((doc) => {
      const A = new Set(a.score.sigs?.[doc] ?? []), B = new Set(b.score.sigs?.[doc] ?? []);
      const inter = [...A].filter((x) => B.has(x)).length;
      const uni = new Set([...A, ...B]).size;
      return uni ? inter / uni : 1;
    });
    return xs.reduce((s, x) => s + x, 0) / xs.length;
  };
  const pairs = (xs, ys) => (ys ? xs.flatMap((a) => ys.map((b) => jac(a, b))) : xs.flatMap((a, i) => xs.slice(i + 1).map((b) => jac(a, b))));
  const fmt = (v) => (v.length ? `${ms(v.map((x) => x * 100))}%（${v.length} 对）` : "-");
  console.log("\n事实集合重合度（Jaccard，按文档平均）：");
  for (const [k, rs] of groups) console.log(`  组内 known=${k}：${fmt(pairs(rs))}`);
  if (groups.length === 2) console.log(`  跨组：${fmt(pairs(groups[0][1], groups[1][1]))}`);
}

if (args.score) {
  record(score(), stats(null), { pairs: pairs(null) });
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
// 没装本体包的库（开放图谱那条路不需要本体）：没有基准时刻，也就没有要删的
if (packTs) psql(`DELETE FROM relation_types WHERE kb_id='${KB}' AND created_at > '${packTs}'::timestamptz + interval '1 second'`);
// 会在抽取之后改本体、增派生事实的开关关掉，两轮的本体与打分口径才一样。治理开着：
// 库生下来就开着它（0050），量的是产品本来的样子
psql(`UPDATE knowledge_bases SET auto_extend_ontology=FALSE, auto_type_resolution=FALSE, materialize_inferences=FALSE, governance=TRUE WHERE id='${KB}'`);
psql(`UPDATE chunks SET extracted_at=NULL WHERE document_id IN (SELECT id FROM documents WHERE kb_id='${KB}' AND filename IN (${names}))`);
console.log(`本体 ${num(`SELECT count(*) FROM relation_types WHERE kb_id='${KB}'`)} 个关系 / ${num(`SELECT count(*) FROM entity_types WHERE kb_id='${KB}'`)} 个类`);

const docs = (await api("GET", `/api/v1/kbs/${KB}/documents?limit=200`)).docs.filter((d) => want.includes(d.filename));
const endpoint = args.reprocess ? "reprocess" : "extract";
// 墙钟从这一刻算到每篇最后一块抽完：取库的时钟，与 extracted_at 同一个钟
const queuedAt = psql("SELECT now()::text");
if (args.known) console.log(`这一轮标 known=${args.known}：服务端的 UTOPIA_EXTRACT_KNOWN_IN_PROMPT 得对得上`);
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

// **抽完不等于这一轮完了**：对齐、治理、时间消解、裁决还在后头跑，一边补事实一边裁对子
// （同一篇财报，抽完那一刻 552 条事实，一小时后 565）。不等它们收尾，每轮记下的就是
// 「抽完那一刻」各不相同的快照，两组之间多出一份与 known 无关的抖动。
// 等到没有该跑的任务：running 的，和 run_at 一分钟之内到点的 queued（对齐排队时带几秒防抖，
// 前一个没完的会带延迟重排；更远的定时任务不算这一轮的）。进展看跑完的任务数，规矩同上：慢不算超时，卡住才算。
// 秒数那一栏读的是 extracted_at，不受这里等多久影响
const pendingJobs = `SELECT count(*) FROM jobs WHERE status = 'running'
                       OR (status = 'queued' AND run_at <= now() + interval '1 minute')`;
const settleFrom = Date.now();
let settled = false;
last = -1;
stall = 0;
for (;;) {
  const left = num(pendingJobs);
  const finished = num(`SELECT count(*) FROM jobs WHERE status IN ('done','failed')`);
  if (left === 0) {
    settled = true;
    break;
  }
  const kinds = psql(`SELECT string_agg(kind || '×' || n, ' ' ORDER BY kind) FROM
    (SELECT kind, count(*) AS n FROM jobs WHERE status = 'running'
        OR (status = 'queued' AND run_at <= now() + interval '1 minute') GROUP BY kind) k`);
  console.log(`${stamp()} 等收尾：还有 ${left} 个任务（${kinds}）`);
  if (finished === last) {
    if (++stall > 30) { console.log("十五分钟没有任务跑完，不等了：这一轮记下的不是收尾之后的数"); break; }
  } else stall = 0;
  last = finished;
  await sleep(30000);
}
const settleSeconds = Math.round((Date.now() - settleFrom) / 1000);
if (settled) console.log(`${stamp()} 收尾完了（抽完之后又等了 ${settleSeconds} 秒）`);

record(score(), stats(queuedAt), { endpoint, pairs: pairs(queuedAt), settled, settleSeconds });
