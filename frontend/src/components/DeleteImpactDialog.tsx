import { useCallback, useEffect, useMemo, useState } from "react"
import { toast } from "sonner"
import { AlertTriangle, Loader2, Trash2 } from "lucide-react"
import { api } from "@/lib/api"
import type { DocumentImpact, DocumentMeta } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"

const key = (ksId: number, axiom: string) => `${ksId}::${axiom}`

export default function DeleteImpactDialog({
  ksId,
  doc,
  open,
  onOpenChange,
  onDeleted,
}: {
  ksId: number
  doc: DocumentMeta | null
  open: boolean
  onOpenChange: (o: boolean) => void
  onDeleted: () => void
}) {
  const [impact, setImpact] = useState<DocumentImpact | null>(null)
  const [loading, setLoading] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [confirmed, setConfirmed] = useState<Set<string>>(new Set())

  useEffect(() => {
    if (!open || !doc) return
    setImpact(null)
    setConfirmed(new Set())
    setLoading(true)
    api
      .getImpact(ksId, doc.id)
      .then(setImpact)
      .catch((e) => toast.error(`Failed to load impact: ${(e as Error).message}`))
      .finally(() => setLoading(false))
  }, [ksId, open, doc])

  const allKeys = useMemo(
    () => (impact?.systems ?? []).flatMap((s) => s.axioms.map((a) => key(s.knowledge_system_id, a.axiom_key))),
    [impact],
  )
  const hasImpact = allKeys.length > 0
  const allConfirmed = !hasImpact || confirmed.size === allKeys.length

  const toggle = (k: string) =>
    setConfirmed((prev) => {
      const next = new Set(prev)
      next.has(k) ? next.delete(k) : next.add(k)
      return next
    })

  const confirmAll = () =>
    setConfirmed((prev) => (prev.size === allKeys.length ? new Set() : new Set(allKeys)))

  const doDelete = useCallback(async () => {
    if (!doc || !allConfirmed) return
    setDeleting(true)
    try {
      await api.deleteDocument(ksId, doc.id)
      toast.success(hasImpact ? `Document deleted, ${allKeys.length} unique axioms removed` : "Document deleted")
      onDeleted()
      onOpenChange(false)
    } catch (e) {
      toast.error(`Failed to delete: ${(e as Error).message}`)
    } finally {
      setDeleting(false)
    }
  }, [doc, allConfirmed, hasImpact, allKeys.length, onDeleted, onOpenChange])

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>Delete "{doc?.original_filename}"</DialogTitle>
          <DialogDescription>
            {loading
              ? "Analyzing this document's impact on the ontology…"
              : hasImpact
                ? "The axioms below come only from this document with no other support; deleting the document removes them too (they can't be kept). Confirm each one to delete."
                : "This document produced no unique axioms in any knowledge system; deleting it won't affect the ontology."}
          </DialogDescription>
        </DialogHeader>

        {loading ? (
          <div className="flex h-24 items-center justify-center">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        ) : hasImpact ? (
          <>
            <div className="flex items-center justify-between rounded-md bg-muted/50 px-3 py-2">
              <label className="flex cursor-pointer items-center gap-2 text-sm">
                <Checkbox checked={confirmed.size === allKeys.length} onCheckedChange={confirmAll} />
                <span className="font-medium">Confirm all</span>
              </label>
              <span className="text-xs text-muted-foreground">{confirmed.size}/{allKeys.length} confirmed</span>
            </div>
            <ScrollArea className="max-h-[46vh] pr-3">
              <div className="space-y-4">
                {impact!.systems.map((s) => (
                  <div key={s.knowledge_system_id} className="rounded-md border">
                    <div className="flex items-center gap-2 border-b bg-muted/40 px-3 py-2">
                      <AlertTriangle className="h-4 w-4 text-amber-500" />
                      <span className="text-sm font-medium">{s.knowledge_system_name}</span>
                      <Badge variant="secondary">{s.axioms.length} unique axioms</Badge>
                    </div>
                    <div className="divide-y">
                      {s.axioms.map((a) => {
                        const k = key(s.knowledge_system_id, a.axiom_key)
                        return (
                          <label key={k} className="flex cursor-pointer items-center gap-2 px-3 py-1.5 hover:bg-muted/40">
                            <Checkbox checked={confirmed.has(k)} onCheckedChange={() => toggle(k)} />
                            <span className="text-sm">{a.description}</span>
                          </label>
                        )
                      })}
                    </div>
                  </div>
                ))}
              </div>
            </ScrollArea>
          </>
        ) : null}

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={deleting}>Cancel</Button>
          <Button variant="destructive" onClick={doDelete} disabled={deleting || loading || !allConfirmed}>
            {deleting ? <Loader2 className="h-4 w-4 animate-spin" /> : <Trash2 className="h-4 w-4" />}
            {hasImpact && !allConfirmed ? `Confirm each first (${confirmed.size}/${allKeys.length})` : "Confirm delete"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
