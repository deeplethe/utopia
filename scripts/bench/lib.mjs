//! 两个映射测量台共用的地基：`mappings.mjs` 量提议，`ask.mjs` 量答案。
//!
//! **判等必须是同一份。** 两边都在做「跑一条 SQL，跟 gold 的结果比数」，
//! 各写一份 `same()` 迟早漂移，而一旦漂移，「提议对了几条」与「答案对了几条」
//! 就不是同一把尺子量出来的，放在一起比较就没有意义。

import { execFileSync } from "node:child_process";

export const BASE = process.env.BENCH_BASE || "http://127.0.0.1:8322";
export const EMAIL = process.env.BENCH_EMAIL || "bench@test.local";
export const PASSWORD = process.env.BENCH_PASSWORD || "benchbench123";
const PSQL = process.env.BENCH_PSQL || "docker exec -e PGPASSWORD=utopia landscapebi-db-1 psql -U utopia -d utopia -tAc";
/// 应用库与语料库都不是 BENCH_PSQL 里写的那个：**测量台跑在自己的库上**
/// （bench/README 的第一条规则），而 BENCH_PSQL 与 govern.mjs 共用，指着开发库
export const APP_DB = process.env.BENCH_APP_DB || "utopia_mapbench";

export function parseArgs(argv) {
  return Object.fromEntries(
    argv.slice(2).reduce((acc, cur, i, arr) => {
      if (cur.startsWith("--")) acc.push([cur.slice(2), arr[i + 1]?.startsWith("--") ? true : (arr[i + 1] ?? true)]);
      return acc;
    }, []),
  );
}

let cookie = "";
export async function api(method, url, body) {
  const init = { method, headers: {} };
  if (cookie) init.headers.cookie = cookie;
  if (body !== undefined) { init.headers["content-type"] = "application/json"; init.body = JSON.stringify(body); }
  const r = await fetch(BASE + url, init);
  for (const c of r.headers.getSetCookie?.() ?? []) cookie = c.split(";")[0];
  const text = await r.text();
  if (!r.ok) throw new Error(`${method} ${url} -> ${r.status} ${text.slice(0, 200)}`);
  return text ? JSON.parse(text) : null;
}
export const cookieHeader = () => cookie;

/// 登录；账号不在就按首用户注册（新库上测量台该能从头跑起来，
/// 而首个注册的人建 org 与 workspace 并且是 admin——注册数据源要 admin）
export async function login() {
  try {
    await api("POST", "/api/v1/auth/login", { email: EMAIL, password: PASSWORD });
  } catch {
    await api("POST", "/api/v1/auth/register", {
      email: EMAIL, password: PASSWORD, display_name: "bench", org_name: "bench",
    });
  }
}

/// 把命令行里的 `-d <库>` 换成指定的库；命令行里本来没写 `-d`（PGDATABASE 那种写法）
/// 就插在 SQL 前面——**不能是「换不到就算了」**：那样每条 onDb() 都落到命令的默认库，
/// 而 DROP SCHEMA 与模型写的 SQL 都走 onDb()
function withDb(cmdline, db) {
  const parts = cmdline.split(" ");
  const i = parts.indexOf("-d");
  if (i >= 0 && i + 1 < parts.length) parts[i + 1] = db;
  else parts.splice(parts.length - 1, 0, "-d", db);
  return parts.join(" ");
}
function run(cmdline, sql) {
  const parts = cmdline.split(" ");
  // stderr 收进异常里，不直接漏到终端：跑不通的 SQL 是 `value()` 的正常返回值
  // （broken 那一栏），不该在报告里刷一屏 ERROR
  return execFileSync(parts[0], [...parts.slice(1), sql], {
    encoding: "utf8", maxBuffer: 64 << 20, stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}
export const psql = (sql) => run(withDb(PSQL, APP_DB), sql);
export const onDb = (db, sql) => run(withDb(PSQL, db), sql);
export const num = (sql) => Number(psql(sql) || 0);

/// 一条 SQL 第一行里的所有数字。
///
/// **模型不写单列查询。** 问「平均行金额」，它跑的是
/// `SELECT COUNT(*), AVG(l_extendedprice), MIN(...), MAX(...)`——一次把上下文
/// 都查出来。只看第一列就拿到 60000（行数），把一条完全正确的查询判成错的；
/// 头一轮基线上五道题栽在这里，全被记成「数对了但 SQL 不对」。
export function firstRow(db, sql) {
  try {
    const out = onDb(db, `SET statement_timeout = '20s'; ${sql}`);
    const first = out.split("\n").map((l) => l.trim()).filter((l) => l !== "" && l !== "SET")[0];
    if (first === undefined) return { empty: true };
    const ns = first.split("|").map((x) => Number(x)).filter((n) => Number.isFinite(n));
    return { ns };
  } catch (e) {
    return { error: String(e.stderr || e.message).split("\n").filter((l) => l.trim())[0]?.slice(0, 120) };
  }
}

/// 一条 SQL 跑出来的第一个值。口径的定义是单值的，用这个；
/// 判模型跑过的 SQL 用 [`firstRow`]。
export function value(db, sql) {
  try {
    const out = onDb(db, `SET statement_timeout = '20s'; ${sql}`);
    // **命令标签也走 stdout。** `SET statement_timeout` 先打一行 `SET`，
    // 而 -tA 下数据行不带标签——不滤掉它，每条读到的第一行都是 `SET`
    const first = out.split("\n").map((l) => l.trim()).filter((l) => l !== "" && l !== "SET")[0];
    if (first === undefined) return { empty: true };
    return { n: Number(String(first).split("|")[0]) };
  } catch (e) {
    return { error: String(e.stderr || e.message).split("\n").filter((l) => l.trim())[0]?.slice(0, 120) };
  }
}

/// 两个数是不是同一个口径算出来的。
///
/// **容差要松到吃得下小数舍入，紧到分得开两个口径**——tpch 里 charge 与
/// order_total 是同一个业务量的两条算法，差在分位上，判成同一条是对的；
/// 而 disc_revenue 与 discount_given 差着一个数量级。
export const same = (a, b) => {
  if (!Number.isFinite(a) || !Number.isFinite(b)) return false;
  if (a === b) return true;
  return Math.abs(a - b) <= 1e-6 * Math.max(Math.abs(a), Math.abs(b), 1);
};

/// 人读答案时的同一个数。**比 `same` 松两个量级**：模型说「约 8.63 亿」
/// 与 862793473.48 是同一个答案，而把它判成错的话，量的就不是问数准不准，
/// 是模型肯不肯把小数点后八位抄全。
export const roughly = (a, b) => {
  if (!Number.isFinite(a) || !Number.isFinite(b)) return false;
  if (same(a, b)) return true;
  return Math.abs(a - b) <= 5e-3 * Math.max(Math.abs(a), Math.abs(b), 1);
};

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const log = (...a) => console.error(new Date().toISOString().slice(11, 19), ...a);

export async function until(fn, everyMs, stallMs) {
  let deadline = Date.now() + stallMs, last = null;
  for (;;) {
    const r = await fn();
    if (r === true) return;
    if (typeof r === "number" && r !== last) { last = r; deadline = Date.now() + stallMs; }
    if (Date.now() > deadline) throw new Error(`等超时：${Math.round(stallMs / 60000)} 分钟没有任何进展`);
    await sleep(everyMs);
  }
}

/// 一轮问答，按人在界面里问的方式走 chat 接口。SSE 帧是 `event: X\ndata: {...}\n\n`。
/// ask.mjs 里有一份同样的；competency.mjs 也要，所以搬到这里
export async function askChat(kb, message) {
  const res = await fetch(`${BASE}/api/v1/kbs/${kb}/chat`, {
    method: "POST",
    headers: { "content-type": "application/json", cookie: cookieHeader() },
    body: JSON.stringify({ message }),
  });
  if (!res.ok) throw new Error(`chat -> ${res.status} ${(await res.text()).slice(0, 200)}`);
  const reader = res.body.getReader();
  const dec = new TextDecoder();
  let buf = "", text = "", conversation = null, error = null;
  const steps = [];
  for (;;) {
    const { done, value: chunk } = await reader.read();
    if (done) break;
    buf += dec.decode(chunk, { stream: true });
    let i;
    while ((i = buf.indexOf("\n\n")) >= 0) {
      const frame = buf.slice(0, i);
      buf = buf.slice(i + 2);
      const ev = /^event: ?(.*)$/m.exec(frame)?.[1];
      const data = frame.split("\n").filter((l) => l.startsWith("data:")).map((l) => l.slice(5).replace(/^ /, "")).join("\n");
      try {
        if (ev === "conversation") conversation = JSON.parse(data).id;
        else if (ev === "delta") text += JSON.parse(data).text ?? "";
        else if (ev === "step") steps.push(JSON.parse(data));
        else if (ev === "error") error = data;
      } catch {
        /* 半帧或非 JSON：下一帧再说 */
      }
    }
  }
  return { conversation, text, steps, error };
}
