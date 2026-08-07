import { useEffect, useState } from "react"
import { Check, Loader2, Sparkles, User as UserIcon } from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip"

/**
 * Small shared pieces for the Review queues (Conflicts / Entity resolution / Validation), which are
 * each a single filterable table of pending + decided rows. Keeping these here avoids re-deriving
 * "who decided this", "when", and the inline-editable rationale in three places.
 */

/** Short date for a "When" column; "—" when there's no timestamp (e.g. a still-pending row). */
export const fmtWhen = (iso: string | null) =>
  iso ? new Date(iso).toLocaleDateString(undefined, { year: "2-digit", month: "short", day: "numeric" }) : "—"

/** agent-vs-human provenance for a decided row; "—" when nobody has decided yet. */
export function By({ by }: { by: string | null }) {
  if (!by) return <span className="text-muted-foreground">—</span>
  const agent = by === "agent" || by.endsWith("-agent")
  return (
    <Badge variant="outline" className={`gap-1 text-[10px] ${agent ? "text-primary" : "text-muted-foreground"}`}>
      {agent ? <Sparkles className="h-3 w-3" /> : <UserIcon className="h-3 w-3" />}{agent ? "agent" : by}
    </Badge>
  )
}

export const REASON_MAX = 200

/** Truncated text that reveals its FULL content in a tooltip on hover (for long reasons/details). */
export function HoverText({ text, className = "" }: { text: string; className?: string }) {
  return (
    <TooltipProvider delayDuration={150}>
      <Tooltip>
        <TooltipTrigger asChild>
          <span className={`block max-w-[22rem] cursor-default truncate ${className}`}>{text}</span>
        </TooltipTrigger>
        <TooltipContent className="max-w-sm whitespace-normal break-words leading-snug">{text}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}

/**
 * Inline-editable rationale cell. Click to edit (writers only); Enter/blur saves, Esc cancels. The
 * saved text is short (≤200) and feeds back into the owning agent's prompt as experience, so editing
 * it steers future auto-decisions.
 */
export function ReasonCell({
  value, canWrite, onSave,
}: {
  value: string | null
  canWrite: boolean
  onSave: (v: string) => Promise<void>
}) {
  const [editing, setEditing] = useState(false)
  const [text, setText] = useState(value ?? "")
  const [saving, setSaving] = useState(false)
  useEffect(() => { setText(value ?? "") }, [value])

  if (!editing) {
    if (!canWrite) return <span className="text-muted-foreground">{value || "—"}</span>
    return (
      <button
        type="button"
        className="max-w-[20rem] truncate text-left text-muted-foreground hover:text-foreground"
        title={value ? `${value} — click to edit` : "Add a reason"}
        onClick={() => setEditing(true)}
      >
        {value || <span className="italic opacity-60">add reason…</span>}
      </button>
    )
  }
  const commit = async () => {
    const next = text.trim().slice(0, REASON_MAX)
    if (next === (value ?? "")) { setEditing(false); return } // no change → no write / no audit noise
    setSaving(true)
    try { await onSave(next); setEditing(false) }
    finally { setSaving(false) }
  }
  return (
    <div className="flex items-center gap-1">
      <Input
        autoFocus value={text} maxLength={REASON_MAX}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit()
          if (e.key === "Escape") { setText(value ?? ""); setEditing(false) }
        }}
        onBlur={commit}
        className="h-7 text-xs"
      />
      <Button size="icon" variant="ghost" className="h-6 w-6 shrink-0" disabled={saving}
        onMouseDown={(e) => e.preventDefault()} onClick={commit}>
        {saving ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Check className="h-3.5 w-3.5" />}
      </Button>
    </div>
  )
}
