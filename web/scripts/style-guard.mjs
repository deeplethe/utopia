// 风格守卫：web/DESIGN.md 的五条规矩在这里变成检查。
//
// 扫 src/**/*.tsx，命中下面任何一条就红。`style-guard.baseline.json` 里列的是
// 还没迁移的页面——它们暂时豁免；每迁一页就从名单里删一行，名单空了这个
// 文件就删。**新文件永远不豁免**：名单只能缩短，不能加长。
//
// `src/ui/` 是组件本身，允许写原生元素、hover、transition——那正是它的活；
// 但颜色与字号的规矩对它一样管。
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SRC = path.join(ROOT, "src");
const BASELINE = path.join(ROOT, "style-guard.baseline.json");

const PALETTE =
  "neutral|zinc|gray|slate|stone|rose|amber|red|green|blue|sky|emerald|violet|indigo|purple|pink|orange|yellow|cyan|teal|lime";

/** 每条规矩：正则 + 一句话说明 + 是否也管组件目录 */
const RULES = [
  {
    id: "type-scale",
    re: /\btext-(xs|sm|base|lg|xl|2xl|3xl|4xl|\[[0-9.]+(px|rem)\])\b/g,
    why: "字号只用五档：text-fine / small / body / title / display（规矩 1）",
    ui: true,
  },
  {
    id: "raw-palette",
    re: new RegExp(
      `\\b(text|bg|border|ring|outline|from|to|via|decoration|divide|placeholder)-(${PALETTE})-[0-9]+\\b`,
      "g",
    ),
    why: "颜色只用令牌：ink / ink-2 / ink-3 / line / surface / ok / warn / danger…（规矩 4）",
    ui: true,
  },
  {
    id: "alpha-white",
    re: /\b(bg|border|text|ring|outline|divide)-(white|black)\/\[?[0-9.]+\]?/g,
    why: "白色透明度是 surface / surface-2 / surface-3 / line 的事，不在页面里调（规矩 4）",
    ui: true,
  },
  {
    id: "token-by-hand",
    re: /\[var\(--u-[a-z0-9-]+\)\]/g,
    why: "令牌已经是 Tailwind 颜色：写 text-danger，不写 text-[var(--u-danger)]（规矩 4）",
    ui: true,
  },
  {
    id: "radius",
    re: /\brounded(-(sm|md|2xl|3xl|none|\[[^\]]+\]))?(?=[\s"'`}])/g,
    why: "圆角两档：rounded-lg 给控件，rounded-xl 给面，rounded-full 给药丸（规矩 3）",
    ui: true,
  },
  {
    id: "spacing",
    // 12 及以上是版面（给浮层留位、页脚净空），不是节奏，放行
    re: /\b-?(p|px|py|pt|pb|pl|pr|m|mx|my|mt|mb|ml|mr|gap|gap-x|gap-y|space-x|space-y)-(0\.5|1\.5|2\.5|3\.5|5|7|9|10|11|\[[^\]]+\])\b/g,
    why: "间距六档：1 2 3 4 6 8；12 以上只给版面净空（规矩 2）",
    ui: false,
  },
  {
    id: "raw-control",
    // 隐藏的文件选择框不算控件（它没有样子），放行
    re: /<(button|textarea|select)\b|<input\b(?![^>]*type="file")/g,
    why: "控件从 ui/ 来：Button / IconButton / Input / Textarea / NativeSelect（规矩 5）",
    ui: false,
  },
  {
    id: "state-in-page",
    re: /\b(hover|focus|focus-visible|active|disabled):[a-z0-9\[\]()/.-]+|\btransition(-[a-z]+)?\b|\bduration-[0-9a-z()-]+/g,
    why: "hover / focus / disabled / 动效在组件里定一次，页面不写（规矩 5）",
    ui: false,
  },
  {
    id: "native-confirm",
    re: /\bwindow\.(confirm|alert)\(|(?<![.\w])(confirm|alert)\(/g,
    why: "确认走 DangerConfirm / Dialog，不用 window.confirm（规矩 5）",
    ui: false,
  },
];

function walk(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else if (entry.name.endsWith(".tsx")) out.push(full);
  }
  return out;
}

const baseline = new Set(
  fs.existsSync(BASELINE) ? JSON.parse(fs.readFileSync(BASELINE, "utf8")) : [],
);
const files = walk(SRC).map((f) => path.relative(ROOT, f).replaceAll("\\", "/"));

let failures = 0;
const staleBaseline = [...baseline].filter((f) => !files.includes(f));
for (const rel of files) {
  if (baseline.has(rel)) continue;
  const inUi = rel.startsWith("src/ui/");
  let text = fs.readFileSync(path.join(ROOT, rel), "utf8");
  // 隐藏的文件选择框可以跨好几行，先整段抹掉（保留换行，行号不变）
  text = text.replace(/<input\b[^>]*type="file"[^>]*>/g, (m) =>
    m.replace(/[^\n]/g, " "),
  );
  const lines = text.split("\n");
  for (const rule of RULES) {
    if (inUi && !rule.ui) continue;
    lines.forEach((line, i) => {
      // 注释里提到旧写法不算（规矩要能在注释里被引用）
      const code = line.replace(/\/\/.*$/, "").replace(/\{\/\*.*?\*\/\}/g, "");
      const hits = code.match(rule.re);
      if (!hits) return;
      failures += 1;
      console.log(`${rel}:${i + 1}  [${rule.id}] ${hits.join(" ")}\n    ${rule.why}`);
    });
  }
}

if (staleBaseline.length) {
  console.log(`baseline 里有已经不存在的文件，删掉它们：\n  ${staleBaseline.join("\n  ")}`);
  failures += 1;
}

if (failures) {
  console.log(`\n${failures} 处不合规矩。规矩在 web/DESIGN.md。`);
  process.exit(1);
}
console.log(`style guard: ${files.length - baseline.size} 个文件合规，${baseline.size} 个在迁移名单上。`);
