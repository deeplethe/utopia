import { useCallback, useEffect, useState } from "react"
import { toast } from "sonner"
import { KeyRound, Loader2, Plus, ShieldCheck, Trash2, UserCog } from "lucide-react"
import { api } from "@/lib/api"
import { useAuth } from "@/lib/auth"
import type { User } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"

export default function UsersPage() {
  const { user: me } = useAuth()
  const [users, setUsers] = useState<User[]>([])
  const [loading, setLoading] = useState(true)
  const [createOpen, setCreateOpen] = useState(false)
  const [uname, setUname] = useState("")
  const [pw, setPw] = useState("")
  const [isAdmin, setIsAdmin] = useState(false)
  const [creating, setCreating] = useState(false)
  const [pwUser, setPwUser] = useState<User | null>(null)
  const [newPw, setNewPw] = useState("")

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      setUsers(await api.listUsers())
    } catch (e) {
      toast.error(`Failed to load users: ${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { refresh() }, [refresh])

  const create = useCallback(async () => {
    if (!uname.trim() || !pw) return
    setCreating(true)
    try {
      await api.createUser(uname.trim(), pw, isAdmin)
      toast.success(`Created user "${uname.trim()}"`)
      setCreateOpen(false); setUname(""); setPw(""); setIsAdmin(false)
      refresh()
    } catch (e) {
      toast.error(`Failed to create: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    } finally {
      setCreating(false)
    }
  }, [uname, pw, isAdmin, refresh])

  const patch = useCallback(async (u: User, p: { is_admin?: boolean; active?: boolean }) => {
    try {
      await api.updateUser(u.id, p)
      refresh()
    } catch (e) {
      toast.error(`Failed to update: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    }
  }, [refresh])

  const resetPw = useCallback(async () => {
    if (!pwUser || !newPw) return
    try {
      await api.updateUser(pwUser.id, { password: newPw })
      toast.success(`Reset password for "${pwUser.username}"`)
      setPwUser(null); setNewPw("")
    } catch (e) {
      toast.error(`Failed to reset: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    }
  }, [pwUser, newPw])

  const del = useCallback(async (u: User) => {
    if (!confirm(`Delete user "${u.username}"? Their grants will be removed too.`)) return
    try {
      await api.deleteUser(u.id)
      toast.success("Deleted")
      refresh()
    } catch (e) {
      toast.error(`Failed to delete: ${(e as Error).message.replace(/^\d+:\s*/, "")}`)
    }
  }, [refresh])

  return (
    <div className="space-y-5">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">User Management</h1>
          <p className="text-sm text-muted-foreground">Create accounts, set admins, enable/disable, reset passwords.</p>
        </div>
        <Button size="sm" onClick={() => setCreateOpen(true)}><Plus className="h-4 w-4" /> New User</Button>
      </div>

      <div className="rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Username</TableHead><TableHead>Role</TableHead><TableHead>Status</TableHead>
              <TableHead className="text-right">Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {loading ? (
              <TableRow><TableCell colSpan={4} className="h-20 text-center text-muted-foreground">Loading…</TableCell></TableRow>
            ) : users.map((u) => {
              const self = u.id === me?.id
              return (
                <TableRow key={u.id}>
                  <TableCell className="font-medium">
                    {u.username}
                    {self && <span className="ml-1.5 text-xs text-muted-foreground">(you)</span>}
                  </TableCell>
                  <TableCell>
                    {u.is_admin ? <Badge className="gap-1"><ShieldCheck className="h-3 w-3" /> Admin</Badge> : <Badge variant="secondary">User</Badge>}
                  </TableCell>
                  <TableCell>
                    {u.active ? <Badge variant="outline" className="border-emerald-500/40 text-emerald-600">Active</Badge> : <Badge variant="outline" className="text-muted-foreground">Disabled</Badge>}
                  </TableCell>
                  <TableCell className="space-x-1 text-right">
                    <Button size="sm" variant="ghost" title="Toggle admin" disabled={self} onClick={() => patch(u, { is_admin: !u.is_admin })}>
                      <UserCog className="h-3.5 w-3.5" /> {u.is_admin ? "Remove admin" : "Make admin"}
                    </Button>
                    <Button size="sm" variant="ghost" title="Enable/disable" disabled={self} onClick={() => patch(u, { active: !u.active })}>
                      {u.active ? "Disable" : "Enable"}
                    </Button>
                    <Button size="icon" variant="ghost" className="h-8 w-8" title="Reset password" onClick={() => { setPwUser(u); setNewPw("") }}>
                      <KeyRound className="h-4 w-4" />
                    </Button>
                    <Button size="icon" variant="ghost" className="h-8 w-8 text-muted-foreground hover:text-destructive" title="Delete" disabled={self} onClick={() => del(u)}>
                      <Trash2 className="h-4 w-4" />
                    </Button>
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      </div>

      {/* Create user */}
      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader><DialogTitle>New User</DialogTitle></DialogHeader>
          <div className="space-y-4 py-2">
            <div className="space-y-1.5">
              <Label htmlFor="new-uname">Username</Label>
              <Input id="new-uname" value={uname} onChange={(e) => setUname(e.target.value)} />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="new-pw">Initial password</Label>
              <Input id="new-pw" type="password" value={pw} onChange={(e) => setPw(e.target.value)} />
            </div>
            <label className="flex cursor-pointer items-center gap-2 text-sm">
              <Checkbox checked={isAdmin} onCheckedChange={(v) => setIsAdmin(!!v)} /> Make admin
            </label>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setCreateOpen(false)}>Cancel</Button>
            <Button onClick={create} disabled={creating || !uname.trim() || !pw}>
              {creating && <Loader2 className="h-4 w-4 animate-spin" />} Create
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Reset password */}
      <Dialog open={!!pwUser} onOpenChange={(o) => !o && setPwUser(null)}>
        <DialogContent>
          <DialogHeader><DialogTitle>Reset password for "{pwUser?.username}"</DialogTitle></DialogHeader>
          <div className="space-y-1.5 py-2">
            <Label htmlFor="reset-pw">New password</Label>
            <Input id="reset-pw" type="password" value={newPw} onChange={(e) => setNewPw(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && resetPw()} />
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPwUser(null)}>Cancel</Button>
            <Button onClick={resetPw} disabled={!newPw}>Reset</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
