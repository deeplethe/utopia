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
//   node scripts/bench/temporal.mjs --kb <id> --stamps '{"wave-1":"…","wave-2":"…"}'
//                                                          # 复用已灌好的库，只重打分
//   node scripts/bench/temporal.mjs --corpus zh             # 换一套语料与题目（corpus.zh.json）
//   node scripts/bench/temporal.mjs --kb … --chat --only a,b # 只重问几题，看它们的工具轨迹
//   node scripts/bench/temporal.mjs --label late --no-declare --reconcile
//                                                          # 本体自己长，灌完再接受声明、对账（#341）
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
  const suffix = args.corpus ? `.${args.corpus}` : "";
  const corpus = JSON.parse(
    fs.readFileSync(path.join(HERE, "temporal", `corpus${suffix}.json`), "utf8"),
  );
  const sheet = JSON.parse(
    fs.readFileSync(path.join(HERE, "temporal", `questions${suffix}.json`), "utf8"),
  );

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
  // 每一波灌完记一个时刻：wave-1、wave-2……记录轴上的题目以它们为界
  const stamps = args.stamps ? JSON.parse(args.stamps) : {};

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
      // 文档 done 之后还有活：本体自动扩展（bootstrap_ontology）在**另一个任务里**
      // 把本体没认下的谓词建出来、把那些事实改写过去；类型消解也是。不等它们，
      // 时刻就记在 `leads` 还不存在的那一刻——记录轴倒回去看不到接任，候选也是空的
      // （`--reconcile` 第一次就是这样报了零条）。等队列清空再记时刻
      await until(async () => {
        const busy = num(
          `SELECT count(*) FROM jobs WHERE payload->>'kb_id'='${kb}' AND status IN ('queued','running')`,
        );
        return busy === 0 ? true : busy;
      });
      // 删除也是记录轴上的事件（#268）：墓碑留着，这一波之前的时刻仍看得见它
      for (const filename of wave.delete || []) {
        const id = psql(
          `SELECT id FROM documents WHERE kb_id='${kb}' AND filename='${filename}' LIMIT 1`,
        );
        if (id) await api("DELETE", `/api/v1/documents/${id}`);
      }
      // 人改一条事实的起点（#311）：不原地改，作废旧行、写修正行——记录轴倒回去
      // 能看见改之前的区间，这正是它要量的
      for (const c of wave.correct || []) {
        const subject = await entityByName(kb, c.subject);
        if (!subject) throw new Error(`更正找不到实体：${c.subject}`);
        const detail = await api("GET", `/api/v1/kbs/${kb}/entities/${subject}`);
        const fact = (detail.facts || []).find(
          (f) =>
            f.other_name === c.object &&
            (!c.valid_from_was || (f.valid_from || "").startsWith(c.valid_from_was)),
        );
        if (!fact) throw new Error(`更正找不到事实：${c.subject} → ${c.object}`);
        await api("PATCH", `/api/v1/kbs/${kb}/facts/${fact.id}`, {
          valid_from: `${c.valid_from}T00:00:00Z`,
          valid_from_precision: c.precision || "day",
          valid_to: fact.valid_to,
          valid_to_precision: fact.valid_to_precision,
          note: c.note,
        });
        process.stderr.write(`  更正：${c.subject} → ${c.object} 起点改为 ${c.valid_from}\n`);
      }
      // 人合并两个拼法（#337 那一刀量的就是合并之前的世界）
      for (const m of wave.merge || []) {
        const source = await entityByName(kb, m.source);
        const target = await entityByName(kb, m.target);
        if (!source || !target) throw new Error(`合并找不到实体：${m.source} / ${m.target}`);
        await api("POST", `/api/v1/kbs/${kb}/entities/merge`, { source, target });
        process.stderr.write(`  合并：${m.source} → ${m.target}\n`);
      }
      // **这一刻就是记录轴上的界**。前后各等一拍，免得同一秒里下一波的写入
      // 也落在这个时刻之内
      await sleep(2000);
      stamps[wave.label] = new Date().toISOString();
      process.stderr.write(`  ${wave.label} 时刻：${stamps[wave.label]}\n`);
      await sleep(2000);
    }
  }
  if (!Object.keys(stamps).length) throw new Error("复用库时要给 --stamps");

  // 声明来晚了（#341）：新用户走的路——先灌、本体自己长、然后才有人接受声明。
  // 三波都灌完再对账，记录轴上的时刻都在对账之前，倒回去看到的仍是三条开放的行
  const reconciled = "reconcile" in args ? await acceptUniqueness(kb, corpus.axioms || []) : null;

  // 实体按名字找一次，后面所有题目复用。**合并掉的实体在这里找不到**——
  // 它已经不是一个节点了；问它的题目从留下的那一方或它挂过的项目那边问
  const ids = new Map();
  async function entityId(name) {
    if (ids.has(name)) return ids.get(name);
    const id = await entityByName(kb, name);
    ids.set(name, id);
    return id;
  }

  const results = [];
  for (const q of sheet.questions) {
    const asOf = q.as_of && q.as_of.startsWith("wave-") ? stamps[q.as_of] : q.as_of || null;
    if (q.as_of && q.as_of.startsWith("wave-") && !asOf) {
      throw new Error(`题目 ${q.id} 引用了没有记下的时刻 ${q.as_of}`);
    }
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
    // **只看碰到主语的边。** 一跳邻域里还有邻居之间的边（李四 works_for 公司），
    // 它们在 `at` 那一刻可能成立，但说的不是主语——把它们两端的名字都收进来，
    // 第一版就是这么把「李四 2023 年在管 Aurora」误判出来的
    const names = new Set(
      graph.edges
        .filter((e) => e.source === subject || e.target === subject)
        .map((e) => (e.source === subject ? e.target : e.source))
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
    // `--only a,b`：改判分、查一道题的工具轨迹时不必把整张卷子重问一遍。
    // 正式的数永远来自不带 --only 的整跑
    const only =
      typeof args.only === "string" ? new Set(args.only.split(",").map((x) => x.trim())) : null;
    for (const r of results) {
      if (r.outcome === "absent") continue;
      if (only && !only.has(r.id)) continue;
      let reply;
      let steps;
      try {
        // `{wave-N}` 填成那一波灌完的时间戳："as of the second ingest" 是卷子内部的
        // 说法，模型无从知道它是哪一刻，只会反问；给它戳，量的才是工具那条路
        const ask = r.ask.replace(/\{(wave-\d+)\}/g, (_, w) => stamps[w] ?? w);
        ({ answer: reply, steps } = await askChat(kb, ask));
      } catch (e) {
        // 接口本身失败不算答错，单独记 error——否则一个 500 会被记成
        // 「对话答不出来」，而它答都没答
        r.chat = { outcome: "error", reply: String(e).slice(0, 200) };
        continue;
      }
      // **空回复永远不算通过**：空字符串当然不含 not 里的名字
      r.chat = {
        outcome: !reply.trim() ? "error" : chatVerdict(r, reply),
        // 整段留下。曾经截到四百字，"…Lin Zhao's salary in June 2024" 后面的数字
        // 正好被截掉，复核时看着像没答——判分用的是整段，留档也得是整段
        reply: reply.slice(0, 2000),
        // 工具轨迹：模型有没有带 `at` / `as_of`、带的是哪个时刻，从 step 的 detail
        // 上一眼能看出来（"5 facts at 2024-08-01, as recorded by 2026-09-05"）
        steps,
      };
    }
  }

  // known_gap 的题目不进主分：它们钉住的是已知还没做对的行为，单独列出来，
  // 修好那天自己翻绿——而不是让主分一直背着一个我们已经知道的数
  const tally = (pick, gaps = false) => {
    const rows = results.filter(
      (r) =>
        (pick ? r.axis === pick : true) && r.outcome !== "absent" && !!r.known_gap === gaps,
    );
    const pass = rows.filter((r) => r.outcome === "pass").length;
    return { asked: rows.length, pass, rate: rows.length ? +(pass / rows.length).toFixed(3) : null };
  };
  const report = {
    label,
    corpus: corpus.name,
    declared: !("no-declare" in args),
    reconciled,
    kb,
    stamps,
    at: new Date().toISOString(),
    absent: results.filter((r) => r.outcome === "absent").map((r) => r.subject),
    ledger: {
      all: tally(null),
      world: tally("world"),
      record: tally("record"),
      known_gaps: tally(null, true),
    },
    chat: args.chat
      ? (() => {
          // 与账本同一个口径：known_gap 的题不进主分。对话在那几题上不可能比账本
          // 答得更对——它问的就是这本账
          const answered = results.filter((r) => r.chat && r.chat.outcome !== "error");
          const rows = answered.filter((r) => !r.known_gap);
          const gaps = answered.filter((r) => r.known_gap);
          const pass = rows.filter((r) => r.chat.outcome === "pass").length;
          const errors = results.filter((r) => r.chat?.outcome === "error").length;
          return {
            asked: rows.length,
            pass,
            errors,
            rate: rows.length ? +(pass / rows.length).toFixed(3) : null,
            known_gaps: {
              asked: gaps.length,
              pass: gaps.filter((r) => r.chat.outcome === "pass").length,
            },
          };
        })()
      : null,
    questions: results,
  };
  console.log(JSON.stringify(report, null, 2));
  process.stderr.write(
    `\n账本：${report.ledger.all.pass}/${report.ledger.all.asked}` +
      `（世界轴 ${report.ledger.world.pass}/${report.ledger.world.asked}，` +
      `记录轴 ${report.ledger.record.pass}/${report.ledger.record.asked}）` +
      (report.ledger.known_gaps.asked
        ? `　已知缺口 ${report.ledger.known_gaps.pass}/${report.ledger.known_gaps.asked} 通过`
        : "") +
      (report.chat ? `　对话：${report.chat.pass}/${report.chat.asked}` : "") +
      `\n`,
  );
}

/// 声明来晚了（#341）：`uniqueness` 报出候选，**语料的公理表替人表态**——只接受语料
/// 本来就声明了那一端的（一个项目一个 leader，一人一份薪资），PATCH 打开它、再 POST
/// reconcile 把账上已有的行对一遍。其余候选原样记下，不收：第一版全收过一次，
/// `leads` 主语侧（一人只管一个项目）和 `works_for` 宾语侧（一家公司只有一个员工）
/// 也被打开了，李四的 Aurora 被 Helios 闭合，分数反而更歪——这正是产品把这一步
/// 留给人的原因。返回每条候选的估算与实际——两者不一致就是引擎的规则变了
async function acceptUniqueness(kb, axioms) {
  const wanted = new Map(
    axioms.map((a) => [a.key, { functional: !!a.functional, inverse_functional: !!a.inverse_functional }]),
  );
  const { candidates } = await api("GET", `/api/v1/kbs/${kb}/ontology/uniqueness`);
  const { relation_types } = await api("GET", `/api/v1/kbs/${kb}/ontology`);
  const byId = new Map(relation_types.map((r) => [r.id, r]));
  const rows = [];
  for (const c of candidates) {
    const r = byId.get(c.predicate_id);
    if (!r) continue;
    const declared = wanted.get(c.key);
    if (!declared || !declared[c.axiom]) {
      rows.push({ key: c.key, axiom: c.axiom, holders: c.holders, skipped: true });
      process.stderr.write(`  候选 ${c.key} ${c.axiom}：${c.holders} 个持有者——语料没声明，不收\n`);
      continue;
    }
    if (!c.declared) {
      // 同一谓词两端都是候选时，第二次 PATCH 要带着第一次打开的那一位
      r.functional = r.functional || c.side === "subject";
      r.inverse_functional = r.inverse_functional || c.side === "object";
      await api("PATCH", `/api/v1/kbs/${kb}/ontology/relation-types/${r.id}`, {
        label: r.label,
        temporal: r.temporal,
        functional: r.functional,
        inverse_functional: r.inverse_functional,
        is_transitive: r.is_transitive,
        is_symmetric: r.is_symmetric,
        is_asymmetric: r.is_asymmetric,
        is_irreflexive: r.is_irreflexive,
        inverse_of: r.inverse_of,
        sub_property_of: r.sub_property_of,
        description: r.description,
        datatype: r.datatype,
        unit: r.unit,
      });
    }
    const rep = await api(
      "POST",
      `/api/v1/kbs/${kb}/ontology/relation-types/${r.id}/reconcile`,
      {},
    );
    rows.push({
      key: c.key,
      axiom: c.axiom,
      holders: c.holders,
      estimated: { close: c.would_close, review: c.would_review },
      actual: { close: rep.corrected, review: rep.conflicts },
    });
    process.stderr.write(
      `  声明 ${c.key} ${c.axiom}：${c.holders} 个持有者，` +
        `估 ${c.would_close} 闭合 / ${c.would_review} 进审，实际 ${rep.corrected} / ${rep.conflicts}\n`,
    );
  }
  return rows;
}

/// 按名字找一个**现在还是节点**的实体。
async function entityByName(kb, name) {
  const found = await api("GET", `/api/v1/kbs/${kb}/entities?q=${encodeURIComponent(name)}`);
  const hit = (found.entities || []).find((e) => e.name.toLowerCase() === name.toLowerCase());
  return hit?.id ?? null;
}

/// 一次对话问答。SSE 流里只取正文。
async function askChat(kb, question) {
  // 帧的形状与 `web/src/api.ts` 里客户端解析的一致：`event:` 一行定类型，
  // `data:` 一行是 JSON；正文在 `delta` 帧的 `{ text }` 里。第一版只读 `data:`
  // 又去找 `.delta`，于是每一题都得到空串——空串不含 `not` 里的名字，
  // 八道 expect=null 的题就这样被记成了通过
  const res = await fetch(`${BASE}/api/v1/kbs/${kb}/chat`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie },
    body: JSON.stringify({ message: question }),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`chat → ${res.status} ${text.slice(0, 200)}`);
  let answer = "";
  let error = null;
  const steps = [];
  for (const frame of text.split("\n\n")) {
    let event = "message";
    let data = "";
    for (const line of frame.split("\n")) {
      if (line.startsWith("event:")) event = line.slice(6).trim();
      else if (line.startsWith("data:")) data += line.slice(5).trim();
    }
    if (event === "delta" && data) {
      try {
        answer += JSON.parse(data).text ?? "";
      } catch {
        /* 半截帧，跳过 */
      }
    } else if (event === "step" && data) {
      try {
        const s = JSON.parse(data);
        steps.push([s.kind, s.label, s.detail].filter(Boolean).join(" · "));
      } catch {
        /* 同上 */
      }
    } else if (event === "error") {
      error = data;
    }
  }
  if (error && !answer) throw new Error(`chat error frame: ${error.slice(0, 200)}`);
  return { answer, steps };
}

/// 对话判分：子串匹配，只做一件归一——千位分隔符。模型写 "28,000 CNY"，题上写
/// 28000，那是同一个数，不是另一个答案；第一版没归一，六道薪资题里答对的也记了错。
/// 否定句里出现 `not` 的名字仍记错（"没有证据表明王五曾经…"）——这是自然语言
/// 判分的已知弱点，README 里单独有数，不在这里用一串否定词去猜
function chatVerdict(q, reply) {
  const text = reply.replace(/(?<=\d),(?=\d)/g, "");
  const ok =
    (q.expect === null || text.includes(q.expect)) &&
    !(q.not || []).some((bad) => text.includes(bad));
  return ok ? "pass" : "fail";
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
