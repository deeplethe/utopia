// 正文里的引用标记。
//
// 模型写的是 `[1]`、`[1][2]`、`[1, 2]`、`[1，2]`。**哪些被画成角标，和哪些被列进
// 落款，必须是同一套判定**——否则正文里点得动的那个号在下面找不到对应的行。
// 所以这里只有一处形状，`liveAnswer.citedSources` 与 `rehypeCitations` 共用它。
// **判定也只有一处**：落款从前对整段原文跑正则，代码块里的 `[1, 2]`、链接
// `[1](…)` 都算进去了，而角标只画在正文文字上；`citedNumbers` 用渲染同一个解析器，
// 跳过同样的元素。
//
// 每次现造一个正则：`g` 的 `lastIndex` 跟着上一次调用走，共用一个实例会让
// 第二段文本从中间开始匹配。

import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import { unified } from "unified";

/** `[1]` / `[1][2]` / `[1, 2]` / `[1，2]`——括号里只有数字与分隔符才算引用 */
export const citeRe = () => /\[(\d+(?:\s*[,，]\s*\d+)*)\]/g;

/** 一个标记里的号：`[1, 2]` 是两个 */
export function citeNumbers(spec: string): number[] {
  return spec
    .split(/[,，]/)
    .map((s) => Number(s.trim()))
    .filter((n) => Number.isInteger(n) && n > 0);
}

export type CitePiece = { text: string } | { cite: number[] };

/** 把一段纯文本切成「文字」与「一组引用号」。
 *
 *  流式中还没收尾的 `[1` 匹配不上——要等右括号，所以角标不会先画出半个再变形。 */
export function splitCitations(text: string): CitePiece[] {
  const out: CitePiece[] = [];
  const re = citeRe();
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const ns = citeNumbers(m[1]);
    if (!ns.length) continue;
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    out.push({ cite: ns });
    last = m.index + m[0].length;
  }
  if (!out.length) return [{ text }];
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/* ── 落款：正文里画成了角标的那些号 ──────────────────────────────────────── */

/** mdast 里我们读的那几个字段（同下面 hast 的理由：结构类型，不多一个依赖） */
type MdNode = { type: string; value?: string; children?: MdNode[] };

/** `OPAQUE`（a / code / pre）在 mdast 里的名字，外加原样 HTML：
 *  react-markdown 把它当 raw 节点交给 rehype，`rehypeCitations` 不在里面画角标 */
const MD_OPAQUE = new Set(["link", "linkReference", "inlineCode", "code", "html"]);

/** 与渲染同一套 remark 解析（remark-gfm 管表格与自动链接） */
const markdown = unified().use(remarkParse).use(remarkGfm);

/** 按正文记忆：一轮对话里每来一个词元都要重算一遍，收了尾的回答文本不再变 */
const citedMemo = new Map<string, ReadonlySet<number>>();
const CITED_MEMO_LIMIT = 200;

/** 正文里会被画成角标的那些号。 */
export function citedNumbers(text: string): ReadonlySet<number> {
  const hit = citedMemo.get(text);
  if (hit) return hit;
  const cited = new Set<number>();
  collectCited(markdown.parse(text) as MdNode, cited);
  if (citedMemo.size >= CITED_MEMO_LIMIT) {
    const oldest = citedMemo.keys().next().value;
    if (oldest !== undefined) citedMemo.delete(oldest);
  }
  citedMemo.set(text, cited);
  return cited;
}

function collectCited(node: MdNode, into: Set<number>): void {
  if (node.type === "text" && typeof node.value === "string") {
    for (const piece of splitCitations(node.value)) {
      if ("cite" in piece) for (const n of piece.cite) into.add(n);
    }
    return;
  }
  if (MD_OPAQUE.has(node.type)) return;
  for (const child of node.children ?? []) collectCited(child, into);
}

/* ── rehype 插件 ────────────────────────────────────────────────────────── */

/** hast 里我们要动的那两种节点。用结构类型而不是 `@types/hast`：
 *  这里只读 `children` / `value` / `tagName`，不值得为它多一个依赖 */
type HastText = { type: "text"; value: string };
type HastNode = {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
};

/** 这些元素里面的方括号不是引用：链接的锚文本可能正好是一个数字，
 *  代码块里的 `[0]` 是下标 */
const OPAQUE = new Set(["a", "code", "pre"]);

const CITE_PREFIX = "#cite-";

/** 正文里的 `[n]` 变成 `<a href="#cite-n">`。
 *
 *  **为什么是 `a` 而不是一个自定义标签**：react-markdown 把 hast 属性转成 JSX
 *  属性，`href` 是它本来就认得的那一个，于是 `components.a` 拿到的是有类型的
 *  props；自定义标签得从 `node.properties` 里摸，摸出来的是 `unknown`。
 *
 *  **为什么是锚点而不是自造一个 `cite:` 协议**：react-markdown 默认只放行
 *  http/https/mailto/tel 与相对地址，别的协议会被洗成空串——角标于是退化成一个
 *  下划线的链接，点不开。`#` 开头是相对地址。 */
export function rehypeCitations() {
  return (tree: HastNode) => walk(tree);
}

function walk(node: HastNode): void {
  const kids = node.children;
  if (!kids?.length) return;
  let changed = false;
  const out: HastNode[] = [];
  for (const child of kids) {
    if (child.type === "text" && typeof child.value === "string") {
      const pieces = splitCitations(child.value);
      if (pieces.length === 1 && "text" in pieces[0]) {
        out.push(child);
        continue;
      }
      changed = true;
      for (const p of pieces) {
        if ("text" in p) {
          if (p.text) out.push({ type: "text", value: p.text } as HastText);
        } else {
          out.push({
            type: "element",
            tagName: "a",
            properties: { href: `${CITE_PREFIX}${p.cite.join(",")}` },
            children: [{ type: "text", value: `[${p.cite.join(", ")}]` }],
          });
        }
      }
      continue;
    }
    if (!(child.type === "element" && OPAQUE.has(child.tagName ?? ""))) {
      walk(child);
    }
    out.push(child);
  }
  if (changed) node.children = out;
}

/** `#cite-1,2` → `[1, 2]`；不是引用链接就给 null */
export function citeHref(href: string | undefined): number[] | null {
  if (!href?.startsWith(CITE_PREFIX)) return null;
  const ns = citeNumbers(href.slice(CITE_PREFIX.length));
  return ns.length ? ns : null;
}
