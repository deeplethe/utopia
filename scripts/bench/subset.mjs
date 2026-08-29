#!/usr/bin/env node
// 把 schema.org 的 TTL 切成前 N 个类的子集，给退化曲线用。
//
// 曲线要回答的是：**内联多少词汇量之后抽取开始掉东西**。那条曲线定
// `ONTOLOGY_PROMPT_BUDGET` 与每块检索多少个候选，不测就是拍脑袋。
//
// 为什么不用"导入全量再限制内联数"：那样量到的是"检索选得准不准"，
// 混进了检索的质量。切子集 + 全量内联，量的才是纯粹的规模效应。
//
// 用法：node scripts/bench/subset.mjs /tmp/schemaorg.ttl 100 > /tmp/schemaorg-100.ttl

import fs from "node:fs";

const [, , src, nRaw] = process.argv;
const N = Number(nRaw);
if (!src || !Number.isFinite(N)) {
  console.error("用法: subset.mjs <schemaorg.ttl> <类数>");
  process.exit(2);
}

const text = fs.readFileSync(src, "utf8");
const lines = text.split("\n");

// 前缀块原样保留：切掉它文件就解析不了
const prefixEnd = lines.findIndex((l) => l.startsWith("@prefix") === false && l.trim() && !l.startsWith("#"));
const prefixes = lines.slice(0, prefixEnd).join("\n");

// TTL 的块结构很规整：`subject a …` 起头，以行尾的 `.` 收尾
const blocks = [];
let cur = [];
for (const line of lines.slice(prefixEnd)) {
  cur.push(line);
  if (/\.\s*$/.test(line) && !/^\s*@/.test(line)) {
    blocks.push(cur.join("\n"));
    cur = [];
  }
}

const subjectOf = (b) => (b.match(/^\s*(\S+)\s+a\s/m) || [])[1] || "";
const isClass = (b) => /\ba\s+rdfs:Class\b/.test(b);
const isProp = (b) => /\ba\s+rdf:Property\b/.test(b);

// 取前 N 个类。**保序而不是随机取**：同一个 N 每次得到同一份子集，
// 两次跑出的差别才归因得到别处
const classes = blocks.filter(isClass);
const keep = new Set(classes.slice(0, N).map(subjectOf).filter(Boolean));

// 属性：domainIncludes 落在保留的类里就留。留下指向被切掉的类的属性没有意义
//——那些 domain 解析不出来，导入时本来就会被跳过
const props = blocks.filter(isProp).filter((b) => {
  const m = b.match(/schema:domainIncludes([^;.]*)/);
  if (!m) return false;
  return m[1]
    .split(",")
    .map((x) => x.trim().replace(/[.;]$/, ""))
    .some((x) => keep.has(x));
});

const kept = blocks.filter((b) => isClass(b) && keep.has(subjectOf(b)));
process.stdout.write(prefixes + "\n\n" + kept.concat(props).join("\n\n") + "\n");
process.stderr.write(`保留 ${kept.length} 个类、${props.length} 个属性\n`);
