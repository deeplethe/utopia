#!/usr/bin/env node
/* 星标曲线：累计星标随日期变化的那一条线，别的都不画。
 *
 * 为什么自己画。用过 `lowlighter/metrics`，它把「累计总数」和「每天新增」
 * 两张图**绑在同一个开关上**（`plugin_stargazers_charts`），模板里没有
 * 只留其一的办法。而想要的就是传统的那一条累计线。
 *
 * 自己画还顺手去掉了一件事：那是个跑在 `contents: write` 之下的第三方
 * action。现在这个权限底下跑的是这份脚本。
 *
 * **GitHub 在 2026-06-30 把星标时间线限制成只有仓库的管理员/协作者可读**，
 * 所以必须带一个够权限的 token——匿名调用现在拿不到，star-history.com
 * 那类站点从此只回一张占位图。
 */
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";

const [owner, repo] = (process.env.REPO ?? "").split("/");
const token = process.env.GITHUB_TOKEN;
const out = process.env.OUT ?? "assets/star-history.svg";
if (!owner || !repo) throw new Error("REPO must be set as owner/name");
if (!token) throw new Error("GITHUB_TOKEN must be set");

/** 每页 100，游标翻到底。1868 颗星 = 19 页，不值得为它上并发。
 *
 * **走 GraphQL，不走 REST。** REST 的 `/repos/{o}/{r}/stargazers` 用同一个
 * token 会回 404：那个接口要求 token 带仓库级 scope，而这个只有 `read:org`；
 * GitHub 对无权访问的资源回 404 而不是 403，免得泄露它存不存在。
 * GraphQL 的 `stargazers` 连接认这个 token——CI 上验过。
 * 改这里，好过为了一个接口去放宽凭据。 */
async function stargazerDates() {
  /* `viewerPermission` 与 `totalCount` 不是装饰。**受限数据 GitHub 回的是
     空集合，不是报错**——只看 `edges` 的话，「没权限」和「真的零颗星」
     长得一模一样。把这两个一起取回来，失败时说得出是哪一种 */
  const query = `query($owner:String!,$name:String!,$cursor:String){
    repository(owner:$owner,name:$name){
      stargazerCount
      viewerPermission
      stargazers(first:100,after:$cursor,orderBy:{field:STARRED_AT,direction:ASC}){
        totalCount
        pageInfo{hasNextPage endCursor}
        edges{starredAt}
      }
    }
  }`;
  const dates = [];
  let cursor = null;
  for (let page = 1; page <= 400; page++) {
    const res = await fetch("https://api.github.com/graphql", {
      method: "POST",
      headers: {
        authorization: `Bearer ${token}`,
        "content-type": "application/json",
        "user-agent": `${owner}-star-history`,
      },
      body: JSON.stringify({ query, variables: { owner, name: repo, cursor } }),
    });
    if (!res.ok) {
      throw new Error(`graphql page ${page}: ${res.status} ${await res.text()}`);
    }
    const body = await res.json();
    // **GraphQL 出错也回 200**，所以必须自己看 `errors`——否则要等到下面
    // 读出 undefined 才发现，而那时错误原文已经丢了
    if (body.errors) {
      throw new Error(`graphql page ${page}: ${JSON.stringify(body.errors)}`);
    }
    const node = body.data?.repository;
    const conn = node?.stargazers;
    if (!conn) throw new Error(`graphql page ${page}: no stargazers in response`);
    if (page === 1) {
      console.log(
        `repo sees ${node.stargazerCount} stars; connection reports ` +
          `${conn.totalCount}; token permission = ${node.viewerPermission}`,
      );
    }
    for (const e of conn.edges) if (e.starredAt) dates.push(new Date(e.starredAt));
    if (!conn.pageInfo.hasNextPage) return dates;
    cursor = conn.pageInfo.endCursor;
  }
  return dates;
}

const dates = (await stargazerDates()).sort((a, b) => a - b);
if (dates.length === 0) {
  // 上面那行日志已经说了 repo 报多少颗星、连接报多少、token 是什么权限。
  // **不要在这里静默出一张空图**——一张画着零的曲线比没有图更糟
  throw new Error(
    "no stargazer timestamps came back — see the line above for what the API " +
      "reported. An empty connection with a non-zero star count means the token " +
      "cannot read the stargazer timeline (restricted to admins and collaborators " +
      "since 2026-06-30).",
  );
}

/* 按天聚合成累计值。**每一天都要有点**，哪怕当天没有新增——
   缺口跳过去的话，横轴的间距就不再代表时间，曲线的斜率也就骗人了 */
const DAY = 86400000;
const day0 = Date.UTC(
  dates[0].getUTCFullYear(),
  dates[0].getUTCMonth(),
  dates[0].getUTCDate(),
);
const today = Date.now();
const perDay = new Map();
for (const d of dates) {
  const k = Math.floor((Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate()) - day0) / DAY);
  perDay.set(k, (perDay.get(k) ?? 0) + 1);
}
const lastDay = Math.floor((today - day0) / DAY);
const series = [];
let total = 0;
for (let k = 0; k <= lastDay; k++) {
  total += perDay.get(k) ?? 0;
  series.push({ t: day0 + k * DAY, v: total });
}

// ---- 画图
const W = 800, H = 400;
/* 顶部留得比别处宽：末点的数值标在它上方，而**末点永远在顶上**——
   累计值单调不减，最后一个点就是最大值 */
const PAD = { top: 40, right: 28, bottom: 40, left: 64 };
const plotW = W - PAD.left - PAD.right;
const plotH = H - PAD.top - PAD.bottom;
const maxV = series[series.length - 1].v;
const x = (i) => PAD.left + (plotW * i) / Math.max(1, series.length - 1);
const y = (v) => PAD.top + plotH - (plotH * v) / Math.max(1, maxV);

/** 轴上的刻度取整齐的数，不取 max/5 那种带小数的 */
function ticks(max, count = 5) {
  const raw = max / count;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? mag * 10;
  const out = [];
  for (let v = 0; v <= max; v += step) out.push(Math.round(v));
  return out;
}
const fmtDate = (t) =>
  new Date(t).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });

/** 平滑成三次贝塞尔，用**单调**插值（Fritsch–Carlson）。
 *
 * 不用普通的 Catmull-Rom：累计星标只增不减，而普通样条会在斜率突变处
 * 过冲，画出一段下凹——**那等于在图上说星标掉了**。单调插值把每段的
 * 切线夹在不制造极值的范围内，曲线因此永远不会往回走。
 *
 * 平的那几天（当天零新增）切线为零，接上去也不会鼓出来。 */
function smoothPath(pts) {
  const n = pts.length;
  if (n < 2) return `M${pts[0].x.toFixed(1)},${pts[0].y.toFixed(1)}`;
  // 每段的斜率
  const dx = [], dy = [], slope = [];
  for (let i = 0; i < n - 1; i++) {
    dx.push(pts[i + 1].x - pts[i].x);
    dy.push(pts[i + 1].y - pts[i].y);
    slope.push(dy[i] / dx[i]);
  }
  // 每个点的切线：相邻两段异号（或有一段是平的）时取零，那正是不过冲的条件
  const m = [slope[0]];
  for (let i = 1; i < n - 1; i++) {
    m.push(slope[i - 1] * slope[i] <= 0 ? 0 : (slope[i - 1] + slope[i]) / 2);
  }
  m.push(slope[n - 2]);
  // Fritsch–Carlson：把切线限制在每段斜率的三倍以内
  for (let i = 0; i < n - 1; i++) {
    if (slope[i] === 0) {
      m[i] = 0;
      m[i + 1] = 0;
      continue;
    }
    const a = m[i] / slope[i];
    const b = m[i + 1] / slope[i];
    const s = a * a + b * b;
    if (s > 9) {
      const t = (3 / Math.sqrt(s)) * slope[i];
      m[i] = t * a;
      m[i + 1] = t * b;
    }
  }
  let d = `M${pts[0].x.toFixed(1)},${pts[0].y.toFixed(1)}`;
  for (let i = 0; i < n - 1; i++) {
    const h = dx[i] / 3;
    d +=
      `C${(pts[i].x + h).toFixed(1)},${(pts[i].y + m[i] * h).toFixed(1)} ` +
      `${(pts[i + 1].x - h).toFixed(1)},${(pts[i + 1].y - m[i + 1] * h).toFixed(1)} ` +
      `${pts[i + 1].x.toFixed(1)},${pts[i + 1].y.toFixed(1)}`;
  }
  return d;
}

const pts = series.map((p, i) => ({ x: x(i), y: y(p.v) }));
const line = smoothPath(pts);
const area = `${line}L${x(series.length - 1).toFixed(1)},${(PAD.top + plotH).toFixed(1)}L${x(0).toFixed(1)},${(PAD.top + plotH).toFixed(1)}Z`;

/* 末点与它的数值。**贴着右边缘时把文字改成右对齐**，
   否则一个四位数会伸出画布外——SVG 不会替你裁，它就是没了 */
const endX = x(series.length - 1);
const endY = y(maxV);
const endAnchor = endX > W - PAD.right - 40 ? "end" : "middle";

const xTickIdx = [...new Set(
  Array.from({ length: 6 }, (_, i) => Math.round((i * (series.length - 1)) / 5)),
)];

/* 出两张，深浅各一，README 用 `<picture>` 按主题选。
 *
 * **白线在浅色主题上是看不见的**——GitHub 浅色底就是白的。想要白色就
 * 必须分两张：`prefers-color-scheme` 写在 SVG 里不算数，README 里的 SVG
 * 是当图片加载的，那条媒体查询问的是操作系统，不是 GitHub 的主题设置，
 * 两者不一致的人就会看到一张空白的图。`<picture>` 问的才是 GitHub 自己。 */
const THEMES = {
  dark: { ink: "#8b949e", accent: "#ffffff", grid: "#8b949e33" },
  light: { ink: "#6e7781", accent: "#1f2328", grid: "#6e778133" },
};

function render({ ink, accent, grid }) {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,Helvetica,Arial,sans-serif">
<defs><linearGradient id="fill" x1="0" y1="0" x2="0" y2="1">
<stop offset="0%" stop-color="${accent}" stop-opacity="0.22"/>
<stop offset="100%" stop-color="${accent}" stop-opacity="0"/>
</linearGradient></defs>
<text x="${PAD.left}" y="24" fill="${ink}" font-size="13">${owner}/${repo}</text>
${ticks(maxV).map((v) => `<g><line x1="${PAD.left}" y1="${y(v).toFixed(1)}" x2="${W - PAD.right}" y2="${y(v).toFixed(1)}" stroke="${grid}"/><text x="${PAD.left - 10}" y="${(y(v) + 4).toFixed(1)}" fill="${ink}" font-size="11" text-anchor="end">${v.toLocaleString("en-US")}</text></g>`).join("")}
${xTickIdx.map((i) => `<text x="${x(i).toFixed(1)}" y="${H - 16}" fill="${ink}" font-size="11" text-anchor="middle">${fmtDate(series[i].t)}</text>`).join("")}
<path d="${area}" fill="url(#fill)"/>
<path d="${line}" fill="none" stroke="${accent}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>
<circle cx="${endX.toFixed(1)}" cy="${endY.toFixed(1)}" r="3.5" fill="${accent}"/>
<text x="${endX.toFixed(1)}" y="${(endY - 12).toFixed(1)}" fill="${accent}" font-size="14" font-weight="600" text-anchor="${endAnchor}">${maxV.toLocaleString("en-US")}</text>
</svg>
`;
}

mkdirSync(dirname(out), { recursive: true });
const lightOut = out.replace(/\.svg$/, "-light.svg");
writeFileSync(out, render(THEMES.dark));
writeFileSync(lightOut, render(THEMES.light));
console.log(`${series.length} days, ${maxV} stars → ${out} + ${lightOut}`);
