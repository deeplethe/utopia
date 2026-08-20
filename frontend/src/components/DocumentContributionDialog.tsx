import { useEffect, useState } from "react"
import { Database, Loader2, Network } from "lucide-react"
import { api } from "@/lib/api"
import type { DocumentContribution, DocumentMeta } from "@/lib/types"
import { useI18n } from "@/lib/i18n"
import { Badge } from "@/components/ui/badge"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"

export default function DocumentContributionDialog({
  ksId, document, open, onOpenChange,
}: {
  ksId: number
  document: DocumentMeta | null
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const { t } = useI18n()
  const [data, setData] = useState<DocumentContribution | null>(null)
  const [error, setError] = useState("")

  useEffect(() => {
    if (!open || !document) return
    setData(null)
    setError("")
    api.getContribution(ksId, document.id).then(setData).catch((reason) => {
      setError((reason as Error).message)
    })
  }, [document, ksId, open])

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>{t("documents.contributionTitle", { name: document?.original_filename ?? "" })}</DialogTitle>
          <DialogDescription>{t("documents.contributionDescription")}</DialogDescription>
        </DialogHeader>
        {!data && !error && <div className="flex h-32 items-center justify-center"><Loader2 className="h-5 w-5 animate-spin" /></div>}
        {error && <p className="rounded-md border border-destructive/30 p-3 text-sm text-destructive">{error}</p>}
        {data && <div className="space-y-4">
          <div className="grid grid-cols-3 gap-2">
            <Metric label={t("documents.tboxAxioms")} value={data.axiom_count} />
            <Metric label={t("documents.aboxFacts")} value={data.abox_fact_count} />
            <Metric label={t("documents.individuals")} value={data.individual_count} />
          </div>
          <ScrollArea className="max-h-[55vh]">
            <div className="space-y-5 pr-3">
              <ContributionSection
                icon={<Network className="h-4 w-4" />}
                title={t("documents.tboxAxioms")}
                empty={t("documents.noTboxContribution")}
                items={data.tbox_axioms.map((item) => ({
                  key: item.axiom_key, description: item.description, shared: item.shared,
                }))}
                sharedLabel={t("documents.sharedContribution")}
              />
              <ContributionSection
                icon={<Database className="h-4 w-4" />}
                title={t("documents.aboxFacts")}
                empty={t("documents.noAboxContribution")}
                items={data.abox_facts.map((item) => ({
                  key: item.fact_key, description: item.description, shared: item.shared,
                }))}
                sharedLabel={t("documents.sharedContribution")}
              />
            </div>
          </ScrollArea>
          {data.truncated && <p className="text-xs text-muted-foreground">{t("documents.contributionTruncated")}</p>}
          <p className="text-xs text-muted-foreground">{t("documents.replaceExplanation")}</p>
        </div>}
      </DialogContent>
    </Dialog>
  )
}

function Metric({ label, value }: { label: string; value: number }) {
  return <div className="rounded-md border bg-muted/20 p-3"><p className="text-[11px] text-muted-foreground">{label}</p><p className="mt-1 text-xl font-semibold tabular-nums">{value}</p></div>
}

function ContributionSection({
  icon, title, empty, items, sharedLabel,
}: {
  icon: React.ReactNode
  title: string
  empty: string
  items: { key: string; description: string; shared: boolean }[]
  sharedLabel: string
}) {
  return <section className="space-y-2"><h3 className="flex items-center gap-2 text-sm font-semibold">{icon}{title}</h3>{items.length === 0 ? <p className="rounded-md border border-dashed p-3 text-xs text-muted-foreground">{empty}</p> : <div className="divide-y rounded-md border">{items.map((item) => <div key={item.key} className="p-3"><div className="flex items-start justify-between gap-3"><p className="text-sm">{item.description}</p>{item.shared && <Badge variant="secondary" className="shrink-0 text-[10px]">{sharedLabel}</Badge>}</div><code className="mt-1 block break-all text-[10px] text-muted-foreground">{item.key}</code></div>)}</div>}</section>
}
