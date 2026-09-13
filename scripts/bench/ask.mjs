#!/usr/bin/env node
// 问数的端到端测量台（#520）：人问一句话，拿回来的数对不对。
//
// **与 `mappings.mjs` 量的不是一回事，而且一个推不出另一个。** 口径确认得
// 再准，答案照样可能错——模型会挑错源、join 错、按错的日期列过滤，或者压根
// 不看语义层、照着 schema 文档自己写 SQL。反过来，一个一条确认映射都没有的
// 库照样答得出问题，靠的是 schema 文档，而且有时是对的。
//
// 判分材料只有两样，因为 `query_data` 记得很少（step 里只有源名与用途）：
//   1. 助手最后那段话；
//   2. `tool_exchange` 里的工具调用——**模型真正跑过的 SQL**。
// 后者是有用的那个：**把它跑过的 SQL 重跑一遍，跟 gold 的结果比数**。SQL 很短，
// 逃得过 `tool_exchange` 的截断，而它的输出逃不过。
//
// 两栏而不是一栏：
//   sql_right    它跑的那条 SQL 算的是问题问的那个数
//   answer_right 它印出来的数就是那个数
// 两者会分家。查对了却把单位说错、四舍五入错、或者转述成另一个数字的模型，
// 在只看 SQL 的分里是对的，而人读到的答案是错的。
//
// 用法：
//   node scripts/bench/ask.mjs --kb <id>              # 跑全部问题
//   node scripts/bench/ask.mjs --kb <id> --only disc_revenue
//   node scripts/bench/ask.mjs --kb <id> --confirm       # 先确认探索提的那些（产品路径）
//   node scripts/bench/ask.mjs --kb <id> --seed          # 先把真值写成确认口径（上界）
//   node scripts/bench/ask.mjs --kb <id> --replay        # 不重问，拿库里上一轮的回答重判
//   node scripts/bench/ask.mjs --kb <id> --parallel 4    # 同时问四题（缺省一题一题来）
//
// 环境变量见 lib.mjs。

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  BASE, api, login, psql, value, firstRow, same, roughly, log, parseArgs, cookieHeader,
} from "./lib.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv);
const corpusName = args.corpus || "tpch";
const truth = JSON.parse(fs.readFileSync(path.join(HERE, "truth", `${corpusName}.mappings.json`), "utf8"));
const qs = JSON.parse(fs.readFileSync(path.join(HERE, "truth", `${corpusName}.questions.json`), "utf8"));
const CORPUS_DB = process.env.BENCH_CORPUS_DB || `bench_${corpusName}`;

/// 一轮问答。SSE 帧是 `event: X\ndata: {...}\n\n`。
async function ask(kb, message) {
  const res = await fetch(`${BASE}/api/v1/kbs/${kb}/chat`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: cookieHeader() },
    body: JSON.stringify({ message }),
  });
  if (!res.ok) throw new Error(`chat -> ${res.status} ${(await res.text()).slice(0, 200)}`);
  const reader = res.body.getReader();
  const dec = new TextDecoder();
  let buf = "", text = "", conversation = null, error = null;
  const steps = [];
  for (;;) {
    const { done, value: chunk } = await reader.read();
    if (done) break;
    buf += dec.decode(chunk, { stream: true });
    let i;
    while ((i = buf.indexOf("\n\n")) >= 0) {
      const frame = buf.slice(0, i);
      buf = buf.slice(i + 2);
      const ev = /^event: ?(.*)$/m.exec(frame)?.[1];
      const data = frame
        .split("\n")
        .filter((l) => l.startsWith("data:"))
        .map((l) => l.slice(5).replace(/^ /, ""))
        .join("\n");
      try {
        if (ev === "conversation") conversation = JSON.parse(data).id;
        else if (ev === "delta") text += JSON.parse(data).text ?? "";
        else if (ev === "step") steps.push(JSON.parse(data));
        else if (ev === "error") error = data;
      } catch {
        /* 半帧或非 JSON：下一帧再说 */
      }
    }
  }
  return { conversation, text, steps, error };
}

/// 模型跑过的 SQL。**递归找**，因为工具调用的 `arguments` 本身是一段 JSON
/// 字符串，而这一层的形状随模型端而变（有的给 tool_calls，有的给 function_call）。
function sqlsIn(node, out = []) {
  if (typeof node === "string") {
    const s = node.trim();
    if (s.startsWith("{") || s.startsWith("[")) {
      try { sqlsIn(JSON.parse(s), out); } catch { /* 就是一段普通文本 */ }
    }
    return out;
  }
  if (Array.isArray(node)) { for (const n of node) sqlsIn(n, out); return out; }
  if (node && typeof node === "object") {
    for (const [k, v] of Object.entries(node)) {
      if (k === "sql" && typeof v === "string" && v.trim()) out.push(v.trim());
      else sqlsIn(v, out);
    }
  }
  return out;
}

/// 上一轮问过的答案，从库里读回来重判。
///
/// **问一轮很贵**（二十四题四十分钟，还有模型的账），而判分的口径会改——
/// 头一轮就改了两次。会话本来就完整存在库里，所以重判不该重问：改了判据
/// 拿旧回答重跑一遍，才谈得上多轮迭代。
///
/// 同一个问题问过多次时取最新的一次。
function replayFromDb(kb) {
  const rows = JSON.parse(psql(`SELECT coalesce(json_agg(x), '[]') FROM (
      SELECT c.id,
             (SELECT content FROM conversation_messages
               WHERE conversation_id = c.id AND role = 'user'
               ORDER BY created_at LIMIT 1) AS ask,
             (SELECT content FROM conversation_messages
               WHERE conversation_id = c.id AND role = 'assistant'
               ORDER BY created_at DESC LIMIT 1) AS said,
             (SELECT coalesce(json_agg(tool_exchange), '[]') FROM conversation_messages
               WHERE conversation_id = c.id AND role = 'assistant') AS ex
        FROM conversations c
       WHERE c.kb_id = '${kb}'
       ORDER BY c.created_at DESC) x`));
  const byAsk = new Map();
  for (const r of rows) if (r.ask && !byAsk.has(r.ask.trim())) byAsk.set(r.ask.trim(), r);
  return byAsk;
}

/// 把真值全部写成确认过的口径（`--seed`）。
///
/// **这是上界，不是产品路径。** 探索提不提得出这些口径是 #501 的事；这里
/// 直接把答案配进语义层，问的是另一个问题：**口径完备时，问数能到多少？**
/// 基线（一条确认映射都没有，模型照着 schema 文档自己写 SQL）与这一轮之差，
/// 就是语义层在这份语料上值多少分。
///
/// 概念实体按名字复用探索建好的那些；`Metric` 类由探索的 `ensure_concept_types`
/// 建下，所以这一步要在跑过一轮探索的库上做。
function seedTruth(kb) {
  const typeId = psql(`SELECT id FROM entity_types WHERE kb_id = '${kb}' AND key = 'metric'`);
  if (!typeId) throw new Error("这个库还没有 Metric 类——先跑一轮探索");
  let n = 0;
  for (const m of [...truth.metrics, ...(truth.plausible ?? [])]) {
    // **概念名加个记号，别撞上探索建的实体。** 头一次跑没加，`Average order
    // value` 正好与探索起的名字同名，`ON CONFLICT … DO UPDATE` 就把那条提议的
    // sql 改成了 gold——上界那一轮顺手污染了产品路径那一轮的语料
    const label = `${m.label} (truth)`.replace(/'/g, "''");
    const gold = m.gold.replace(/'/g, "''");
    // **说明要是真的说明。** 从前写的是 `label — from`（wide 上就是「Paid orders — bench
    // truth」），嵌进去的文字基本是句废话；中文问题对着它一个词都不沾，recall@8 只有
    // 13/18（#574）。真值里有 `summary` 就用它——那是一个人会写的那句话，用提问的语言
    const summary = (m.summary ?? m.note ?? `${m.label} — ${m.from ?? "bench truth"}`).replace(/'/g, "''");
    const ent = psql(`
      WITH found AS (SELECT id FROM entities
                      WHERE kb_id = '${kb}' AND lower(canonical_name) = lower('${label}')
                        AND merged_into IS NULL LIMIT 1),
           made AS (INSERT INTO entities (id, kb_id, type_id, canonical_name)
                    SELECT gen_random_uuid(), '${kb}', '${typeId}', '${label}'
                     WHERE NOT EXISTS (SELECT 1 FROM found) RETURNING id)
      SELECT id FROM found UNION ALL SELECT id FROM made`);
    psql(`
      INSERT INTO concept_mappings (id, kb_id, concept_id, source, table_name, sql, summary, status)
      VALUES (gen_random_uuid(), '${kb}', '${ent}', '${corpusName}', NULL, '${gold}', '${summary}', 'confirmed')
      ON CONFLICT (kb_id, concept_id, source)
      DO UPDATE SET sql = EXCLUDED.sql, summary = EXCLUDED.summary,
                    status = 'confirmed', updated_at = now()`);
    n++;
  }
  log(`真值已写成 ${n} 条确认口径`);
}

/// 把探索提出的口径全部确认（`--confirm`），模拟人审完一轮。
///
/// **这是产品路径上那一格。** `--seed` 把真值直接配进去，量的是上界；
/// 什么都不配，量的是下界（模型照着 schema 文档自己写 SQL）。真实的库
/// 落在中间：探索提多少、人确认多少，问数就拿到多少。
///
/// 全部确认而不是只确认对的那些，因为这一轮 tpch 上探索的 `wrong` 是 0
/// （#501），一个照着看的人会把它们都点过——**审阅者不知道哪条是对的，
/// 那正是他要判断的事**。
function confirmProposals(kb) {
  const before = Number(psql(`SELECT count(*) FROM concept_mappings
                               WHERE kb_id = '${kb}' AND status = 'proposed'`));
  psql(`UPDATE concept_mappings
           SET status = 'confirmed', decided_at = now(),
               decided_by = (SELECT id FROM users ORDER BY created_at LIMIT 1)
         WHERE kb_id = '${kb}' AND status = 'proposed'`);
  log(`探索提议已确认 ${before} 条`);
}

/// 答案文本里的数字。千分位逗号去掉；引用标记 `[3]` 也会被算进来，
/// 但它撞上一条真值的概率可以忽略——真值里最小的是 1.21
function numbersIn(text) {
  const out = [];
  for (const m of text.matchAll(/-?\d[\d,]*\.?\d*/g)) {
    const n = Number(m[0].replace(/,/g, ""));
    if (Number.isFinite(n)) out.push(n);
  }
  return out;
}

const main = async () => {
  await login();
  const kb = args.kb;
  if (!kb || kb === true) throw new Error("要一个 --kb <id>");

  // --conventions：把真值文件里的约定写进库（人写约定那条路，#570）
  if (args.conventions) {
    if (!truth.conventions?.length) throw new Error(`${corpusName} 的真值里没有 conventions`);
    await api("PATCH", `/api/v1/kbs/${kb}`, { data_conventions: truth.conventions.join("\n") });
    log(`约定已写进库：${truth.conventions.length} 条`);
  }
  if (args.seed) seedTruth(kb);
  if (args.confirm) confirmProposals(kb);

  // 真值：单值口径 + 那些站得住但 22 条查询没说的
  const gold = new Map(
    [...truth.metrics, ...(truth.plausible ?? [])].map((m) => [m.id, { ...m, value: value(CORPUS_DB, m.gold).n }]),
  );

  // **这个问题需要的口径，有没有一条确认过的映射？** 闭环就在这一句上：
  // 答错的题分两种，一种是口径没配（去补映射），一种是配了还错（去看提示词
  // 或工具）。两种该做的事完全不同，而从前它们在结果里长得一样
  // 只看这份语料那个源上的口径：双源库里另一半引用的是另一个库的表，
  // 拿到这个语料库上跑只会报「关系不存在」（#574 的双源那轮刷了一屏）
  const confirmed = JSON.parse(psql(`SELECT coalesce(json_agg(x), '[]') FROM (
      SELECT m.id, m.source, m.table_name, m.expr, m.sql FROM concept_mappings m
       WHERE m.kb_id = '${kb}' AND m.status = 'confirmed'
         AND lower(m.source) = lower('${corpusName}')) x`));
  const mapped = new Set();
  // 每条真值口径由哪几行确认映射算出来（按数判，不看名字）——recall@k 的答案卷
  const rowsFor = new Map();
  for (const m of confirmed) {
    const sql = m.sql?.trim()
      || (m.expr && m.table_name
        ? `SELECT ${m.expr} FROM ${m.table_name.includes(".") ? m.table_name : `${truth.db_schema}.${m.table_name}`}`
        : null);
    if (!sql) continue;
    const got = value(CORPUS_DB, sql);
    if (got.n === undefined) continue;
    for (const [id, g] of gold) if (same(g.value, got.n)) {
      mapped.add(id);
      if (!rowsFor.has(id)) rowsFor.set(id, new Set());
      rowsFor.get(id).add(m.id);
    }
  }

  // --recall K：不问，只量检索——那条对的口径在不在前 K 里（#574）。
  // 几秒钟一轮，不调对话模型；漏在 `near` 对上的，才是 reranker 的活
  if (args.recall) {
    const K = Number(args.recall) || 8;
    const qs2 = qs.questions.filter((q) => rowsFor.has(q.id));
    let hit = 0; const misses = [];
    for (const q of qs2) {
      const r = await api("GET", `/api/v1/kbs/${kb}/mappings/relevant?q=${encodeURIComponent(q.ask)}&k=${K}`);
      const top = r.items.map((m) => m.id);
      const want = rowsFor.get(q.id);
      const at = top.findIndex((id) => want.has(id));
      if (at >= 0) hit++;
      else {
        const g = gold.get(q.id);
        misses.push(`${q.id}${g?.near ? ` (near ${g.near})` : ""}: 前 ${K} 是 ${r.items.map((m) => m.concept_name).join(" | ")}`);
      }
    }
    console.log(JSON.stringify({ kb, corpus: corpusName, k: K, questions_with_a_definition: qs2.length,
      recall: `${hit}/${qs2.length} (${qs2.length ? Math.round(100 * hit / qs2.length) : 0}%)` }, null, 2));
    if (misses.length) console.log("\nMISSED\n  " + misses.join("\n  "));
    return;
  }

  // --replay：不重问，拿库里上一轮的回答重判
  const replay = args.replay ? replayFromDb(kb) : null;
  const questions = qs.questions.filter((q) => (args.only && args.only !== true ? q.id === args.only : true));
  const c = { right: 0, sql_only: 0, answer_only: 0, wrong: 0, no_sql: 0, failed: 0 };
  const rows = [];
  // **一题一个会话，互不共享状态，所以能并发问**（`--parallel N`，缺省 1）。
  // 串行时一题 2–6 个模型回合、平均一分半，十八题半小时；服务端的
  // `model_concurrency` 缺省 10，闸门不是我们。上限别开太高：模型端会限流
  async function one(q, i) {
    const g = gold.get(q.id);
    if (!g || !Number.isFinite(g.value)) { log(`跳过 ${q.id}：真值算不出来`); return; }
    let r;
    if (replay) {
      const prev = replay.get(q.ask.trim());
      if (!prev) { log(`跳过 ${q.id}：库里没有问过这一句`); return; }
      r = { conversation: prev.id, text: prev.said ?? "", exchange: prev.ex ?? [] };
    } else {
      try {
        r = await ask(kb, q.ask);
      } catch (e) {
        c.failed++;
        rows.push({ i, text: `FAILED  ${q.id} — ${String(e.message).slice(0, 120)}` });
        return;
      }
      r.exchange = r.conversation
        ? JSON.parse(psql(`SELECT coalesce(json_agg(tool_exchange), '[]') FROM conversation_messages
                            WHERE conversation_id = '${r.conversation}' AND role = 'assistant'`))
        : [];
    }
    const sqls = sqlsIn(r.exchange);
    // **第一行的每一列都算数。** 模型问「平均行金额」跑的是
    // `SELECT COUNT(*), AVG(l_extendedprice), MIN(…), MAX(…)`，只看第一列
    // 拿到的是行数，一条完全正确的查询会被判成错的
    const ran = sqls.map((s) => ({ s, ns: firstRow(CORPUS_DB, s.replace(/;\s*$/, "")).ns ?? [] }));
    // **这里用 `roughly` 而不是 `same`。** 模型会自己 `ROUND(…, 2)`——那是
    // 它的格式选择，不是另一个口径；594.75 与 594.74915 的相对误差刚好越过
    // `same` 的 1e-6，于是一条完全正确的查询被判成算了别的东西。
    // `mapped` 那边仍然用 `same`，因为它是在二十七条真值里挑中一条，认错了
    // 就是认错了
    const sqlRight = ran.some((x) => x.ns.some((n) => roughly(n, g.value)));
    const answerRight = numbersIn(r.text).some((n) => roughly(n, g.value));

    let verdict;
    if (sqlRight && answerRight) { verdict = "RIGHT"; c.right++; }
    else if (sqlRight) { verdict = "SQL-ONLY"; c.sql_only++; }
    else if (answerRight) { verdict = "ANSWER-ONLY"; c.answer_only++; }
    else if (sqls.length === 0) { verdict = "NO-SQL"; c.no_sql++; }
    else { verdict = "WRONG"; c.wrong++; }

    const flag = mapped.has(q.id) ? "mapped" : "UNMAPPED";
    log(`${verdict.padEnd(11)} ${q.id} (${flag})`);
    if (verdict !== "RIGHT") {
      rows.push({ i, text:
        `${verdict}  ${q.id} (${flag})  truth ${g.value}\n` +
        `    asked: ${q.ask}\n` +
        (ran.length
          ? ran.map((x) => `    ran:   ${x.s.replace(/\s+/g, " ").slice(0, 150)}  → ${x.ns.join(", ")}`).join("\n")
          : "    ran:   (没跑任何 SQL)") +
        `\n    said:  ${r.text.replace(/\s+/g, " ").slice(0, 200)}`,
      });
    }
  }
  const parallel = Math.max(1, Math.min(8, Number(args.parallel) || 1));
  let next = 0;
  await Promise.all(Array.from({ length: parallel }, async () => {
    while (next < questions.length) {
      const i = next++;
      await one(questions[i], i);
    }
  }));
  // 并发之后完成顺序是乱的；报告按题目顺序排，两轮之间才好对着看
  rows.sort((a, b) => a.i - b.i);

  const n = questions.length;
  const pct = (a) => (n ? `${Math.round((a / n) * 100)}%` : "—");
  console.log(JSON.stringify({
    kb, corpus: corpusName, questions: n,
    right: `${c.right}/${n} (${pct(c.right)})`,
    sql_only: c.sql_only,
    answer_only: c.answer_only,
    wrong: c.wrong,
    no_sql: c.no_sql,
    failed: c.failed,
    // 口径有确认映射的题占多少。**「映射全」有了确定的意思**：这一栏满了，
    // 剩下的错就都不是覆盖率的问题
    mapped_definitions: `${[...gold.keys()].filter((id) => mapped.has(id)).length}/${gold.size}`,
  }, null, 2));
  if (rows.length) console.log("\n" + rows.map((r) => r.text).join("\n\n"));
};

main().catch((e) => { console.error(e); process.exit(1); });
