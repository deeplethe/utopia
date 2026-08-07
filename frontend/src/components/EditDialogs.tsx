import { useEffect, useState } from "react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import type { EditOp, EditResult, OntologyClass, OntologyProperty } from "@/lib/types"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Combobox } from "@/components/ui/combobox"

const NONE = "__none__"
const DATATYPES = ["string", "integer", "decimal", "boolean", "date", "dateTime"]

async function apply(ksId: number, op: EditOp, onSaved: (r: EditResult) => void, done: () => void) {
  try {
    const res = await api.editOntology(ksId, op)
    onSaved(res)
    done()
  } catch (e) {
    toast.error(`Operation failed: ${(e as Error).message}`)
  }
}

// --------------------------------------------------------------------------- //
export function ClassDialog({
  ksId, open, onOpenChange, onSaved, initial,
}: {
  ksId: number
  open: boolean
  onOpenChange: (o: boolean) => void
  onSaved: (r: EditResult) => void
  initial?: OntologyClass | null
}) {
  const [label, setLabel] = useState("")
  const [comment, setComment] = useState("")
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (open) {
      setLabel(initial?.label ?? "")
      setComment(initial?.comment ?? "")
    }
  }, [open, initial])

  const save = async () => {
    if (!label.trim()) return
    setSaving(true)
    const op: EditOp = initial
      ? { op: "update_class", iri: initial.iri, label: label.trim(), comment }
      : { op: "add_class", label: label.trim(), comment }
    await apply(ksId, op, onSaved, () => onOpenChange(false))
    setSaving(false)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>{initial ? "Edit Class" : "Add Class"}</DialogTitle></DialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-2">
            <Label>Label</Label>
            <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="e.g. Centrifugal Pump" />
          </div>
          <div className="space-y-2">
            <Label>Description (optional)</Label>
            <Textarea value={comment} onChange={(e) => setComment(e.target.value)} />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={save} disabled={saving || !label.trim()}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// --------------------------------------------------------------------------- //
export function PropertyDialog({
  ksId, open, onOpenChange, onSaved, classes, initial,
}: {
  ksId: number
  open: boolean
  onOpenChange: (o: boolean) => void
  onSaved: (r: EditResult) => void
  classes: OntologyClass[]
  initial?: (OntologyProperty & { kind: "object" | "data" }) | null
}) {
  const [kind, setKind] = useState<"object" | "data">("object")
  const [label, setLabel] = useState("")
  const [domain, setDomain] = useState(NONE)
  const [range, setRange] = useState(NONE)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (open) {
      setKind(initial?.kind ?? "object")
      setLabel(initial?.label ?? "")
      setDomain(initial?.domain ?? NONE)
      // A data property's `range` is the full XSD IRI, which matches none of the bare datatype
      // options and would silently clobber the type to xsd:string on save — derive the bare name
      // from range_label ("xsd:integer" → "integer"). Object properties keep the class IRI.
      const isData = (initial?.kind ?? "object") === "data"
      const rl = initial?.range_label
      setRange(isData ? (rl ? rl.replace(/^xsd:/, "") : NONE) : (initial?.range ?? NONE))
    }
  }, [open, initial])

  const save = async () => {
    if (!label.trim()) return
    setSaving(true)
    const base = initial
      ? { op: "update_property", iri: initial.iri, label: label.trim() }
      : { op: "add_property", kind, label: label.trim() }
    const op: EditOp = { ...base }
    if (domain === NONE) op.clear_domain = true
    else op.domain = domain
    if (range === NONE) op.clear_range = true
    else op.range = range
    await apply(ksId, op, onSaved, () => onOpenChange(false))
    setSaving(false)
  }

  const effectiveKind = initial?.kind ?? kind

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>{initial ? "Edit Property" : "Add Property"}</DialogTitle></DialogHeader>
        <div className="space-y-4 py-2">
          {!initial && (
            <div className="space-y-2">
              <Label>Type</Label>
              <Select value={kind} onValueChange={(v) => setKind(v as "object" | "data")}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="object">Object Property (Class → Class)</SelectItem>
                  <SelectItem value="data">Data Property (Class → Literal)</SelectItem>
                </SelectContent>
              </Select>
            </div>
          )}
          <div className="space-y-2">
            <Label>Label</Label>
            <Input value={label} onChange={(e) => setLabel(e.target.value)} placeholder="e.g. contains / rated pressure" />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-2">
              <Label>Domain</Label>
              <Combobox
                value={domain} onChange={setDomain}
                options={[{ value: NONE, label: "(none)" }, ...classes.map((c) => ({ value: c.iri, label: c.label }))]}
                searchPlaceholder="Search classes…"
              />
            </div>
            <div className="space-y-2">
              <Label>Range</Label>
              <Combobox
                value={range} onChange={setRange}
                options={effectiveKind === "object"
                  ? [{ value: NONE, label: "(none)" }, ...classes.map((c) => ({ value: c.iri, label: c.label }))]
                  : [{ value: NONE, label: "(none)" }, ...DATATYPES.map((d) => ({ value: d, label: d }))]}
                searchPlaceholder={effectiveKind === "object" ? "Search classes…" : "Search types…"}
              />
            </div>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={save} disabled={saving || !label.trim()}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// --------------------------------------------------------------------------- //
export function AxiomDialog({
  ksId, open, onOpenChange, onSaved, classes,
}: {
  ksId: number
  open: boolean
  onOpenChange: (o: boolean) => void
  onSaved: (r: EditResult) => void
  classes: OntologyClass[]
}) {
  const [type, setType] = useState<"subclass" | "disjoint" | "equivalent">("subclass")
  const [a, setA] = useState("")
  const [b, setB] = useState("")
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (open) { setType("subclass"); setA(""); setB("") }
  }, [open])

  const save = async () => {
    if (!a || !b || a === b) return
    setSaving(true)
    const op: EditOp = type === "subclass"
      ? { op: "add_axiom", type, sub: a, super: b }
      : { op: "add_axiom", type, a, b }
    await apply(ksId, op, onSaved, () => onOpenChange(false))
    setSaving(false)
  }

  const labelA = type === "subclass" ? "Subclass (sub)" : "Class A"
  const labelB = type === "subclass" ? "Superclass (super)" : "Class B"

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>Add Axiom</DialogTitle></DialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-2">
            <Label>Relation Type</Label>
            <Select value={type} onValueChange={(v) => setType(v as typeof type)}>
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="subclass">Subclass (subClassOf) (A ⊑ B)</SelectItem>
                <SelectItem value="disjoint">Disjoint (disjointWith) (A ⟂ B)</SelectItem>
                <SelectItem value="equivalent">Equivalent (equivalentClass) (A ≡ B)</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-2">
              <Label>{labelA}</Label>
              <Combobox
                value={a} onChange={setA} placeholder="Select a class" searchPlaceholder="Search classes…"
                options={classes.map((c) => ({ value: c.iri, label: c.label }))}
              />
            </div>
            <div className="space-y-2">
              <Label>{labelB}</Label>
              <Combobox
                value={b} onChange={setB} placeholder="Select a class" searchPlaceholder="Search classes…"
                options={classes.map((c) => ({ value: c.iri, label: c.label }))}
              />
            </div>
          </div>
          {a && b && a === b && <p className="text-xs text-destructive">The two classes must be different.</p>}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={save} disabled={saving || !a || !b || a === b}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
