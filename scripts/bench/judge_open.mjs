#!/usr/bin/env node
// 开放陈述的裁判（0044 第 1 刀，#729）：**图里的每一条陈述，文档到底说没说。**
//
// recall.mjs 量的是「该有的进没进」，这里量反面：进了的有多少不是原文说的。判据交给一个
// 模型读原文——先抄证据、再下结论（二分之前先引证据，原型上与人工标注一致 21/24）。
// 裁判模型和抽取模型该分开：BENCH_JUDGE_BASE / BENCH_JUDGE_KEY / BENCH_JUDGE_MODEL 指一个
// OpenAI 兼容端点；没给就读工作区的对话模型（llm_settings），那时裁判和抽取是同一个模型，
// 数字要打折看。
//
// 三档：stated（原文说了，陈述说对了）/ misworded（原文说了两者有关系，陈述的说法不对）/
// not_stated（原文没说）。0044 的门槛：not_stated 不超过 2%。
//
// 判成 misworded 的再走第二遍，标一个 kind：extrapolated（说过头了：限定、时间或细节是原文给
// 别的东西的或没给的）或 contradicted（说反了：方向反了、短语说的是原文否认的、值是别的行或
// 列的）——prior-work 第 8 条，Yue 2023 的三分。判决那一遍一字不改，kind 单独一遍：把三档改成
// 四档，同一批 909 条的 misworded 从 5.0% 掉到 0.8%；在同一次调用里追问 kind，掉到 2.1%；
// 裁判自己的抖动是同一批判两遍 5.0% / 3.6%（逐条一致 97%）。
//
// 用法：
//   node scripts/bench/judge_open.mjs --kb <id> [--sample 200] [--seed 7] [--out judged.json]
//   node scripts/bench/judge_open.mjs --calibrate scripts/bench/truth/open-statements.json
//       （人工标注的样本：[{"kb","fact","verdict"}]，报裁判与人的一致率）
//
// 环境变量：BENCH_PSQL（同 recall.mjs）。

import fs from "node:fs";
import { execFileSync } from "node:child_process";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, cur, i, arr) => {
    if (cur.startsWith("--")) acc.push([cur.slice(2), arr[i + 1]?.startsWith("--") ? true : (arr[i + 1] ?? true)]);
    return acc;
  }, []),
);
const KB = args.kb;
if (!KB && !args.calibrate) {
  console.error("要一个 --kb <id>（或 --calibrate <标注文件>）");
  process.exit(1);
}

function psql(sql) {
  const cmd = process.env.BENCH_PSQL || "docker exec -e PGPASSWORD=utopia landscapebi-db-1 psql -U utopia -d utopia -tAc";
  const parts = cmd.split(" ");
  return execFileSync(parts[0], [...parts.slice(1), sql], { encoding: "utf8", maxBuffer: 64 << 20 }).trim();
}
const stamp = () => new Date().toISOString().slice(11, 19);

// 裁判端点：显式给的优先；否则读工作区的对话模型
function judgeEndpoint(kb) {
  if (process.env.BENCH_JUDGE_BASE) {
    return { base: process.env.BENCH_JUDGE_BASE, key: process.env.BENCH_JUDGE_KEY || "", model: process.env.BENCH_JUDGE_MODEL || "" };
  }
  const row = psql(`SELECT concat_ws(chr(31), s.chat_base_url, s.chat_api_key, s.chat_model)
                    FROM llm_settings s JOIN knowledge_bases k ON k.workspace_id = s.workspace_id WHERE k.id = '${kb}'`);
  const [base, key, model] = row.split("");
  if (!base || !model) throw new Error("工作区没配对话模型，也没给 BENCH_JUDGE_*");
  // 库里的密钥是封印过的（服务端用 secret.key 封），读出来是密文，拿它调用只会 401
  if (key.startsWith("enc:")) throw new Error("库里的 chat_api_key 是封印过的密文，裁判读不了它：给 BENCH_JUDGE_BASE / _KEY / _MODEL");
  console.error("裁判用的是工作区的对话模型——和抽取同一个模型，数字要打折看");
  return { base, key, model };
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// 限流（429）和端点出错（5xx）退避后重试，与 typed.mjs 同一个口径（#1518）：
// 一次调用失败就丢一整批 25 条，而丢掉的不是随机样本——是同一块原文上的那些
async function chat(ep, messages) {
  let r, t;
  for (let attempt = 1; ; attempt++) {
    r = await fetch(`${ep.base.replace(/\/$/, "")}/chat/completions`, {
      method: "POST",
      headers: { "content-type": "application/json", ...(ep.key ? { authorization: `Bearer ${ep.key}` } : {}) },
      body: JSON.stringify({ model: ep.model, messages, temperature: 0, max_tokens: 6000, response_format: { type: "json_object" } }),
    });
    t = await r.text();
    if (r.ok) break;
    if ((r.status === 429 || r.status >= 500) && attempt < 6) {
      await sleep(Number(r.headers.get("retry-after")) * 1000 || 15000 * attempt);
      continue;
    }
    throw new Error(`${r.status} ${t.slice(0, 200)}`);
  }
  const content = JSON.parse(t).choices?.[0]?.message?.content || "";
  const s = content.indexOf("{"), e = content.lastIndexOf("}");
  return JSON.parse(content.slice(s, e + 1));
}

const JUDGE = `You check statements extracted from a document. Each numbered statement links a subject to an object or a value through a relation phrase taken from the document; some carry qualifiers (role: value) and the time words the document used.
For each, first copy the words of the document that bear on it ("evidence", "" if none). Then answer:
- "stated": the document states this, or a careful reader takes it directly from the document, and the statement describes it correctly;
- "misworded": the document does state a relationship between this subject and this object or value, but the statement describes it wrongly (the wrong phrase, the wrong direction, a qualifier or time that belongs to something else);
- "not_stated": the document does not state any such relationship between this subject and this object or value (it is invented, the object belongs to something else, or it goes beyond what the document says).
Then, separately, answer "alone": true when "subject —phrase→ object" reads as a complete proposition by itself, without the document; false when a reader could not tell what is being said because the phrase is a bare verb cut from a longer verb phrase, or the object or subject is a fragment of a longer noun phrase.
Judge only from the document. Output one JSON object: {"results":[{"i":0,"evidence":"...","verdict":"stated|misworded|not_stated","alone":true}]}`;

// 取样：现行的开放陈述，连它的证据块（原文）、限定与时间词
function loadStatements(kb, sample, seed) {
  const rows = psql(`
    SELECT concat_ws(chr(31), f.id, c.id, s.canonical_name, f.phrase,
           coalesce(o.canonical_name, f.object_value #>> '{value}', ''),
           coalesce((SELECT string_agg(sq.role || ': ' || coalesce(sq.value #>> '{}', qe.canonical_name, ''), '; ')
                       FROM statement_qualifiers sq LEFT JOIN entities qe ON qe.id = sq.entity_id WHERE sq.fact_id = f.id), ''),
           coalesce((SELECT string_agg(tm.text, ' / ') FROM time_mentions tm WHERE tm.fact_id = f.id), ''),
           d.filename)
    FROM facts f
    JOIN fact_evidence fe ON fe.fact_id = f.id
    JOIN chunks c ON c.id = fe.chunk_id
    JOIN documents d ON d.id = c.document_id
    JOIN entities s ON s.id = f.subject_id
    LEFT JOIN entities o ON o.id = f.object_id
    WHERE f.kb_id = '${kb}' AND f.layer = 'open' AND f.invalidated_at IS NULL
    ORDER BY f.id`)
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      const [fact, chunk, subj, phrase, obj, quals, times, file] = l.split("");
      return { fact, chunk, subj, phrase, obj, quals, times, file };
    });
  // 可重复的抽样：按种子打乱
  let x = Number(seed) || 7;
  const rnd = () => ((x = (x * 1103515245 + 12345) % 2147483648) / 2147483648);
  const picked = rows.map((r) => [rnd(), r]).sort((a, b) => a[0] - b[0]).slice(0, Number(sample) || rows.length).map(([, r]) => r);
  return { total: rows.length, picked };
}

function chunkText(chunkId) {
  return psql(`SELECT text FROM chunks WHERE id = '${chunkId}'`);
}

const render = (r) => `${r.subj} —${r.phrase}→ ${r.obj}${r.quals ? ` (${r.quals})` : ""}${r.times ? ` [${r.times}]` : ""}`;

// 第二遍：只问判成 misworded 的，说过头了还是说反了
const KIND = `You check statements extracted from a document. For each numbered statement the document does state a relationship between the subject and the object or value, but the statement describes it wrongly. Say which way:
- "extrapolated": the statement goes beyond what the document gives this pair: a qualifier, a time or a detail the document gives for something else or does not give, or a phrase that claims more than the document's words;
- "contradicted": the document says otherwise: the direction is reversed, the phrase says what the document denies, or the value is another row's or column's.
First copy the words of the document that bear on it ("evidence"). Judge only from the document. Output one JSON object: {"results":[{"i":0,"evidence":"...","kind":"extrapolated|contradicted"}]}`;

async function kindPass(kb, judged) {
  const ep = judgeEndpoint(kb);
  const byChunk = new Map();
  for (const it of judged) if (it.verdict === "misworded") (byChunk.get(it.chunk) || byChunk.set(it.chunk, []).get(it.chunk)).push(it);
  for (const [chunk, list] of byChunk) {
    const text = chunkText(chunk);
    const body = `Document (${list[0].file}):\n${text}\n\nStatements:\n${list.map((x, j) => `${j}. ${x.statement}`).join("\n")}`;
    let res;
    try {
      res = await chat(ep, [{ role: "system", content: KIND }, { role: "user", content: body }]);
    } catch (e) {
      console.error(`${stamp()} kind 调用失败：${e.message}（这 ${list.length} 条 misworded 不分类）`);
      continue;
    }
    for (const r of res.results || []) {
      const it = list[r.i];
      if (it && ["extrapolated", "contradicted"].includes(r.kind)) it.kind = r.kind;
    }
  }
  return judged;
}

const unjudged = (it) => ({ ...it, statement: render(it), evidence: "", verdict: "unjudged", alone: null });

async function judgeAll(kb, items) {
  const ep = judgeEndpoint(kb);
  const byChunk = new Map();
  for (const it of items) (byChunk.get(it.chunk) || byChunk.set(it.chunk, []).get(it.chunk)).push(it);
  const out = [];
  for (const [chunk, list] of byChunk) {
    const text = chunkText(chunk);
    for (let i = 0; i < list.length; i += 25) {
      const part = list.slice(i, i + 25);
      const body = `Document (${part[0].file}):\n${text}\n\nStatements:\n${part.map((x, j) => `${j}. ${render(x)}`).join("\n")}`;
      let res;
      try {
        res = await chat(ep, [{ role: "system", content: JUDGE }, { role: "user", content: body }]);
      } catch (e) {
        // 判不了也要记下来：从前这里 `continue`，这一批就从 out 里消失，
        // 一条没判成的跑次照样报「过」（#1518）
        console.error(`${stamp()} 裁判调用失败：${e.message}（这 ${part.length} 条记为未判）`);
        for (const it of part) out.push(unjudged(it));
        continue;
      }
      const seen = new Set();
      for (const r of res.results || []) {
        const it = part[r.i];
        if (!it || seen.has(r.i)) continue;
        seen.add(r.i);
        out.push({
          ...it,
          statement: render(it),
          evidence: r.evidence || "",
          verdict: ["stated", "misworded", "not_stated"].includes(r.verdict) ? r.verdict : "unjudged",
          // 读不读得通：单看「主语 —短语→ 宾语」能不能明白在说什么（ReVerb 说的 uninformative 那一类）
          alone: typeof r.alone === "boolean" ? r.alone : null,
        });
      }
      // 模型漏掉的条目同样记为未判，不悄悄少掉
      part.forEach((it, j) => seen.has(j) || out.push(unjudged(it)));
      console.error(`${stamp()} 判了 ${out.length}/${items.length}`);
    }
  }
  return out;
}

function report(judged, total) {
  const count = (v) => judged.filter((j) => j.verdict === v).length;
  // 比例只算判成了的：未判的不是 stated，算进分母会把 not_stated 的比例摊薄（#1518）
  const n = judged.length - count("unjudged");
  const pct = (a) => `${a}/${n}（${n ? ((100 * a) / n).toFixed(1) : 0}%）`;
  const kind = (k) => judged.filter((j) => j.verdict === "misworded" && j.kind === k).length;
  console.log(`\n开放陈述 ${total} 条，抽 ${judged.length} 条，判成 ${n} 条：stated ${pct(count("stated"))}，misworded ${pct(count("misworded"))}（extrapolated ${kind("extrapolated")}，contradicted ${kind("contradicted")}），not_stated ${pct(count("not_stated"))}，未判 ${count("unjudged")}`);
  if (n === 0) {
    console.log("0044 的门槛：一条也没判成，判不了过没过");
    process.exitCode = 1;
  } else if (count("not_stated") / n <= 0.02) {
    console.log("0044 的门槛：not_stated ≤ 2%，过");
  } else {
    console.log("0044 的门槛：not_stated ≤ 2%，没过");
    process.exitCode = 1;
  }
  const fragments = judged.filter((j) => j.alone === false);
  console.log(`单看读不通（alone = false）：${pct(fragments.length)}`);
  const bad = judged.filter((j) => !["stated", "unjudged"].includes(j.verdict)).slice(0, 20);
  if (bad.length) {
    console.log("\n判不对的（样本）：");
    for (const j of bad) console.log(`  [${j.verdict}] ${j.statement}\n      证据：${j.evidence.slice(0, 160)}`);
  }
  if (fragments.length) {
    console.log("\n读不通的（样本）：");
    for (const j of fragments.slice(0, 20)) console.log(`  ${j.statement}\n      证据：${j.evidence.slice(0, 160)}`);
  }
}

if (args.calibrate) {
  // 人工标注的样本：逐条按库取原文再判，报一致率
  const labels = JSON.parse(fs.readFileSync(args.calibrate, "utf8"));
  const byKb = new Map();
  for (const l of labels) (byKb.get(l.kb) || byKb.set(l.kb, []).get(l.kb)).push(l);
  let agree = 0, n = 0;
  for (const [kb, list] of byKb) {
    const { picked } = loadStatements(kb, Infinity, 1);
    const items = picked.filter((p) => list.some((l) => l.fact === p.fact));
    const judged = await judgeAll(kb, items);
    for (const j of judged) {
      const human = list.find((l) => l.fact === j.fact)?.verdict;
      if (!human) continue;
      if (j.verdict === "unjudged") {
        console.log(`  未判：${j.statement}`);
        continue;
      }
      n++;
      if ((human === "stated") === (j.verdict === "stated")) agree++;
      else console.log(`  不一致：人 ${human} / 裁判 ${j.verdict} — ${j.statement}`);
    }
  }
  console.log(`\n二分一致 ${agree}/${n}`);
  process.exit(0);
}

const { total, picked } = loadStatements(KB, args.sample, args.seed);
console.error(`${stamp()} 库里 ${total} 条开放陈述，抽 ${picked.length} 条`);
const judged = await kindPass(KB, await judgeAll(KB, picked));
report(judged, total);
if (args.out) fs.writeFileSync(args.out, JSON.stringify(judged, null, 1));
