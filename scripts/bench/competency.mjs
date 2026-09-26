#!/usr/bin/env node
// competency bench（ADR 0061 决定 5）：把一个库接受了的能力问题按人在 chat 里问的方式问一遍，
// 判答没答上，把结果写回问题（last_result），再读服务端算的两个数。
//
//   BENCH_BASE=http://127.0.0.1:1524 BENCH_PSQL="docker exec ... -d utopia_bench4 -tAc" \
//   BENCH_JUDGE_BASE=... BENCH_JUDGE_KEY=... BENCH_JUDGE_MODEL=... \
//   node scripts/bench/competency.mjs --kb <kb-id> [--seed truth/redocred-typed.questions.json] [--rejudge] [--only-report]
//
// --rejudge：不再问 chat，拿上次存下的回答（last_result.answer）重判。问一轮几分钟、还有模型的账，
// 而判分口径会改（同 ask.mjs）。
// 有期望答案的问题交给裁判模型判；没有的只查它需要的形状（needs 里的类与属性）是否存在并且有事实。
// 服务端不自己问：chat 还没有进程内入口，而"按人问的方式问"正是走接口的意思。
import fs from "node:fs";
import { api, login, askChat, parseArgs, log, psql } from "./lib.mjs";

const args = parseArgs(process.argv);
const KB = args.kb;
if (!KB) { console.error("--kb <knowledge base id> 是必须的"); process.exit(2); }

function judgeEndpoint() {
  if (process.env.BENCH_JUDGE_BASE) return { base: process.env.BENCH_JUDGE_BASE, key: process.env.BENCH_JUDGE_KEY || "", model: process.env.BENCH_JUDGE_MODEL || "" };
  const [base, key, model] = psql(`SELECT s.chat_base_url, s.chat_api_key, s.chat_model FROM llm_settings s JOIN knowledge_bases k ON k.workspace_id=s.workspace_id WHERE k.id='${KB}'`).split("|");
  if (!base || !model) throw new Error("工作区没配对话模型，也没给 BENCH_JUDGE_*");
  if (key.startsWith("enc:")) throw new Error("库里的 chat_api_key 是封印过的密文，裁判读不了它：给 BENCH_JUDGE_BASE / _KEY / _MODEL");
  return { base, key, model };
}
async function judgeChat(ep, messages) {
  const r = await fetch(`${ep.base.replace(/\/$/, "")}/chat/completions`, {
    method: "POST", headers: { "content-type": "application/json", ...(ep.key ? { authorization: `Bearer ${ep.key}` } : {}) },
    body: JSON.stringify({ model: ep.model, temperature: 0, messages }),
  });
  if (!r.ok) throw new Error(`judge -> ${r.status} ${(await r.text()).slice(0, 200)}`);
  const j = await r.json();
  return j.choices?.[0]?.message?.content ?? "";
}
const JUDGE = `You grade an answer a knowledge base gave to a question, against the expected answer a person wrote.
Judge only whether the answer states the expected answer (the same entities, values or dates; wording and extra correct detail do not matter). An answer that says the base does not know, or names something else, is wrong.
Output one JSON object: {"correct": true|false, "why": "one short sentence"}`;

async function main() {
  await login();
  // --seed：把写好的问题灌进库（已有同一句的跳过），status = accepted
  if (args.seed) {
    const have = await api("GET", `/api/v1/kbs/${KB}/questions`);
    const texts = new Set(have.map((q) => q.question.trim().toLowerCase()));
    let n = 0;
    for (const q of JSON.parse(fs.readFileSync(args.seed, "utf8"))) {
      if (texts.has(q.question.trim().toLowerCase())) continue;
      await api("POST", `/api/v1/kbs/${KB}/questions`, { question: q.question, expected_answer: q.expected_answer ?? null, needs: q.needs ?? null });
      n++;
    }
    log(`seeded ${n} questions`);
  }
  const questions = (await api("GET", `/api/v1/kbs/${KB}/questions`)).filter((q) => q.status === "accepted");
  if (!args["only-report"]) {
    const ontology = await api("GET", `/api/v1/kbs/${KB}/ontology`);
    const classKeys = new Set(ontology.entity_types.map((c) => c.key));
    const propByKey = new Map(ontology.relation_types.map((r) => [r.key, r]));
    const ep = judgeEndpoint();
    for (const q of questions) {
      const t0 = Date.now();
      let answered = false, judged_by, detail = {}, answer = "";
      try {
        if (args.rejudge) {
          answer = q.last_result?.answer ?? "";
          if (!answer) throw new Error("no stored answer to rejudge");
        } else {
          const r = await askChat(KB, q.question);
          answer = r.text;
          if (r.error) detail.error = String(r.error).slice(0, 300);
        }
        if (q.expected_answer) {
          judged_by = "expected";
          const verdict = await judgeChat(ep, [
            { role: "system", content: JUDGE },
            { role: "user", content: `Question: ${q.question}\nExpected answer: ${q.expected_answer}\nAnswer given:\n${answer.slice(0, 4000)}` },
          ]);
          const m = /\{[\s\S]*\}/.exec(verdict);
          const j = m ? JSON.parse(m[0]) : { correct: false, why: "judge gave no JSON" };
          answered = !!j.correct; detail.why = j.why;
        } else {
          // 没有期望答案：查它需要的形状——类都在、属性都在并且各有类型化事实
          judged_by = "shape";
          const needs = q.needs ?? {};
          const missingClasses = (needs.classes ?? []).filter((k) => !classKeys.has(k));
          const missingProps = (needs.properties ?? []).filter((k) => !propByKey.has(k));
          const emptyProps = (needs.properties ?? []).filter((k) => propByKey.has(k) && !(propByKey.get(k).usage > 0));
          detail = { missing_classes: missingClasses, missing_properties: missingProps, empty_properties: emptyProps };
          answered = missingClasses.length === 0 && missingProps.length === 0 && emptyProps.length === 0
            && ((needs.classes ?? []).length + (needs.properties ?? []).length > 0);
        }
      } catch (e) {
        detail.error = String(e.message || e).slice(0, 300);
      }
      detail.seconds = Math.round((Date.now() - t0) / 1000);
      await api("POST", `/api/v1/kbs/${KB}/questions/${q.id}/result`, { answered, answer, judged_by, detail });
      log(`${answered ? "✓" : "✗"} [${judged_by}] ${q.question} — ${detail.why ?? JSON.stringify(detail)}`);
    }
  }
  const report = await api("GET", `/api/v1/kbs/${KB}/questions/report`);
  const qs = report.questions, ps = report.proposals;
  console.log(`questions: ${qs.answered}/${qs.checked} answered (${qs.accepted} accepted, ${qs.proposed} proposed)`);
  console.log(`proposals: ${ps.changed}/${ps.decided} changed before adoption or rejected` + (ps.changed_share != null ? ` (${(ps.changed_share * 100).toFixed(0)}%)` : "") + `, ${ps.open} open`);
}
main().catch((e) => { console.error(e); process.exit(1); });
