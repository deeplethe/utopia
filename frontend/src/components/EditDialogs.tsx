import { useEffect, useMemo, useState } from "react"
import { AlertTriangle, Loader2, Trash2, X } from "lucide-react"
import { toast } from "sonner"
import { api } from "@/lib/api"
import { useI18n, type Translate } from "@/lib/i18n"
import type { EditOp, EditResult, OntologyClass, OntologyImpact, OntologyProperty } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
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
const DATATYPES = ["string", "integer", "decimal", "boolean", "date", "dateTime", "time", "anyURI"]

type OperationSubmitter = (op: EditOp) => void | Promise<void>

async function submitOperation({
  ksId,
  op,
  onSaved,
  onSubmitOperation,
  done,
  t,
}: {
  ksId: number
  op: EditOp
  onSaved?: (result: EditResult) => void
  onSubmitOperation?: OperationSubmitter
  done: () => void
  t: Translate
}) {
  try {
    if (onSubmitOperation) {
      await onSubmitOperation(op)
    } else {
      const result = await api.editOntology(ksId, op)
      onSaved?.(result)
    }
    done()
  } catch (error) {
    toast.error(t("edit.operationFailed", { error: (error as Error).message }))
  }
}

export interface ClassDialogProps {
  ksId: number
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved?: (result: EditResult) => void
  onSubmitOperation?: OperationSubmitter
  initial?: OntologyClass | null
}

// --------------------------------------------------------------------------- //
export function ClassDialog({
  ksId, open, onOpenChange, onSaved, onSubmitOperation, initial,
}: ClassDialogProps) {
  const { t } = useI18n()
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
    await submitOperation({
      ksId, op, onSaved, onSubmitOperation,
      done: () => onOpenChange(false), t,
    })
    setSaving(false)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>{initial ? t("edit.class.edit") : t("edit.class.add")}</DialogTitle></DialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-2">
            <Label htmlFor="class-label">{t("edit.label")}</Label>
            <Input id="class-label" value={label} onChange={(event) => setLabel(event.target.value)} placeholder={t("edit.class.placeholder")} />
          </div>
          <div className="space-y-2">
            <Label htmlFor="class-comment">{t("edit.descriptionOptional")}</Label>
            <Textarea id="class-comment" value={comment} onChange={(event) => setComment(event.target.value)} />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={saving}>{t("common.cancel")}</Button>
          <Button onClick={save} disabled={saving || !label.trim()}>
            {saving && <Loader2 className="h-4 w-4 animate-spin" />}
            {onSubmitOperation ? t("edit.addToChanges") : t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function unique(values: (string | null | undefined)[]) {
  return [...new Set(values.filter((value): value is string => Boolean(value)))]
}

function propertyMembers(property: OntologyProperty | null | undefined, slot: "domain" | "range") {
  if (!property) return []
  const members = property[`${slot}_members`]
  // A union's direct value is an anonymous blank node. The flattened *_members array is the
  // canonical editable representation and must win, otherwise merely renaming a property can
  // silently replace (or clear) A ∪ B.
  return unique(members?.length ? members : [property[slot]])
}

function sameMembers(a: string[], b: string[]) {
  return a.length === b.length && [...a].sort().every((value, index) => value === [...b].sort()[index])
}

function datatypeName(property: OntologyProperty | null | undefined) {
  const iri = propertyMembers(property, "range")[0]
  const candidate = iri ?? property?.range_label
  if (!candidate) return NONE
  const local = candidate.replace(/^xsd:/, "").split(/[/#]/).pop() ?? ""
  return DATATYPES.includes(local) ? local : NONE
}

function ClassMultiSelect({
  value,
  onChange,
  classes,
  ariaLabel,
}: {
  value: string[]
  onChange: (value: string[]) => void
  classes: OntologyClass[]
  ariaLabel: string
}) {
  const { t } = useI18n()
  const [selection, setSelection] = useState<string | null>(null)
  const labels = useMemo(() => new Map(classes.map((item) => [item.iri, item.label])), [classes])
  const options = classes
    .filter((item) => !value.includes(item.iri))
    .map((item) => ({ value: item.iri, label: item.label, hint: item.local }))

  const add = (iri: string) => {
    if (!value.includes(iri)) onChange([...value, iri])
    setSelection(null)
  }

  return (
    <div className="space-y-2" aria-label={ariaLabel}>
      <Combobox
        value={selection}
        onChange={add}
        options={options}
        placeholder={value.length ? t("edit.addAnotherClass") : t("edit.none")}
        searchPlaceholder={t("edit.searchClasses")}
        emptyText={t("edit.noMoreClasses")}
      />
      {value.length > 0 && (
        <div className="flex flex-wrap gap-1.5 rounded-md border bg-muted/25 p-2">
          {value.map((iri) => (
            <Badge key={iri} variant="secondary" className="h-6 gap-1 pl-2 pr-1">
              <span className="max-w-44 truncate">{labels.get(iri) ?? iri.split(/[/#]/).pop() ?? iri}</span>
              <button
                type="button"
                className="rounded-full p-0.5 hover:bg-background/80"
                aria-label={t("edit.removeClass", { name: labels.get(iri) ?? iri })}
                onClick={() => onChange(value.filter((item) => item !== iri))}
              >
                <X className="h-3 w-3" />
              </button>
            </Badge>
          ))}
        </div>
      )}
      {value.length > 1 && <p className="text-xs text-muted-foreground">{t("edit.unionHint", { count: value.length })}</p>}
    </div>
  )
}

export interface PropertyDialogProps {
  ksId: number
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved?: (result: EditResult) => void
  onSubmitOperation?: OperationSubmitter
  classes: OntologyClass[]
  initial?: (OntologyProperty & { kind: "object" | "data" }) | null
  /** Selects the creation form opened by an explicit Object/Data property entry. */
  initialKind?: "object" | "data"
}

// --------------------------------------------------------------------------- //
export function PropertyDialog({
  ksId, open, onOpenChange, onSaved, onSubmitOperation, classes, initial, initialKind = "object",
}: PropertyDialogProps) {
  const { t } = useI18n()
  const [kind, setKind] = useState<"object" | "data">(initialKind)
  const [label, setLabel] = useState("")
  const [comment, setComment] = useState("")
  const [domainMembers, setDomainMembers] = useState<string[]>([])
  const [rangeMembers, setRangeMembers] = useState<string[]>([])
  const [dataRange, setDataRange] = useState(NONE)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!open) return
    const effectiveKind = initial?.kind ?? initialKind
    setKind(effectiveKind)
    setLabel(initial?.label ?? "")
    setComment(initial?.comment ?? "")
    setDomainMembers(propertyMembers(initial, "domain"))
    setRangeMembers(effectiveKind === "object" ? propertyMembers(initial, "range") : [])
    setDataRange(effectiveKind === "data" ? datatypeName(initial) : NONE)
  }, [open, initial, initialKind])

  const save = async () => {
    if (!label.trim()) return
    setSaving(true)
    const op: EditOp = initial
      ? { op: "update_property", iri: initial.iri, label: label.trim(), comment }
      : { op: "add_property", kind, label: label.trim(), comment }

    const initialDomains = propertyMembers(initial, "domain")
    if (!initial || !sameMembers(domainMembers, initialDomains)) {
      if (domainMembers.length === 0) op.clear_domain = true
      else if (domainMembers.length === 1) op.domain = domainMembers[0]
      else op.domain_members = domainMembers
    }

    if (kind === "object") {
      const initialRanges = propertyMembers(initial, "range")
      if (!initial || !sameMembers(rangeMembers, initialRanges)) {
        if (rangeMembers.length === 0) op.clear_range = true
        else if (rangeMembers.length === 1) op.range = rangeMembers[0]
        else op.range_members = rangeMembers
      }
    } else {
      const initialDataRange = datatypeName(initial)
      if (!initial || dataRange !== initialDataRange) {
        if (dataRange === NONE) op.clear_range = true
        else op.range = dataRange
      }
    }

    await submitOperation({
      ksId, op, onSaved, onSubmitOperation,
      done: () => onOpenChange(false), t,
    })
    setSaving(false)
  }

  const effectiveKind = initial?.kind ?? kind

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>{initial ? t("edit.property.edit") : effectiveKind === "data" ? t("edit.dataProperty.add") : t("edit.objectProperty.add")}</DialogTitle>
          <DialogDescription>{effectiveKind === "object" ? t("edit.objectPropertyDescription") : t("edit.dataPropertyDescription")}</DialogDescription>
        </DialogHeader>
        <div className="space-y-4 py-2">
          {!initial && (
            <div className="space-y-2">
              <Label>{t("common.type")}</Label>
              <Select value={kind} onValueChange={(value) => setKind(value as "object" | "data")}>
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="object">{t("edit.objectProperty")}</SelectItem>
                  <SelectItem value="data">{t("edit.dataProperty")}</SelectItem>
                </SelectContent>
              </Select>
            </div>
          )}
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label htmlFor="property-label">{t("edit.label")}</Label>
              <Input id="property-label" value={label} onChange={(event) => setLabel(event.target.value)} placeholder={t("edit.property.placeholder")} />
            </div>
            <div className="space-y-2 sm:col-span-2">
              <Label htmlFor="property-comment">{t("edit.descriptionOptional")}</Label>
              <Textarea id="property-comment" value={comment} onChange={(event) => setComment(event.target.value)} />
            </div>
          </div>
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label>{t("entities.domain")}</Label>
              <ClassMultiSelect
                value={domainMembers}
                onChange={setDomainMembers}
                classes={classes}
                ariaLabel={t("entities.domain")}
              />
            </div>
            <div className="space-y-2">
              <Label>{t("entities.range")}</Label>
              {effectiveKind === "object" ? (
                <ClassMultiSelect
                  value={rangeMembers}
                  onChange={setRangeMembers}
                  classes={classes}
                  ariaLabel={t("entities.range")}
                />
              ) : (
                <Combobox
                  value={dataRange}
                  onChange={setDataRange}
                  options={[{ value: NONE, label: t("edit.none") }, ...DATATYPES.map((datatype) => ({ value: datatype, label: `xsd:${datatype}` }))]}
                  searchPlaceholder={t("edit.searchTypes")}
                />
              )}
            </div>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={saving}>{t("common.cancel")}</Button>
          <Button onClick={save} disabled={saving || !label.trim()}>
            {saving && <Loader2 className="h-4 w-4 animate-spin" />}
            {onSubmitOperation ? t("edit.addToChanges") : t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export interface AxiomDialogProps {
  ksId: number
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved?: (result: EditResult) => void
  onSubmitOperation?: OperationSubmitter
  classes: OntologyClass[]
}

// --------------------------------------------------------------------------- //
export function AxiomDialog({
  ksId, open, onOpenChange, onSaved, onSubmitOperation, classes,
}: AxiomDialogProps) {
  const { t } = useI18n()
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
    await submitOperation({
      ksId, op, onSaved, onSubmitOperation,
      done: () => onOpenChange(false), t,
    })
    setSaving(false)
  }

  const labelA = type === "subclass" ? t("edit.subclass") : t("edit.classA")
  const labelB = type === "subclass" ? t("edit.superclass") : t("edit.classB")

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>{t("edit.axiom.add")}</DialogTitle></DialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-2">
            <Label>{t("edit.relationType")}</Label>
            <Select value={type} onValueChange={(value) => setType(value as typeof type)}>
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="subclass">{t("edit.axiom.subclass")}</SelectItem>
                <SelectItem value="disjoint">{t("edit.axiom.disjoint")}</SelectItem>
                <SelectItem value="equivalent">{t("edit.axiom.equivalent")}</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-2">
              <Label>{labelA}</Label>
              <Combobox
                value={a} onChange={setA} placeholder={t("edit.selectClass")} searchPlaceholder={t("edit.searchClasses")}
                options={classes.map((item) => ({ value: item.iri, label: item.label }))}
              />
            </div>
            <div className="space-y-2">
              <Label>{labelB}</Label>
              <Combobox
                value={b} onChange={setB} placeholder={t("edit.selectClass")} searchPlaceholder={t("edit.searchClasses")}
                options={classes.map((item) => ({ value: item.iri, label: item.label }))}
              />
            </div>
          </div>
          {a && b && a === b && <p className="text-xs text-destructive">{t("edit.classesDifferent")}</p>}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={saving}>{t("common.cancel")}</Button>
          <Button onClick={save} disabled={saving || !a || !b || a === b}>
            {saving && <Loader2 className="h-4 w-4 animate-spin" />}
            {onSubmitOperation ? t("edit.addToChanges") : t("common.save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function OntologyDeleteImpactDialog({
  open,
  onOpenChange,
  kind,
  label,
  impact,
  loading = false,
  confirming = false,
  onConfirm,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  kind: "class" | "property"
  label: string
  impact: OntologyImpact | null
  loading?: boolean
  confirming?: boolean
  onConfirm: () => void | Promise<void>
}) {
  const { t } = useI18n()
  const rows = impact ? [
    ["edit.deleteImpact.ontologyTriples", impact.tbox_triples ?? 0],
    ["edit.deleteImpact.axioms", impact.referencing_axioms ?? 0],
    ["edit.deleteImpact.subclasses", impact.subclasses.length],
    ["edit.deleteImpact.properties", impact.properties_using_class.length],
    ["edit.deleteImpact.typeAssertions", impact.abox_type_assertions ?? 0],
    ["edit.deleteImpact.propertyAssertions", impact.abox_property_assertions ?? 0],
    ["edit.deleteImpact.affectedIndividuals", impact.affected_individual_count ?? impact.affected_individuals.length],
    ["edit.deleteImpact.retypedIndividuals", impact.individuals_retyped_count ?? impact.individuals_retyped.length],
    ["edit.deleteImpact.individuals", impact.individuals_deleted_count ?? impact.individuals_deleted.length],
  ] as const : []
  const hasImpact = rows.some(([, count]) => count > 0)

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("edit.deleteImpact.title", { name: label })}</DialogTitle>
          <DialogDescription>
            {loading
              ? t("edit.deleteImpact.loading")
              : kind === "class"
                ? t("edit.deleteImpact.classDescription")
                : t("edit.deleteImpact.propertyDescription")}
          </DialogDescription>
        </DialogHeader>
        {loading ? (
          <div className="flex h-28 items-center justify-center"><Loader2 className="h-5 w-5 animate-spin text-muted-foreground" /></div>
        ) : (
          <div className="space-y-3">
            <div className="grid grid-cols-2 gap-2">
              {rows.filter(([, count]) => count > 0).map(([key, count]) => (
                <div key={key} className="rounded-md border bg-muted/30 px-3 py-2">
                  <div className="text-lg font-semibold tabular-nums">{count}</div>
                  <div className="text-xs text-muted-foreground">{t(key)}</div>
                </div>
              ))}
              {!hasImpact && <p className="col-span-2 py-4 text-center text-sm text-muted-foreground">{t("edit.deleteImpact.noDependencies")}</p>}
            </div>
            {(impact?.warnings ?? []).map((warning) => (
              <div key={warning} className="flex gap-2 rounded-md border border-amber-500/30 bg-amber-500/5 p-3 text-sm">
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-600" />
                <span>{warning}</span>
              </div>
            ))}
          </div>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={confirming}>{t("common.cancel")}</Button>
          <Button onClick={onConfirm} disabled={loading || confirming}>
            {confirming ? <Loader2 className="h-4 w-4 animate-spin" /> : <Trash2 className="h-4 w-4" />}
            {t("edit.deleteImpact.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
