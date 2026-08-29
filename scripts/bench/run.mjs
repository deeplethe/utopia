#!/usr/bin/env node
// 类型消解的测量台：**每一组一个新库**。
//
// 存在的理由是一次踩过的坑：连着三轮在同一个库上调检索，而那个库带着前几轮的
// 改类结果——容易的实体早已精化，拒绝理由里写着 "already correctly typed as
// pharmacy"。后两轮的数字跟第一轮根本不可比，我却拿它们当依据改了两次代码。
//
// 一组 = 新建知识库 → 灌固定语料 → 可选导入本体 → 跑类型消解 → 对标准答案打分。
// 语料与标准答案都在库里（scripts/bench/），所以任何人重跑得到同一批数字。
//
// 用法：
//   node scripts/bench/run.mjs --corpus pharma --label seeds-only
//   node scripts/bench/run.mjs --corpus pharma --ontology /tmp/schemaorg.ttl --label schemaorg
//
// 环境变量：BENCH_BASE（默认 http://localhost:18080）、BENCH_EMAIL / BENCH_PASSWORD、
//           BENCH_PSQL（默认 docker exec … psql；本体段字符数要直接查库）。

import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.env.BENCH_BASE || "http://localhost:18080";
const EMAIL = process.env.BENCH_EMAIL || "bench@test.local";
const PASSWORD = process.env.BENCH_PASSWORD || "benchbench123";

// 实测 4.0 字符 ≈ 1 token（377,735↔81,855、396,716↔99,041，两次都是 4.0）。
//
// 抽取提示词的真实 token 数拿不到——它在 LLM 客户端里，穿出来要改一路签名。
// 本体段字符数与它稳定成比例，而这里要量的正是"本体规模"，够用且不动客户端。
const CHARS_PER_TOKEN = 4.0;

// 种子本体的类。判断"本不该改的有没有被改"要用它：留在种子类上就是没动。
const SEED = [
  "concept",
  "dimension",
  "event",
  "location",
  "metric",
  "organization",
  "person",
  "product",
  "project",
];

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, cur, i, arr) => {
    if (cur.startsWith("--")) acc.push([cur.slice(2), arr[i + 1]]);
    return acc;
  }, []),
);
const corpusName = args.corpus || "pharma";
const label = args.label || corpusName;

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
  const text = await r.text();
  if (!r.ok) throw new Error(method + " " + url + " -> " + r.status + " " + text.slice(0, 200));
  return text ? JSON.parse(text) : null;
}

function psql(sql) {
  const cmd =
    process.env.BENCH_PSQL ||
    "docker exec -e PGPASSWORD=utopia landscapebi-db-1 psql -U utopia -d utopia -tAc";
  const parts = cmd.split(" ");
  return execFileSync(parts[0], [...parts.slice(1), sql], { encoding: "utf8" }).trim();
}
const num = (sql) => Number(psql(sql) || 0);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(fn, everyMs, capMs) {
  const deadline = Date.now() + (capMs || 900000);
  for (;;) {
    if (await fn()) return;
    if (Date.now() > deadline) throw new Error("等超时");
    await sleep(everyMs || 5000);
  }
}

async function main() {
  const corpus = JSON.parse(
    fs.readFileSync(path.join(HERE, "corpora", corpusName + ".json"), "utf8"),
  );
  const truth = JSON.parse(
    fs.readFileSync(path.join(HERE, "truth", corpusName + ".json"), "utf8"),
  ).expect;

  try {
    await api("POST", "/api/v1/auth/register", {
      email: EMAIL,
      display_name: "bench",
      password: PASSWORD,
    });
  } catch {
    // 已经注册过，走登录
  }
  await api("POST", "/api/v1/auth/login", { email: EMAIL, password: PASSWORD });
  psql("UPDATE users SET is_admin=TRUE WHERE email='" + EMAIL + "'");
  await api("POST", "/api/v1/auth/login", { email: EMAIL, password: PASSWORD });

  const ws = (await api("GET", "/api/v1/workspaces"))[0].id;
  // **每组一个新库**：这一条是整个脚本存在的理由，别为了省几分钟去复用
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  const kb = (
    await api("POST", "/api/v1/workspaces/" + ws + "/kbs", {
      name: "bench " + label + " " + stamp,
    })
  ).id;
  // 自动扩本体会在测量中途改本体，那样两组比的就不是同一件事了
  psql("UPDATE knowledge_bases SET auto_extend_ontology=FALSE WHERE id='" + kb + "'");
  await sleep(6000);

  const t0 = Date.now();
  for (const [filename, content] of corpus.docs) {
    await api("POST", "/api/v1/kbs/" + kb + "/ingest", { filename, content });
  }
  await until(
    async () =>
      num(
        "SELECT count(*) FROM documents WHERE kb_id='" +
          kb +
          "' AND graph_status='done'",
      ) >= corpus.docs.length,
  );
  const extractMs = Date.now() - t0;

  let importMs = 0;
  if (args.ontology) {
    const t1 = Date.now();
    const form = new FormData();
    form.append(
      "file",
      new Blob([fs.readFileSync(args.ontology)]),
      path.basename(args.ontology),
    );
    await api("POST", "/api/v1/kbs/" + kb + "/ontology/imports", form, true);
    // 类向量建完才谈得上检索。关系那一半有后台任务补，类型消解用不到。
    //
    // 一千个类的冷启动要几分钟到几十分钟——跟别的库的补齐任务抢同一个嵌入
    // 并发信号量。所以放宽到 40 分钟，并把剩余数打到 stderr：静默地等二十
    // 分钟，分不清是在跑还是卡死了。
    await until(
      async () => {
        const left = num(
          "SELECT count(*) FILTER (WHERE embedding IS NULL) FROM entity_types WHERE kb_id='" +
            kb +
            "'",
        );
        if (left) process.stderr.write("  类向量还差 " + left + "\n");
        return left === 0;
      },
      10000,
      2400000,
    );
    importMs = Date.now() - t1;
  }

  const classChars = num(
    "SELECT coalesce(sum(length('- '||key||coalesce(': '||nullif(description,''),''))+1),0)" +
      " FROM entity_types WHERE kb_id='" +
      kb +
      "'",
  );
  const relChars = num(
    "SELECT coalesce(sum(length('- '||key||' (x)'||coalesce(': '||nullif(description,''),''))+1),0)" +
      " FROM relation_types WHERE kb_id='" +
      kb +
      "' AND kind<>'attribute'",
  );
  const attrChars = num(
    "SELECT coalesce(sum((length('- '||r.key||' (text)'||coalesce(': '||nullif(r.description,''),''))+1)" +
      " * greatest(1,(SELECT count(*) FROM relation_type_domains d WHERE d.relation_type_id=r.id))),0)" +
      " FROM relation_types r WHERE r.kb_id='" +
      kb +
      "' AND r.kind='attribute'",
  );

  const t2 = Date.now();
  const outcome = await api("POST", "/api/v1/kbs/" + kb + "/ontology/type-resolution");
  const resolveMs = Date.now() - t2;

  // 打分。**待人工的按"没改"算**——它确实还没改，算成命中就是把人的活记在机器账上。
  const rows = psql(
    "SELECT e.canonical_name || '|' || t.key FROM entities e" +
      " JOIN entity_types t ON t.id=e.type_id" +
      " WHERE e.kb_id='" +
      kb +
      "' AND e.merged_into IS NULL",
  )
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      const i = l.lastIndexOf("|");
      return [l.slice(0, i), l.slice(i + 1)];
    });

  let hit = 0;
  let miss = 0;
  let correctlyLeft = 0;
  let wronglyChanged = 0;
  let absent = 0;
  const notes = [];
  for (const [frag, accept] of Object.entries(truth)) {
    // 按片段匹配而不是全等：抽取给的名字每次略有出入
    //（"星云科技" / "星云科技(上海)有限公司"），全等会把这种变化算成失败
    const found = rows.filter(([name]) => name.includes(frag));
    if (found.length === 0) {
      absent += 1;
      continue;
    }
    const keys = found.map((r) => r[1]);
    if (accept.length === 0) {
      // 本体里没有对得上的类：正确行为是**不动**，动了才算错
      if (keys.some((k) => !SEED.includes(k))) {
        wronglyChanged += 1;
        notes.push(frag + "：本不该改，却成了 " + keys.join("/"));
      } else correctlyLeft += 1;
    } else if (keys.some((k) => accept.includes(k))) {
      hit += 1;
    } else {
      miss += 1;
      notes.push(frag + "：期望 " + accept.join("|") + "，实得 " + keys.join("/"));
    }
  }

  console.log(
    JSON.stringify(
      {
        label,
        corpus: corpusName,
        ontology: args.ontology ? path.basename(args.ontology) : null,
        kb_id: kb,
        ontology_size: {
          classes: num("SELECT count(*) FROM entity_types WHERE kb_id='" + kb + "'"),
          relations: num(
            "SELECT count(*) FROM relation_types WHERE kb_id='" +
              kb +
              "' AND kind<>'attribute'",
          ),
          attributes: num(
            "SELECT count(*) FROM relation_types WHERE kb_id='" +
              kb +
              "' AND kind='attribute'",
          ),
          prompt_chars: classChars + relChars + attrChars,
          prompt_tokens_est: Math.round(
            (classChars + relChars + attrChars) / CHARS_PER_TOKEN,
          ),
        },
        graph: {
          entities: num(
            "SELECT count(*) FROM entities WHERE kb_id='" +
              kb +
              "' AND merged_into IS NULL",
          ),
          facts: num(
            "SELECT count(*) FROM facts WHERE kb_id='" +
              kb +
              "' AND invalidated_at IS NULL",
          ),
        },
        resolution: {
          retyped: outcome.retyped,
          for_review: outcome.for_review.length,
          left_alone: outcome.left_alone.length,
        },
        score: { hit, miss, correctlyLeft, wronglyChanged, absent, notes },
        ms: { extract: extractMs, import: importMs, resolve: resolveMs },
      },
      null,
      2,
    ),
  );
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
