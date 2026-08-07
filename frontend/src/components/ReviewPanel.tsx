import { useCallback, useEffect, useState } from "react"
import { api } from "@/lib/api"
import type { Conflict } from "@/lib/types"
import ConflictsPanel from "@/components/ConflictsPanel"
import ResolutionPanel from "@/components/ResolutionPanel"
import ValidationPanel from "@/components/ValidationPanel"

export type ReviewSub = "conflicts" | "resolution" | "validation"

/**
 * Review is a second-level menu: the sidebar picks the active sub-page and this thin container
 * renders it. Conflicts, entity resolution and validation are the "needs a human" queues;
 * Learned memory curates what the agents learned. (Only the conflicts list is loaded here — the
 * other panels fetch their own data.)
 */
export default function ReviewPanel({
  ksId, sub, canWrite, onChanged,
}: {
  ksId: number
  sub: string
  canWrite: boolean
  onChanged?: () => void
}) {
  const [conflicts, setConflicts] = useState<Conflict[]>([])

  const loadConflicts = useCallback(async () => {
    try {
      setConflicts(await api.listConflicts(ksId, "open"))
    } catch {
      /* ConflictsPanel surfaces its own errors */
    }
  }, [ksId])

  useEffect(() => { if (sub === "conflicts") loadConflicts() }, [sub, loadConflicts])

  const reload = useCallback(() => { loadConflicts(); onChanged?.() }, [loadConflicts, onChanged])

  return (
    <>
      {sub === "conflicts" && <ConflictsPanel ksId={ksId} conflicts={conflicts} canWrite={canWrite} onChanged={reload} />}
      {sub === "resolution" && <ResolutionPanel ksId={ksId} canWrite={canWrite} onChanged={reload} />}
      {sub === "validation" && <ValidationPanel ksId={ksId} canWrite={canWrite} onChanged={reload} />}
    </>
  )
}
