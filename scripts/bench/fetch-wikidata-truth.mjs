#!/usr/bin/env node
// 外部答案卷（0016 的 C1）：语料里的实体该是什么类，让 Wikidata 说，不让人填。
//
// `truth/*.json` 到今天都是手填的。手填的答案有两个毛病：写窄了（`心血管健康论坛`
// 只写了 business_event，系统答 conference_event 明明是对的）、写完就成了「系统这次
// 答了什么」的记录——改答案还是改系统，分不清。0006 的预算曲线、0008 的混合包
// 对比都需要一把不随我们的判断变的尺子。
//
// 这把尺子：语料出自维基百科，每篇的主题在 Wikidata 上都有一个 P31（instance of）。
// 文中链接出去的条目也是实体，同样有 P31。把它们抓下来，就是一份不经人手的答案。
// 这里只抓**原始材料**（QID、P31、类的标签与上一级），不做「P31 → 本体里的类」的
// 映射——映射是一张要经人审的小表，另放一个文件，两者分开才看得出哪一边错了。
//
// 两个来源：维基百科 API 给 QID 与页内链接，Wikidata 的 SPARQL 端点给 P31 与标签
// （一次一批，比逐个 wbgetentities 拉全部 claims 省两个数量级）。
//
// 页内链接**按正文过滤**：条目底部的导航框把几百个不相干的页面也算作链接
// （Anthropic 那篇 400 个，「2026 Iran war」都在里面），只留名字确实出现在
// 正文里的——抽取看得见的，才可能被抽成实体。
//
// 用法：
//   node scripts/bench/fetch-wikidata-truth.mjs --corpus ai-timeline
//   node scripts/bench/fetch-wikidata-truth.mjs --corpus ai-timeline-ends --no-links
// 输出：truth/<corpus>.wikidata.raw.json（可重抓、可 diff）

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const UA = "Utopia-bench/0.1 (+https://utopia.bi; truth builder)";

// **走 curl，不走 fetch**：理由见 fetch-ai-timeline.mjs（本机代理与 Node 20 的 undici）
const curl = (url) =>
  execFileSync("curl", ["-sSL", "--compressed", "--max-time", "90", "-A", UA, url], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
const pause = (ms) => new Promise((r) => setTimeout(r, ms));

const args = process.argv.slice(2).reduce((acc, cur, i, arr) => {
  if (cur.startsWith("--")) {
    const key = cur.slice(2);
    const next = arr[i + 1];
    acc[key] = next && !next.startsWith("--") ? next : true;
  }
  return acc;
}, {});

if (!args.corpus) {
  console.error("usage: fetch-wikidata-truth.mjs --corpus <name> [--out file] [--no-links]");
  process.exit(2);
}

const corpusPath = path.join(HERE, "corpora", args.corpus + ".json");
const corpus = JSON.parse(fs.readFileSync(corpusPath, "utf8"));
if (!String(corpus.source ?? "").includes("wikipedia.org")) {
  console.error(`${args.corpus} did not come from Wikipedia; there is no Wikidata answer for it`);
  process.exit(2);
}
const outPath = args.out ?? path.join(HERE, "truth", args.corpus + ".wikidata.raw.json");
const withLinks = args["no-links"] !== true;

// ---------- 维基百科 ----------

function wikiUrl(params) {
  const u = new URL("https://en.wikipedia.org/w/api.php");
  for (const [k, v] of Object.entries(params)) u.searchParams.set(k, v);
  u.searchParams.set("format", "json");
  u.searchParams.set("formatversion", "2");
  return u.toString();
}

/// 一篇条目：解析重定向后的标题、QID、页内链接（正文命名空间，翻页取全）
async function pageOf(title) {
  let links = [];
  let resolved = title;
  let qid = null;
  let cont = {};
  for (;;) {
    const params = {
      action: "query",
      prop: withLinks ? "pageprops|links" : "pageprops",
      ppprop: "wikibase_item",
      redirects: "1",
      titles: title,
      ...(withLinks ? { pllimit: "max", plnamespace: "0" } : {}),
      ...cont,
    };
    const d = JSON.parse(curl(wikiUrl(params)));
    const page = d.query?.pages?.[0];
    if (!page || page.missing) return null;
    resolved = page.title;
    qid = page.pageprops?.wikibase_item ?? qid;
    for (const l of page.links ?? []) links.push(l.title);
    if (!d.continue) break;
    cont = d.continue;
    await pause(150);
  }
  return { title: resolved, qid, links };
}

/// 一批标题 → { 原标题: { title, qid } }。重定向按响应里的 redirects 表回填
async function qidsFor(titles) {
  const out = {};
  for (let i = 0; i < titles.length; i += 50) {
    const batch = titles.slice(i, i + 50);
    const d = JSON.parse(
      curl(
        wikiUrl({
          action: "query",
          prop: "pageprops",
          ppprop: "wikibase_item",
          redirects: "1",
          titles: batch.join("|"),
        }),
      ),
    );
    const to = new Map();
    for (const r of d.query?.redirects ?? []) to.set(r.from, r.to);
    for (const n of d.query?.normalized ?? []) to.set(n.from, n.to);
    const byTitle = new Map();
    for (const p of d.query?.pages ?? []) byTitle.set(p.title, p);
    for (const t of batch) {
      let key = t;
      for (let hop = 0; hop < 3 && to.has(key); hop++) key = to.get(key);
      const p = byTitle.get(key);
      if (p && !p.missing && p.pageprops?.wikibase_item) {
        out[t] = { title: p.title, qid: p.pageprops.wikibase_item };
      }
    }
    await pause(150);
  }
  return out;
}

// ---------- Wikidata ----------

function sparql(query) {
  const u = new URL("https://query.wikidata.org/sparql");
  u.searchParams.set("query", query);
  u.searchParams.set("format", "json");
  return JSON.parse(curl(u.toString())).results.bindings;
}
const qidOf = (b) => b.value.split("/").pop();

/// QID → [P31 类]
async function classesFor(qids) {
  const out = {};
  for (let i = 0; i < qids.length; i += 100) {
    const batch = qids.slice(i, i + 100);
    const rows = sparql(
      `SELECT ?item ?class WHERE { VALUES ?item { ${batch.map((q) => "wd:" + q).join(" ")} } ?item wdt:P31 ?class }`,
    );
    for (const b of rows) (out[qidOf(b.item)] ??= []).push(qidOf(b.class));
    await pause(300);
  }
  for (const q of Object.keys(out)) out[q] = [...new Set(out[q])].sort();
  return out;
}

/// 锚类：P31 的类有几百个（daily newspaper、benefit corporation、very large online
/// platform……），逐个映射到本体不现实；顺着 P279（subclass of）往上走，绝大多数都落在
/// 这几十个锚下面。映射表只对锚类写，人审得过来。一个类可以落在多个锚下
/// （software company 既是 business 也是 company），映射时都算
const ANCHORS = {
  Q5: "human",
  Q43229: "organization",
  Q4830453: "business",
  Q783794: "company",
  Q163740: "nonprofit organization",
  Q3918: "university",
  Q31855: "research institute",
  Q327333: "government agency",
  Q7278: "political party",
  Q7397: "software",
  Q115305900: "large language model",
  Q117349473: "artificial intelligence model",
  Q870780: "chatbot",
  Q35127: "website",
  Q1668024: "service on Internet",
  Q2424752: "product",
  Q431289: "brand",
  Q1656682: "event",
  Q1190554: "occurrence",
  Q515: "city",
  Q6256: "country",
  Q56061: "administrative territorial entity",
  Q82794: "geographic region",
  Q1002697: "periodical",
  Q11032: "newspaper",
  Q41298: "magazine",
  Q7889: "video game",
  Q11424: "film",
  Q571: "book",
  Q11862829: "academic discipline",
  Q151885: "concept",
  Q11514315: "historical period",
  Q486972: "human settlement",
  Q10929058: "product model",
  Q7725634: "literary work",
  Q34770: "language",
};

/// 类 → { label, subclass_of, anchors }：映射表靠标签认出它是什么，靠锚类归并同义的细类
async function describeClasses(qids) {
  const out = {};
  const anchorValues = Object.keys(ANCHORS)
    .map((q) => "wd:" + q)
    .join(" ");
  for (let i = 0; i < qids.length; i += 50) {
    const batch = qids.slice(i, i + 50);
    const values = batch.map((q) => "wd:" + q).join(" ");
    const rows = sparql(
      `SELECT ?c ?cLabel ?parent WHERE { VALUES ?c { ${values} } OPTIONAL { ?c wdt:P279 ?parent } SERVICE wikibase:label { bd:serviceParam wikibase:language "en". } }`,
    );
    for (const b of rows) {
      const q = qidOf(b.c);
      const entry = (out[q] ??= { label: b.cLabel?.value ?? q, subclass_of: [], anchors: [] });
      if (b.parent) entry.subclass_of.push(qidOf(b.parent));
    }
    await pause(300);
    // 传递闭包由 SPARQL 算：一个类落在哪些锚下
    const under = sparql(
      `SELECT ?c ?anchor WHERE { VALUES ?c { ${values} } VALUES ?anchor { ${anchorValues} } ?c wdt:P279* ?anchor }`,
    );
    for (const b of under) {
      const q = qidOf(b.c);
      (out[q] ??= { label: q, subclass_of: [], anchors: [] }).anchors.push(qidOf(b.anchor));
    }
    await pause(300);
  }
  for (const q of Object.keys(out)) {
    out[q].subclass_of = [...new Set(out[q].subclass_of)].sort();
    out[q].anchors = [...new Set(out[q].anchors)].sort();
  }
  return out;
}

// ---------- 主流程 ----------

/// 条目标题 → 抽取会看到的名字：去掉消歧括号（"Claude (AI)" → "Claude"）
const nameOf = (title) => title.replace(/\s*\([^)]*\)\s*$/, "").trim();

/// 正文里出现过、又像个东西的链接才留下：数字开头的是日期与年份，"List of" 是清单页
function keepLink(title, text) {
  const name = nameOf(title);
  if (name.length < 3) return false;
  if (/^\d/.test(name)) return false;
  if (/^List of /.test(name)) return false;
  return text.includes(name);
}

const subjects = [];
const entities = {};
const docs = corpus.docs.map(([file, text]) => ({ file, text, title: text.split("\n")[0].trim() }));

for (const doc of docs) {
  process.stderr.write(`${doc.title} … `);
  const page = await pageOf(doc.title);
  if (!page) {
    process.stderr.write("missing\n");
    continue;
  }
  subjects.push({ doc: doc.file, title: page.title, name: nameOf(page.title), qid: page.qid });
  const kept = page.links.filter((t) => keepLink(t, doc.text));
  process.stderr.write(`${page.qid ?? "no QID"}, ${page.links.length} links, ${kept.length} in the text\n`);
  for (const t of kept) (entities[t] ??= { from: [] }).from.push(doc.file);
  await pause(150);
}

// 链接标题 → QID（重定向后的标题一并记下，名字以它为准）
const linkTitles = Object.keys(entities).sort();
const resolved = await qidsFor(linkTitles);
const byName = {};
for (const t of linkTitles) {
  const r = resolved[t];
  if (!r) continue;
  const name = nameOf(r.title);
  const e = (byName[name] ??= { qid: r.qid, title: r.title, p31: [], from: [] });
  for (const f of entities[t].from) if (!e.from.includes(f)) e.from.push(f);
}
// 主题条目自己也是实体
for (const s of subjects) {
  if (!s.qid) continue;
  const e = (byName[s.name] ??= { qid: s.qid, title: s.title, p31: [], from: [] });
  if (!e.from.includes(s.doc)) e.from.push(s.doc);
}

const allQids = [...new Set(Object.values(byName).map((e) => e.qid))].sort();
process.stderr.write(`${allQids.length} entities → Wikidata\n`);
const classes = await classesFor(allQids);
for (const e of Object.values(byName)) e.p31 = classes[e.qid] ?? [];
for (const s of subjects) s.p31 = s.qid ? (classes[s.qid] ?? []) : [];
const classQids = [...new Set(Object.values(classes).flat())].sort();
process.stderr.write(`${classQids.length} classes → labels\n`);
const described = await describeClasses(classQids);

const sortedEntities = Object.fromEntries(
  Object.keys(byName)
    .sort((a, b) => a.localeCompare(b))
    .map((k) => [k, { ...byName[k], from: byName[k].from.sort() }]),
);
const out = {
  corpus: args.corpus,
  generated_at: new Date().toISOString(),
  source:
    "Wikipedia API (pageprops.wikibase_item, links) + Wikidata SPARQL (wdt:P31, wdt:P279, rdfs:label)",
  note:
    "Raw material for an answer key: which Wikidata class each entity is an instance of. " +
    "The mapping from these classes to ontology classes lives in p31-to-classes.json; keep the two apart.",
  anchors: ANCHORS,
  subjects,
  entities: sortedEntities,
  classes: Object.fromEntries(Object.keys(described).sort().map((k) => [k, described[k]])),
};
fs.writeFileSync(outPath, JSON.stringify(out, null, 2) + "\n");
process.stderr.write(
  `wrote ${path.relative(process.cwd(), outPath)}: ${subjects.length} subjects, ${Object.keys(sortedEntities).length} entities, ${classQids.length} classes\n`,
);
