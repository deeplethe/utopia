import type { ReactNode } from "react"
import { AlertTriangle, CircleAlert, GitMerge } from "lucide-react"
import { Badge } from "@/components/ui/badge"

type Tone = "error" | "warning" | "info"

const TONE: Record<Tone, { bar: string; Icon: typeof CircleAlert; color: string }> = {
  error: { bar: "border-l-destructive", Icon: CircleAlert, color: "text-destructive" },
  warning: { bar: "border-l-amber-500", Icon: AlertTriangle, color: "text-amber-500" },
  info: { bar: "border-l-primary/50", Icon: GitMerge, color: "text-primary" },
}

/**
 * One card in the Review inbox — shared by Conflicts, Entity resolution and Validation so all
 * three lists look identical: a tone-coloured left bar + icon, a title with an optional type
 * badge and meta, an optional detail line and chips, an optional top-right dismiss, and a
 * bottom action row.
 */
export default function ReviewItem({
  tone = "info", icon, title, typeLabel, meta, detail, chips, dismiss, actions,
}: {
  tone?: Tone
  icon?: ReactNode
  title: ReactNode
  typeLabel?: string
  meta?: ReactNode
  detail?: ReactNode
  chips?: { key: string; label: string }[]
  dismiss?: ReactNode
  actions?: ReactNode
}) {
  const t = TONE[tone]
  return (
    <div className={`rounded-lg border border-l-4 ${t.bar} bg-card p-3.5`}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 space-y-1.5">
          <div className="flex flex-wrap items-center gap-2">
            {icon ?? <t.Icon className={`h-4 w-4 shrink-0 ${t.color}`} />}
            <span className="text-sm font-medium">{title}</span>
            {typeLabel && <Badge variant="outline" className="text-[10px]">{typeLabel}</Badge>}
            {meta && <span className="text-xs text-muted-foreground">{meta}</span>}
          </div>
          {detail && <p className="text-sm text-muted-foreground">{detail}</p>}
          {chips && chips.length > 0 && (
            <div className="flex flex-wrap gap-1.5 pt-0.5">
              {chips.map((c) => <Badge key={c.key} variant="secondary" className="font-normal">{c.label}</Badge>)}
            </div>
          )}
        </div>
        {dismiss && <div className="shrink-0">{dismiss}</div>}
      </div>
      {actions && <div className="mt-3 flex flex-wrap gap-2 border-t pt-3">{actions}</div>}
    </div>
  )
}
