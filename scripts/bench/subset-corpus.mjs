#!/usr/bin/env node
// 从一份语料里挑出几个条目，做成一份新语料。
//
// 存在的理由是 `wiki-history` 跑不完：219 篇快照约 7000 块，按实测速率要六到七
// 小时，还会撞上端点的每分钟 token 配额。而**它回答的两个问题对语料的要求不一样**：
//
// - 「违反率归零站不站得住」是统计题。60 块出 149 条可校验事实，零违反的
//   置信上界按三倍律是 `3/n`，几百块就已经结论性了，再多跑不增加信息。
// - 「图会不会真的改主意」是结构题，统计帮不上忙。它要的是**整条快照链按
//   `doc_time` 顺序进去**，因为 `supersedes` 只在同一条目的相邻快照之间发生。
//
// 所以切法是**整条目取，不切块**：随机抽块能满足第一个问题，会把第二个问题
// 直接废掉。挑几个互相咬合的条目，比多跑几千块有用。
//
// 输出按 `doc_time` 升序排列。灌入顺序就是认知生长的顺序，乱序进去的图
// 在溯源时态那根轴上是没有意义的。
//
// 用法：node scripts/bench/subset-corpus.mjs <语料.json> <条目,条目,…> > 新语料.json
//
// 例（2023 年 11 月 OpenAI 那场风波，七个共享实体的条目）：
//   node scripts/bench/subset-corpus.mjs scripts/bench/corpora/wiki-history.json \
//     openai,removal-of-sam-altman-from-openai,sam-altman,ilya-sutskever,\
//     mira-murati,greg-brockman,emmett-shear \
//     > scripts/bench/corpora/wiki-nov2023.json
//
// 不带条目参数就只列出源语料有哪些条目、各多少快照，不输出语料。

import fs from "node:fs";

const [, , src, titlesRaw] = process.argv;
if (!src) {
  console.error("用法: subset-corpus.mjs <语料.json> [条目,条目,…]");
  console.error("      不给条目则只列出源语料的条目清单");
  process.exit(2);
}

const corpus = JSON.parse(fs.readFileSync(src, "utf8"));
if (!Array.isArray(corpus.docs)) {
  console.error(`${src} 里没有 docs 数组，不像是一份语料`);
  process.exit(2);
}

/// 文件名形如 `openai@2023-11-19.txt`，`@` 之前是条目。
/// 当前版语料（fetch-ai-timeline）没有 `@`，整个文件名就是条目。
const titleOf = (filename) => filename.replace(/@.*$/, "").replace(/\.txt$/, "");

// 条目清单：快照数与体量。给挑之前看用
const groups = new Map();
for (const [filename, text] of corpus.docs) {
  const t = titleOf(filename);
  const g = groups.get(t) || { n: 0, chars: 0 };
  g.n += 1;
  g.chars += text.length;
  groups.set(t, g);
}

if (!titlesRaw) {
  const rows = [...groups].sort((a, b) => b[1].chars - a[1].chars);
  const total = rows.reduce((s, [, g]) => s + g.chars, 0);
  for (const [t, g] of rows) {
    const pct = ((100 * g.chars) / total).toFixed(1).padStart(5);
    console.error(
      `${String(g.n).padStart(4)} 张  ${String(Math.round(g.chars / 1000)).padStart(6)}k  ${pct}%  ${t}`,
    );
  }
  console.error(`\n共 ${rows.length} 个条目，${corpus.docs.length} 张快照，${(total / 1e6).toFixed(2)}M 字符`);
  process.exit(0);
}

const wanted = new Set(titlesRaw.split(",").map((t) => t.trim()).filter(Boolean));

// **认不出的条目名要报错，不能静默产出一份小语料。** 打错一个字就少一个条目，
// 而少了的那个条目正是共享实体的来源，图会散成互不相连的星团——
// 而这在结果里看起来只是"效果没那么好"，查不到根上。
const unknown = [...wanted].filter((t) => !groups.has(t));
if (unknown.length) {
  console.error(`源语料里没有这些条目：${unknown.join(", ")}`);
  console.error(`不带条目参数重跑一次可以看到全部条目名。`);
  process.exit(2);
}

const docs = corpus.docs
  .filter(([filename]) => wanted.has(titleOf(filename)))
  // 按 doc_time 升序。第三个元素缺席时（当前版语料）退回文件名排序，
  // 至少是确定的
  .sort((a, b) => String(a[2] ?? a[0]).localeCompare(String(b[2] ?? b[0])));

const chars = docs.reduce((s, d) => s + d[1].length, 0);

process.stdout.write(
  JSON.stringify({
    name: `${corpus.name}-subset`,
    note:
      `${corpus.name} 的子集，条目：${[...wanted].join("、")}。` +
      `整条目取并按 doc_time 升序，因为 supersedes 只在同一条目的相邻快照之间发生。` +
      (corpus.note ? ` 源语料说明：${corpus.note}` : ""),
    source: corpus.source,
    license: corpus.license,
    sampling: corpus.sampling,
    subset_of: corpus.name,
    docs,
  }),
);

// 统计走 stderr，这样 stdout 可以直接重定向成语料文件
console.error(
  `${docs.length} 张快照，${Math.round(chars / 1000)}k 字符，` +
    // 1200 字符预算、150 重叠，见 utopia-ingest 的 chunk_text
    `约 ${Math.round(chars / 1050)} 块`,
);
