#!/usr/bin/env node
// 治理的测量台（0025）：agent 自己裁的与人标的一致不一致，还剩多少等人。
//
// 真值是 truth/<corpus>.duplicates.json：一次导入里消解器提出的每一对，按两个名字
// 键（不分左右、不分大小写）人工标成 same / different / unknown。同一份语料再导一次，
// 抽出来的名字会略有出入，对不上真值的对单独算一栏（unlabeled），不算对也不算错。
//
// 用法：
//   node scripts/bench/govern.mjs --fresh                 # 新库、灌语料（治理开着）、跑完打分
//   node scripts/bench/govern.mjs --kb <id>               # 已有的库：撤掉 agent 的决定 → 再跑 → 打分
//   node scripts/bench/govern.mjs --kb <id> --score       # 只打分，不动库
//   node scripts/bench/govern.mjs --kb <id> --reset       # 只撤（agent 与模拟的人做的合并、分开、建议全部还原成等人）
//
// --reset 走的是 revert_merge 同一套还原（事实搬回、作废的恢复、时态修正撤销、别名与
// 画像回退），直接在库里做而不经 API：经 API 会在台账上留下 merge.revert，下一轮 agent
// 会把它当成人的先例。台账里 agent 自己的行（actor 为空）留着无妨——先例只认人的。
//
// 环境变量：BENCH_BASE（默认 http://127.0.0.1:8322）、BENCH_EMAIL / BENCH_PASSWORD、
//           BENCH_PSQL（默认 docker exec … psql -d utopia）。

import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.env.BENCH_BASE || "http://127.0.0.1:8322";
const EMAIL = process.env.BENCH_EMAIL || "bench@test.local";
const PASSWORD = process.env.BENCH_PASSWORD || "benchbench123";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, cur, i, arr) => {
    if (cur.startsWith("--")) acc.push([cur.slice(2), arr[i + 1]?.startsWith("--") ? true : (arr[i + 1] ?? true)]);
    return acc;
  }, []),
);
const corpusName = args.corpus || "ai-timeline";
const truth = JSON.parse(fs.readFileSync(path.join(HERE, "truth", `${corpusName}.duplicates.json`), "utf8"));

let cookie = "";
async function api(method, url, body) {
  const init = { method, headers: {} };
  if (cookie) init.headers.cookie = cookie;
  if (body !== undefined) { init.headers["content-type"] = "application/json"; init.body = JSON.stringify(body); }
  const r = await fetch(BASE + url, init);
  for (const c of r.headers.getSetCookie?.() ?? []) cookie = c.split(";")[0];
  const text = await r.text();
  if (!r.ok) throw new Error(`${method} ${url} -> ${r.status} ${text.slice(0, 200)}`);
  return text ? JSON.parse(text) : null;
}
function psql(sql) {
  const cmd = process.env.BENCH_PSQL || "docker exec -e PGPASSWORD=utopia landscapebi-db-1 psql -U utopia -d utopia -tAc";
  const parts = cmd.split(" ");
  return execFileSync(parts[0], [...parts.slice(1), sql], { encoding: "utf8", maxBuffer: 64 << 20 }).trim();
}
const num = (sql) => Number(psql(sql) || 0);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.error(new Date().toISOString().slice(11, 19), ...a);
async function until(fn, everyMs, stallMs) {
  let deadline = Date.now() + stallMs, last = null;
  for (;;) {
    const r = await fn();
    if (r === true) return;
    if (typeof r === "number" && r !== last) { last = r; deadline = Date.now() + stallMs; }
    if (Date.now() > deadline) throw new Error(`等超时：${Math.round(stallMs / 60000)} 分钟没有任何进展`);
    await sleep(everyMs);
  }
}

// ---------- 新库 + 灌语料 ----------
async function fresh() {
  const corpus = JSON.parse(fs.readFileSync(path.join(HERE, "corpora", `${corpusName}.json`), "utf8"));
  const keep = new Set(truth.docs || corpus.docs.map((d) => d[0]));
  const docs = corpus.docs.filter((d) => keep.has(d[0]));
  const ws = (await api("GET", "/api/v1/workspaces"))[0].id;
  const stamp = new Date().toISOString().slice(0, 16).replace(/[:T]/g, "-");
  const kb = (await api("POST", `/api/v1/workspaces/${ws}/kbs`, { name: `govern ${args.label || corpusName} ${stamp}`, ontology_packs: ["schema-org"] })).id;
  await sleep(6000);
  await api("PATCH", `/api/v1/kbs/${kb}`, { governance: true });
  log(`kb ${kb}: ${docs.length} docs going in`);
  for (const [filename, content, docTime] of docs) {
    const body = { filename, content };
    if (docTime) body.doc_time = docTime;
    await api("POST", `/api/v1/kbs/${kb}/ingest`, body);
  }
  await until(async () => {
    // 失败了但抽取任务还排着的（本体索引没嵌完、延后重试）不算完
    const done = num(`SELECT count(*) FROM documents d WHERE d.kb_id='${kb}' AND (d.graph_status = 'done'
      OR (d.graph_status = 'failed' AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.kind = 'extract_document'
          AND j.status IN ('queued','running') AND j.payload->>'document_id' = d.id::text)))`);
    if (done >= docs.length) return true;
    const chunks = num(`SELECT count(*) FROM chunks WHERE kb_id='${kb}' AND extracted_at IS NOT NULL`);
    log(`  抽取 ${chunks} 块 / ${done} 篇完成`);
    return chunks;
  }, 20000, 1200000);
  return kb;
}

// ---------- 撤掉 agent（与模拟的人）的决定 ----------
function reset(kb) {
  psql(`DO $$
DECLARE m RECORD;
BEGIN
  FOR m IN SELECT * FROM entity_merges WHERE kb_id = '${kb}' AND reverted_at IS NULL ORDER BY created_at DESC LOOP
    UPDATE facts SET subject_id = m.source_id WHERE id = ANY(m.moved_subject_facts);
    UPDATE facts SET object_id = m.source_id WHERE id = ANY(m.moved_object_facts);
    UPDATE facts SET invalidated_at = NULL WHERE id = ANY(m.invalidated_facts);
    UPDATE facts SET invalidated_at = NULL WHERE id IN (
      SELECT supersedes FROM facts WHERE id = ANY(m.temporal_corrections) AND invalidated_at IS NULL AND supersedes IS NOT NULL);
    UPDATE facts SET invalidated_at = now() WHERE id = ANY(m.temporal_corrections) AND invalidated_at IS NULL;
    -- 名字是 known_as 上的事实（0041），跟着 moved_subject_facts 搬回去了，不用另外还
    UPDATE entities SET
      profile_embedding = m.target_profile_before, profile_n = m.target_profile_n_before,
      type_id = coalesce(m.target_type_before, type_id), updated_at = now()
    WHERE id = m.target_id;
    UPDATE entities SET merged_into = NULL, updated_at = now() WHERE id = m.source_id;
  END LOOP;
  DELETE FROM entity_merges WHERE kb_id = '${kb}';
  -- 同一对实体可能有一行已裁、一行又等着（消解器裁完之后又提了一次）：等着的那行留着，
  -- 已裁的重复行删掉；一对只剩一行已裁的，重开成等人
  DELETE FROM resolution_reviews rr WHERE rr.kb_id = '${kb}' AND rr.status <> 'pending'
     AND EXISTS (SELECT 1 FROM resolution_reviews p WHERE p.kb_id = rr.kb_id AND p.status = 'pending'
                 AND least(p.left_id, p.right_id) = least(rr.left_id, rr.right_id)
                 AND greatest(p.left_id, p.right_id) = greatest(rr.left_id, rr.right_id));
  DELETE FROM resolution_reviews rr WHERE rr.kb_id = '${kb}' AND rr.status <> 'pending'
     AND rr.id <> (SELECT p.id FROM resolution_reviews p WHERE p.kb_id = rr.kb_id AND p.status <> 'pending'
                   AND least(p.left_id, p.right_id) = least(rr.left_id, rr.right_id)
                   AND greatest(p.left_id, p.right_id) = greatest(rr.left_id, rr.right_id)
                   ORDER BY p.decided_at DESC NULLS LAST, p.created_at DESC LIMIT 1);
  UPDATE resolution_reviews SET status = 'pending', stage = 'human', decided_at = NULL, decided_by = NULL,
         reason = CASE WHEN reason LIKE 'governed|%' OR reason = 'proposed' OR reason LIKE 'escalate_%' THEN NULL ELSE reason END
   WHERE kb_id = '${kb}' AND status <> 'pending';
  UPDATE resolution_reviews SET reason = NULL WHERE kb_id = '${kb}' AND (reason LIKE 'governed|%' OR reason = 'proposed' OR reason LIKE 'escalate_%');
  DELETE FROM agent_decisions WHERE kb_id = '${kb}';
END $$;`);
  log(`reset: ${num(`SELECT count(*) FROM resolution_reviews WHERE kb_id='${kb}' AND status='pending'`)} pairs waiting, 0 agent rows`);
}

// ---------- 跑一轮 ----------
async function run(kb) {
  await api("PATCH", `/api/v1/kbs/${kb}`, { governance: true });
  if (num(`SELECT count(*) FROM jobs WHERE kind='govern' AND status IN ('queued','running') AND payload->>'kb_id'='${kb}'`) === 0) {
    psql(`INSERT INTO jobs (kind, payload) VALUES ('govern', jsonb_build_object('kb_id', '${kb}'))`);
  }
  const t0 = Date.now();
  await until(async () => {
    const busy = num(`SELECT count(*) FROM jobs WHERE kind='govern' AND status IN ('queued','running') AND payload->>'kb_id'='${kb}'`);
    const left = num(`SELECT count(*) FROM resolution_reviews rr WHERE rr.kb_id='${kb}' AND rr.status='pending'
      AND NOT EXISTS (SELECT 1 FROM agent_decisions d WHERE d.target_id = rr.id AND d.status = 'proposed')`);
    if (busy === 0 && left === 0) return true;
    if (busy === 0) {
      // 任务停了但还有没看的：失败重试耗尽？再排一个
      const failed = psql(`SELECT left(COALESCE(last_error,''),120) FROM jobs WHERE kind='govern' AND payload->>'kb_id'='${kb}' ORDER BY id DESC LIMIT 1`);
      log(`  job stopped with ${left} pairs unseen: ${failed}`);
      psql(`INSERT INTO jobs (kind, payload) VALUES ('govern', jsonb_build_object('kb_id', '${kb}'))`);
    }
    const rows = num(`SELECT count(*) FROM agent_decisions WHERE kb_id='${kb}'`);
    log(`  ${left} pairs not yet looked at, ${rows} agent rows`);
    return left * 10000 + rows;
  }, 15000, 900000);
  log(`run took ${Math.round((Date.now() - t0) / 60000)} min`);
}

// ---------- 打分 ----------
function score(kb) {
  const key = (a, b) => [a.toLowerCase(), b.toLowerCase()].sort().join(" | ");
  const want = new Map(truth.pairs.map((p) => [key(p.left, p.right), p.verdict]));
  const rows = JSON.parse(psql(`SELECT COALESCE(json_agg(json_build_object(
      'l', a.canonical_name, 'r', b.canonical_name, 'status', rr.status, 'reason', rr.reason, 'by_person', rr.decided_by IS NOT NULL,
      'agent', (SELECT json_build_object('action', d.action, 'status', d.status, 'confidence', d.confidence, 'calls', d.calls, 'reason', d.reason)
                FROM agent_decisions d WHERE d.target_id = rr.id ORDER BY d.created_at DESC LIMIT 1))), '[]')
    FROM resolution_reviews rr JOIN entities a ON a.id = rr.left_id JOIN entities b ON b.id = rr.right_id
    WHERE rr.kb_id = '${kb}'`));
  const c = { pairs: rows.length, labeled: 0, unlabeled: 0, unknown: 0,
    applied: 0, right: 0, wrong_merge: 0, wrong_keep: 0, applied_unknown: 0,
    proposed: 0, proposed_same: 0, proposed_different: 0, unsure: 0, by_person: 0, unseen: 0, moot: 0, loop_rows: 0, loop_calls: 0 };
  const wrong = [], stuck = [];
  for (const r of rows) {
    const t = want.get(key(r.l, r.r));
    if (t === undefined) { c.unlabeled++; }
    else if (t === "unknown") c.unknown++;
    else c.labeled++;
    const d = r.agent;
    if (d?.calls > 0) { c.loop_rows++; c.loop_calls += d.calls; }
    if (r.by_person) { c.by_person++; continue; }
    // 一侧并进了别人之后，这一对本身不存在了：库里关成 kept、reason 说明是连带的
    if (!d && r.reason === "superseded by merge") { c.moot++; continue; }
    if (!d) { c.unseen++; continue; }
    if (d.status === "applied") {
      c.applied++;
      if (t === undefined || t === "unknown") { c.applied_unknown++; continue; }
      const ok = (d.action === "merge" && t === "same") || (d.action === "keep" && t === "different");
      if (ok) c.right++;
      else { if (d.action === "merge") c.wrong_merge++; else c.wrong_keep++; wrong.push(`${d.action}@${d.confidence} on "${r.l}" ≟ "${r.r}" (truth ${t}) — ${d.reason ?? ""}`); }
    } else if (d.status === "proposed") {
      c.proposed++;
      if (d.action === "unsure") c.unsure++;
      if (t === "same") c.proposed_same++; else if (t === "different") c.proposed_different++;
      stuck.push(`${d.action}@${d.confidence} "${r.l}" ≟ "${r.r}" (truth ${t ?? "?"})`);
    } else c.unseen++;
  }
  const pct = (a, b) => (b ? `${Math.round((a / b) * 1000) / 10}%` : "-");
  const out = {
    kb, pairs: c.pairs, labeled: c.labeled, unlabeled: c.unlabeled, unknown: c.unknown,
    decided_on_its_own: c.applied, agreed_with_labels: `${c.right}/${c.applied - c.applied_unknown} (${pct(c.right, c.applied - c.applied_unknown)})`,
    wrong_merges: c.wrong_merge, wrong_keeps: c.wrong_keep,
    left_for_people: c.proposed, of_which_unsure: c.unsure, people_would_merge: c.proposed_same, people_would_keep: c.proposed_different,
    decided_by_person: c.by_person, moot_after_a_merge: c.moot, not_looked_at: c.unseen,
    automatic_share: pct(c.applied, c.pairs - c.by_person - c.moot),
    loop: `${c.loop_rows} pairs, ${c.loop_calls} calls`,
  };
  console.log(JSON.stringify(out, null, 2));
  if (wrong.length) console.log("\nWRONG\n" + wrong.join("\n"));
  if (args.stuck && stuck.length) console.log("\nLEFT FOR PEOPLE\n" + stuck.join("\n"));
  return out;
}

async function main() {
  await api("POST", "/api/v1/auth/login", { email: EMAIL, password: PASSWORD });
  let kb = args.kb;
  if (args.fresh) kb = await fresh();
  if (!kb) throw new Error("--kb <id> or --fresh");
  const only = args.score || args.reset;
  if (args.reset) reset(kb);
  if (!only || args.run) {
    if (!args.fresh && !args.reset && !args.run) reset(kb);
    if (!args.fresh) await run(kb);
    else await run(kb);
  }
  if (!args.reset || args.score) score(kb);
}
main().catch((e) => { console.error(e); process.exit(1); });
