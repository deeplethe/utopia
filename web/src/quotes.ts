// 原文里被引到的那几句（#968 的后续）。
//
// 图谱工具给一条事实印的号打开的是它第一条有效证据的那一块；来源条目的 `quotes` 是块里
// 说出这条（或这几条）事实的那几句。预览浮窗和文档页据此把它们标出来，号打开的就是那句话。
//
// 每一句只找第一次出现；找不到的（引文与原文对不上）不标；重叠或相接的并成一段。
// 空白按「一处或多处空白」对：引文里的一个空格对得上原文里的换行。

export type QuotePiece = { text: string; quoted: boolean };

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** 一句引文在原文里第一次出现的位置，`[起, 止)`；对不上是 null */
function locate(text: string, quote: string): [number, number] | null {
  const words = quote.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return null;
  const m = new RegExp(words.map(escape).join("\\s+")).exec(text);
  return m ? [m.index, m.index + m[0].length] : null;
}

/** 把原文切成「普通」与「被引」两种片段，按原来的顺序 */
export function quotePieces(text: string, quotes: readonly string[] | undefined): QuotePiece[] {
  const spans = (quotes ?? [])
    .map((q) => locate(text, q))
    .filter((s): s is [number, number] => s !== null)
    .sort((a, b) => a[0] - b[0]);
  const merged: [number, number][] = [];
  for (const [start, end] of spans) {
    const last = merged[merged.length - 1];
    if (last && start <= last[1]) last[1] = Math.max(last[1], end);
    else merged.push([start, end]);
  }
  const out: QuotePiece[] = [];
  let at = 0;
  for (const [start, end] of merged) {
    if (start > at) out.push({ text: text.slice(at, start), quoted: false });
    out.push({ text: text.slice(start, end), quoted: true });
    at = end;
  }
  if (at < text.length) out.push({ text: text.slice(at), quoted: false });
  return out;
}

/** 落款行上那一条的摘要：有被引的句子就是第一句，否则是这一块的开头 */
export function sourceLead(source: { excerpt: string; quotes?: string[] }): string {
  return source.quotes?.[0] ?? source.excerpt;
}

/** 进地址栏的那一句最长多少字。地址会留在浏览器历史和代理的访问日志里，刷新时整条重发：
 *  一个汉字编码后约九个字节，几百字就顶到常见的 8k 请求头上限。超长的一句不带——
 *  截断了在原文里找不到，标不出来，不如不标 */
export const QUOTE_PARAM_MAX = 300;

/** 能进地址栏的一句：空白的、超长的都等于没给 */
export function quoteParam(quote: unknown): string | undefined {
  return typeof quote === "string" && quote.trim() && quote.length <= QUOTE_PARAM_MAX
    ? quote
    : undefined;
}

/** 文档页的查询参数：引用跳转到哪一块，块里标出哪一句 */
export function docSearch(search: Record<string, unknown>): { chunk?: string; quote?: string } {
  return {
    chunk: typeof search.chunk === "string" ? search.chunk : undefined,
    quote: quoteParam(search.quote),
  };
}
