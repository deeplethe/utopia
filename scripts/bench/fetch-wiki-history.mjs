#!/usr/bin/env node
// 抓「维基百科条目的历史快照」语料：同一个条目在不同时刻的版本 → bench 语料格式。
//
// **与 fetch-ai-timeline.mjs 的区别是整份语料的意义所在。** 那一份抓的是每个条目的
// **当前**版本——15 篇回顾性总述，灌一篇 openai.txt 进去，2015 到今天的整条时间线
// 一次性全出来。它演得了世界时间（句子里带日期），演不了**认知时间**：所有文档同一刻
// 录入，`recorded_at` 全挤在一起，`supersedes` 只在单篇内部发生。
//
// 双时态有两根轴，我们此前只演过一根。
//
// 这一份抓的是**历史版本**。每个快照就是「那个时刻人们知道什么」，按 `doc_time`
// 顺序灌进去，图谱会真的随时间生长、并且真的改主意：
//
//   OpenAI 条目 2015-12-12 建立时 1,317 字节（一个 stub），到 2026 已 60KB+
//   Removal of Sam Altman 条目 11-19 的版本里 Murati 是临时 CEO，11-22 的版本里 Altman 回来了
//
// **采样按「变了多少」，不按日历。** 早期一个月一变，后期半年不动——按季度取会
// 在后期抓一堆几乎相同的快照（白烧抽取），在前期又漏掉最剧烈的那段。所以用
// 体量增量当信号：涨够 GROWTH_PCT 且不少于 GROWTH_ABS 才取一张，两张之间至少隔 MIN_GAP_DAYS。
//
// 许可：维基百科正文 CC BY-SA 4.0，可再分发但**要求署名与相同方式共享**。
// 跟公共领域的国情咨文不同，语料文件里单独标了 license，别当成仓库主许可。
//
// **正文与清单都不进仓库**（见 .gitignore）：正文约 7MB 的 CC BY-SA 文本，
// 清单是一次采样的产物。进仓库的只有这个脚本。
//
// 但**同一次跑测内部必须钉住修订号**：采样是按当下的修订历史算的，条目还在被编辑，
// 隔一阵重跑 --dry 会挑出另一组快照。所以先 --manifest 写一份清单，
// 之后一律 --from-manifest 重建——`action=parse&oldid` 不可变，
// 按同一份清单任何时候重抓都是逐字节相同的文本。对照实验靠这个才成立。
//
// 用法：node scripts/bench/fetch-wiki-history.mjs --dry       # 只报采样结果与体量
//       node scripts/bench/fetch-wiki-history.mjs --manifest  # 写清单（不抓正文）
//       node scripts/bench/fetch-wiki-history.mjs --from-manifest > scripts/bench/corpora/wiki-history.json

import { execFileSync } from "node:child_process";
import fs from "node:fs";

const UA = "Utopia-bench/0.1 (+https://utopia.bi; corpus builder)";
const DRY = process.argv.includes("--dry");
const WRITE_MANIFEST = process.argv.includes("--manifest");
const FROM_MANIFEST = process.argv.includes("--from-manifest");
const MANIFEST_PATH = "scripts/bench/corpora/wiki-history.manifest.json";

// **走 curl，不走 fetch。** 这台机器上 HTTP(S)_PROXY 指向本地代理，
// Node 20 的 undici 不读这两个环境变量，于是 fetch 全部 UND_ERR_CONNECT_TIMEOUT，
// 而同一个地址 curl 返回 200。与 fetch-ai-timeline.mjs 同一个理由。
const curl = (url) =>
  execFileSync(
    "curl",
    ["-sSL", "--compressed", "--max-time", "90", "-A", UA, url],
    {
      encoding: "utf8",
      maxBuffer: 128 * 1024 * 1024,
    },
  );

const api = (params) => {
  const u = new URL("https://en.wikipedia.org/w/api.php");
  u.searchParams.set("format", "json");
  u.searchParams.set("formatversion", "2");
  for (const [k, v] of Object.entries(params)) u.searchParams.set(k, v);
  return JSON.parse(curl(u.toString()));
};

// 取一张快照的门槛。**两个条件同时满足**才取。
//
// 第一版写的是「或」，结果 Elon Musk 那篇（涨到 340KB）每 6KB 就取一张，
// 单篇 89 张、占整份语料六成——而它大半在讲 Tesla/SpaceX/政治，会把图冲淡。
// 改成「且」之后，大条目按比例走成对数增长，小条目仍有绝对下限挡住噪声。
const GROWTH_PCT = 0.18; // 比上一张涨/缩 18%
const GROWTH_ABS = 6000; // 且绝对变化不少于 6KB
const MIN_GAP_DAYS = 45; // 两张之间至少隔这么久（高密度窗口不受此限）

// **条目选得互相咬合**：机构 × 人 × 产物。人是交叉链接的来源——
// Altman 出现在 OpenAI / Y Combinator / Worldcoin，Musk 出现在 OpenAI / Tesla / xAI，
// Sutskever 出现在 OpenAI / SSI / Google Brain。只抓机构的话，图是几个互不相连的星团。
const TITLES = [
  // 一、认知改变的主角：六天四个值。这一篇按天取，不按增量
  {
    title: "Removal of Sam Altman from OpenAI",
    daily: ["2023-11-19", "2023-11-29"],
  },

  // 二、机构
  { title: "OpenAI" },
  { title: "Anthropic" },
  { title: "DeepMind" },
  { title: "XAI (company)" },
  { title: "Mistral AI" },
  { title: "Hugging Face" },
  { title: "Stability AI" },
  { title: "Inflection AI" },
  { title: "Safe Superintelligence" },
  { title: "Scale AI" },
  { title: "Cohere" },

  // 三、人。**这一层是「有关联」的来源**：同一个人在多个机构里出现，
  //    而且他们的从属关系随时间改变——正是双时态要演的东西
  { title: "Sam Altman" },
  { title: "Elon Musk" },
  { title: "Ilya Sutskever" },
  { title: "Greg Brockman" },
  { title: "Mira Murati" },
  { title: "Dario Amodei" },
  { title: "Demis Hassabis" },
  { title: "Emmett Shear" },
  { title: "Satya Nadella" },

  // 四、产物：版本互相取代，天然是一条 valid_from/valid_to 链
  { title: "GPT-4" },
  { title: "ChatGPT" },
  { title: "Claude (language model)" },
  { title: "Gemini (language model)" },
  { title: "Llama (language model)" },
];

/// 列一个条目的全部修订（时间戳 + 体量）。分页取满。
function revisions(title) {
  const out = [];
  let cont = null;
  for (let page = 0; page < 40; page++) {
    const p = {
      action: "query",
      prop: "revisions",
      titles: title,
      redirects: "1",
      rvlimit: "500",
      rvprop: "ids|timestamp|size",
      rvdir: "newer",
    };
    if (cont) p.rvcontinue = cont;
    const j = api(p);
    const pg = j.query.pages[0];
    if (pg.missing) throw new Error(`${title}: 条目不存在`);
    out.push(...(pg.revisions || []));
    cont = j.continue?.rvcontinue;
    if (!cont) return { real: pg.title, revs: out };
  }
  return { real: title, revs: out };
}

const day = (ts) => ts.slice(0, 10);
const days = (a, b) => (new Date(b) - new Date(a)) / 86400000;

/// 这个体量有没有持续下来。
///
/// **「变了很多」包含「有人把页面清空了」。** 实测撞上过：Elon Musk 条目
/// 2018-05-24T09:35:49 那一版只有 33 字节（编辑摘要 "Replaced content with…"），
/// 141637 → 33 两个门槛都满足，于是被选中；30 秒后被回退，而基线已经被拉到 33，
/// 于是「恢复」也成了一次巨变、又被选中一次。**一次破坏产出两张垃圾快照，
/// 还把后续采样的基线搅乱了。**
///
/// 设一个体量下限挡不住这个：条目被拆分（内容移去子条目）是合法的大幅缩水，
/// 与破坏在「变了多少」上分不开。分得开的是**持续时间**——破坏几分钟内就被回退，
/// 拆分则一直保持。所以看这一版之后 PERSIST_DAYS 天的体量还在不在同一量级。
const PERSIST_DAYS = 1;
function persists(revs, i) {
  const r = revs[i];
  for (let j = i + 1; j < revs.length; j++) {
    if (days(r.timestamp, revs[j].timestamp) < PERSIST_DAYS) continue;
    const hi = Math.max(revs[j].size, r.size);
    return hi === 0 || Math.abs(revs[j].size - r.size) / hi < 0.5;
  }
  return true; // 之后没有更晚的修订了：它就是当前状态
}

/// 按「变了多少」挑快照。首版与末版一定要。
function sampleByGrowth(revs) {
  const picked = [revs[0]];
  for (let i = 1; i < revs.length; i++) {
    const r = revs[i];
    const last = picked[picked.length - 1];
    const d = Math.abs(r.size - last.size);
    const grew = d >= GROWTH_ABS && d >= last.size * GROWTH_PCT;
    if (
      grew &&
      days(last.timestamp, r.timestamp) >= MIN_GAP_DAYS &&
      persists(revs, i)
    )
      picked.push(r);
  }
  const last = revs[revs.length - 1];
  if (picked[picked.length - 1].revid !== last.revid) picked.push(last);
  return picked;
}

/// 高密度窗口：窗内每天取当天最后一版。演的是「一天之内我们改了几次主意」。
function sampleDaily(revs, [from, to]) {
  const byDay = new Map();
  for (let i = 0; i < revs.length; i++) {
    const d = day(revs[i].timestamp);
    // 当天末版本身也可能是破坏（当天最后一次编辑恰好是清空），一样过持续性检查
    if (d >= from && d < to && persists(revs, i)) byDay.set(d, revs[i]);
  }
  return [...byDay.values()];
}

// 末尾的 References / External links 全是链接和模板残渣，抽取器会把它们当正文。切掉。
//
// **收尾侧写 `=+` 而不是 `==+`，是有意宽一格的。** 这里曾经两边都是 `==+`，
// 而下面 h 标签的转换开标签按层级铺 `=`、闭标签硬编码了一个，于是二级标题
// 落成 `== References =`，这条正则一次都没匹配上——**每一篇快照的整个参考
// 文献区都进了抽取**。实测 414 块里 223 块（54%）是引文残渣，产出的是
// `Wired --employee--> Steven Levy` 这种把记者署名当雇佣关系的事实，
// 还占着 supersedes 机制。
//
// 闭标签那侧已经修好，但这条仍然放宽：标题两侧对不对称是渲染的事，而这里
// 要判的是「从哪儿开始不要了」。宽一格换来同类错配再也打不穿它，代价是可能
// 多切一个 `= Foo =` 形状的一级标题——那种标题在条目正文里不出现。
const CUT =
  /\n==+ ?(References|External links|See also|Further reading|Notes|Bibliography|Sources|Citations) ?=+/i;

/// 取某一版的正文。历史版本没有 `prop=extracts`（那个只认当前版），
/// 所以走 `action=parse&oldid=` 拿渲染后的 HTML，再剥成纯文本。
function plaintext(revid) {
  const j = api({
    action: "parse",
    oldid: String(revid),
    prop: "text",
    disablelimitreport: "1",
  });
  let h = j.parse.text;
  return h
    .replace(/<style[\s\S]*?<\/style>/gi, "")
    .replace(/<script[\s\S]*?<\/script>/gi, "")
    .replace(/<table[\s\S]*?<\/table>/gi, "") // 信息框/导航框：全是模板残渣
    .replace(/<sup class="reference"[\s\S]*?<\/sup>/gi, "") // 脚注角标
    .replace(/<span class="mw-editsection"[\s\S]*?<\/span>/gi, "")
    .replace(/<h([1-6])[^>]*>/gi, (_, n) => "\n\n" + "=".repeat(+n) + " ")
    // 闭标签也按层级铺，跟上一行对称。硬编码一个 `=` 会让二级标题落成
    // `== References =`，而 CUT 那边在等 `==`，于是尾节永远切不掉。
    .replace(/<\/h([1-6])>/gi, (_, n) => " " + "=".repeat(+n) + "\n")
    .replace(/<li[^>]*>/gi, "\n- ")
    .replace(/<\/(p|div|li|tr)>/gi, "\n")
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<[^>]+>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#(\d+);/g, (_, n) => String.fromCharCode(+n))
    .replace(/\[edit\]/g, "")
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

const docs = [];
let plan = [];

if (FROM_MANIFEST) {
  // 按钉住的修订号精确重建。不碰采样逻辑，也不看条目当下的历史
  const m = JSON.parse(fs.readFileSync(MANIFEST_PATH, "utf8"));
  plan = m.titles.map((t) => ({
    real: t.real,
    total: t.total,
    picked: t.picked,
  }));
  process.stderr.write(
    `按清单重建：${m.titles.length} 个条目、${plan.reduce((n, q) => n + q.picked.length, 0)} 张快照\n`,
  );
} else
  for (const spec of TITLES) {
    try {
      const { real, revs } = revisions(spec.title);
      if (!revs.length) throw new Error("没有修订");
      const picked = spec.daily
        ? sampleDaily(revs, spec.daily)
        : sampleByGrowth(revs);
      plan.push({ real, total: revs.length, picked, slugBase: real });
      process.stderr.write(
        `${real.padEnd(36)} 修订 ${String(revs.length).padStart(5)} 条 → 取 ${String(picked.length).padStart(3)} 张` +
          `  ${day(picked[0].timestamp)} → ${day(picked[picked.length - 1].timestamp)}` +
          `  ${Math.round(picked[0].size / 1024)}KB → ${Math.round(picked[picked.length - 1].size / 1024)}KB\n`,
      );
    } catch (e) {
      process.stderr.write(`ERR ${spec.title}: ${e.message}\n`);
    }
  }

const snapshots = plan.reduce((n, p) => n + p.picked.length, 0);
const rawBytes = plan.reduce(
  (n, p) => n + p.picked.reduce((m, r) => m + r.size, 0),
  0,
);
process.stderr.write(
  `\n合计 ${snapshots} 张快照，原始 ${(rawBytes / 1048576).toFixed(1)} MB（含模板，剥完约剩一半）\n`,
);

if (WRITE_MANIFEST) {
  fs.writeFileSync(
    MANIFEST_PATH,
    JSON.stringify(
      {
        note:
          "wiki-history 语料的修订号清单。正文不进仓库（约 6MB 的 CC BY-SA 文本），" +
          "按这份清单 --from-manifest 可逐字节重建：action=parse&oldid 是不可变的。",
        sampling: { GROWTH_PCT, GROWTH_ABS, MIN_GAP_DAYS },
        titles: plan.map((q) => ({
          real: q.real,
          total: q.total,
          picked: q.picked.map((r) => ({
            revid: r.revid,
            timestamp: r.timestamp,
            size: r.size,
          })),
        })),
      },
      null,
      1,
    ),
  );
  process.stderr.write(`--manifest：清单已写入 ${MANIFEST_PATH}\n`);
  process.exit(0);
}

if (DRY) {
  process.stderr.write("--dry：只报采样结果，未抓正文\n");
  process.exit(0);
}

process.stderr.write("\n开始抓正文…\n");
for (const p of plan) {
  const slug = p.real
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  for (const r of p.picked) {
    try {
      const body = plaintext(r.revid).split(CUT)[0].trim();
      if (body.length < 400) {
        process.stderr.write(
          `  skip ${slug}@${day(r.timestamp)} 正文只有 ${body.length} 字符\n`,
        );
        continue;
      }
      // 第三个元素是 doc_time——快照的**真实修订时刻**。抽取提示词会拿到它
      // （`extraction.rs` 把 doc_time 按 %Y-%m-%d 塞进去），文内的相对日期才解得开；
      // 而按它排序灌入，`recorded_at` 才会散开成一条线而不是挤成一点
      docs.push([
        `${slug}@${day(r.timestamp)}.txt`,
        `${p.real}\n\n${body}\n`,
        r.timestamp,
      ]);
    } catch (e) {
      process.stderr.write(`  ERR ${slug}@${day(r.timestamp)}: ${e.message}\n`);
    }
  }
  process.stderr.write(`OK  ${p.real}\n`);
}

// **按时间排序**：语料的意义就在顺序上。乱序灌入等于回到「所有文档同一刻录入」
docs.sort((a, b) => (a[2] < b[2] ? -1 : 1));

const total = docs.reduce((n, [, t]) => n + t.length, 0);
process.stderr.write(
  `\n共 ${docs.length} 篇、${total.toLocaleString()} 字符，约 ${Math.round(total / 950)} 块\n`,
);

process.stdout.write(
  JSON.stringify(
    {
      name: "wiki-history",
      note:
        "维基百科条目的历史快照，按体量增量采样（高密度窗口按天）。与 ai-timeline 抓同一批主题，" +
        "但抓的是**历史版本而非当前版本**——那一份的 15 篇都是回顾性总述，灌一篇进去整条时间线一次性全出来，" +
        "只演得了世界时间；这一份每个快照是「那个时刻人们知道什么」，按 doc_time 顺序灌入，" +
        "recorded_at 会散开成一条线，supersedes 发生在文档之间而不是单篇内部。" +
        "条目选得互相咬合（机构 × 人 × 产物），人那一层是交叉链接的来源。" +
        "已切掉 References/External links 等尾节与信息框表格。" +
        "注意：这批主题在模型训练数据里极常见，适合做 demo（认得出是优点），不适合当准确率基准（量到的是背诵）。",
      source:
        "https://en.wikipedia.org/ — action=query&prop=revisions + action=parse&oldid",
      license: "CC BY-SA 4.0（署名-相同方式共享，与仓库主许可不同）",
      sampling: { GROWTH_PCT, GROWTH_ABS, MIN_GAP_DAYS },
      /// docs 的第三个元素是 doc_time（ISO 时刻）。旧语料只有两个元素，run.mjs 兼容
      docs,
    },
    null,
    1,
  ),
);
