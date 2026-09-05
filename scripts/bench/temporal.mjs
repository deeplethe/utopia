#!/usr/bin/env node
// 时态问答的测量台（#306）。
//
// 这个仓库唯一能引用的数字是 Ontology2SQL 在 BIRD Mini-Dev 上的成绩，而它围绕着
// 造的那件事——**按某个日期回答问题，且在事实变过之后仍答对**——没有任何东西在量。
// 别人家的记忆基准测的是对话回忆，没有一个会问「2023 年三月这个项目是谁在管」，
// 而语料里那个答案还变过两次。在别人的基准上拿个中游名次，说明的东西还不如
// 把那个缺失的基准公布出来。
//
// **两根轴各问一遍**，这是这份题目里别处没有的一维：
//   world  —— 那时世界是什么样（参数 at）
//   record —— 那时我们以为世界是什么样（参数 as_of，0019）
//
// 记录轴不靠改时钟造出来：语料分两波灌，**第一波灌完记下一个时刻**，第二波带来
// 交接、更正与一次删除。`as_of: "wave-1"` 的题目问的就是那一刻——那时账本里还
// 没有后来的事。这样量到的是产品真实走过的路，不是 UPDATE 出来的场面。
//
// 用法：
//   node scripts/bench/temporal.mjs --label rc5
//   node scripts/bench/temporal.mjs --label rc5 --chat     # 顺带量对话（要一个会话模型）
//   node scripts/bench/temporal.mjs --kb <id> --only-score # 复用已灌好的库，只重打分
//
// 环境变量：BENCH_BASE / BENCH_EMAIL / BENCH_PASSWORD / BENCH_PSQL（同 run.mjs）。

import fs from "node:fs";
import path from "node:path";
import { execSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.env.BENCH_BASE || "http://localhost:18080";
const EMAIL = process.env.BENCH_EMAIL || "bench@test.local";
const PASSWORD = process.env.BENCH_PASSWORD || "benchbench123";

const args = Object.fromEntries(
  process.argv.slice(2).map((a, i, all) => {
    const m = a.match(/^--([^=]+)(?:=(.*))?$/);
    return m ? [m[1], m[2] ?? (all[i + 1]?.startsWith("--") ? true : all[i + 1] ?? true)] : [a, true];
  }),
);
const label = args.label || "temporal";

let cookie = "";
async function api(method, url, body) {
  const res = await fetch(BASE + url, {
    method,
    headers: {
      ...(body ? { "content-type": "application/json" } : {}),
      ...(cookie ? { cookie } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  // getSetCookie（复数）：多个 Set-Cookie 只取第一个会丢掉会话那条
  for (const c of res.headers.getSetCookie?.() ?? []) cookie = c.split(";")[0];
  const text = await res.text();
  if (!res.ok) throw new Error(`${method} ${url} → ${res.status} ${text.slice(0, 300)}`);
  return text ? JSON.parse(text) : null;
}

function psql(sql) {
  const cmd =
    process.env.BENCH_PSQL ||
    "docker exec -e PGPASSWORD=utopia landscapebi-db-1 psql -U utopia -d utopia -tAc";
  return execSync(`${cmd} "${sql.replace(/"/g, '\\"')}"`, { encoding: "utf8" }).trim();
}
const num = (sql) => Number(psql(sql) || 0);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/// 卡住才算超时，慢不算（与 run.mjs 同一条理由：抽取在服务端排队，驱动脚本
/// 死掉不影响它们，而按总时长封顶会把跑成功的组报成失败）。
async function until(fn, everyMs = 8000, stallMs = 8 * 60 * 1000) {
  let last = null;
  let deadline = Date.now() + stallMs;
  for (;;) {
    const r = await fn();
    if (r === true) return;
    if (r !== last) {
      last = r;
      deadline = Date.now() + stallMs;
    }
    if (Date.now() > deadline) throw new Error("等超时：八分钟没有任何进展");
    await sleep(everyMs);
  }
}

/// 一条事实在世界轴上是否覆盖 `at`。起点未知按**保守包含**处理，与服务端
/// `edges_among` 的口径一致（0019 之前就是这么定的）。
function coversWorldTime(fact, at) {
  if (!at) return !fact.valid_to || new Date(fact.valid_to) > new Date();
  const t = new Date(at);
  if (fact.valid_from && new Date(fact.valid_from) > t) return false;
  if (fact.valid_to && new Date(fact.valid_to) <= t) return false;
  return true;
}

function valueOf(fact) {
  const v = fact.object_value;
  if (v === null || v === undefined) return null;
  if (typeof v === "object") return v.value ?? v.summary ?? null;
  return v;
}

async function main() {
  const corpus = JSON.parse(fs.readFileSync(path.join(HERE, "temporal", "corpus.json"), "utf8"));
  const sheet = JSON.parse(fs.readFileSync(path.join(HERE, "temporal", "questions.json"), "utf8"));

  try {
    await api("POST", "/api/v1/auth/register", {
      email: EMAIL,
      display_name: "bench",
      password: PASSWORD,
    });
  } catch {
    /* 已注册，走登录 */
  }
  await api("POST", "/api/v1/auth/login", { email: EMAIL, password: PASSWORD });
  psql(`UPDATE users SET is_admin=TRUE WHERE email='${EMAIL}'`);
  await api("POST", "/api/v1/auth/login", { email: EMAIL, password: PASSWORD });

  let kb = args.kb;
  let waveOne = args["wave-1"];

  if (!kb) {
    const ws = (await api("GET", "/api/v1/workspaces"))[0].id;
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    // **每次一个新库**：带着上一轮结果的库量出来的数字不可比（run.mjs 的头
    // 那段坑就是这么来的）
    kb = (
      await api("POST", `/api/v1/workspaces/${ws}/kbs`, {
        name: `temporal ${label} ${stamp}`,
        ontology_packs: [],
      })
    ).id;
    await sleep(4000);

    // **声明是被测配置的一部分。** 时态引擎的自动闭合只按本体走，而
    // `functional` / `inverse_functional` 永不自动推断（bootstrap_ontology.rs 写了
    // 理由：它驱动的是自动改写账本）。所以缺省先把语料声明的那几条建出来；
    // `--no-declare` 量的是「本体自己长出来」的那种库，两组的差就是这条声明值多少
    const declare = !("no-declare" in args);
    if (declare) {
      const classIds = new Map();
      for (const c of corpus.classes || []) {
        const made = await api("POST", `/api/v1/kbs/${kb}/ontology/entity-types`, {
          key: c.key,
          label: c.label,
          description: c.description || "",
        });
        classIds.set(c.key, made.id);
      }
      for (const a of corpus.axioms || []) {
        const body = {
          key: a.key,
          label: a.label,
          kind: a.kind,
          temporal: a.temporal || "state",
          functional: !!a.functional,
          inverse_functional: !!a.inverse_functional,
          is_transitive: !!a.is_transitive,
          description: a.description || "",
        };
        if (a.kind === "attribute") {
          body.datatype = a.datatype;
          body.unit = a.unit;
          body.domains = [classIds.get(a.domain)].filter(Boolean);
        }
        await api("POST", `/api/v1/kbs/${kb}/ontology/relation-types`, body);
      }
      process.stderr.write(
        `  声明：${(corpus.classes || []).length} 个类、${(corpus.axioms || []).length} 条关系/属性\n`,
      );
    } else {
      process.stderr.write("  不声明：本体交给自动扩展长\n");
    }

    for (const wave of corpus.waves) {
      process.stderr.write(`\n== ${wave.label} ==\n`);
      for (const doc of wave.documents) {
        await api("POST", `/api/v1/kbs/${kb}/ingest`, {
          filename: doc.filename,
          content: doc.text,
          // 语料里写的是日期（读起来清楚），接口要的是时刻
          doc_time: `${doc.doc_time}T00:00:00Z`,
        });
      }
      const want = num(
        `SELECT count(*) FROM documents WHERE kb_id='${kb}' AND deleted_at IS NULL`,
      );
      await until(async () => {
        const done = num(
          `SELECT count(*) FROM documents WHERE kb_id='${kb}' AND graph_status='done' AND deleted_at IS NULL`,
        );
        process.stderr.write(`  抽取 ${done}/${want} 篇\n`);
        return done >= want ? true : done;
      });
      // 删除也是记录轴上的事件（#268）：墓碑留着，这一波之前的时刻仍看得见它
      for (const filename of wave.delete || []) {
        const id = psql(
          `SELECT id FROM documents WHERE kb_id='${kb}' AND filename='${filename}' LIMIT 1`,
        );
        if (id) await api("DELETE", `/api/v1/documents/${id}`);
      }
      if (wave.label === "wave-1") {
        // **这一刻就是记录轴上的界**。等一拍再取，免得同一秒里第二波的写入
        // 也落在这个时刻之内
        await sleep(2000);
        waveOne = new Date().toISOString();
        process.stderr.write(`  wave-1 时刻：${waveOne}\n`);
        await sleep(2000);
      }
    }
  }
  if (!waveOne) throw new Error("复用库时要给 --wave-1 <ISO 时刻>");

  // 实体按名字找一次，后面所有题目复用
  const ids = new Map();
  async function entityId(name) {
    if (ids.has(name)) return ids.get(name);
    const found = await api("GET", `/api/v1/kbs/${kb}/entities?q=${encodeURIComponent(name)}`);
    const hit = (found.entities || []).find(
      (e) => e.name.toLowerCase() === name.toLowerCase(),
    );
    ids.set(name, hit?.id ?? null);
    return hit?.id ?? null;
  }

  const results = [];
  for (const q of sheet.questions) {
    const asOf = q.as_of === "wave-1" ? waveOne : q.as_of || null;
    const subject = await entityId(q.subject);
    if (!subject) {
      // 抽取没抽出这个实体：不是时态答错，单独一档（与 run.mjs 的 absent 同理）
      results.push({ ...q, outcome: "absent", saw: [] });
      continue;
    }
    const qs = new URLSearchParams({ entity: subject, hops: "1" });
    if (q.at) qs.set("at", q.at);
    if (asOf) qs.set("as_of", asOf);
    const graph = await api("GET", `/api/v1/kbs/${kb}/graph/neighborhood?${qs}`);
    const names = new Set(
      graph.edges
        .flatMap((e) => [e.source, e.target])
        .filter((id) => id !== subject)
        .map((id) => graph.nodes.find((n) => n.id === id)?.name)
        .filter(Boolean),
    );

    // 属性事实（薪资、职位）不是边，从实体面板取。**世界轴在这里由脚本过滤**：
    // 那个接口今天只有 as_of，没有 at——量出来的缺口照实写在报告里
    const detail = await api(
      "GET",
      `/api/v1/kbs/${kb}/entities/${subject}${asOf ? `?as_of=${encodeURIComponent(asOf)}` : ""}`,
    );
    const values = (detail.facts || [])
      .filter((f) => coversWorldTime(f, q.at))
      .map(valueOf)
      .filter((v) => v !== null)
      .map(String);

    const saw = [...names, ...values];
    const hasExpect = q.expect === null || saw.some((s) => s.includes(q.expect));
    const hasWrong = (q.not || []).filter((bad) => saw.some((s) => s.includes(bad)));
    results.push({
      ...q,
      as_of_resolved: asOf,
      outcome: hasExpect && hasWrong.length === 0 ? "pass" : "fail",
      wrong_seen: hasWrong,
      saw,
    });
  }

  // 对话那一路可选：它量的是**产品的答案**（模型带着工具自己去问），
  // 上面那一路量的是账本本身。两者差在哪，正是 agent 那一层的水平
  if (args.chat) {
    for (const r of results) {
      if (r.outcome === "absent") continue;
      const reply = await askChat(kb, r.ask);
      r.chat = {
        outcome:
          (r.expect === null || reply.includes(r.expect)) &&
          !(r.not || []).some((bad) => reply.includes(bad))
            ? "pass"
            : "fail",
        reply: reply.slice(0, 400),
      };
    }
  }

  const tally = (pick) => {
    const rows = results.filter((r) => (pick ? r.axis === pick : true) && r.outcome !== "absent");
    const pass = rows.filter((r) => r.outcome === "pass").length;
    return { asked: rows.length, pass, rate: rows.length ? +(pass / rows.length).toFixed(3) : null };
  };
  const report = {
    label,
    declared: !("no-declare" in args),
    kb,
    wave_one: waveOne,
    at: new Date().toISOString(),
    absent: results.filter((r) => r.outcome === "absent").map((r) => r.subject),
    ledger: { all: tally(null), world: tally("world"), record: tally("record") },
    chat: args.chat
      ? (() => {
          const rows = results.filter((r) => r.chat);
          const pass = rows.filter((r) => r.chat.outcome === "pass").length;
          return { asked: rows.length, pass, rate: rows.length ? +(pass / rows.length).toFixed(3) : null };
        })()
      : null,
    questions: results,
  };
  console.log(JSON.stringify(report, null, 2));
  process.stderr.write(
    `\n账本：${report.ledger.all.pass}/${report.ledger.all.asked}` +
      `（世界轴 ${report.ledger.world.pass}/${report.ledger.world.asked}，` +
      `记录轴 ${report.ledger.record.pass}/${report.ledger.record.asked}）` +
      (report.chat ? `　对话：${report.chat.pass}/${report.chat.asked}` : "") +
      `\n`,
  );
}

/// 一次对话问答。SSE 流里只取正文。
async function askChat(kb, question) {
  const res = await fetch(`${BASE}/api/v1/kbs/${kb}/chat`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie },
    body: JSON.stringify({ message: question }),
  });
  const text = await res.text();
  let answer = "";
  for (const line of text.split("\n")) {
    if (!line.startsWith("data:")) continue;
    const payload = line.slice(5).trim();
    if (!payload || payload === "[DONE]") continue;
    try {
      const frame = JSON.parse(payload);
      if (typeof frame === "string") answer += frame;
      else if (frame.delta) answer += frame.delta;
    } catch {
      answer += payload;
    }
  }
  return answer;
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
