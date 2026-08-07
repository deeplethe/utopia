import { useEffect, useState } from "react"
import { toast } from "sonner"
import { Crown, Shield } from "lucide-react"
import { api } from "@/lib/api"
import type { MemberDetail, Role } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet"

function RoleBadge({ role }: { role: Role }) {
  if (role === "owner") return <Badge className="gap-1"><Crown className="h-3 w-3" /> Owner</Badge>
  if (role === "editor") return <Badge variant="secondary" className="gap-1"><Shield className="h-3 w-3" /> Editor</Badge>
  return <Badge variant="outline">Viewer</Badge>
}

const CAT_LABEL: Record<string, string> = {
  ontology: "Ontology", abox: "Instance", conflict: "Conflict", extraction: "Extraction",
  document: "Document", member: "Member", ks: "Settings", system: "Rollback",
}

/** Detail drawer for a member: their role across the knowledge systems you can see, and their
 *  recent activity. Opened by clicking a member row. */
export default function MemberDetailSheet({
  ksId, userId, onClose,
}: {
  ksId: number
  userId: number
  onClose: () => void
}) {
  const [detail, setDetail] = useState<MemberDetail | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    let cancelled = false
    api.getMemberDetail(ksId, userId)
      .then((d) => { if (!cancelled) setDetail(d) })
      .catch((e) => toast.error(`Failed to load member: ${(e as Error).message}`))
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [ksId, userId])

  return (
    <Sheet open onOpenChange={(o) => !o && onClose()}>
      <SheetContent className="w-full overflow-y-auto sm:max-w-lg">
        <SheetHeader>
          <SheetTitle className="flex items-center gap-2">
            {detail?.user.username ?? (loading ? "Loading…" : "Member")}
            {detail?.user.is_admin && <Badge variant="outline" className="text-[10px]">Admin</Badge>}
            {detail && !detail.user.active && <Badge variant="destructive" className="text-[10px]">Disabled</Badge>}
          </SheetTitle>
          <SheetDescription>Access and recent activity across the knowledge systems you can see.</SheetDescription>
        </SheetHeader>

        {detail && (
          <div className="space-y-5 px-4 pb-8">
            <section>
              <h4 className="mb-1.5 text-xs font-medium text-muted-foreground">Knowledge system access ({detail.access.length})</h4>
              {detail.access.length === 0 ? (
                <p className="text-sm text-muted-foreground">No access to any knowledge system you can see.</p>
              ) : (
                <div className="divide-y rounded-lg border">
                  {detail.access.map((a) => (
                    <div key={a.ks_id} className="flex items-center justify-between gap-2 px-3 py-2 text-sm">
                      <span className="truncate font-medium">{a.ks_name}</span>
                      <RoleBadge role={a.role} />
                    </div>
                  ))}
                </div>
              )}
            </section>

            <section>
              <h4 className="mb-1.5 text-xs font-medium text-muted-foreground">Recent activity</h4>
              {detail.activity.length === 0 ? (
                <p className="text-sm text-muted-foreground">No recorded activity yet.</p>
              ) : (
                <ScrollArea className="h-80 rounded-lg border">
                  <div className="divide-y">
                    {detail.activity.map((ev, i) => (
                      <div key={i} className="px-3 py-2">
                        <div className="mb-0.5 flex items-center gap-2 text-[10px] text-muted-foreground">
                          <Badge variant="secondary" className="text-[10px]">{CAT_LABEL[ev.action.split(".")[0]] ?? ev.action}</Badge>
                          <span className="truncate">{ev.ks_name}</span>
                          <span className="ml-auto shrink-0">{new Date(ev.created_at).toLocaleString()}</span>
                        </div>
                        <p className="text-sm">{ev.summary}</p>
                      </div>
                    ))}
                  </div>
                </ScrollArea>
              )}
            </section>
          </div>
        )}
      </SheetContent>
    </Sheet>
  )
}
