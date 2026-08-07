import type { ReactNode } from "react"
import { useEffect, useState } from "react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import type { Chunk, DocumentContribution, DocumentMeta } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet"

function humanSize(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

function Row({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 py-1 text-sm">
      <span className="text-muted-foreground">{label}</span>
      <span className="text-right">{value}</span>
    </div>
  )
}

const fmt = (s: string | null) => (s ? new Date(s).toLocaleString() : "—")

/**
 * Per-document detail (opened by clicking a document row): info, parse & extraction history,
 * what it contributed to the ontology, and its chunks — folding what used to be the separate
 * "Sources" section and the chunks dialog into the document itself.
 */
export default function DocumentDetailSheet({
  ksId, doc, onClose,
}: {
  ksId: number
  doc: DocumentMeta
  onClose: () => void
}) {
  const [contrib, setContrib] = useState<DocumentContribution | null>(null)
  const [chunks, setChunks] = useState<Chunk[]>([])

  useEffect(() => {
    let cancelled = false
    Promise.all([
      api.getContribution(ksId, doc.id),
      doc.chunk_count > 0 ? api.getChunks(ksId, doc.id) : Promise.resolve([] as Chunk[]),
    ])
      .then(([c, ch]) => { if (!cancelled) { setContrib(c); setChunks(ch) } })
      .catch((e) => toast.error(`Failed to load document: ${(e as Error).message}`))
    return () => { cancelled = true }
  }, [ksId, doc.id, doc.chunk_count])

  return (
    <Sheet open onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="w-full overflow-y-auto sm:max-w-lg">
        <SheetHeader>
          <SheetTitle className="truncate">{doc.original_filename}</SheetTitle>
          <SheetDescription>{doc.folder}</SheetDescription>
        </SheetHeader>

        <div className="space-y-5 px-4 pb-8">
          <section>
            <h4 className="mb-1.5 text-xs font-medium text-muted-foreground">Info</h4>
            <div className="rounded-lg border px-3 py-1">
              <Row label="Type" value={<Badge variant="outline" className="uppercase">{doc.ext}</Badge>} />
              <Row label="Size" value={humanSize(doc.size_bytes)} />
              <Row label="Parse status" value={doc.parse_status + (doc.parser_backend ? ` · ${doc.parser_backend}` : "")} />
              <Row label="Characters" value={doc.text_char_count?.toLocaleString() ?? "—"} />
              <Row label="Chunks" value={doc.chunk_count || "—"} />
              <Row label="Uploaded" value={fmt(doc.uploaded_at)} />
            </div>
          </section>

          <section>
            <h4 className="mb-1.5 text-xs font-medium text-muted-foreground">Extraction history</h4>
            <div className="rounded-lg border px-3 py-1">
              <Row label="Schema (TBox)"
                value={doc.tbox_extracted_at ? <span className="text-emerald-600 dark:text-emerald-400">✓ {fmt(doc.tbox_extracted_at)}</span> : <span className="text-muted-foreground">not extracted</span>} />
              <Row label="Instances (ABox)"
                value={doc.abox_extracted_at ? <span className="text-emerald-600 dark:text-emerald-400">✓ {fmt(doc.abox_extracted_at)}</span> : <span className="text-muted-foreground">not extracted</span>} />
            </div>
          </section>

          <section>
            <h4 className="mb-1.5 text-xs font-medium text-muted-foreground">Contribution to the ontology</h4>
            <div className="grid grid-cols-2 gap-2">
              <div className="rounded-lg border p-3 text-center">
                <div className="text-2xl font-semibold">{contrib?.axiom_count ?? "—"}</div>
                <div className="text-xs text-muted-foreground">TBox axioms</div>
              </div>
              <div className="rounded-lg border p-3 text-center">
                <div className="text-2xl font-semibold">{contrib?.individual_count ?? "—"}</div>
                <div className="text-xs text-muted-foreground">ABox individuals</div>
              </div>
            </div>
          </section>

          <section>
            <h4 className="mb-1.5 text-xs font-medium text-muted-foreground">Chunks{chunks.length ? ` (${chunks.length})` : ""}</h4>
            {chunks.length === 0 ? (
              <p className="text-sm text-muted-foreground">Not parsed yet.</p>
            ) : (
              <ScrollArea className="h-64 rounded-lg border">
                <div className="divide-y">
                  {chunks.map((ck) => (
                    <div key={ck.id} className="p-2.5">
                      <div className="mb-1 flex items-center gap-2 text-[10px] text-muted-foreground">
                        <Badge variant="secondary" className="text-[10px]">#{ck.idx}</Badge>
                        {ck.text.length.toLocaleString()} chars · ~{ck.token_estimate} tokens
                      </div>
                      <p className="line-clamp-3 whitespace-pre-wrap text-xs leading-relaxed">{ck.text}</p>
                    </div>
                  ))}
                </div>
              </ScrollArea>
            )}
          </section>
        </div>
      </SheetContent>
    </Sheet>
  )
}
