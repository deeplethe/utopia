import { useMemo } from "react"
import katex from "katex"
import "katex/dist/katex.min.css"

/**
 * Axiom typesetting. The class/property labels are plain text (they're user data — often Chinese —
 * and mixing them into KaTeX math mode would mean escaping every special char), but the DL
 * operators are rendered with KaTeX so `⊑`, `⟂`, `≡` come out as proper math glyphs.
 */
export type AxOp = "sub" | "disjoint" | "equiv"

const SYMBOL: Record<AxOp, string> = {
  sub: "\\sqsubseteq",
  disjoint: "\\perp",
  equiv: "\\equiv",
}

function TeXSymbol({ op }: { op: AxOp }) {
  const html = useMemo(
    () => katex.renderToString(SYMBOL[op], { throwOnError: false, displayMode: false }),
    [op],
  )
  return <span className="mx-1.5 align-middle" aria-hidden="true" dangerouslySetInnerHTML={{ __html: html }} />
}

const OP_WORD: Record<AxOp, string> = { sub: "is a subclass of", disjoint: "is disjoint with", equiv: "is equivalent to" }

/** `left <op> right`, e.g. 游梁式抽油机 ⊑ 设备, with the operator set in KaTeX. */
export function AxiomTeX({ left, op, right, className }: { left: string; op: AxOp; right: string; className?: string }) {
  return (
    <span className={className} role="math" aria-label={`${left} ${OP_WORD[op]} ${right}`}>
      <span className="font-medium">{left}</span>
      <TeXSymbol op={op} />
      <span className="font-medium">{right}</span>
    </span>
  )
}
