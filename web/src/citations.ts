// 正文里的引用标记。
//
// 模型写的是 `[1]`、`[1][2]`、`[1, 2]`、`[1，2]`。**哪些被画成角标，和哪些被列进
// 落款，必须是同一套判定**——否则正文里点得动的那个号在下面找不到对应的行。
// 所以这里只有一处形状，`liveAnswer.citedSources` 与 `rehypeCitations` 共用它。
//
// 每次现造一个正则：`g` 的 `lastIndex` 跟着上一次调用走，共用一个实例会让
// 第二段文本从中间开始匹配。

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

/** 正文里的 `[n]` 变成 `<a href="cite:n">`。
 *
 *  **为什么是 `a` 而不是一个自定义标签**：react-markdown 把 hast 属性转成 JSX
 *  属性，`href` 是它本来就认得的那一个，于是 `components.a` 拿到的是有类型的
 *  props；自定义标签得从 `node.properties` 里摸，摸出来的是 `unknown`。 */
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
            properties: { href: `cite:${p.cite.join(",")}` },
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

/** `cite:1,2` → `[1, 2]`；不是引用链接就给 null */
export function citeHref(href: string | undefined): number[] | null {
  if (!href?.startsWith("cite:")) return null;
  const ns = citeNumbers(href.slice(5));
  return ns.length ? ns : null;
}
