import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { ChevronLeft, ChevronRight, Crown, Loader2, Plus, Search, Shield, ShieldCheck, Trash2, UserPlus, X } from "lucide-react"
import { api } from "@/lib/api"
import type { GrantableUser, Member, Role } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import MemberDetailSheet from "@/components/MemberDetailSheet"

const ROLE_LABEL: Record<Role, string> = { owner: "Owner", editor: "Editor", viewer: "Viewer" }
const PAGE_SIZE = 15

function RoleBadge({ role }: { role: Role }) {
  if (role === "owner") return <Badge className="gap-1"><Crown className="h-3 w-3" /> Owner</Badge>
  if (role === "editor") return <Badge variant="secondary" className="gap-1"><Shield className="h-3 w-3" /> Editor</Badge>
  return <Badge variant="outline">Viewer</Badge>
}

export default function MembersPanel({ ksId, canManage }: { ksId: number; canManage: boolean }) {
  const [members, setMembers] = useState<Member[]>([])
  const [loading, setLoading] = useState(true)
  const [role, setRole] = useState<Role>("viewer")
  const [adding, setAdding] = useState(false)
  const [addOpen, setAddOpen] = useState(false)
  const [pickerOpen, setPickerOpen] = useState(false)
  const [pickerQ, setPickerQ] = useState("")
  const [candidates, setCandidates] = useState<GrantableUser[]>([])
  const [candLoading, setCandLoading] = useState(false)
  const [selected, setSelected] = useState<GrantableUser | null>(null)
  const [q, setQ] = useState("")
  const [page, setPage] = useState(0)
  const [detailUserId, setDetailUserId] = useState<number | null>(null)
  useEffect(() => { setPage(0) }, [q])

  // Search grantable users (debounced) only once the user types — an empty box shows no list.
  useEffect(() => {
    if (!addOpen) return
    const query = pickerQ.trim()
    if (!query) { setCandidates([]); setCandLoading(false); return }
    setCandLoading(true)
    const t = setTimeout(async () => {
      try { setCandidates(await api.grantableUsers(ksId, query)) }
      catch (e) { toast.error(`Failed to load users: ${(e as Error).message}`) }
      finally { setCandLoading(false) }
    }, 200)
    return () => clearTimeout(t)
  }, [addOpen, pickerQ, ksId])

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      setMembers(await api.listMembers(ksId))
    } catch (e) {
      toast.error(`Failed to load members: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [ksId])

  useEffect(() => { refresh() }, [refresh])

  const add = useCallback(async () => {
    if (!selected) return
    setAdding(true)
    try {
      setMembers(await api.addMember(ksId, selected.username, role))
      toast.success(`Granted "${selected.username}" ${ROLE_LABEL[role]} access`)
      setAddOpen(false)
    } catch (e) {
      toast.error(`Failed to add: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    } finally {
      setAdding(false)
    }
  }, [ksId, selected, role])

  const remove = useCallback(async (m: Member) => {
    if (!confirm(`Remove access for member "${m.username}"?`)) return
    try {
      await api.removeMember(ksId, m.user_id)
      toast.success("Removed")
      refresh()
    } catch (e) {
      toast.error(`Failed to remove: ${(e as Error).message}`)
    }
  }, [ksId, refresh])

  const filtered = members.filter((m) => m.username.toLowerCase().includes(q.trim().toLowerCase()))
  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE))
  const p = Math.min(page, pageCount - 1)
  const shown = filtered.slice(p * PAGE_SIZE, p * PAGE_SIZE + PAGE_SIZE)

  return (
    <div className="space-y-4">
      {(members.length > 0 || canManage) && (
        <div className="flex flex-wrap items-center justify-between gap-2">
          {members.length > 0 ? (
            <div className="relative">
              <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search members…" className="h-8 w-44 pl-7 text-sm" />
            </div>
          ) : <span />}
          {canManage && (
            <Button size="sm" onClick={() => { setPickerQ(""); setPickerOpen(false); setSelected(null); setRole("viewer"); setCandidates([]); setAddOpen(true) }}>
              <UserPlus className="h-4 w-4" /> Add member
            </Button>
          )}
        </div>
      )}

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Username</TableHead><TableHead>Role</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={3} className="h-16 text-center text-muted-foreground">Loading…</TableCell></TableRow>
            ) : shown.length === 0 ? (
              <TableRow><TableCell colSpan={3} className="h-16 text-center text-muted-foreground">{q ? "No matching members." : "No members yet."}</TableCell></TableRow>
            ) : shown.map((m) => (
              <TableRow key={m.user_id} className="cursor-pointer" onClick={() => setDetailUserId(m.user_id)}>
                <TableCell className="font-medium">{m.username}</TableCell>
                <TableCell><RoleBadge role={m.role} /></TableCell>
                <TableCell className="text-right" onClick={(e) => e.stopPropagation()}>
                  {canManage && m.role !== "owner" ? (
                    <Button size="icon" variant="ghost" className="h-8 w-8 text-muted-foreground hover:text-destructive"
                      title="Remove" onClick={() => remove(m)}>
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  ) : <span className="text-xs text-muted-foreground">—</span>}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>

      {filtered.length > PAGE_SIZE && (
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>{p * PAGE_SIZE + 1}–{Math.min(filtered.length, (p + 1) * PAGE_SIZE)} of {filtered.length}</span>
          <div className="flex gap-1">
            <Button size="sm" variant="outline" className="h-7 w-7 p-0" disabled={p === 0} onClick={() => setPage(p - 1)}>
              <ChevronLeft className="h-4 w-4" />
            </Button>
            <Button size="sm" variant="outline" className="h-7 w-7 p-0" disabled={p >= pageCount - 1} onClick={() => setPage(p + 1)}>
              <ChevronRight className="h-4 w-4" />
            </Button>
          </div>
        </div>
      )}

      {!canManage && (
        <p className="text-xs text-muted-foreground">
          <Plus className="mr-1 inline h-3 w-3" />Only the owner can add or remove members.
        </p>
      )}

      {/* Add member: search an existing user, pick one, choose a role */}
      <Dialog open={addOpen} onOpenChange={setAddOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add member</DialogTitle>
            <DialogDescription>Search an existing user and grant them access to this knowledge system.</DialogDescription>
          </DialogHeader>
          <div className="space-y-3 py-1">
            {/* Search combobox: matches pop in a dropdown as you type, and close once you pick one. */}
            <div className="relative">
              <Search className="absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={pickerQ}
                onChange={(e) => { setPickerQ(e.target.value); setPickerOpen(true) }}
                onFocus={() => setPickerOpen(true)}
                onBlur={() => setTimeout(() => setPickerOpen(false), 150)}
                placeholder="Search users by name…" className="pl-8" autoFocus
              />
              {pickerOpen && pickerQ.trim() && (
                <div className="absolute z-50 mt-1 max-h-60 w-full overflow-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-md">
                  {candLoading ? (
                    <div className="flex h-14 items-center justify-center text-sm text-muted-foreground">
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" /> Searching…
                    </div>
                  ) : candidates.length === 0 ? (
                    <div className="flex h-14 items-center justify-center text-sm text-muted-foreground">No matching users.</div>
                  ) : candidates.map((u) => (
                    <button
                      key={u.id} type="button"
                      // onMouseDown (not onClick) so the pick registers before the input blur closes the menu.
                      onMouseDown={(e) => { e.preventDefault(); setSelected(u); setPickerQ(""); setPickerOpen(false) }}
                      className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent hover:text-accent-foreground"
                    >
                      <span className="truncate font-medium">{u.username}</span>
                      {u.is_admin && (
                        <Badge variant="secondary" className="gap-1 text-[10px]"><ShieldCheck className="h-3 w-3" /> Admin</Badge>
                      )}
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* Chosen user */}
            {selected && (
              <div className="flex items-center justify-between gap-2 rounded-md border bg-muted/40 px-3 py-2 text-sm">
                <span className="flex min-w-0 items-center gap-2">
                  <UserPlus className="h-4 w-4 shrink-0 text-primary" />
                  <span className="truncate font-medium">{selected.username}</span>
                  {selected.is_admin && (
                    <Badge variant="secondary" className="gap-1 text-[10px]"><ShieldCheck className="h-3 w-3" /> Admin</Badge>
                  )}
                </span>
                <button type="button" onClick={() => setSelected(null)} title="Clear"
                  className="shrink-0 text-muted-foreground hover:text-foreground">
                  <X className="h-4 w-4" />
                </button>
              </div>
            )}

            <div className="flex items-center gap-2">
              <Label className="text-sm">Role</Label>
              <Select value={role} onValueChange={(v) => setRole(v as Role)}>
                <SelectTrigger className="w-32"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="viewer">Viewer</SelectItem>
                  <SelectItem value="editor">Editor</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setAddOpen(false)}>Cancel</Button>
            <Button onClick={add} disabled={adding || !selected}>
              {adding ? <Loader2 className="h-4 w-4 animate-spin" /> : <UserPlus className="h-4 w-4" />} Grant access
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {detailUserId != null && (
        <MemberDetailSheet ksId={ksId} userId={detailUserId} onClose={() => setDetailUserId(null)} />
      )}
    </div>
  )
}
