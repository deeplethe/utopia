// 数字覆盖：一个库里，段落写了的钱、百分比、带单位或带千分位的数，有多少没进它那块的任何一条
// 陈述（宾语、字面值、限定词、时间词都算）。只看形状，不看词；年份不算数。表格行（以 | 起头）
// 与散文分开数。用法：node scripts/bench/figures.mjs --kb <id>；库用 BENCH_PSQL 指定，同 judge_open。
import { execFileSync } from "node:child_process";
const KB = process.argv[process.argv.indexOf("--kb") + 1];
if (!KB || KB.startsWith("--")) { console.error("要一个 --kb <id>"); process.exit(1); }
function psql(sql) {
  const cmd = process.env.BENCH_PSQL || "docker exec -e PGPASSWORD=utopia landscapebi-db-1 psql -U utopia -d utopia -tAc";
  const parts = cmd.split(" ");
  return execFileSync(parts[0], [...parts.slice(1), sql], { encoding: "utf8", maxBuffer: 64 << 20 }).trim();
}
const SEP = String.fromCharCode(31);
const chunks = psql(`SELECT concat_ws(chr(31), c.id, d.filename, replace(c.text, chr(10), chr(30))) FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.kb_id='${KB}' ORDER BY d.filename, c.id`)
  .split("\n").filter(Boolean).map(l => { const [id, file, t] = l.split(SEP); return { id, file, text: t.replace(/\x1e/g, "\n") }; });
const stmts = psql(`SELECT concat_ws(chr(31), fe.chunk_id, s.canonical_name, f.phrase, coalesce(o.canonical_name, f.object_value #>> '{value}', ''),
  coalesce((SELECT string_agg(coalesce(sq.value #>> '{}', qe.canonical_name, ''), ' ') FROM statement_qualifiers sq LEFT JOIN entities qe ON qe.id=sq.entity_id WHERE sq.fact_id=f.id), ''),
  coalesce((SELECT string_agg(tm.text, ' ') FROM time_mentions tm WHERE tm.fact_id=f.id), ''))
  FROM facts f JOIN fact_evidence fe ON fe.fact_id=f.id JOIN entities s ON s.id=f.subject_id LEFT JOIN entities o ON o.id=f.object_id
  WHERE f.kb_id='${KB}' AND f.layer='open' AND f.invalidated_at IS NULL`)
  .split("\n").filter(Boolean).map(l => { const [chunk, ...rest] = l.split(SEP); return { chunk, text: rest.join(" ") }; });
const byChunk = new Map();
for (const s of stmts) byChunk.set(s.chunk, (byChunk.get(s.chunk) || "") + " " + s.text);
const norm = (s) => s.replace(/[,\s]/g, "").toLowerCase();
// 一个「数字」：可带 $ / 百分号 / billion|million 单位；两位以上或带符号
const FIG = /(?:\$\s?)?\d[\d,]*(?:\.\d+)?(?:\s?(?:billion|million|thousand|%|percent|bps|亿元|万元|亿美元|万美元|亿|万|元|个百分点|吨|人次|人|户|家|件|亩|公顷|例|种|项|台|辆))?/gi;
const isFigure = (m) => /[$%]|billion|million|thousand|percent|bps|亿|万|元|个百分点|吨|人|户|家|件|亩|公顷|例|种|项|台|辆/i.test(m) || /\d,\d{3}/.test(m);
const perFile = {};
const samples = [];
for (const c of chunks) {
  const have = norm(byChunk.get(c.id) || "");
  for (const line of c.text.split("\n")) {
    const table = line.trim().startsWith("|");
    const sentences = table ? [line] : line.split(/(?<=[.;。；])\s+/);
    for (const sent of sentences) {
      for (const m of sent.match(FIG) || []) {
        if (!isFigure(m)) continue;
        if (/^(19|20)\d\d$/.test(m)) continue; // 年份不算数字
        const key = norm(m).replace(/^\$/, "");
        const pf = perFile[c.file] ||= { prose: [0, 0], table: [0, 0] };
        const slot = pf[table ? "table" : "prose"];
        slot[0]++;
        if (!have.includes(key)) { slot[1]++; if (!table && samples.length < 40) samples.push([c.file, m, sent.trim().slice(0, 160)]); }
      }
    }
  }
}
let T = [0, 0], P = [0, 0];
for (const [f, v] of Object.entries(perFile)) { console.log(`${f}: prose ${v.prose[1]}/${v.prose[0]} missing, table ${v.table[1]}/${v.table[0]} missing`); P[0]+=v.prose[0]; P[1]+=v.prose[1]; T[0]+=v.table[0]; T[1]+=v.table[1]; }
console.log(`ALL: prose ${P[1]}/${P[0]} (${(100*P[1]/P[0]).toFixed(0)}%) missing, table ${T[1]}/${T[0]} (${(100*T[1]/T[0]).toFixed(0)}%) missing`);
console.log("-- prose samples:"); for (const [f, m, s] of samples) console.log(`  [${f.replace("nvda-","")}] ${m}  ⇐ ${s}`);
