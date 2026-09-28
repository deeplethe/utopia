#!/usr/bin/env node
// 时间词的测量台（ADR 0064）：写好的文档里每句话带什么时间是已知的，量
//   - 这句话抽出来了没有；它的时间说法记成时间词了没有；
//   - 起点算对了没有（落在期望的区间里）；没写时间的句子有没有被硬给一个起点；
//   - 「截至」对不对（0064 第 3 刀之前这一栏是 0）；
//   - 同一份文档跑几遍，每句话的结果一不一样；
//   - token（服务端日志的 llm usage 行）。
//
//   BENCH_BASE=http://127.0.0.1:1524 BENCH_PSQL="docker exec ... -d utopia_bench4 -tAc" \
//   BENCH_SERVER_LOG=/path/to/server.log \
//   node scripts/bench/timewords.mjs --db utopia_bench4 [--runs 3] [--out file.json]
//
// 每一遍建一个新库；不删任何东西。
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { BASE, api, login, cookieHeader, parseArgs, log, onDb, sleep } from "./lib.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv);
const DB = args.db;
const RUNS = Number(args.runs || 3);
const LOG = process.env.BENCH_SERVER_LOG;
if (!DB) { console.error("--db <bench database> 是必须的"); process.exit(2); }
const corpus = JSON.parse(fs.readFileSync(path.join(HERE, "truth", "timewords.json"), "utf8"));
const q = (sql) => onDb(DB, sql);
const esc = (s) => String(s).replace(/'/g, "''");

async function run(n) {
  const ws = (await api("GET", "/api/v1/workspaces"))[0];
  const kb = await api("POST", `/api/v1/workspaces/${ws.id}/kbs`, { name: `timewords-${Date.now()}-${n}`, ontology_packs: [] });
  const KB = kb.id;
  const from = LOG ? fs.statSync(LOG).size : 0;
  for (const d of corpus.docs) {
    const fd = new FormData();
    fd.append("files", new Blob([d.text], { type: "text/markdown" }), d.filename);
    const r = await fetch(`${BASE}/api/v1/kbs/${KB}/documents`, { method: "POST", headers: { cookie: cookieHeader() }, body: fd });
    if (!r.ok) throw new Error(`upload ${d.filename} -> ${r.status} ${(await r.text()).slice(0, 200)}`);
  }
  for (let i = 0; i < 60; i++) { await sleep(3000); if (q(`SELECT count(*) FROM documents WHERE kb_id='${KB}' AND status<>'ready'`) === "0") break; }
  // 管线解析完自己排抽取；手动抽取是强制全量，只补没排上的，否则每篇抽两遍
  for (const d of (await api("GET", `/api/v1/kbs/${KB}/documents?limit=50`)).docs) {
    if (!["queued", "extracting", "done"].includes(d.graph_status)) await api("POST", `/api/v1/documents/${d.id}/extract`, {});
  }
  for (let i = 0; i < 240; i++) {
    await sleep(5000);
    const pending = q(`SELECT count(*) FROM jobs WHERE status IN ('queued','running') AND kind IN ('extract_document','resolve_time','process_document') AND (payload->>'kb_id'='${KB}' OR payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id='${KB}'))`);
    if (pending === "0" && i > 3) break;
  }
  // 每句话：这篇文档里出处引文含 key 的开放陈述
  const rows = [];
  for (const d of corpus.docs) for (const e of d.expect) {
    const facts = JSON.parse(q(`SELECT coalesce(json_agg(json_build_object(
        'from', f.valid_from, 'to', f.valid_to, 'grade', f.valid_from_grade,
        'attested', f.attested_from, 'recorded', f.recorded_at,
        'mentions', (SELECT count(*) FROM time_mentions m WHERE m.fact_id = f.id),
        'phrase', f.phrase)), '[]')
      FROM facts f JOIN fact_evidence fe ON fe.fact_id = f.id JOIN documents doc ON doc.id = fe.document_id
     WHERE f.kb_id='${KB}' AND f.layer='open' AND f.invalidated_at IS NULL
       AND doc.filename='${esc(d.filename)}' AND fe.quote LIKE '%${esc(e.key)}%'`));
    const day = (t) => (t ? String(t).slice(0, 10) : null);
    const within = (t, range) => !!t && !!range && day(t) >= range[0] && day(t) <= range[1];
    const timed = e.kind !== "none";
    const found = facts.length > 0;
    const recorded = facts.some((f) => f.mentions > 0);
    const dated = facts.some((f) => f.from);
    // 期望没有起点的（没写时间、或起算的事件没日期）：给了起点就是错
    const right = !found ? false
      : e.from ? facts.some((f) => within(f.from, e.from))
      : !dated;
    // 「截至」：锚点落在期望的区间里，而且不是处理文档的那一刻
    const asOf = e.as_of ? facts.some((f) => within(f.attested, e.as_of) && day(f.attested) !== day(f.recorded)) : null;
    rows.push({ doc: d.filename, key: e.key, kind: e.kind, timed, found, recorded, dated, right, asOf,
      got: facts.map((f) => `${day(f.from) ?? "-"}${f.grade ? "/" + f.grade : ""}`).join(",") });
  }
  let tokens = null;
  if (LOG) {
    // eslint-disable-next-line no-control-regex
    const text = fs.readFileSync(LOG).subarray(from).toString("utf8").replace(/\x1b\[[0-9;]*m/g, "");
    const usage = [...text.matchAll(/llm usage .*prompt=(\d+) completion=(\d+)/g)];
    tokens = { calls: usage.length, prompt: usage.reduce((a, m) => a + +m[1], 0), completion: usage.reduce((a, m) => a + +m[2], 0) };
  }
  const chunks = Number(q(`SELECT count(*) FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.kb_id='${KB}' AND c.superseded_at IS NULL`));
  const statements = Number(q(`SELECT count(*) FROM facts WHERE kb_id='${KB}' AND layer='open' AND invalidated_at IS NULL`));
  return { kb: KB, rows, tokens, chunks, statements };
}

async function main() {
  await login();
  const runs = [];
  for (let n = 1; n <= RUNS; n++) { log(`run ${n} of ${RUNS}`); runs.push(await run(n)); }
  const pct = (a, b) => (b ? `${a}/${b} (${Math.round((100 * a) / b)}%)` : "–");
  const col = (f) => runs.map((r) => String(f(r)).padStart(16)).join(" ");
  const line = (label, f) => console.log(`${label.padEnd(52)} ${col(f)}`);
  console.log(`${"".padEnd(52)} ${runs.map((_, i) => `run ${i + 1}`.padStart(16)).join(" ")}`);
  line("statements in the base", (r) => r.statements);
  line("sentences found (any statement from the sentence)", (r) => pct(r.rows.filter((x) => x.found).length, r.rows.length));
  line("time expressions recorded as time words", (r) => pct(r.rows.filter((x) => x.timed && x.recorded).length, r.rows.filter((x) => x.timed).length));
  for (const kind of ["absolute", "relative_now", "relative_event", "section"])
    line(`  start right: ${kind}`, (r) => pct(r.rows.filter((x) => x.kind === kind && x.right).length, r.rows.filter((x) => x.kind === kind).length));
  line("  undated event as anchor: no start written", (r) => pct(r.rows.filter((x) => x.kind === "relative_undated_event" && x.right).length, r.rows.filter((x) => x.kind === "relative_undated_event").length));
  line("no time words: no start written", (r) => pct(r.rows.filter((x) => x.kind === "none" && x.found && !x.dated).length, r.rows.filter((x) => x.kind === "none" && x.found).length));
  line("no time words: as-of right", (r) => pct(r.rows.filter((x) => x.asOf === true).length, r.rows.filter((x) => x.asOf !== null).length));
  if (LOG) {
    line("model calls", (r) => r.tokens.calls);
    line("tokens (prompt + completion)", (r) => r.tokens.prompt + r.tokens.completion);
    line("tokens per chunk", (r) => Math.round((r.tokens.prompt + r.tokens.completion) / Math.max(1, r.chunks)));
  }
  // 几遍之间一不一样：每句话的（抽到、记下、对）三样都相同才算一致
  const keys = runs[0].rows.map((x) => `${x.doc}|${x.key}`);
  const same = keys.filter((k, i) => runs.every((r) => ["found", "recorded", "right"].every((f) => r.rows[i][f] === runs[0].rows[i][f]))).length;
  console.log(`${"sentences with the same outcome in every run".padEnd(52)} ${pct(same, keys.length)}`);
  console.log("\nper sentence (runs left to right; ✓ start right, · not):");
  keys.forEach((k, i) => console.log(`  ${runs.map((r) => (r.rows[i].right ? "✓" : "·") + (r.rows[i].recorded ? "w" : "-") + (r.rows[i].found ? "" : "?")).join(" ")}  [${runs[0].rows[i].kind}] ${k}  → ${runs.map((r) => r.rows[i].got || "none").join(" | ")}`));
  if (args.out) fs.writeFileSync(args.out, JSON.stringify(runs, null, 1));
}
main().catch((e) => { console.error(e); process.exit(1); });
