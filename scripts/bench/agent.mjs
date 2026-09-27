#!/usr/bin/env node
// 本体代理的 A/B 测量台（ADR 0061）：同一个库状态、同一批形状，各臂跑一轮，量
//   - 失败：调用失败或读不出的批次、坏项、模型提了已有键；
//   - 重复：提案其实是已有属性（或它的反向）——裁词表的主要风险，裁判拿着全词表判；
//   - 「已有」答案的准确率：形状真的说的是那条属性、方向对不对；
//   - token（服务端日志的 llm usage 行）。
//
//   BENCH_BASE=http://127.0.0.1:1524 BENCH_PSQL="docker exec ... -d utopia_bench4 -tAc" \
//   BENCH_SERVER_LOG=/path/to/server.log BENCH_JUDGE_BASE=... BENCH_JUDGE_KEY=... BENCH_JUDGE_MODEL=... \
//   node scripts/bench/agent.mjs --kb <kb-id> --db utopia_bench4 --reset [--arms trimmed,full] [--out file.json]
//
// **--reset 会删这个库里代理的提案（没采纳的）和看过的形状记录**，每臂开跑前各删一次，好让两臂
// 看到同一批形状。只在测量库上用；不给 --reset 就不跑。
import fs from "node:fs";
import { api, login, parseArgs, log, onDb, sleep } from "./lib.mjs";

const args = parseArgs(process.argv);
const KB = args.kb, DB = args.db;
const ARMS = String(args.arms || "trimmed,full").split(",").map((s) => s.trim()).filter(Boolean);
const LOG = process.env.BENCH_SERVER_LOG;
if (!KB || !DB) { console.error("--kb <id> 与 --db <bench database> 是必须的"); process.exit(2); }
if (!args.reset) { console.error("这台子要重置代理的提案和记录才能让两臂看到同一批形状：确认是测量库后加 --reset"); process.exit(2); }
if (!LOG) { console.error("给 BENCH_SERVER_LOG：失败数和 token 从服务端日志里读"); process.exit(2); }
const q = (sql) => onDb(DB, sql);

const ep = { base: process.env.BENCH_JUDGE_BASE, key: process.env.BENCH_JUDGE_KEY || "", model: process.env.BENCH_JUDGE_MODEL };
if (!ep.base || !ep.model) { console.error("给 BENCH_JUDGE_BASE / _KEY / _MODEL"); process.exit(2); }
async function judge(system, user) {
  for (let attempt = 0; attempt < 3; attempt++) {
    const r = await fetch(`${ep.base.replace(/\/$/, "")}/chat/completions`, {
      method: "POST", headers: { "content-type": "application/json", ...(ep.key ? { authorization: `Bearer ${ep.key}` } : {}) },
      body: JSON.stringify({ model: ep.model, temperature: 0, messages: [{ role: "system", content: system }, { role: "user", content: user }] }),
    });
    if (r.ok) {
      const text = (await r.json()).choices?.[0]?.message?.content ?? "";
      const m = /\{[\s\S]*\}/.exec(text);
      if (m) { try { return JSON.parse(m[0]); } catch { /* 再问一次 */ } }
    } else if (r.status !== 429 && r.status < 500) throw new Error(`judge -> ${r.status} ${(await r.text()).slice(0, 200)}`);
    await sleep(3000 * (attempt + 1));
  }
  return null;
}

const DUP = `You review proposals to extend an ontology. You are given the full ontology (every class and property with its definition) and a list of proposed new elements. For each proposal decide whether the ontology ALREADY has it:
- "duplicate": an existing property or class means the same thing (name it);
- "inverse": an existing property is the same relation read in the other direction (name it);
- "new": nothing existing covers it.
Be strict about meaning, not wording. Output one JSON object: {"results":[{"i":0,"verdict":"duplicate|inverse|new","of":"existing_key or null"}]}`;
const EXIST = `You check decisions that a statement shape is an instance of an existing ontology property. Each item gives a phrase between a subject class and an object class (or a value), one example statement, the property with its definition, and the direction claimed: forward = the statement's subject is the property's subject; reverse = the statement reads the property backwards.
- "right": the phrase asserts this property in this direction;
- "wrong_direction": the property is right but the direction is not;
- "wrong": the phrase does not assert this property (too vague, or a different meaning).
Output one JSON object: {"results":[{"i":0,"verdict":"right|wrong_direction|wrong"}]}`;

function logWindow(from) {
  const buf = fs.readFileSync(LOG);
  // eslint-disable-next-line no-control-regex
  return buf.subarray(from).toString("utf8").replace(/\x1b\[[0-9;]*m/g, "");
}

async function arm(name) {
  log(`===== arm ${name}: reset`);
  q(`DELETE FROM ontology_agent_reviews WHERE kb_id='${KB}'`);
  q(`DELETE FROM ontology_proposals WHERE kb_id='${KB}' AND proposed_by='agent' AND status<>'adopted'`);
  q(`DELETE FROM jobs WHERE kind='propose_ontology' AND status='queued' AND payload->>'kb_id'='${KB}'`);
  const from = fs.statSync(LOG).size;
  const t0 = Date.now();
  await api("POST", `/api/v1/kbs/${KB}/ontology/propose`, name === "full" ? { glossary: "full" } : {});
  for (;;) {
    await sleep(10000);
    const st = q(`SELECT status FROM jobs WHERE kind='propose_ontology' AND payload->>'kb_id'='${KB}' ORDER BY id DESC LIMIT 1`);
    if (st !== "queued" && st !== "running") { if (st !== "done") log(`job ended as ${st}`); break; }
  }
  const text = logWindow(from);
  const started = /本体代理开始 .*shapes=(\d+) kind_words=(\d+) questions=(\d+) calls=(\d+)/.exec(text);
  const ended = /本体代理结束 .*proposals=(\d+) failed=(\d+) malformed=(\d+) skipped_existing=(\d+)/.exec(text);
  // 坏项各是为什么坏的（按原因归并，键去掉）
  const why = {};
  for (const m of text.matchAll(/本体代理：坏项 .*why=(.*)$/gm)) {
    const k = m[1].replace(/^"|"$/g, "").replace(/:\s[^:]*$/, "").trim();
    why[k] = (why[k] ?? 0) + 1;
  }
  const usage = [...text.matchAll(/llm usage .*prompt=(\d+) completion=(\d+)/g)];
  const prompt = usage.reduce((n, m) => n + Number(m[1]), 0), completion = usage.reduce((n, m) => n + Number(m[2]), 0);
  const rows = JSON.parse(q(`SELECT coalesce(json_agg(json_build_object('section',section,'key',key,'payload',payload,'signatures',signatures) ORDER BY section, key),'[]') FROM ontology_proposals WHERE kb_id='${KB}' AND proposed_by='agent' AND status='open'`));
  const proposals = rows.filter((r) => r.section !== "map_to");
  const maps = rows.filter((r) => r.section === "map_to");
  const reviews = Object.fromEntries(q(`SELECT kind||'/'||outcome||'|'||count(*) FROM ontology_agent_reviews WHERE kb_id='${KB}' GROUP BY kind, outcome`).split("\n").filter(Boolean).map((l) => l.split("|")).map(([k, n]) => [k, Number(n)]));

  // 裁判一：提案是不是已有的。拿全词表判
  const ontology = await api("GET", `/api/v1/kbs/${KB}/ontology`);
  const classKey = new Map(ontology.entity_types.map((c) => [c.id, c.key]));
  const glossary = [
    "Classes:", ...ontology.entity_types.map((c) => `- ${c.key} · ${c.label} · ${c.description ?? ""}`),
    "Properties:", ...ontology.relation_types.map((r) => `- ${r.key} · ${r.label} · ${r.kind} · ${(r.domains ?? []).map((d) => classKey.get(d)).join("|")} → ${(r.ranges ?? []).map((d) => classKey.get(d)).join("|")} · ${r.description ?? ""}`),
  ].join("\n");
  // shown = 被判重复/反向的里，服务端给的"最近的已有元素"里就有裁判说的那一个：审的人看得见
  const dup = { duplicate: 0, inverse: 0, new: 0, unjudged: 0, shown: 0, items: [] };
  for (let i = 0; i < proposals.length; i += 15) {
    const batch = proposals.slice(i, i + 15);
    const list = batch.map((p, j) => `${j}: [${p.section}] ${p.key} · ${p.payload.label} · ${(p.payload.domains ?? p.payload.parents ?? []).join("|")} → ${(p.payload.ranges ?? []).join("|")} · ${p.payload.description} · phrases: ${(p.payload.forms ?? []).join("; ")}${(p.payload.kind_words ?? []).length ? " · kind words: " + p.payload.kind_words.join("; ") : ""}`).join("\n");
    const out = await judge(DUP, `${glossary}\n\nProposals:\n${list}`);
    batch.forEach((p, j) => {
      const r = out?.results?.find((x) => Number(x.i) === j);
      const v = ["duplicate", "inverse", "new"].includes(r?.verdict) ? r.verdict : "unjudged";
      const closest = (p.payload.closest ?? []).map((c) => c.key);
      const shown = (v === "duplicate" || v === "inverse") && closest.includes(String(r?.of ?? ""));
      if (shown) dup.shown++;
      dup[v]++; dup.items.push({ key: p.key, verdict: v, of: r?.of ?? null, closest, shown });
    });
  }
  // 裁判二：「已有」答得对不对。每条形状配一条例句
  const propByKey = new Map(ontology.relation_types.map((r) => [r.key, r]));
  const shapes = [];
  for (const m of maps) {
    const prop = propByKey.get(m.key);
    if (!prop) continue;
    for (const s of m.signatures?.phrases ?? []) {
      const ex = q(`SELECT se.canonical_name||' — '||f.phrase||' → '||coalesce(oe.canonical_name, f.object_value->>'value', f.object_value#>>'{}', '?') FROM facts f JOIN entities se ON se.id=f.subject_id LEFT JOIN entities oe ON oe.id=f.object_id WHERE f.kb_id='${KB}' AND f.layer='open' AND f.phrase='${String(s.phrase).replace(/'/g, "''")}' LIMIT 1`);
      shapes.push({ key: m.key, shape: s, text: `${s.subject ?? "?"} — "${s.phrase}" → ${s.value ? "value" : (s.object ?? "?")} · e.g. ${ex || "(no example)"} · property ${prop.key} (${prop.label}): ${prop.description ?? ""} · direction claimed: ${s.direction ?? "forward"}` });
    }
  }
  const exist = { right: 0, wrong_direction: 0, wrong: 0, unjudged: 0, items: [] };
  for (let i = 0; i < shapes.length; i += 20) {
    const batch = shapes.slice(i, i + 20);
    const out = await judge(EXIST, batch.map((s, j) => `${j}: ${s.text}`).join("\n"));
    batch.forEach((s, j) => {
      const r = out?.results?.find((x) => Number(x.i) === j);
      const v = ["right", "wrong_direction", "wrong"].includes(r?.verdict) ? r.verdict : "unjudged";
      exist[v]++; exist.items.push({ key: s.key, phrase: s.shape.phrase, direction: s.shape.direction ?? "forward", verdict: v });
    });
  }
  return {
    arm: name, seconds: Math.round((Date.now() - t0) / 1000),
    offered: started ? { shapes: +started[1], kind_words: +started[2], questions: +started[3], calls: +started[4] } : null,
    run: ended ? { proposals_and_maps: +ended[1], failed_batches: +ended[2], malformed_items: +ended[3], proposed_existing_key: +ended[4] } : null,
    tokens: { calls: usage.length, prompt, completion, total: prompt + completion, prompt_per_call: usage.length ? Math.round(prompt / usage.length) : 0 },
    malformed_reasons: why,
    proposals: proposals.length, map_to: maps.length, map_to_shapes: shapes.length, reviews, duplicates: dup, existing: exist,
  };
}

async function main() {
  await login();
  const results = [];
  for (const a of ARMS) results.push(await arm(a));
  const pct = (n, d) => (d ? `${((100 * n) / d).toFixed(0)}%` : "–");
  const row = (label, f) => console.log(`${label.padEnd(44)} ${results.map((r) => String(f(r)).padStart(14)).join(" ")}`);
  console.log(`${"".padEnd(44)} ${results.map((r) => r.arm.padStart(14)).join(" ")}`);
  row("shapes / kind words offered", (r) => `${r.offered?.shapes}/${r.offered?.kind_words}`);
  row("calls", (r) => r.tokens.calls);
  row("failed batches (call or unreadable reply)", (r) => r.run?.failed_batches);
  row("malformed items", (r) => r.run?.malformed_items);
  row("  reasons", (r) => Object.entries(r.malformed_reasons).map(([k, n]) => `${n}×${k}`).join("; ").slice(0, 14) || "–");
  row("proposed a key that already exists", (r) => r.run?.proposed_existing_key);
  row("prompt tokens / call", (r) => r.tokens.prompt_per_call);
  row("tokens / round", (r) => r.tokens.total);
  row("proposals (new elements)", (r) => r.proposals);
  row("  judged duplicate of an existing element", (r) => `${r.duplicates.duplicate} (${pct(r.duplicates.duplicate, r.proposals)})`);
  row("  judged inverse of an existing property", (r) => `${r.duplicates.inverse} (${pct(r.duplicates.inverse, r.proposals)})`);
  row("  of those, the existing one is shown beside", (r) => `${r.duplicates.shown} of ${r.duplicates.duplicate + r.duplicates.inverse}`);
  row("  judged new", (r) => `${r.duplicates.new} (${pct(r.duplicates.new, r.proposals)})`);
  row("\"already in the ontology\" shapes", (r) => r.map_to_shapes);
  row("  judged right", (r) => `${r.existing.right} (${pct(r.existing.right, r.map_to_shapes)})`);
  row("  judged wrong direction", (r) => `${r.existing.wrong_direction} (${pct(r.existing.wrong_direction, r.map_to_shapes)})`);
  row("  judged wrong", (r) => `${r.existing.wrong} (${pct(r.existing.wrong, r.map_to_shapes)})`);
  row("shapes declined", (r) => r.reviews["phrase/declined"] ?? 0);
  for (const r of results) if (Object.keys(r.malformed_reasons).length) console.log(`malformed in ${r.arm}: ${JSON.stringify(r.malformed_reasons)}`);
  if (args.out) fs.writeFileSync(args.out, JSON.stringify(results, null, 1));
}
main().catch((e) => { console.error(e); process.exit(1); });
