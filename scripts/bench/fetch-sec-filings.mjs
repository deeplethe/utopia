#!/usr/bin/env node
// 抓四份真实的 SEC 公开文件，作为「召回」测量台的语料。
//
// 为什么是 SEC 文件而不是维基百科：这个台子量的是**企业文档里的量化事实丢不丢**——
// 金额、容量、期限、票数、职务。维基条目讲的是叙事，一段话里常常一个数都没有；
// 而一份 8-K 的全部价值就是那几个数（收购价、交割时点、每位董事的四类票数）。
// 四份各压一类形态：
//
//   09-02  收购公告   —— 一句话里三个数（对价、留任池、交割时点）
//   08-17  多方新闻稿 —— 七个参与方、四位高管带职务、六个量级不同的数
//   q2fy27 财报新闻稿 —— 财务报表，行列关系全在表格里
//   06-24  投票结果   —— 二十一张无表头的表，十位董事各四类票数
//
// **不把文件存进仓库。** 它们是各公司提交给 SEC 的文件，公开可取但版权不属于我们；
// 维基语料那边是 CC BY-SA 才随语料文件一起存的。这里存脚本，跑一次就有。
//
// 用法：node scripts/bench/fetch-sec-filings.mjs
//   → scripts/bench/corpora/nvda-public-docs/*.html

import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.join(HERE, "corpora", "nvda-public-docs");

// SEC 要求带能联系上的 User-Agent，否则 403。改成你自己的。
const UA = process.env.SEC_USER_AGENT || "Utopia-bench (bench@example.com)";
const BASE = "https://www.sec.gov/Archives/edgar/data/1045810";

const DOCS = [
  ["nvda-8k-2026-09-02.html", "000104581026000078/nvda-20260902.htm"],
  ["nvda-8k-2026-08-17-exhibit.html", "000104581026000069/sbeoainvidia-portsrelease.htm"],
  ["nvda-q2fy27-earnings-release.html", "000104581026000073/q2fy27pr.htm"],
  ["nvda-8k-2026-06-24.html", "000104581026000056/nvda-20260624.htm"],
];

// **走 curl，不走 fetch**：与 fetch-ai-timeline.mjs 同一个理由——
// 这台机器上 HTTP(S)_PROXY 指向本地代理，Node 20 的 undici 不读它。
function get(url) {
  return execFileSync("curl", ["-sSL", "--fail", "--max-time", "90", "-H", `User-Agent: ${UA}`, url], {
    encoding: "buffer",
    maxBuffer: 64 * 1024 * 1024,
  });
}

fs.mkdirSync(OUT, { recursive: true });
for (const [name, suffix] of DOCS) {
  const dest = path.join(OUT, name);
  if (fs.existsSync(dest) && fs.statSync(dest).size > 0) {
    console.log("skip", name);
    continue;
  }
  const body = get(`${BASE}/${suffix}`);
  fs.writeFileSync(dest, body);
  console.log("ok  ", name, body.length);
}
console.log(`\n语料在 ${OUT}\n真值在 scripts/bench/truth/nvda-public-docs.json\n跑一轮：node scripts/bench/recall.mjs`);
