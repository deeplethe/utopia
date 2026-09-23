#!/usr/bin/env node
// 类型图测量台的语料与答案卷（0044 §Measurement「Typed graph」：Re-DocRED，它的属性当已批准的本体）。
//
// 拿 Re-DocRED（MIT，tonytan48/Re-DocRED）的测试集，按种子抽 N 篇，写成三份文件：
//   corpora/redocred-<N>.json          每篇一段正文（标题 + 句子），灌库用
//   truth/redocred-<N>.json            金标三元组：(主语提及名集合, 属性, 宾语提及名集合, 同句与否)
//   truth/redocred-ontology.json       95 条属性：Wikidata 的标签与定义，加上**从训练集统计出来的**
//                                      定义域/值域（6 个粗类型：PER ORG LOC TIME NUM MISC）
//
// 定义域/值域为什么从训练集统计而不留空：不声明的属性对每条签名都是候选，95 条一齐进候选
// 就超过对齐器的上限（60），每条签名都会溢出成 undecided，对齐根本跑不起来。原型（0044 §4）
// 靠 Wikidata↔schema.org 的等价切片；这里用金标自己的类型分布，训练集学、测试集评，不碰
// 测试集的标签。一个属性只声明训练集里见过的 (主语类型, 宾语类型)。
//
// 用法：node scripts/bench/fetch-redocred.mjs --n 100 --seed 1 [--dir /tmp/redocred]
// `--dir` 放原始 json（test_revised.json、train_revised.json，各 3 MB / 19 MB，不进仓库）；
// 没有就下载。rel_info.json 从 Wikidata 取标签（DocRED 仓库里那份路径已不可用）。

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "./lib.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv);
const N = Number(args.n || 100);
const SEED = Number(args.seed || 1);
const DIR = args.dir || path.join(process.env.TMPDIR || "/tmp", "redocred");
fs.mkdirSync(DIR, { recursive: true });

const RAW = "https://raw.githubusercontent.com/tonytan48/Re-DocRED/main/data/";
async function fetchTo(file, url) {
  const p = path.join(DIR, file);
  if (fs.existsSync(p)) return p;
  console.error(`下载 ${url}`);
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url} -> ${r.status}`);
  fs.writeFileSync(p, Buffer.from(await r.arrayBuffer()));
  return p;
}

// 可复现的抽样：mulberry32，同一个种子同一批文档
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const test = JSON.parse(fs.readFileSync(await fetchTo("test_revised.json", RAW + "test_revised.json"), "utf8"));
const train = JSON.parse(fs.readFileSync(await fetchTo("train_revised.json", RAW + "train_revised.json"), "utf8"));

const rand = rng(SEED);
const order = test.map((d, i) => [rand(), i]).sort((a, b) => a[0] - b[0]).map(([, i]) => i);
const picked = order.slice(0, N).sort((a, b) => a - b).map((i) => test[i]);

// 属性标签：先看本地的 rel_info.json，没有就问 Wikidata（批量 50）
const relInfoPath = path.join(DIR, "rel_info.json");
let relInfo = fs.existsSync(relInfoPath) ? JSON.parse(fs.readFileSync(relInfoPath, "utf8")) : {};
const used = [...new Set(test.flatMap((d) => d.labels.map((l) => l.r)))].sort();
const missing = used.filter((p) => !relInfo[p]);
for (let i = 0; i < missing.length; i += 50) {
  const ids = missing.slice(i, i + 50);
  const url = "https://www.wikidata.org/w/api.php?" + new URLSearchParams({
    action: "wbgetentities", ids: ids.join("|"), props: "labels|descriptions", languages: "en", format: "json",
  });
  const r = await fetch(url, { headers: { "user-agent": "utopia-bench/0.1 (typed-graph bench)" } });
  const j = await r.json();
  for (const [pid, e] of Object.entries(j.entities)) {
    relInfo[pid] = { label: e.labels?.en?.value ?? pid, description: e.descriptions?.en?.value ?? "" };
  }
}
fs.writeFileSync(relInfoPath, JSON.stringify(relInfo, null, 1));

// 定义域/值域：训练集里每个属性观察到的 (主语类型, 宾语类型)
const TYPES = ["PER", "ORG", "LOC", "TIME", "NUM", "MISC"];
const seen = {};
for (const d of train) {
  for (const l of d.labels) {
    const h = d.vertexSet[l.h][0].type, t = d.vertexSet[l.t][0].type;
    const s = (seen[l.r] ??= { domains: {}, ranges: {}, n: 0 });
    s.domains[h] = (s.domains[h] || 0) + 1;
    s.ranges[t] = (s.ranges[t] || 0) + 1;
    s.n += 1;
  }
}
// 只留占该属性 ≥ 2% 的类型：训练集的标注噪声（一个 LOC 被标成 MISC）不该把声明撑到不限
const keep = (counts, n) => TYPES.filter((t) => (counts[t] || 0) >= Math.max(1, n * 0.02));
const ontology = used.map((pid) => {
  const s = seen[pid] ?? { domains: {}, ranges: {}, n: 0 };
  return {
    key: pid,
    label: relInfo[pid]?.label ?? pid,
    description: relInfo[pid]?.description ?? "",
    domains: keep(s.domains, s.n),
    ranges: keep(s.ranges, s.n),
    train_count: s.n,
  };
});

// 语料：标题 + 句子；实体提及名照原文
const corpus = picked.map((d, i) => ({
  filename: `redocred-${String(i).padStart(3, "0")}.txt`,
  title: d.title,
  text: `${d.title}\n\n${d.sents.map((s) => s.join(" ")).join(" ")}`,
}));
// 答案卷：一条金标 = 主语的全部提及名、属性、宾语的全部提及名、两端有没有共同出现在一句里
const truth = picked.map((d, i) => {
  const sentsOf = (v) => new Set(d.vertexSet[v].map((m) => m.sent_id));
  return {
    filename: corpus[i].filename,
    entities: d.vertexSet.map((v) => ({ names: [...new Set(v.map((m) => m.name))], type: v[0].type })),
    facts: d.labels.map((l) => ({
      h: l.h, r: l.r, t: l.t,
      same_sentence: [...sentsOf(l.h)].some((s) => sentsOf(l.t).has(s)),
    })),
  };
});

const out = (rel, data) => { const p = path.join(HERE, rel); fs.writeFileSync(p, JSON.stringify(data, null, 1)); return p; };
console.log(out(`corpora/redocred-${N}.json`, { source: "Re-DocRED test_revised.json (MIT, tonytan48/Re-DocRED)", seed: SEED, docs: corpus }));
console.log(out(`truth/redocred-${N}.json`, { seed: SEED, docs: truth }));
console.log(out(`truth/redocred-ontology.json`, { source: "labels: Wikidata; domains/ranges: Re-DocRED train_revised.json", properties: ontology }));
const gold = truth.reduce((n, d) => n + d.facts.length, 0);
const same = truth.reduce((n, d) => n + d.facts.filter((f) => f.same_sentence).length, 0);
console.log(`${N} 篇，${gold} 条金标（同句 ${same}，跨句 ${gold - same}），${ontology.length} 条属性`);
