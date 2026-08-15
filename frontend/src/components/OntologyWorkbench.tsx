import type { Dispatch, KeyboardEvent as ReactKeyboardEvent, MouseEvent, ReactNode, SetStateAction } from "react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import {
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  GitBranchPlus,
  Layers,
  Link2,
  Loader2,
  Network,
  Pencil,
  Plus,
  Search,
  Table2,
  Trash2,
  X,
} from "lucide-react"
import { toast } from "sonner"
import type { AgentProposalReviewItem, EditOp, OntologyClass, OntologyImpact, OntologyProperty, OntologyView } from "@/lib/types"
import { useI18n } from "@/lib/i18n"
import { Button } from "@/components/ui/button"
import { Combobox } from "@/components/ui/combobox"
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
import { ScrollArea } from "@/components/ui/scroll-area"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import EntitiesPanel, { type AxiomGroup } from "@/components/EntitiesPanel"
import type { OntologyPreviewData } from "@/components/OntologyImpactPreview"
import SigmaOntologyGraph from "@/components/SigmaOntologyGraph"
import { ApiError } from "@/lib/api"

// --------------------------------------------------------------------------- //
// Integration contract: the workbench owns preview/confirmation; the page owns transport.
// --------------------------------------------------------------------------- //
export type WorkbenchPreview = OntologyPreviewData

export type WorkbenchCommitResult = {
  revision?: string
  view?: OntologyView
  [key: string]: unknown
}

export type ExternalWorkbenchOperation = {
  id: string | number
  operations: EditOp[]
  labels?: string[]
  reviewItems?: AgentProposalReviewItem[]
  baseRevision?: string
  preview?: WorkbenchPreview
}

export type OntologyWorkbenchProps = {
  view: OntologyView
  canWrite: boolean
  initialLens?: Lens
  initialTab?: "classes" | "object" | "data" | "axioms"
  onLensChange?: (lens: Lens) => void
  onTabChange?: (tab: "classes" | "object" | "data" | "axioms") => void
  axioms: AxiomGroup[]
  onAddClass: () => void
  onEditClass: (c: OntologyClass) => void
  onDeleteClass: (c: OntologyClass) => void
  onAddProperty: (kind?: "object" | "data") => void
  onEditProperty: (p: OntologyProperty, kind: "object" | "data") => void
  onDeleteProperty: (p: OntologyProperty) => void
  onAddAxiom: () => void
  revision?: string
  onPreviewOperations?: (ops: EditOp[], expectedRevision?: string) => Promise<WorkbenchPreview>
  onCommitOperations?: (
    ops: EditOp[],
    expectedRevision?: string,
    reason?: string,
  ) => Promise<void | WorkbenchCommitResult>
  externalOperation?: ExternalWorkbenchOperation | null
  onExternalOperationConsumed?: (id: string | number) => void
  onSelectionChange?: (selection: { iri: string; label: string } | null) => void
  onRevisionConflict?: () => void | Promise<void>
}

type PendingBatch = {
  requestId: number
  operations: EditOp[]
  labels: string[]
  reviewItems: AgentProposalReviewItem[]
  baseRevision?: string
  preview: WorkbenchPreview | null
}

function isRevisionConflict(error: unknown) {
  return error instanceof ApiError && error.status === 409
}

function isUnprocessableChangeSet(error: unknown) {
  return error instanceof ApiError && error.status === 422
}

function summarizeOntologyPreview(preview: OntologyPreviewData) {
  return {
    errors: preview.structural_validation?.error_count ?? 0,
    warnings: preview.structural_validation?.warning_count ?? 0,
    committable: preview.structural_validation?.committable !== false,
  }
}

// --------------------------------------------------------------------------- //
// Relation maps: hierarchy + property adjacency, computed once per view.
// --------------------------------------------------------------------------- //
type Maps = {
  byIri: Map<string, OntologyClass>
  parentsOf: Map<string, string[]>
  childrenOf: Map<string, string[]>
  roots: string[]
  isolated: string[]
  dataByDomain: Map<string, OntologyView["data_properties"]>
  objByDomain: Map<string, OntologyView["object_properties"]>
  objByRange: Map<string, OntologyView["object_properties"]>
  disjointOf: Map<string, string[]>
  equivOf: Map<string, string[]>
}

function push<T>(map: Map<string, T[]>, key: string, value: T) {
  if (!map.has(key)) map.set(key, [])
  map.get(key)!.push(value)
}

function pushUnique(map: Map<string, string[]>, key: string, value: string) {
  if (!map.has(key)) map.set(key, [])
  if (!map.get(key)!.includes(value)) map.get(key)!.push(value)
}

function buildMaps(view: OntologyView): Maps {
  const byIri = new Map(view.classes.map((item) => [item.iri, item]))
  const parentsOf = new Map<string, string[]>()
  const childrenOf = new Map<string, string[]>()
  for (const item of view.classes) {
    const parents = item.superclasses.filter((parent) => byIri.has(parent))
    parentsOf.set(item.iri, parents)
    for (const parent of parents) push(childrenOf, parent, item.iri)
  }
  const labelOf = (iri: string) => byIri.get(iri)?.label ?? iri
  for (const children of childrenOf.values()) children.sort((a, b) => labelOf(a).localeCompare(labelOf(b)))
  const roots = view.classes
    .filter((item) => (parentsOf.get(item.iri) ?? []).length === 0)
    .map((item) => item.iri)
    .sort((a, b) => labelOf(a).localeCompare(labelOf(b)))

  const dataByDomain = new Map<string, OntologyView["data_properties"]>()
  for (const property of view.data_properties) {
    for (const domain of property.domain_members?.length ? property.domain_members : property.domain ? [property.domain] : []) {
      push(dataByDomain, domain, property)
    }
  }
  const objByDomain = new Map<string, OntologyView["object_properties"]>()
  const objByRange = new Map<string, OntologyView["object_properties"]>()
  for (const property of view.object_properties) {
    for (const domain of property.domain_members?.length ? property.domain_members : property.domain ? [property.domain] : []) {
      push(objByDomain, domain, property)
    }
    for (const range of property.range_members?.length ? property.range_members : property.range ? [property.range] : []) {
      push(objByRange, range, property)
    }
  }
  const propertyLinked = new Set<string>()
  for (const property of view.data_properties) {
    for (const iri of property.domain_members ?? (property.domain ? [property.domain] : [])) propertyLinked.add(iri)
  }
  for (const property of view.object_properties) {
    for (const iri of property.domain_members ?? (property.domain ? [property.domain] : [])) propertyLinked.add(iri)
    for (const iri of property.range_members ?? (property.range ? [property.range] : [])) propertyLinked.add(iri)
  }
  const isolated = view.classes
    .filter((item) => (
      (parentsOf.get(item.iri) ?? []).length === 0
      && (childrenOf.get(item.iri) ?? []).length === 0
      && !propertyLinked.has(item.iri)
    ))
    .map((item) => item.iri)
  const disjointOf = new Map<string, string[]>()
  for (const relation of view.axioms.disjoint_with) {
    pushUnique(disjointOf, relation.a, relation.b)
    pushUnique(disjointOf, relation.b, relation.a)
  }
  const equivOf = new Map<string, string[]>()
  for (const relation of view.axioms.equivalent_class) {
    pushUnique(equivOf, relation.a, relation.b)
    pushUnique(equivOf, relation.b, relation.a)
  }
  return { byIri, parentsOf, childrenOf, roots, isolated, dataByDomain, objByDomain, objByRange, disjointOf, equivOf }
}

type Lens = "graph" | "table"
type GraphMode = "explore" | "full"
type RelationType = "subclass" | "disjoint" | "equivalent"

function operationLabel(op: EditOp, label: (iri: string) => string, zh: boolean) {
  const name = op.op
  if (name === "add_axiom" || name === "delete_axiom") {
    const add = name === "add_axiom"
    const type = String(op.type ?? "")
    const left = String(type === "subclass" ? op.sub ?? "" : op.a ?? "")
    const right = String(type === "subclass" ? op.super ?? "" : op.b ?? "")
    const symbol = type === "subclass" ? "⊆" : type === "disjoint" ? "⊥" : "≡"
    return `${add ? (zh ? "添加" : "Add") : (zh ? "移除" : "Remove")} ${label(left)} ${symbol} ${label(right)}`
  }
  if (name === "delete_class" || name === "delete_property") return `${zh ? "删除" : "Delete"} ${label(String(op.iri ?? ""))}`
  if (name === "update_class" || name === "update_property") return `${zh ? "更新" : "Update"} ${label(String(op.iri ?? ""))}`
  if (name === "add_class" || name === "add_property") return `${zh ? "添加" : "Add"} ${String(op.label ?? "")}`
  if (name === "merge_classes") return `${zh ? "合并" : "Merge"} ${label(String(op.source ?? ""))} → ${label(String(op.target ?? ""))}`
  if (name === "merge_properties" || name === "subordinate_properties") {
    const sources = Array.isArray(op.sources) ? op.sources.map((iri) => label(String(iri))) : []
    const target = op.target ? label(String(op.target)) : String(op.target_label ?? "")
    if (name === "merge_properties") {
      return zh
        ? `合并属性 ${sources.join("、")} → ${target}`
        : `Merge properties ${sources.join(", ")} → ${target}`
    }
    return zh
      ? `将 ${sources.join("、")} 设为 ${target} 的子属性`
      : `Make ${sources.join(", ")} subproperties of ${target}`
  }
  if (name === "set_property_union") {
    const members = Array.isArray(op.members) ? op.members.map((iri) => label(String(iri))) : []
    const slot = op.slot === "domain" ? (zh ? "定义域" : "domain") : (zh ? "值域" : "range")
    return zh
      ? `将 ${label(String(op.iri ?? ""))} 的${slot}设为 ${members.join(" ∪ ")}`
      : `Set ${label(String(op.iri ?? ""))} ${slot} to ${members.join(" ∪ ")}`
  }
  return `${zh ? "应用" : "Apply"} ${name}`
}

function operationTypeLabel(op: EditOp, zh: boolean) {
  const names: Record<string, [string, string]> = {
    add_axiom: ["添加公理", "Add axiom"],
    add_class: ["新建类", "Add class"],
    add_property: ["新建属性", "Add property"],
    delete_axiom: ["移除公理", "Remove axiom"],
    delete_class: ["删除类", "Delete class"],
    delete_property: ["删除属性", "Delete property"],
    merge_classes: ["合并类", "Merge classes"],
    merge_properties: ["合并属性", "Merge properties"],
    subordinate_properties: ["建立属性层级", "Create property hierarchy"],
    set_property_union: ["设置联合范围", "Set property union"],
    update_class: ["编辑类", "Edit class"],
    update_property: ["编辑属性", "Edit property"],
  }
  const value = names[op.op]
  return value ? value[zh ? 0 : 1] : op.op.replaceAll("_", " ")
}

function operationContent(op: EditOp, label: (iri: string) => string, zh: boolean) {
  if (op.op === "merge_classes") {
    return `${label(String(op.source ?? ""))} / ${label(String(op.target ?? ""))}`
  }
  if (op.op === "merge_properties" || op.op === "subordinate_properties") {
    const sources = Array.isArray(op.sources) ? op.sources.map((iri) => label(String(iri))) : []
    return `${zh ? "属性" : "Properties"}：${sources.join(zh ? "、" : ", ")}`
  }
  if (op.op === "add_axiom" || op.op === "delete_axiom") {
    const left = op.type === "subclass" ? op.sub : op.a
    const right = op.type === "subclass" ? op.super : op.b
    return `${label(String(left ?? ""))} / ${label(String(right ?? ""))}`
  }
  if (op.op === "set_property_union") {
    return `${label(String(op.iri ?? ""))} · ${op.slot === "domain" ? (zh ? "定义域" : "domain") : (zh ? "值域" : "range")}`
  }
  const entity = op.iri ? label(String(op.iri)) : String(op.label ?? "")
  return entity || operationTypeLabel(op, zh)
}

function operationInformation(
  op: EditOp,
  impacts: OntologyImpact[],
  label: (iri: string) => string,
  zh: boolean,
) {
  let description = ""
  if (op.op === "merge_classes") {
    description = zh
      ? `来源类 ${label(String(op.source ?? ""))} 的引用与实例类型将迁移到目标类 ${label(String(op.target ?? ""))}。`
      : `References and instance types from ${label(String(op.source ?? ""))} will move to ${label(String(op.target ?? ""))}.`
  } else if (op.op === "merge_properties") {
    const count = Array.isArray(op.sources) ? op.sources.length : 0
    description = zh ? `${count} 个来源属性将归并为一个目标属性。` : `${count} source properties will be consolidated into one target property.`
  } else if (op.op === "subordinate_properties") {
    description = zh ? "来源属性会保留，并建立到通用属性的子属性关系。" : "The source properties remain and become subproperties of the general property."
  } else if (op.op === "set_property_union") {
    const members = Array.isArray(op.members) ? op.members.map((iri) => label(String(iri))) : []
    description = zh ? `允许的成员为：${members.join("、")}。` : `Allowed members: ${members.join(", ")}.`
  } else if (op.op === "delete_class" || op.op === "delete_property") {
    description = zh ? "该实体及其相关引用将被移除。" : "The entity and its related references will be removed."
  } else if (op.op === "add_class" || op.op === "add_property") {
    description = String(op.comment ?? (zh ? "将在本体中创建新的命名实体。" : "A new named entity will be created in the ontology."))
  } else if (op.op === "update_class" || op.op === "update_property") {
    const fields = Object.keys(op).filter((key) => !["op", "iri"].includes(key))
    description = zh ? `将更新字段：${fields.join("、") || "实体信息"}。` : `Fields to update: ${fields.join(", ") || "entity metadata"}.`
  } else if (op.op === "add_axiom" || op.op === "delete_axiom") {
    description = zh ? "该决定会调整两个本体实体之间的结构关系。" : "This decision changes the structural relationship between two ontology entities."
  }

  const affected = impacts.reduce((sum, item) => sum + (item.affected_individual_count ?? item.affected_individuals?.length ?? 0), 0)
  const deleted = impacts.reduce((sum, item) => sum + (item.individuals_deleted_count ?? item.individuals_deleted?.length ?? 0), 0)
  const retyped = impacts.reduce((sum, item) => sum + (item.individuals_retyped_count ?? item.individuals_retyped?.length ?? 0), 0)
  const references = impacts.reduce((sum, item) => sum + (item.referencing_axioms ?? 0), 0)
  const impactParts = [
    affected > 0 ? (zh ? `${affected} 个受影响实例` : `${affected} affected individuals`) : "",
    retyped > 0 ? (zh ? `${retyped} 个实例将重新归类` : `${retyped} individuals retyped`) : "",
    deleted > 0 ? (zh ? `${deleted} 个实例将删除` : `${deleted} individuals deleted`) : "",
    references > 0 ? (zh ? `${references} 条引用公理` : `${references} referencing axioms`) : "",
  ].filter(Boolean)
  if (impactParts.length > 0) return `${description} ${zh ? "预检影响" : "Preflight impact"}：${impactParts.join(" · ")}`.trim()
  if (impacts.length > 0) return `${description} ${zh ? "预检未发现实例数据影响。" : "No instance-data impact was found in preflight."}`.trim()
  return description || (zh ? "提交前将进行结构与影响校验。" : "Structural and impact validation will run before commit.")
}

function queueLabel(queue: string, zh: boolean) {
  const labels: Record<string, [string, string]> = {
    conflicts: ["冲突", "Conflict"],
    validation: ["校验", "Validation"],
    terminology: ["术语", "Terminology"],
    entity_resolution: ["实体消歧", "Entity resolution"],
  }
  return labels[queue]?.[zh ? 0 : 1] ?? queue.replaceAll("_", " ")
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`
  if (value && typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => `${JSON.stringify(key)}:${stableJson(item)}`)
    return `{${entries.join(",")}}`
  }
  return JSON.stringify(value) ?? "undefined"
}

function previewMatches(
  preview: WorkbenchPreview | undefined,
  operations: EditOp[],
  baseRevision: string | undefined,
  currentRevision: string | undefined,
) {
  return Boolean(
    preview
    && baseRevision
    && currentRevision
    && preview.base_revision === baseRevision
    && baseRevision === currentRevision
    && Array.isArray(preview.operations)
    && stableJson(preview.operations) === stableJson(operations),
  )
}

/** A single modeling surface with shared selection and preview-before-commit editing. */
export default function OntologyWorkbench({
  view,
  canWrite,
  initialLens = "graph",
  initialTab = "classes",
  onLensChange,
  onTabChange,
  axioms,
  onAddClass,
  onEditClass,
  onDeleteClass,
  onAddProperty,
  onEditProperty,
  onDeleteProperty,
  onAddAxiom,
  revision: revisionProp,
  onPreviewOperations,
  onCommitOperations,
  externalOperation,
  onExternalOperationConsumed,
  onSelectionChange,
  onRevisionConflict,
}: OntologyWorkbenchProps) {
  const { locale, t } = useI18n()
  const zh = locale === "zh-CN"
  const revision = revisionProp ?? (view as OntologyView & { revision?: string }).revision
  const maps = useMemo(() => buildMaps(view), [view])
  const [lens, setLens] = useState<Lens>(initialLens)
  const [graphMode, setGraphMode] = useState<GraphMode>("explore")
  const [depth, setDepth] = useState(1)
  const [focus, setFocus] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [query, setQuery] = useState("")
  const [pending, setPending] = useState<PendingBatch | null>(null)
  const [busy, setBusy] = useState<"preview" | "commit" | null>(null)
  const [relationOpen, setRelationOpen] = useState(false)
  const [relationType, setRelationType] = useState<RelationType>("subclass")
  const [relationTarget, setRelationTarget] = useState("")
  const [commitReason, setCommitReason] = useState("")
  const consumedExternal = useRef(new Set<string | number>())
  const workbenchRef = useRef<HTMLDivElement>(null)
  const pendingRef = useRef<PendingBatch | null>(null)
  const activeRequestId = useRef(0)

  const label = useCallback((iri: string) => maps.byIri.get(iri)?.label ?? view.labels[iri] ?? iri.split(/[#/]/).pop() ?? iri, [maps, view.labels])
  const explore = useCallback((iri: string) => { setFocus(iri); setSelected(iri) }, [])
  const select = useCallback((iri: string) => setSelected(iri), [])
  const drillIn = useCallback((iri: string) => { setFocus(iri); setSelected(iri); setGraphMode("explore") }, [])

  useEffect(() => setLens(initialLens), [initialLens])

  const replacePending = useCallback((next: PendingBatch | null) => {
    pendingRef.current = next
    setPending(next)
  }, [])

  const discardPending = useCallback(() => {
    activeRequestId.current += 1
    replacePending(null)
    setCommitReason("")
    setBusy((current) => current === "preview" ? null : current)
  }, [replacePending])

  const requestChange = useCallback((
    operations: EditOp[],
    labels?: string[],
    operationBaseRevision?: string,
    reviewItems: AgentProposalReviewItem[] = [],
    prefetchedPreview?: WorkbenchPreview,
  ) => {
    if (!operations.length || pendingRef.current) return false

    const requestId = ++activeRequestId.current
    const baseRevision = operationBaseRevision ?? prefetchedPreview?.base_revision ?? revision
    const reusablePreview = previewMatches(prefetchedPreview, operations, baseRevision, revision)
      ? prefetchedPreview ?? null
      : null
    if (!reusablePreview && !onPreviewOperations) return false
    const batch: PendingBatch = {
      requestId,
      operations: [...operations],
      labels: operations.map((op, index) => labels?.[index] ?? operationLabel(op, label, zh)),
      reviewItems: reviewItems.map((item) => ({ ...item })),
      baseRevision,
      preview: reusablePreview,
    }
    replacePending(batch)
    setCommitReason("")
    if (reusablePreview) {
      setBusy(null)
      if (reusablePreview.structural_validation?.committable === false) {
        toast.warning(zh ? "预检发现新的结构错误，提交已阻止" : "Preflight found new structural errors; commit is blocked")
      }
      return true
    }
    setBusy("preview")

    void (async () => {
      try {
        const result = await onPreviewOperations!(batch.operations, batch.baseRevision)
        if (activeRequestId.current !== requestId || pendingRef.current?.requestId !== requestId) return
        replacePending({
          ...batch,
          // Dry-run responses carry the authoritative baseline. This fallback keeps
          // commits revision-safe even if a caller could not supply a visible revision.
          baseRevision: batch.baseRevision ?? result.base_revision,
          preview: result,
        })
        if (result.structural_validation?.committable === false) {
          toast.warning(zh ? "预检发现新的结构错误，提交已阻止" : "Preflight found new structural errors; commit is blocked")
        }
      } catch (error) {
        if (activeRequestId.current !== requestId) return
        replacePending(null)
        if (isRevisionConflict(error)) {
          await onRevisionConflict?.()
          toast.warning(zh ? "本体已被其他操作更新，请基于最新版本重新编辑" : "The ontology changed elsewhere. Edit again from the latest version")
        } else {
          toast.error(zh ? `预览失败：${error instanceof Error ? error.message : String(error)}` : `Preview failed: ${error instanceof Error ? error.message : String(error)}`)
        }
      } finally {
        if (activeRequestId.current === requestId) setBusy(null)
      }
    })()
    return true
  }, [label, onPreviewOperations, onRevisionConflict, replacePending, revision, zh])

  useEffect(() => {
    if ((!focus || !maps.byIri.has(focus)) && maps.roots.length) {
      setFocus(maps.roots[0]); setSelected(maps.roots[0])
    }
  }, [maps, focus])

  useEffect(() => {
    const item = selected ? maps.byIri.get(selected) : null
    onSelectionChange?.(item ? { iri: item.iri, label: item.label } : null)
  }, [maps, onSelectionChange, selected])

  useEffect(() => {
    if (!selected) return
    setExpanded((current) => {
      const next = new Set(current)
      const stack = [...(maps.parentsOf.get(selected) ?? [])]
      const seen = new Set<string>()
      while (stack.length) {
        const parent = stack.pop()!
        if (seen.has(parent)) continue
        seen.add(parent); next.add(parent)
        stack.push(...(maps.parentsOf.get(parent) ?? []))
      }
      return next
    })
  }, [selected, maps])

  useEffect(() => {
    // Consume at most one immutable batch. The next FIFO item stays with the parent
    // until this preview is cancelled, committed, or rejected.
    if (busy !== null || pending !== null || !externalOperation || consumedExternal.current.has(externalOperation.id)) return
    if (!externalOperation.operations.length) {
      consumedExternal.current.add(externalOperation.id)
      onExternalOperationConsumed?.(externalOperation.id)
      return
    }
    const accepted = requestChange(
      externalOperation.operations,
      externalOperation.labels,
      externalOperation.baseRevision,
      externalOperation.reviewItems,
      externalOperation.preview,
    )
    if (accepted) {
      consumedExternal.current.add(externalOperation.id)
      onExternalOperationConsumed?.(externalOperation.id)
    }
  }, [busy, externalOperation, onExternalOperationConsumed, pending, requestChange])

  const sel = selected ? maps.byIri.get(selected) : null
  const needle = query.trim().toLocaleLowerCase()
  const matches = needle
    ? view.classes.filter((item) => [item.label, item.local, item.iri, item.comment].some((value) => value?.toLocaleLowerCase().includes(needle)))
    : []

  const confirmRelation = () => {
    if (!selected || !relationTarget || relationTarget === selected) return
    const op: EditOp = relationType === "subclass"
      ? { op: "add_axiom", type: relationType, sub: selected, super: relationTarget }
      : { op: "add_axiom", type: relationType, a: selected, b: relationTarget }
    if (requestChange([op])) {
      setRelationTarget("")
      setRelationOpen(false)
    }
  }

  const confirmRelationDelete = (type: RelationType, source: string, target: string) => {
    requestChange([type === "subclass"
      ? { op: "delete_axiom", type, sub: source, super: target }
      : { op: "delete_axiom", type, a: source, b: target }])
  }

  const runCommit = async () => {
    const batch = pendingRef.current
    if (!onCommitOperations || !batch?.preview || batch.preview.structural_validation?.committable === false) return
    setBusy("commit")
    try {
      await onCommitOperations(batch.operations, batch.baseRevision, commitReason.trim() || undefined)
      if (pendingRef.current?.requestId !== batch.requestId) return
      replacePending(null)
      setCommitReason("")
      toast.success(zh ? "变更集已提交" : "Change set committed")
    } catch (error) {
      if (isRevisionConflict(error)) {
        replacePending(null)
        setCommitReason("")
        await onRevisionConflict?.()
        toast.warning(zh ? "提交前本体发生变化，本次操作已取消，请基于最新版本重新编辑" : "The ontology changed before commit. This change was cancelled; edit again from the latest version")
      } else {
        if (isUnprocessableChangeSet(error)) {
          replacePending(null)
          setCommitReason("")
        }
        toast.error(zh ? `提交失败：${error instanceof Error ? error.message : String(error)}` : `Commit failed: ${error instanceof Error ? error.message : String(error)}`)
      }
    } finally { setBusy(null) }
  }

  const interactionLocked = busy !== null || pending !== null
  const previewCommittable = pending?.preview?.structural_validation?.committable !== false
  const pendingRevisionChanged = Boolean(pending?.baseRevision && revision && pending.baseRevision !== revision)
  const previewSummary = pending?.preview ? summarizeOntologyPreview(pending.preview) : null

  return (
    <div
      ref={workbenchRef}
      tabIndex={-1}
      className="flex h-[calc(100svh-3.5rem)] min-h-[440px] flex-col outline-none"
      onMouseDownCapture={(event) => {
        if ((event.target as HTMLElement).tagName === "CANVAS") workbenchRef.current?.focus({ preventScroll: true })
      }}
    >
      <div className="flex flex-wrap items-center gap-2 border-b px-3 py-1.5">
        <Segmented
          value={lens}
          onChange={(next) => { setLens(next); onLensChange?.(next) }}
          ariaLabel={zh ? "本体视图" : "Ontology view"}
          options={[
            { v: "graph", label: t("workbench.graph"), icon: <Network className="h-3.5 w-3.5" /> },
            { v: "table", label: t("workbench.table"), icon: <Table2 className="h-3.5 w-3.5" /> },
          ]}
        />
        {lens === "graph" && (
          <>
            <Segmented
              value={graphMode}
              onChange={setGraphMode}
              ariaLabel={zh ? "图谱模式" : "Graph mode"}
              options={[{ v: "explore", label: t("workbench.explore") }, { v: "full", label: t("workbench.fullGraph") }]}
            />
            {graphMode === "explore" && (
              <>
                <span className="ml-1 text-[11px] text-muted-foreground">{t("workbench.levels")}</span>
                <Segmented ariaLabel={zh ? "探索层级" : "Exploration depth"} value={String(depth)} onChange={(value) => setDepth(Number(value))} options={[{ v: "1", label: "1" }, { v: "2", label: "2" }, { v: "3", label: "3" }]} />
              </>
            )}
          </>
        )}
        {canWrite && (
          <div className="flex w-full flex-wrap items-center gap-1.5 sm:ml-auto sm:w-auto">
            <Button size="sm" variant="outline" className="h-7" disabled={interactionLocked} onClick={onAddClass}><Plus className="h-3.5 w-3.5" /> {t("entities.addClass")}</Button>
            <Button size="sm" variant="outline" className="h-7" disabled={interactionLocked} onClick={() => onAddProperty("object")}><Link2 className="h-3.5 w-3.5" /> {zh ? "对象属性" : "Object property"}</Button>
            <Button size="sm" variant="outline" className="h-7" disabled={interactionLocked} onClick={() => onAddProperty("data")}><Plus className="h-3.5 w-3.5" /> {zh ? "数据属性" : "Data property"}</Button>
            <Button size="sm" variant="outline" className="h-7" disabled={!sel || interactionLocked} onClick={() => setRelationOpen(true)}><GitBranchPlus className="h-3.5 w-3.5" /> {zh ? "添加关系" : "Add relation"}</Button>
          </div>
        )}
      </div>

      {lens === "table" ? (
        <ScrollArea className="min-h-0 flex-1">
          <div className="p-4 md:p-6">
            <EntitiesPanel
              view={view}
              canWrite={canWrite && !interactionLocked}
              initialTab={initialTab}
              onTabChange={onTabChange}
              axioms={axioms}
              selectedIri={selected}
              onSelectEntity={explore}
              onAddClass={onAddClass}
              onEditClass={onEditClass}
              onDeleteClass={onDeleteClass}
              onAddProperty={onAddProperty}
              onEditProperty={onEditProperty}
              onDeleteProperty={onDeleteProperty}
              onAddAxiom={onAddAxiom}
            />
          </div>
        </ScrollArea>
      ) : (
        <div className="grid min-h-0 min-w-0 flex-1 grid-cols-1 grid-rows-[minmax(120px,0.4fr)_minmax(220px,1fr)_minmax(160px,0.65fr)] overflow-x-hidden overflow-y-auto md:grid-cols-[220px_minmax(0,1fr)] md:grid-rows-[minmax(300px,1fr)_minmax(190px,0.65fr)] md:overflow-hidden xl:grid-cols-[260px_minmax(320px,1fr)_340px] xl:grid-rows-1">
          <div className="flex min-h-0 min-w-0 flex-col border-b bg-muted/20 md:border-b-0 md:border-r">
            <div className="border-b p-2">
              <div className="relative">
                <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
                <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={zh ? "搜索标签、描述或 IRI…" : "Search label, description, or IRI…"} className="h-8 pl-7 text-sm" />
              </div>
            </div>
            <ScrollArea className="min-h-0 flex-1">
              <div className="p-1.5">
                {query.trim() ? (
                  matches.length === 0 ? (
                    <p className="px-2 py-4 text-xs text-muted-foreground">{t("workbench.noMatches")}</p>
                  ) : matches.map((item) => (
                    <TreeButton key={item.iri} label={item.label} depth={0} active={item.iri === selected} onClick={() => { explore(item.iri); setQuery("") }} />
                  ))
                ) : maps.roots.map((iri) => (
                  <TreeRow key={iri} iri={iri} depth={0} path={[]} maps={maps} expanded={expanded} setExpanded={setExpanded} selected={selected} onSelect={explore} label={label} />
                ))}
              </div>
            </ScrollArea>
            <div className="border-t px-2 py-1.5 text-[11px] text-muted-foreground">
              {t("workbench.classRootCount", { classes: view.classes.length, roots: maps.roots.length, isolated: maps.isolated.length })}
            </div>
          </div>

          <div className="relative min-h-0 min-w-0 bg-muted/40">
            <SigmaOntologyGraph view={view} maps={maps} focus={focus} selected={selected} mode={graphMode} depth={depth} onSelect={select} onExplore={drillIn} />
            {canWrite && !sel && (
              <div className="pointer-events-none absolute inset-x-0 bottom-5 flex justify-center">
                <span className="rounded-full border bg-background/90 px-3 py-1.5 text-xs text-muted-foreground shadow-sm backdrop-blur">
                  {zh ? "选择节点后可在右侧直接建模" : "Select a node to model its relations in the inspector"}
                </span>
              </div>
            )}
          </div>

          <div className="min-h-0 min-w-0 border-t bg-background md:col-span-2 xl:col-span-1 xl:border-l xl:border-t-0">
            <ScrollArea className="h-full">
              {sel ? (
                <div className="space-y-5 px-4 py-4 sm:px-5 xl:px-4">
                  <div>
                    <div className="flex items-start justify-between gap-2">
                      <div className="min-w-0">
                        <h3 className="truncate text-base font-semibold">{sel.label}</h3>
                        <code className="mt-1 block truncate text-[10px] text-muted-foreground" title={sel.iri}>{sel.iri}</code>
                      </div>
                      {canWrite && !interactionLocked && (
                        <div className="flex shrink-0 gap-1">
                          <Button size="icon" variant="ghost" className="h-7 w-7" aria-label={`${t("common.edit")}: ${sel.label}`} title={t("common.edit")} onClick={() => onEditClass(sel)}><Pencil className="h-3.5 w-3.5" /></Button>
                          <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive" aria-label={`${t("common.delete")}: ${sel.label}`} title={t("common.delete")} onClick={() => onDeleteClass(sel)}><Trash2 className="h-3.5 w-3.5" /></Button>
                        </div>
                      )}
                    </div>
                    {sel.comment && <p className="mt-2 text-sm leading-relaxed text-muted-foreground">{sel.comment}</p>}
                    {canWrite && !interactionLocked && (
                      <Button size="sm" variant="outline" className="mt-3 h-7 w-full border-dashed" onClick={() => setRelationOpen(true)}>
                        <GitBranchPlus className="h-3.5 w-3.5" /> {zh ? "为此类添加关系或公理" : "Add relation or axiom"}
                      </Button>
                    )}
                  </div>

                  <Relations title={t("workbench.superclasses")} iris={maps.parentsOf.get(sel.iri) ?? []} label={label} onSelect={explore} icon={<Layers className="h-3.5 w-3.5" />} canWrite={canWrite && !interactionLocked} onRemove={(iri) => confirmRelationDelete("subclass", sel.iri, iri)} />
                  <Relations title={t("workbench.subclasses")} iris={maps.childrenOf.get(sel.iri) ?? []} label={label} onSelect={explore} icon={<Layers className="h-3.5 w-3.5" />} canWrite={canWrite && !interactionLocked} onRemove={(iri) => confirmRelationDelete("subclass", iri, sel.iri)} />
                  <PropertySection title={t("workbench.objectPropertiesOut")} rows={(maps.objByDomain.get(sel.iri) ?? []).map((property) => ({ property, kind: "object" as const, values: propertyValues(property, "range", label) }))} onSelect={explore} canWrite={canWrite && !interactionLocked} onEdit={onEditProperty} onDelete={onDeleteProperty} />
                  <PropertySection title={t("workbench.objectPropertiesIn")} rows={(maps.objByRange.get(sel.iri) ?? []).map((property) => ({ property, kind: "object" as const, values: propertyValues(property, "domain", label) }))} onSelect={explore} canWrite={canWrite && !interactionLocked} onEdit={onEditProperty} onDelete={onDeleteProperty} />
                  <PropertySection title={t("workbench.dataProperties")} rows={(maps.dataByDomain.get(sel.iri) ?? []).map((property) => ({ property, kind: "data" as const, values: propertyValues(property, "range", label, false) }))} onSelect={explore} canWrite={canWrite && !interactionLocked} onEdit={onEditProperty} onDelete={onDeleteProperty} />
                  <Relations title={t("workbench.disjoint")} iris={maps.disjointOf.get(sel.iri) ?? []} label={label} onSelect={explore} icon={<Link2 className="h-3.5 w-3.5" />} canWrite={canWrite && !interactionLocked} onRemove={(iri) => confirmRelationDelete("disjoint", sel.iri, iri)} />
                  <Relations title={t("workbench.equivalent")} iris={maps.equivOf.get(sel.iri) ?? []} label={label} onSelect={explore} icon={<Link2 className="h-3.5 w-3.5" />} canWrite={canWrite && !interactionLocked} onRemove={(iri) => confirmRelationDelete("equivalent", sel.iri, iri)} />
                </div>
              ) : <div className="p-4 text-sm text-muted-foreground">{t("workbench.nothingSelected")}</div>}
            </ScrollArea>
          </div>
        </div>
      )}

      <RelationDialog open={relationOpen} onOpenChange={setRelationOpen} source={sel ?? null} classes={view.classes} type={relationType} onTypeChange={setRelationType} target={relationTarget} onTargetChange={setRelationTarget} onConfirm={confirmRelation} zh={zh} />

      <Dialog
        open={Boolean(pending)}
        onOpenChange={(open) => {
          if (!open && busy !== "commit") discardPending()
        }}
      >
        <DialogContent className="max-h-[92svh] gap-0 overflow-hidden p-0 sm:max-w-3xl">
          <DialogHeader className="gap-2 border-b px-5 py-4 pr-12">
            <DialogTitle className="text-lg">{zh ? "审阅变更集" : "Review change set"}</DialogTitle>
            <DialogDescription className="text-xs">
              {zh ? "逐项核对操作与影响；确认后将作为一个原子变更集写入。" : "Verify the operations and impact. Once confirmed, they are written as one atomic change set."}
            </DialogDescription>
            {pending && previewSummary && (
              <p className="text-[11px] text-muted-foreground">
                <span className={`font-medium ${previewSummary.committable ? "text-foreground" : "text-amber-700 dark:text-amber-300"}`}>
                  {previewSummary.committable ? (zh ? "结构校验通过" : "Structure validation passed") : (zh ? "结构校验未通过" : "Structure validation blocked")}
                </span>
                {(previewSummary.errors > 0 || previewSummary.warnings > 0) && (
                  <span> · {previewSummary.errors} {zh ? "个错误" : "errors"} · {previewSummary.warnings} {zh ? "个警告" : "warnings"}</span>
                )}
              </p>
            )}
          </DialogHeader>

          {pendingRevisionChanged && (
            <div className="flex items-center gap-2 border-b px-5 py-2 text-[11px] font-medium text-amber-700 dark:text-amber-300">
              <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
              <span>{zh ? "本体版本已更新，本次预览已失效。取消后可基于最新版本重新编辑。" : "The ontology has changed and this preview is stale. Cancel, then edit again from the latest revision."}</span>
            </div>
          )}

          {pending && (
            <section className="min-h-0 max-h-[min(58svh,34rem)] overflow-y-auto">
              {busy === "preview" && !pending.preview && (
                <div className="flex items-center gap-2 border-b bg-primary/[0.035] px-5 py-3 text-xs text-muted-foreground">
                  <Loader2 className="h-4 w-4 shrink-0 animate-spin text-primary" />
                  <span>{zh ? "正在校验变更影响与本体结构…" : "Validating change impact and ontology structure…"}</span>
                </div>
              )}

              {pending.preview?.structural_validation?.committable === false && (
                <div className="border-b bg-amber-500/5 px-5 py-3 text-xs text-amber-800 dark:text-amber-200">
                  <div className="flex items-center gap-2 font-medium">
                    <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
                    <span>{zh ? "预检发现新的结构错误，当前变更集不能提交。" : "Preflight found new structural errors, so this change set cannot be committed."}</span>
                  </div>
                  {(pending.preview.structural_validation.new_error_signatures?.length ?? 0) > 0 && (
                    <ul className="mt-2 list-inside list-disc space-y-1 pl-5 text-[10px]">
                      {pending.preview.structural_validation.new_error_signatures.slice(0, 6).map((signature) => (
                        <li key={signature} className="break-all">{signature}</li>
                      ))}
                    </ul>
                  )}
                </div>
              )}

              <ol className="divide-y">
                {pending.operations.map((operation, index) => {
                  const reviewItem = pending.reviewItems.find((item) => item.operation_index === index)
                  const previewImpacts = pending.preview?.impact?.operations ?? []
                  const indexedImpacts = previewImpacts.filter((item) => item.index === index)
                  const impacts = indexedImpacts.length > 0
                    ? indexedImpacts
                    : previewImpacts[index] ? [previewImpacts[index]] : []
                  const decisionFallback = operationLabel(operation, label, zh)
                  const itemLabel = pending.labels[index]
                  const content = reviewItem?.content?.trim()
                    || (itemLabel !== decisionFallback ? itemLabel : operationContent(operation, label, zh))
                  const information = reviewItem?.information?.trim()
                    || operationInformation(operation, impacts, label, zh)
                  const decision = reviewItem?.decision?.trim() || decisionFallback
                  return (
                    <li key={`${pending.requestId}-${index}`} className="px-5 py-4">
                      <div className="mb-3 flex flex-wrap items-center gap-1.5 text-[10px] text-muted-foreground">
                        {reviewItem && (
                          <>
                            <span>{queueLabel(reviewItem.queue, zh)} #{reviewItem.item_id}</span>
                            <span aria-hidden="true">·</span>
                          </>
                        )}
                        <span title={operation.op}>{operationTypeLabel(operation, zh)}</span>
                      </div>
                      <dl className="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-4 gap-y-2.5 text-xs leading-5">
                        <dt className="text-muted-foreground">{zh ? "待审批内容" : "Review item"}</dt>
                        <dd className="min-w-0 break-words text-sm font-medium text-foreground">{content}</dd>
                        <dt className="text-muted-foreground">{zh ? "信息" : "Information"}</dt>
                        <dd className="min-w-0 break-words text-muted-foreground">{information}</dd>
                        <dt className="text-muted-foreground">{zh ? "审批决定" : "Decision"}</dt>
                        <dd className="min-w-0 break-words font-medium text-foreground">{decision}</dd>
                      </dl>
                    </li>
                  )
                })}
              </ol>
            </section>
          )}

          <div className="border-t bg-background px-4 py-3">
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
              {pending?.preview && (
                <div className="flex min-w-0 flex-1 items-center gap-2">
                  <Label htmlFor="change-reason" className="shrink-0 text-xs text-muted-foreground">{zh ? "变更说明" : "Reason"}</Label>
                  <Input id="change-reason" value={commitReason} onChange={(event) => setCommitReason(event.target.value)} placeholder={zh ? "可选，用于审核与追溯" : "Optional audit note"} className="h-8 min-w-0" />
                </div>
              )}
              <div className="flex shrink-0 justify-end gap-2">
                <Button variant="outline" onClick={discardPending} disabled={busy === "commit"}>{t("common.cancel")}</Button>
                {pending?.preview && (
                  <Button
                    disabled={!canWrite || !onCommitOperations || busy !== null || !previewCommittable || pendingRevisionChanged}
                    onClick={runCommit}
                  >
                    {busy === "commit" && <Loader2 className="h-4 w-4 animate-spin" />}
                    {zh ? `提交 ${pending.operations.length} 项修改` : `Commit ${pending.operations.length} change${pending.operations.length === 1 ? "" : "s"}`}
                  </Button>
                )}
              </div>
            </div>
          </div>
        </DialogContent>
      </Dialog>

    </div>
  )
}

function RelationDialog({ open, onOpenChange, source, classes, type, onTypeChange, target, onTargetChange, onConfirm, zh }: {
  open: boolean
  onOpenChange: (open: boolean) => void
  source: OntologyClass | null
  classes: OntologyClass[]
  type: RelationType
  onTypeChange: (type: RelationType) => void
  target: string
  onTargetChange: (iri: string) => void
  onConfirm: () => void
  zh: boolean
}) {
  const { t } = useI18n()
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{zh ? "添加关系" : "Add relation"}</DialogTitle>
          <DialogDescription>{zh ? "下一步将预览精确影响；确认提交前不会写入本体。" : "Next, review the exact impact. Nothing is written until you confirm the commit."}</DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <Label>{zh ? "起点" : "Source"}</Label>
            <div className="rounded-md border bg-muted/30 px-3 py-2 text-sm font-medium">{source?.label ?? "—"}</div>
          </div>
          <div className="space-y-2">
            <Label>{zh ? "关系类型" : "Relation type"}</Label>
            <Select value={type} onValueChange={(value) => onTypeChange(value as RelationType)}>
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="subclass">{zh ? "是目标类的子类" : "is a subclass of"}</SelectItem>
                <SelectItem value="disjoint">{zh ? "与目标类互斥" : "is disjoint with"}</SelectItem>
                <SelectItem value="equivalent">{zh ? "与目标类等价" : "is equivalent to"}</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-2">
            <Label>{zh ? "目标类" : "Target class"}</Label>
            <Combobox value={target} onChange={onTargetChange} placeholder={zh ? "选择目标类" : "Select target class"} searchPlaceholder={t("edit.searchClasses")} options={classes.filter((item) => item.iri !== source?.iri).map((item) => ({ value: item.iri, label: item.label, hint: item.local }))} />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
          <Button disabled={!source || !target || source.iri === target} onClick={onConfirm}><Plus className="h-4 w-4" /> {zh ? "预览影响" : "Preview impact"}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function Segmented<T extends string>({ value, onChange, options, ariaLabel }: { value: T; onChange: (value: T) => void; options: { v: T; label: string; icon?: ReactNode }[]; ariaLabel: string }) {
  const moveFocus = (event: ReactKeyboardEvent<HTMLButtonElement>, index: number) => {
    let next = index
    if (event.key === "ArrowRight" || event.key === "ArrowDown") next = (index + 1) % options.length
    else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = (index - 1 + options.length) % options.length
    else if (event.key === "Home") next = 0
    else if (event.key === "End") next = options.length - 1
    else return
    event.preventDefault()
    const option = options[next]
    onChange(option.v)
    const tabs = event.currentTarget.parentElement?.querySelectorAll<HTMLElement>("[role=tab]")
    tabs?.[next]?.focus()
  }
  return (
    <div role="tablist" aria-label={ariaLabel} className="inline-flex rounded-md border bg-background p-0.5">
      {options.map((option, index) => (
        <button key={option.v} type="button" role="tab" aria-selected={value === option.v} tabIndex={value === option.v ? 0 : -1} onKeyDown={(event) => moveFocus(event, index)} onClick={() => onChange(option.v)} className={`inline-flex items-center gap-1 rounded px-2.5 py-1 text-xs font-medium ${value === option.v ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:text-foreground"}`}>
          {option.icon}{option.label}
        </button>
      ))}
    </div>
  )
}

function TreeRow({ iri, depth, path, maps, expanded, setExpanded, selected, onSelect, label }: {
  iri: string
  depth: number
  path: string[]
  maps: Maps
  expanded: Set<string>
  setExpanded: Dispatch<SetStateAction<Set<string>>>
  selected: string | null
  onSelect: (iri: string) => void
  label: (iri: string) => string
}) {
  const children = maps.childrenOf.get(iri) ?? []
  const hasChildren = children.length > 0
  const isExpanded = expanded.has(iri)
  const cyclic = path.includes(iri)
  const toggle = (event: MouseEvent) => {
    event.stopPropagation()
    setExpanded((current) => {
      const next = new Set(current)
      if (next.has(iri)) next.delete(iri); else next.add(iri)
      return next
    })
  }
  return (
    <>
      <TreeButton label={label(iri)} depth={depth} active={iri === selected} onClick={() => onSelect(iri)} chevron={hasChildren ? (isExpanded ? "down" : "right") : "none"} onChevron={toggle} count={hasChildren ? children.length : undefined} />
      {hasChildren && isExpanded && !cyclic && children.map((child) => (
        <TreeRow key={`${iri}/${child}`} iri={child} depth={depth + 1} path={[...path, iri]} maps={maps} expanded={expanded} setExpanded={setExpanded} selected={selected} onSelect={onSelect} label={label} />
      ))}
    </>
  )
}

function TreeButton({ label, depth, active, onClick, chevron = "none", onChevron, count }: {
  label: string
  depth: number
  active: boolean
  onClick: () => void
  chevron?: "down" | "right" | "none"
  onChevron?: (event: MouseEvent) => void
  count?: number
}) {
  const { t } = useI18n()
  return (
    <div style={{ paddingLeft: 6 + depth * 14 }} className={`flex items-center gap-1 rounded text-sm ${active ? "bg-accent font-medium text-accent-foreground" : "hover:bg-muted"}`}>
      {chevron === "none" ? <span className="w-4 shrink-0" /> : (
        <button type="button" onClick={onChevron} className="shrink-0 opacity-70 hover:opacity-100" aria-label={t(chevron === "down" ? "workbench.collapseClass" : "workbench.expandClass", { name: label })}>
          {chevron === "down" ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
        </button>
      )}
      <button type="button" onClick={onClick} className="flex flex-1 items-center gap-1 overflow-hidden py-1 pr-1 text-left">
        <span className="truncate">{label}</span>
        {count !== undefined && <span className="ml-auto shrink-0 text-[10px] text-muted-foreground">{count}</span>}
      </button>
    </div>
  )
}

function Relations({ title, iris, label, onSelect, icon, canWrite, onRemove }: {
  title: string
  iris: string[]
  label: (iri: string) => string
  onSelect: (iri: string) => void
  icon: ReactNode
  canWrite: boolean
  onRemove: (iri: string) => void
}) {
  const { locale } = useI18n()
  if (iris.length === 0) return null
  return (
    <section>
      <div className="mb-1.5 flex items-center gap-1.5 text-xs font-semibold text-muted-foreground">{icon} {title} ({iris.length})</div>
      <div className="flex flex-wrap gap-1.5">
        {iris.map((iri) => (
          <span key={iri} className="group inline-flex max-w-full items-stretch rounded border bg-background text-xs hover:border-primary">
            <button type="button" onClick={() => onSelect(iri)} className="min-w-0 break-words px-2 py-1 text-left hover:text-primary">{label(iri)}</button>
            {canWrite && (
              <button
                type="button"
                onClick={() => onRemove(iri)}
                className="border-l px-1 py-0.5 text-muted-foreground opacity-0 hover:text-destructive group-hover:opacity-100 focus:opacity-100"
                aria-label={locale === "zh-CN" ? `预览移除 ${label(iri)} 的影响` : `Preview removing ${label(iri)}`}
                title={locale === "zh-CN" ? "预览移除影响" : "Preview removal impact"}
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </span>
        ))}
      </div>
    </section>
  )
}

function propertyValues(property: OntologyProperty, slot: "domain" | "range", label: (iri: string) => string, link = true) {
  const members = property[`${slot}_members`]
  const iris = members?.length ? members : property[slot] ? [property[slot]!] : []
  if (iris.length) return iris.map((iri) => ({ label: label(iri), iri: link ? iri : null }))
  const fallback = slot === "domain" ? property.domain_label : property.range_label?.replace(/^xsd:/, "")
  return fallback ? [{ label: fallback, iri: null }] : []
}

function PropertySection({ title, rows, onSelect, canWrite, onEdit, onDelete }: {
  title: string
  rows: { property: OntologyProperty; kind: "object" | "data"; values: { label: string; iri: string | null }[] }[]
  onSelect: (iri: string) => void
  canWrite: boolean
  onEdit: (property: OntologyProperty, kind: "object" | "data") => void
  onDelete: (property: OntologyProperty) => void
}) {
  const { t } = useI18n()
  if (rows.length === 0) return null
  return (
    <div>
      <div className="mb-1.5 text-xs font-semibold text-muted-foreground">{title} ({rows.length})</div>
      <div className="space-y-1">
        {rows.map((row) => (
          <div key={`${row.kind}-${row.property.iri}`} className="group relative flex min-w-0 flex-wrap items-baseline gap-x-1.5 gap-y-0.5 py-0.5 pr-10 text-xs leading-5">
            <span className="break-words font-medium">{row.property.label}</span>
            {row.values.length > 0 && (
              <>
                <span aria-hidden="true" className="text-muted-foreground">→</span>
                <span className="flex min-w-0 flex-wrap items-baseline gap-x-1.5 text-muted-foreground">
                  {row.values.map((value, index) => (
                    <span key={`${value.iri ?? value.label}-${index}`} className="inline-flex min-w-0 items-baseline gap-1">
                      {index > 0 && <span aria-hidden="true">∪</span>}
                      {value.iri
                        ? <button type="button" onClick={() => onSelect(value.iri!)} className="break-words text-left text-primary hover:underline" title={value.label}>{value.label}</button>
                        : <span className="break-words" title={value.label}>{value.label}</span>}
                    </span>
                  ))}
                </span>
              </>
            )}
            {canWrite && (
              <span className="absolute right-0 top-0 flex opacity-50 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
                <button type="button" className="p-1 text-muted-foreground hover:text-foreground" aria-label={`${t("common.edit")}: ${row.property.label}`} title={t("common.edit")} onClick={() => onEdit(row.property, row.kind)}><Pencil className="h-3 w-3" /></button>
                <button type="button" className="p-1 text-muted-foreground hover:text-destructive" aria-label={`${t("common.delete")}: ${row.property.label}`} title={t("common.delete")} onClick={() => onDelete(row.property)}><Trash2 className="h-3 w-3" /></button>
              </span>
            )}
          </div>
        ))}
      </div>
    </div>
  )
}
