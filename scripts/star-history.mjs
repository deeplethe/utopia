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
  const query = `query($owner:String!,$name:String!,$cursor:String){
    repository(owner:$owner,name:$name){
      stargazers(first:100,after:$cursor,orderBy:{field:STARRED_AT,direction:ASC}){
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
    const conn = body.data?.repository?.stargazers;
    if (!conn) throw new Error(`graphql page ${page}: no stargazers in response`);
    for (const e of conn.edges) if (e.starredAt) dates.push(new Date(e.starredAt));
    if (!conn.pageInfo.hasNextPage) return dates;
    cursor = conn.pageInfo.endCursor;
  }
  return dates;
}

const dates = (await stargazerDates()).sort((a, b) => a - b);
if (dates.length === 0) throw new Error("no stargazer timestamps came back");

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

const line = series.map((p, i) => `${i === 0 ? "M" : "L"}${x(i).toFixed(1)},${y(p.v).toFixed(1)}`).join("");
const area = `${line}L${x(series.length - 1).toFixed(1)},${(PAD.top + plotH).toFixed(1)}L${x(0).toFixed(1)},${(PAD.top + plotH).toFixed(1)}Z`;

/* 颜色不跟随主题。GitHub 的 README 里 SVG 是当图片渲染的，
   `prefers-color-scheme` 不一定生效——挑一组在浅色和深色底上都读得出的中间调，
   比赌媒体查询可靠 */
const INK = "#8b949e";
const ACCENT = "#e3b341";
const GRID = "#8b949e33";

/* 末点与它的数值。**贴着右边缘时把文字改成右对齐**，
   否则一个四位数会伸出画布外——SVG 不会替你裁，它就是没了 */
const endX = x(series.length - 1);
const endY = y(maxV);
const endAnchor = endX > W - PAD.right - 40 ? "end" : "middle";

const xTickIdx = [...new Set(
  Array.from({ length: 6 }, (_, i) => Math.round((i * (series.length - 1)) / 5)),
)];

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}" font-family="-apple-system,BlinkMacSystemFont,Segoe UI,Helvetica,Arial,sans-serif">
<defs><linearGradient id="fill" x1="0" y1="0" x2="0" y2="1">
<stop offset="0%" stop-color="${ACCENT}" stop-opacity="0.28"/>
<stop offset="100%" stop-color="${ACCENT}" stop-opacity="0"/>
</linearGradient></defs>
<text x="${PAD.left}" y="24" fill="${INK}" font-size="13">${owner}/${repo}</text>
${ticks(maxV).map((v) => `<g><line x1="${PAD.left}" y1="${y(v).toFixed(1)}" x2="${W - PAD.right}" y2="${y(v).toFixed(1)}" stroke="${GRID}"/><text x="${PAD.left - 10}" y="${(y(v) + 4).toFixed(1)}" fill="${INK}" font-size="11" text-anchor="end">${v.toLocaleString("en-US")}</text></g>`).join("")}
${xTickIdx.map((i) => `<text x="${x(i).toFixed(1)}" y="${H - 16}" fill="${INK}" font-size="11" text-anchor="middle">${fmtDate(series[i].t)}</text>`).join("")}
<path d="${area}" fill="url(#fill)"/>
<path d="${line}" fill="none" stroke="${ACCENT}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>
<circle cx="${endX.toFixed(1)}" cy="${endY.toFixed(1)}" r="3.5" fill="${ACCENT}"/>
<text x="${endX.toFixed(1)}" y="${(endY - 12).toFixed(1)}" fill="${ACCENT}" font-size="14" font-weight="600" text-anchor="${endAnchor}">${maxV.toLocaleString("en-US")}</text>
</svg>
`;

mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, svg);
console.log(`${series.length} days, ${maxV} stars → ${out}`);
