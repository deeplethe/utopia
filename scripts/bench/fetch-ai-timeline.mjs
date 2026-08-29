#!/usr/bin/env node
// 抓「AI 公司发展节点」语料：维基百科正文 → bench 语料格式。
//
// 为什么是这一批而不是国情咨文：国情咨文量过一次，758 条事实里只有 33 条带
// 日期（4.4%），supersedes 只有 2 条。政论在**主张**，不在**记录**，句子里
// 本来就没有日期。这里选的每一篇，事实都长成「某年某月某日，X 做了 Y」。
//
// 更重要的是**同一个谓词会改值**：OpenAI 的 CEO 在 2023-11-17 到 11-22 六天里
// 换了四次（Altman → Murati → Shear → Altman）。双时态要演的就是这个——
// 不是「知识库里有时间字段」，是「同一件事我们先后信过三个答案，且三个都还在」。
//
// 许可：维基百科正文 CC BY-SA 4.0，可再分发但**要求署名与相同方式共享**。
// 这跟公共领域的国情咨文不同，语料文件里单独标了 license，别当成仓库主许可。
//
// 用法：node scripts/bench/fetch-ai-timeline.mjs > scripts/bench/corpora/ai-timeline.json

import { execFileSync } from "node:child_process";

const UA = "Utopia-bench/0.1 (+https://utopia.bi; corpus builder)";

// **走 curl，不走 fetch。** 这台机器上 HTTP(S)_PROXY 指向本地代理，
// Node 20 的 undici 不读这两个环境变量（NODE_USE_ENV_PROXY 是 24 才加的），
// 于是 fetch 全部 UND_ERR_CONNECT_TIMEOUT，而同一个地址 curl 返回 200。
// 语料脚本是一次性工具，为它引一个 undici ProxyAgent 依赖不值当。
const curl = (url) =>
  execFileSync("curl", ["-sSL", "--compressed", "--max-time", "60", "-A", UA, url], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });

// 分三层，各自承担 demo 里的一个镜头
const TITLES = [
  // 一、认知改变：同一个谓词、六天、四个值。这是整份语料的主角
  "Removal of Sam Altman from OpenAI",
  // 二、机构：成立日、融资轮、估值、人事，全部带日期且会变
  "OpenAI",
  "Anthropic",
  "DeepMind",
  "Mistral AI",
  "Hugging Face",
  "Stability AI",
  "Inflection AI",
  "Safe Superintelligence",
  "XAI (company)",
  // 三、产物：版本互相取代，天然是一条 valid_from/valid_to 链
  "GPT-4",
  "ChatGPT",
  "Claude (language model)",
  "Gemini (language model)",
  "Llama (language model)",
];

function extract(title) {
  const u = new URL("https://en.wikipedia.org/w/api.php");
  u.searchParams.set("action", "query");
  u.searchParams.set("prop", "extracts");
  u.searchParams.set("explaintext", "1");
  u.searchParams.set("redirects", "1");
  u.searchParams.set("format", "json");
  u.searchParams.set("formatversion", "2");
  u.searchParams.set("titles", title);
  const page = JSON.parse(curl(u.toString())).query.pages[0];
  if (page.missing) throw new Error(`${title}: 条目不存在`);
  return { title: page.title, text: page.extract || "" };
}

// 末尾的 References / External links / See also 全是链接和模板残渣，
// 抽取器会把它们当正文，产出一堆没有关系的孤点。切掉。
const CUT = /\n==+ ?(References|External links|See also|Further reading|Notes|Bibliography|Sources) ?==+/i;
const clean = (t) => t.split(CUT)[0].replace(/\n{3,}/g, "\n\n").trim();

const docs = [];
for (const title of TITLES) {
  try {
    const { title: real, text } = extract(title);
    const body = clean(text);
    const slug = real.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
    docs.push([`${slug}.txt`, `${real}\n\n${body}\n`]);
    process.stderr.write(`OK  ${real.padEnd(38)} ${String(body.length).padStart(7)} 字符\n`);
  } catch (e) {
    process.stderr.write(`ERR ${title}: ${e.message}\n`);
  }
}

const total = docs.reduce((n, [, t]) => n + t.length, 0);
process.stderr.write(`\n共 ${docs.length} 篇、${total.toLocaleString()} 字符，约 ${Math.round(total / 950)} 块\n`);

process.stdout.write(JSON.stringify({
  name: "ai-timeline",
  note: "AI 公司发展节点。选它是因为国情咨文那份量下来时间几乎是空的（758 条事实只有 33 条带 valid_from，supersedes 只有 2 条）——政论在主张，不在记录。这里每篇的句子本身就带日期，且同一个谓词会改值：OpenAI 的 CEO 在 2023-11-17 到 11-22 之间换了四次，是双时态最直白的一段素材。已切掉 References/External links 等尾节，那些是链接残渣，只会产出孤点。注意：这批公司在模型训练数据里极常见，所以它适合做 demo（认得出是优点），不适合当准确率基准（量到的是背诵）。",
  source: "https://en.wikipedia.org/ — MediaWiki action=query&prop=extracts",
  license: "CC BY-SA 4.0（署名-相同方式共享，与仓库主许可不同）",
  docs,
}, null, 1));
