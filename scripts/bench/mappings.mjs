#!/usr/bin/env node
// 映射探索的测量台（#501）：agent 从 schema 提议的口径，对不对、漏了多少。
//
// **打分不看名字，看数。** 治理那边的真值按两个名字键，因为它判的是二分类；
// 这里概念名是模型自己起的，「Revenue」与「已支付 GMV」按名字对不上任何一条。
// 所以真值一条是「一个业务口径 + 一条 gold SQL」，打分把提议真跑一遍，
// 跟 gold 的结果比数——数一样就是同一个口径，叫什么无所谓。
//
// 四栏，含义分开：
//   covered  真值 N 条，被至少一条提议算出来的有几条（漏没漏）
//   right    提议 K 条，跑得通且对上某条真值的有几条
//   wrong    **跑得通但一条都对不上**，附它算出的数与最接近的真值
//   broken   跑不通（列不存在、语法错）
//
// wrong 是决定 #504 能不能默认开的那个数。跑不通的提议无害，它失败得很响；
// 跑得通而算错的才是全部风险——问数会拿它印出一个看起来完全正常的数字。
//
// 用法：
//   node scripts/bench/mappings.mjs --fresh                  # 新库 → 挂源 → 探索 → 打分
//   node scripts/bench/mappings.mjs --fresh --no-comments    # 同上，但语料不带列注释
//   node scripts/bench/mappings.mjs --kb <id> --score        # 只打分，不动库
//
// 环境变量：BENCH_BASE（默认 http://127.0.0.1:8322）、BENCH_EMAIL / BENCH_PASSWORD、
//           BENCH_PSQL（默认 docker exec … psql -d utopia -tAc）、
//           BENCH_CORPUS_CONN（服务端用来连语料库的连接串）。

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
// **判等与连库的那几样住在 lib.mjs，两个测量台共用。** 各写一份 `same()`
// 迟早漂移，而一旦漂移，「提议对了几条」与「答案对了几条」就不是同一把尺子
// 量出来的（#520）
import { api, login, psql, onDb, num, value, same, log, until, sleep, parseArgs } from "./lib.mjs";
import { scoreAlignments } from "./alignment-score.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv);
const corpusName = args.corpus || "tpch";
const truth = JSON.parse(fs.readFileSync(path.join(HERE, "truth", `${corpusName}.mappings.json`), "utf8"));
const CORPUS_DB = process.env.BENCH_CORPUS_DB || `bench_${corpusName}`;
const CORPUS_CONN = process.env.BENCH_CORPUS_CONN || `postgres://utopia:utopia@localhost:5432/${CORPUS_DB}`;
const corpusPsql = (sql) => onDb(CORPUS_DB, `SET statement_timeout = '20s'; ${sql}`);

// ---------- 语料 ----------
//
// **每一组一个新库**（bench/README 的第一条规则），这里还多一层理由：
// `propose` 的 `ON CONFLICT … WHERE status = 'proposed'` 让第二轮探索
// 继承第一轮的行，同一个库上跑两次，第二次的分不是第二次的。
function loadCorpus() {
  const db = CORPUS_DB;
  const exists = onDb("postgres", `SELECT 1 FROM pg_database WHERE datname='${db}'`);
  if (!exists) {
    onDb("postgres", `CREATE DATABASE ${db}`);
    log(`建库 ${db}`);
  }
  corpusPsql(fs.readFileSync(path.join(HERE, "schemas", `${corpusName}.sql`), "utf8"));
  // 行可以单独一个文件：`wide` 的建表与造数各自都不短，放一起读不动
  const data = path.join(HERE, "schemas", `${corpusName}.data.sql`);
  if (fs.existsSync(data)) corpusPsql(fs.readFileSync(data, "utf8"));
  log(`${corpusName} 表与行就位`);
  // 注释是自变量：不加载就是一份同构但没有注释的语料，两轮之差即注释值多少分
  if (!args["no-comments"]) {
    corpusPsql(fs.readFileSync(path.join(HERE, "schemas", `${corpusName}.comments.sql`), "utf8"));
    log("列注释已加载");
  } else {
    log("列注释**未**加载（--no-comments）");
  }
}

// ---------- 新库 + 挂源 + 探索 ----------
async function fresh() {
  loadCorpus();
  const ws = (await api("GET", "/api/v1/workspaces"))[0].id;
  const stamp = new Date().toISOString().slice(0, 16).replace(/[:T]/g, "-");
  const label = args.label || (args["no-comments"] ? "no-comments" : "commented");
  const kb = (await api("POST", `/api/v1/workspaces/${ws}/kbs`, { name: `mappings ${corpusName} ${label} ${stamp}` })).id;
  await sleep(4000);

  // **源名就叫语料名，各轮复用同一个源。**
  //
  // 第一轮给它起了 `tpch-2026-09-08-12-41`（想让各轮的源并存着比），结果是
  // 零提议：`explore_mappings` 拿模型回的 `source` 去比挂载源的名字
  // （`eq_ignore_ascii_case`），而模型看着 schema 回的是 `tpch`，对不上就
  // 整条 `continue`，十二条一条不剩。任务照样 done，页面上只有一条
  // 「没提出来」的告警。**源名是提议能不能落地的隐性依赖**，真实的库里
  // 一样会踩——它该进 #503 的覆盖率报告，而不是靠人猜。
  const dsName = corpusName;
  const existing = (await api("GET", "/api/v1/admin/data-sources")).data_sources.find((d) => d.name === dsName);
  const ds = existing?.id
    ?? (await api("POST", "/api/v1/admin/data-sources", { name: dsName, conn_string: CORPUS_CONN })).id;
  await api("PUT", `/api/v1/admin/data-sources/${ds}/grants/${ws}`);
  const mounted = await api("PUT", `/api/v1/kbs/${kb}/data-sources/${ds}`);
  log(`kb ${kb}，源 ${dsName} 已挂载，schema 文档 ${mounted.schema_tables ?? "?"} 张表`);
  if (mounted.schema_error) log(`  schema 同步报错：${mounted.schema_error}`);
  // --also <corpus>：再挂一个源进同一个库（那份语料得已经建好）。两个源、四十多条
  // 口径，才撑得爆提示词那个 30 的上限，检索才有得量（#574）
  if (args.also && args.also !== true) {
    const other = args.also;
    const ex = (await api("GET", "/api/v1/admin/data-sources")).data_sources.find((d) => d.name === other);
    const dsOther = ex?.id
      ?? (await api("POST", "/api/v1/admin/data-sources", {
        name: other, conn_string: process.env.BENCH_ALSO_CONN || `postgres://utopia:utopia@localhost:5432/bench_${other}`,
      })).id;
    await api("PUT", `/api/v1/admin/data-sources/${dsOther}/grants/${ws}`);
    const m2 = await api("PUT", `/api/v1/kbs/${kb}/data-sources/${dsOther}`);
    log(`源 ${other} 也挂上了，schema 文档 ${m2.schema_tables ?? "?"} 张表`);
  }

  // --conventions：探索之前把真值文件里的约定写进库，它的提示词会读（#570）。
  // 这是探索在 wide 上从 0/18 动起来的第一个机会：schema 里没有「测试单不算数」
  if (args.conventions) {
    if (!truth.conventions?.length) throw new Error(`${corpusName} 的真值里没有 conventions`);
    await api("PATCH", `/api/v1/kbs/${kb}`, { data_conventions: truth.conventions.join("\n") });
    log(`约定已写进库：${truth.conventions.length} 条`);
  }
  await api("POST", `/api/v1/kbs/${kb}/data-sources/explore`);
  log("探索已入队，等提议落库");
  let said = "";
  await until(async () => {
    const row = psql(`SELECT status || E'\\t' || attempts || E'\\t' || coalesce(left(last_error, 160), '')
                        FROM jobs WHERE kind='explore_mappings' AND payload->>'kb_id'='${kb}'
                        ORDER BY id DESC LIMIT 1`);
    const [status, attempts, err] = (row || "queued\t0\t").split("\t");
    // **任务重试期间就把错话说出来。** 头一轮等满十分钟拿到的是「没有任何进展」，
    // 而真正该看见的是「模型密钥解不开」——它第一次失败时就已经写在 last_error 里了
    if (err && err !== said) { said = err; log(`  第 ${attempts} 次失败：${err}`); }
    if (status === "done" || status === "failed") return true;
    // Wait for the run to finish: an early table is not the complete result.
    return false;
  }, 5000, 600000);
  return kb;
}

// ---------- 打分 ----------
const qualify = (t) => (t && t.includes(".") ? t : `${truth.db_schema}.${t}`);

// 提议怎么变成一条能跑的 SQL：给了 sql 就用 sql，否则 expr + table 拼一条。
// 两者都没有就只有 table_name——那不是一个可执行的口径，算 broken。
function proposalSql(m) {
  if (m.sql && m.sql.trim()) return m.sql.trim().replace(/;\s*$/, "");
  if (m.expr && m.table_name) return `SELECT ${m.expr} FROM ${qualify(m.table_name)}`;
  return null;
}


function score(kb) {
  const rows = JSON.parse(psql(`SELECT coalesce(json_agg(x), '[]') FROM (
      SELECT m.id, e.canonical_name AS concept, t.key AS kind, m.source, m.table_name,
             m.expr, m.sql, m.unit, m.status
        FROM concept_mappings m
        JOIN entities e ON e.id = m.concept_id
        JOIN entity_types t ON t.id = e.type_id
       WHERE m.kb_id = '${kb}' ORDER BY e.canonical_name) x`));

  const gold = truth.metrics.map((m) => ({ ...m, value: value(CORPUS_DB, m.gold).n }));
  // 22 条查询没说、但读得懂这个 schema 的人不会反对的口径（退货率、客均余额）。
  // **单独一栏，不算对也不算错**——头一轮把退货率记成 wrong，而它没有任何毛病，
  // 错的是真值不全。govern.mjs 的 `unlabeled` 是同一件事
  const plausible = (truth.plausible || []).map((m) => ({ ...m, value: value(CORPUS_DB, m.gold).n }));
  const dimCols = new Set(truth.dimensions.map((d) => d.column.toLowerCase()));
  // 列名不带表限定的那一份：同一个维度可能从事实表读，也可能从维表读
  // （`shop_nm` 两边都有），而两种都对
  const dimNames = new Set([...dimCols].map((c) => c.split(".").pop()));
  // 陷阱分角色。**同一列在两个角色下不是同一件事**：`p_size` 当维度是对的
  // （Q16 就按它分组），当指标求和才没有意义；`l_comment` 反过来。
  // 第三轮上这条把一条正确的维度提议记成了踩陷阱
  const trapFor = (role) =>
    new Map(
      truth.traps
        .filter((t) => (t.as ?? "both") === "both" || t.as === role)
        .map((t) => [t.column.toLowerCase(), t.why]),
    );
  const dimTraps = trapFor("dimension");
  const trapCols = trapFor("metric");

  const c = { right: 0, wrong: 0, broken: 0, traps: 0, plausible: 0, dim_right: 0, dim_wrong: 0 };
  const hit = new Set(), wrong = [], broken = [], fair = [];

  for (const m of rows) {
    // 维度没有数可比：判它指的那一列在不在真值的 group-by 集合里，
    // 以及有没有落到陷阱列上（把一个键或一段自由文本当成维度）
    if (m.kind === "dimension") {
      // **维度不一定是一个裸列名。** 模型给渠道的定义是
      // `CASE chnl WHEN 1 THEN 'APP' … END`——把码翻译成人看的名字，这正是
      // 一个维度该做的事。只认裸列名的话，三条正确的提议被判成错的（wide 第一轮）。
      // 所以从表达式里把标识符抽出来，命中任一真值维度列即算数
      const table = qualify(m.table_name).toLowerCase();
      const ids = (m.expr || "").toLowerCase().match(/[a-z_][a-z0-9_]*/g) ?? [];
      const col = ids.find((id) => dimCols.has(`${table}.${id}`) || dimNames.has(id))
        ?? `${table}.${(m.expr || "").replace(/[^\w.]/g, "")}`.toLowerCase();
      if (dimCols.has(col) || dimNames.has(col.split(".").pop())) c.dim_right++;
      else { c.dim_wrong++; wrong.push(`dim  "${m.concept}" → ${col}${dimTraps.has(col) ? ` **trap: ${dimTraps.get(col)}**` : ""}`); }
      if (dimTraps.has(col)) c.traps++;
      continue;
    }
    const sql = proposalSql(m);
    if (!sql) { c.broken++; broken.push(`"${m.concept}" 没有可执行的定义（只有 table_name=${m.table_name}）`); continue; }
    const got = value(CORPUS_DB, sql);
    if (got.error) { c.broken++; broken.push(`"${m.concept}" → ${got.error}`); continue; }
    if (got.empty) { c.broken++; broken.push(`"${m.concept}" 返回空`); continue; }

    const matched = gold.filter((g) => same(g.value, got.n));
    if (matched.length) { c.right++; matched.forEach((g) => hit.add(g.id)); continue; }
    const plaus = plausible.find((g) => same(g.value, got.n));
    if (plaus) { c.plausible++; fair.push(`"${m.concept}" = ${plaus.id} (${plaus.label})`); continue; }

    c.wrong++;
    // 最接近的真值：告诉人这条错在哪个方向，而不是只说它错了
    const near = gold
      .filter((g) => Number.isFinite(g.value))
      .sort((a, b) => Math.abs(a.value - got.n) - Math.abs(b.value - got.n))[0];
    const trap = [...trapCols.keys()].find((k) => (m.expr || "").toLowerCase().includes(k.split(".").pop()));
    if (trap) c.traps++;
    wrong.push(
      `"${m.concept}" = ${sql}\n     算出 ${got.n}，最近的真值 ${near?.id} = ${near?.value}` +
      (trap ? `\n     **trap: ${trapCols.get(trap)}**` : ""),
    );
  }

  const pct = (a, b) => (b ? `${Math.round((a / b) * 100)}%` : "—");
  const missed = gold.filter((g) => !hit.has(g.id));
  // **那一轮带没带注释，问库名而不是问命令行参数。** `--score` 重打一次分时
  // 命令行上没有 `--no-comments`，照参数写就把不带注释的那轮报成带注释的
  const kbName = psql(`SELECT name FROM knowledge_bases WHERE id = '${kb}'`);
  const out = {
    kb, corpus: corpusName,
    comments: !kbName.includes("no-comments"),
    proposals: rows.length,
    metrics: { right: c.right, plausible: c.plausible, wrong: c.wrong, broken: c.broken },
    dimensions: { right: c.dim_right, wrong: c.dim_wrong },
    covered: `${hit.size}/${gold.length} (${pct(hit.size, gold.length)})`,
    traps_hit: c.traps,
  };
  console.log(JSON.stringify(out, null, 2));
  if (fair.length) console.log("\nPLAUSIBLE — 22 条查询没说，但站得住的口径\n  " + fair.join("\n  "));
  if (wrong.length) console.log("\nWRONG — 跑得通，算的不是任何一条真值\n  " + wrong.join("\n  "));
  if (broken.length) console.log("\nBROKEN — 跑不通（无害，人一眼看得见）\n  " + broken.join("\n  "));
  if (missed.length) console.log("\nMISSED — 真值里没人提的口径\n  " + missed.map((g) => `${g.id} (${g.label})`).join("\n  "));
}

// ---------- 主流程 ----------
const main = async () => {
  await login();
  const kb = args.kb && args.kb !== true ? args.kb : await fresh();
  if (args.alignments) {
    if (!truth.alignment) throw new Error(`No alignment truth for ${corpusName}; use --corpus wide`);
    const { items } = await api("GET", `/api/v1/kbs/${kb}/table-alignments`);
    console.log(JSON.stringify({ kb, corpus: corpusName, ...scoreAlignments(items, truth.alignment) }, null, 2));
  } else {
    score(kb); // Historical SQL definitions still have the #501 numeric score.
  }
};
main().catch((e) => { console.error(e); process.exit(1); });
