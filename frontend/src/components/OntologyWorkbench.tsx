import type { Dispatch, MouseEvent, ReactNode, SetStateAction } from "react"
import { useCallback, useEffect, useMemo, useState } from "react"
import { ChevronDown, ChevronRight, Layers, Link2, Network, Pencil, Search, Table2, Trash2 } from "lucide-react"
import type { OntologyClass, OntologyProperty, OntologyView } from "@/lib/types"
import { useI18n } from "@/lib/i18n"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import EntitiesPanel, { type AxiomGroup } from "@/components/EntitiesPanel"
import SigmaOntologyGraph from "@/components/SigmaOntologyGraph"

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

function push<T>(m: Map<string, T[]>, k: string, v: T) {
  if (!m.has(k)) m.set(k, [])
  m.get(k)!.push(v)
}
function push2(m: Map<string, string[]>, k: string, v: string) {
  if (!m.has(k)) m.set(k, [])
  if (!m.get(k)!.includes(v)) m.get(k)!.push(v)
}

function buildMaps(view: OntologyView): Maps {
  const byIri = new Map(view.classes.map((c) => [c.iri, c]))
  const parentsOf = new Map<string, string[]>()
  const childrenOf = new Map<string, string[]>()
  for (const c of view.classes) {
    const parents = c.superclasses.filter((p) => byIri.has(p))
    parentsOf.set(c.iri, parents)
    for (const p of parents) push(childrenOf, p, c.iri)
  }
  const labelOf = (iri: string) => byIri.get(iri)?.label ?? iri
  for (const arr of childrenOf.values()) arr.sort((a, b) => labelOf(a).localeCompare(labelOf(b)))
  const roots = view.classes
    .filter((c) => (parentsOf.get(c.iri) ?? []).length === 0)
    .map((c) => c.iri)
    .sort((a, b) => labelOf(a).localeCompare(labelOf(b)))

  const dataByDomain = new Map<string, OntologyView["data_properties"]>()
  for (const p of view.data_properties) {
    for (const domain of p.domain_members?.length ? p.domain_members : p.domain ? [p.domain] : []) {
      push(dataByDomain, domain, p)
    }
  }
  const objByDomain = new Map<string, OntologyView["object_properties"]>()
  const objByRange = new Map<string, OntologyView["object_properties"]>()
  for (const p of view.object_properties) {
    for (const domain of p.domain_members?.length ? p.domain_members : p.domain ? [p.domain] : []) {
      push(objByDomain, domain, p)
    }
    for (const range of p.range_members?.length ? p.range_members : p.range ? [p.range] : []) {
      push(objByRange, range, p)
    }
  }
  const propertyLinked = new Set<string>()
  for (const p of view.data_properties) {
    for (const iri of p.domain_members ?? (p.domain ? [p.domain] : [])) propertyLinked.add(iri)
  }
  for (const p of view.object_properties) {
    for (const iri of p.domain_members ?? (p.domain ? [p.domain] : [])) propertyLinked.add(iri)
    for (const iri of p.range_members ?? (p.range ? [p.range] : [])) propertyLinked.add(iri)
  }
  const isolated = view.classes
    .filter((c) => (
      (parentsOf.get(c.iri) ?? []).length === 0
      && (childrenOf.get(c.iri) ?? []).length === 0
      && !propertyLinked.has(c.iri)
    ))
    .map((c) => c.iri)
  const disjointOf = new Map<string, string[]>()
  for (const r of view.axioms.disjoint_with) { push2(disjointOf, r.a, r.b); push2(disjointOf, r.b, r.a) }
  const equivOf = new Map<string, string[]>()
  for (const r of view.axioms.equivalent_class) { push2(equivOf, r.a, r.b); push2(equivOf, r.b, r.a) }

  return { byIri, parentsOf, childrenOf, roots, isolated, dataByDomain, objByDomain, objByRange, disjointOf, equivOf }
}

// --------------------------------------------------------------------------- //
type Lens = "graph" | "table"
type GraphMode = "explore" | "full"

/**
 * The single Ontology workbench: one selection viewed through two lenses. The Graph lens is a
 * hierarchy tree + a focused relation graph or packed hierarchy overview + a detail inspector;
 * the Table lens is the flat classes / properties / axioms browser.
 * Selecting a class in either lens carries to the other, so there's one place to explore the ontology.
 */
export default function OntologyWorkbench({
  view, canWrite, initialLens = "graph", initialTab = "classes", axioms,
  onAddClass, onEditClass, onDeleteClass, onAddProperty, onEditProperty, onDeleteProperty, onAddAxiom,
}: {
  view: OntologyView
  canWrite: boolean
  initialLens?: Lens
  initialTab?: "classes" | "object" | "data" | "axioms"
  axioms: AxiomGroup[]
  onAddClass: () => void
  onEditClass: (c: OntologyClass) => void
  onDeleteClass: (c: OntologyClass) => void
  onAddProperty: () => void
  onEditProperty: (p: OntologyProperty, kind: "object" | "data") => void
  onDeleteProperty: (p: OntologyProperty) => void
  onAddAxiom: () => void
}) {
  const { t } = useI18n()
  const maps = useMemo(() => buildMaps(view), [view])
  const [lens, setLens] = useState<Lens>(initialLens)
  const [graphMode, setGraphMode] = useState<GraphMode>("explore")
  const [depth, setDepth] = useState(1)
  // `focus` = what the graph is centred on (double-click / tree / chip); `selected` = what's
  // highlighted + shown in the inspector (single-click a node). They're usually the same, but a
  // single-click moves only the highlight so the view doesn't jump.
  const [focus, setFocus] = useState<string | null>(null)
  const [selected, setSelected] = useState<string | null>(null)
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [query, setQuery] = useState("")

  const label = (iri: string) => maps.byIri.get(iri)?.label ?? iri
  const explore = useCallback((iri: string) => { setFocus(iri); setSelected(iri) }, [])
  const select = useCallback((iri: string) => setSelected(iri), [])
  // Double-click a node = "go in": centre on it AND switch to Explore, so double-clicking in the
  // Full Graph overview drills straight into that class's neighbourhood.
  const drillIn = useCallback((iri: string) => { setFocus(iri); setSelected(iri); setGraphMode("explore") }, [])

  // Default focus = first root; keep valid as the ontology changes.
  useEffect(() => {
    if ((!focus || !maps.byIri.has(focus)) && maps.roots.length) { setFocus(maps.roots[0]); setSelected(maps.roots[0]) }
  }, [maps, focus])

  // Reveal the selected class in the tree by expanding its ancestors.
  useEffect(() => {
    if (!selected) return
    setExpanded((prev) => {
      const next = new Set(prev)
      const stack = [...(maps.parentsOf.get(selected) ?? [])]
      const seen = new Set<string>()
      while (stack.length) {
        const p = stack.pop()!
        if (seen.has(p)) continue
        seen.add(p); next.add(p)
        stack.push(...(maps.parentsOf.get(p) ?? []))
      }
      return next
    })
  }, [selected, maps])

  const sel = selected ? maps.byIri.get(selected) : null
  const matches = query.trim()
    ? view.classes.filter((c) => c.label.toLowerCase().includes(query.trim().toLowerCase()))
    : []

  return (
    <div className="flex h-[calc(100svh-3.5rem)] min-h-[440px] flex-col">
      {/* Toolbar: lens switch + (graph-only) Explore / Full Graph */}
      <div className="flex flex-wrap items-center gap-2 border-b px-3 py-1.5">
        <Segmented
          value={lens} onChange={setLens}
          options={[{ v: "graph", label: t("workbench.graph"), icon: <Network className="h-3.5 w-3.5" /> },
                    { v: "table", label: t("workbench.table"), icon: <Table2 className="h-3.5 w-3.5" /> }]}
        />
        {lens === "graph" && (
          <>
            <Segmented
              value={graphMode} onChange={setGraphMode}
              options={[{ v: "explore", label: t("workbench.explore") }, { v: "full", label: t("workbench.fullGraph") }]}
            />
            {graphMode === "explore" && (
              <>
                <span className="ml-1 text-[11px] text-muted-foreground">{t("workbench.levels")}</span>
                <Segmented
                  value={String(depth)} onChange={(v) => setDepth(Number(v))}
                  options={[{ v: "1", label: "1" }, { v: "2", label: "2" }, { v: "3", label: "3" }]}
                />
              </>
            )}
          </>
        )}
      </div>

      {lens === "table" ? (
        <ScrollArea className="min-h-0 flex-1">
          <div className="p-4 md:p-6">
            <EntitiesPanel
              view={view} canWrite={canWrite} initialTab={initialTab} axioms={axioms}
              selectedIri={selected} onSelectEntity={explore}
              onAddClass={onAddClass} onEditClass={onEditClass} onDeleteClass={onDeleteClass}
              onAddProperty={onAddProperty} onEditProperty={onEditProperty} onDeleteProperty={onDeleteProperty}
              onAddAxiom={onAddAxiom}
            />
          </div>
        </ScrollArea>
      ) : (
        <div className="grid min-h-0 flex-1 grid-cols-[260px_1fr_320px]">
          {/* LEFT: hierarchy tree + search */}
          <div className="flex min-h-0 flex-col border-r bg-muted/20">
            <div className="border-b p-2">
              <div className="relative">
                <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
                <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("workbench.searchClasses")} className="h-8 pl-7 text-sm" />
              </div>
            </div>
            <ScrollArea className="min-h-0 flex-1">
              <div className="p-1.5">
                {query.trim() ? (
                  matches.length === 0 ? (
                    <p className="px-2 py-4 text-xs text-muted-foreground">{t("workbench.noMatches")}</p>
                  ) : matches.map((c) => (
                    <TreeButton key={c.iri} label={c.label} depth={0} active={c.iri === selected}
                      onClick={() => { explore(c.iri); setQuery("") }} />
                  ))
                ) : (
                  maps.roots.map((iri) => (
                    <TreeRow key={iri} iri={iri} depth={0} path={[]} maps={maps} expanded={expanded}
                      setExpanded={setExpanded} selected={selected} onSelect={explore} label={label} />
                  ))
                )}
              </div>
            </ScrollArea>
            <div className="border-t px-2 py-1.5 text-[11px] text-muted-foreground">
              {t("workbench.classRootCount", {
                classes: view.classes.length,
                roots: maps.roots.length,
                isolated: maps.isolated.length,
              })}
            </div>
          </div>

          {/* CENTER: interactive relation graph / packed hierarchy overview */}
          <div className="relative min-h-0 bg-muted/40">
            <SigmaOntologyGraph
              view={view} maps={maps} focus={focus} selected={selected}
              mode={graphMode} depth={depth} onSelect={select} onExplore={drillIn}
            />
          </div>

          {/* RIGHT: inspector */}
          <div className="min-h-0 border-l">
            <ScrollArea className="h-full">
              {sel ? (
                <div className="space-y-4 p-4">
                  <div>
                    <div className="flex items-start justify-between gap-2">
                      <h3 className="text-base font-semibold">{sel.label}</h3>
                      {canWrite && (
                        <div className="flex gap-1">
                          <Button size="icon" variant="ghost" className="h-7 w-7" title={t("common.edit")} onClick={() => onEditClass(sel)}>
                            <Pencil className="h-3.5 w-3.5" />
                          </Button>
                          <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive" title={t("common.delete")}
                            onClick={() => onDeleteClass(sel)}>
                            <Trash2 className="h-3.5 w-3.5" />
                          </Button>
                        </div>
                      )}
                    </div>
                    {sel.comment && <p className="mt-1 text-sm text-muted-foreground">{sel.comment}</p>}
                  </div>

                  <Relations title={t("workbench.superclasses")} iris={maps.parentsOf.get(sel.iri) ?? []} label={label} onSelect={explore} icon={<Layers className="h-3.5 w-3.5" />} />
                  <Relations title={t("workbench.subclasses")} iris={maps.childrenOf.get(sel.iri) ?? []} label={label} onSelect={explore} icon={<Layers className="h-3.5 w-3.5" />} />
                  <PropSection title={t("workbench.objectPropertiesOut")} rows={(maps.objByDomain.get(sel.iri) ?? []).map((p) => ({ k: p.label, v: p.range_label, iri: p.range }))} onSelect={explore} />
                  <PropSection title={t("workbench.objectPropertiesIn")} rows={(maps.objByRange.get(sel.iri) ?? []).map((p) => ({ k: `${p.domain_label ?? "?"} · ${p.label}`, v: sel.label, iri: p.domain }))} onSelect={explore} />
                  <PropSection title={t("workbench.dataProperties")} rows={(maps.dataByDomain.get(sel.iri) ?? []).map((p) => ({ k: p.label, v: p.range_label ? p.range_label.replace(/^xsd:/, "") : null, iri: null }))} onSelect={explore} />
                  <Relations title={t("workbench.disjoint")} iris={maps.disjointOf.get(sel.iri) ?? []} label={label} onSelect={explore} icon={<Link2 className="h-3.5 w-3.5" />} />
                  <Relations title={t("workbench.equivalent")} iris={maps.equivOf.get(sel.iri) ?? []} label={label} onSelect={explore} icon={<Link2 className="h-3.5 w-3.5" />} />
                </div>
              ) : (
                <div className="p-4 text-sm text-muted-foreground">{t("workbench.nothingSelected")}</div>
              )}
            </ScrollArea>
          </div>
        </div>
      )}
    </div>
  )
}

// --------------------------------------------------------------------------- //
function Segmented<T extends string>({
  value, onChange, options,
}: {
  value: T
  onChange: (v: T) => void
  options: { v: T; label: string; icon?: ReactNode }[]
}) {
  return (
    <div className="inline-flex rounded-md border bg-background p-0.5">
      {options.map((o) => (
        <button
          key={o.v}
          onClick={() => onChange(o.v)}
          className={`inline-flex items-center gap-1 rounded px-2.5 py-1 text-xs font-medium ${
            value === o.v ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:text-foreground"
          }`}
        >
          {o.icon}{o.label}
        </button>
      ))}
    </div>
  )
}

function TreeRow({
  iri, depth, path, maps, expanded, setExpanded, selected, onSelect, label,
}: {
  iri: string; depth: number; path: string[]; maps: Maps
  expanded: Set<string>; setExpanded: Dispatch<SetStateAction<Set<string>>>
  selected: string | null; onSelect: (iri: string) => void; label: (iri: string) => string
}) {
  const kids = maps.childrenOf.get(iri) ?? []
  const hasKids = kids.length > 0
  const isExp = expanded.has(iri)
  const cyclic = path.includes(iri)
  const toggle = (e: MouseEvent) => {
    e.stopPropagation()
    setExpanded((current) => {
      const next = new Set(current)
      if (next.has(iri)) next.delete(iri)
      else next.add(iri)
      return next
    })
  }
  return (
    <>
      <TreeButton label={label(iri)} depth={depth} active={iri === selected} onClick={() => onSelect(iri)}
        chevron={hasKids ? (isExp ? "down" : "right") : "none"} onChevron={toggle} count={hasKids ? kids.length : undefined} />
      {hasKids && isExp && !cyclic && kids.map((k) => (
        <TreeRow key={`${iri}/${k}`} iri={k} depth={depth + 1} path={[...path, iri]} maps={maps}
          expanded={expanded} setExpanded={setExpanded} selected={selected} onSelect={onSelect} label={label} />
      ))}
    </>
  )
}

function TreeButton({
  label, depth, active, onClick, chevron = "none", onChevron, count,
}: {
  label: string; depth: number; active: boolean; onClick: () => void
  chevron?: "down" | "right" | "none"; onChevron?: (e: MouseEvent) => void; count?: number
}) {
  return (
    <div style={{ paddingLeft: 6 + depth * 14 }}
      className={`flex items-center gap-1 rounded text-sm ${active ? "bg-accent font-medium text-accent-foreground" : "hover:bg-muted"}`}>
      {chevron === "none" ? <span className="w-4 shrink-0" /> : (
        <button onClick={onChevron} className="shrink-0 opacity-70 hover:opacity-100">
          {chevron === "down" ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
        </button>
      )}
      <button onClick={onClick} className="flex flex-1 items-center gap-1 overflow-hidden py-1 pr-1 text-left">
        <span className="truncate">{label}</span>
        {count !== undefined && <span className="ml-auto shrink-0 text-[10px] text-muted-foreground">{count}</span>}
      </button>
    </div>
  )
}

function Relations({
  title, iris, label, onSelect, icon,
}: {
  title: string; iris: string[]; label: (iri: string) => string; onSelect: (iri: string) => void; icon: ReactNode
}) {
  if (iris.length === 0) return null
  return (
    <div>
      <div className="mb-1.5 flex items-center gap-1.5 text-xs font-semibold text-muted-foreground">{icon} {title} ({iris.length})</div>
      <div className="flex flex-wrap gap-1.5">
        {iris.map((iri) => (
          <button key={iri} onClick={() => onSelect(iri)}
            className="rounded border bg-background px-2 py-0.5 text-xs hover:border-primary hover:text-primary">
            {label(iri)}
          </button>
        ))}
      </div>
    </div>
  )
}

function PropSection({
  title, rows, onSelect,
}: {
  title: string; rows: { k: string; v: string | null; iri: string | null }[]; onSelect: (iri: string) => void
}) {
  if (rows.length === 0) return null
  return (
    <div>
      <div className="mb-1.5 text-xs font-semibold text-muted-foreground">{title} ({rows.length})</div>
      <div className="space-y-1">
        {rows.map((r, i) => (
          <div key={i} className="flex items-center gap-1 text-xs">
            <span className="font-medium">{r.k}</span>
            {r.v && <span className="text-muted-foreground">→</span>}
            {r.v && (r.iri ? (
              <button onClick={() => onSelect(r.iri!)} className="text-primary hover:underline">{r.v}</button>
            ) : <span className="text-muted-foreground">{r.v}</span>)}
          </div>
        ))}
      </div>
    </div>
  )
}
