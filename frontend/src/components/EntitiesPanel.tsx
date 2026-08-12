import { useEffect, useMemo, useState } from "react"
import { ChevronLeft, ChevronRight, Pencil, Plus, Search, Trash2 } from "lucide-react"
import type { OntologyClass, OntologyProperty, OntologyView } from "@/lib/types"
import { useI18n } from "@/lib/i18n"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { AxiomTeX, type AxOp } from "@/components/tex"

const PAGE_SIZE = 12
type Tab = "classes" | "object" | "data" | "axioms"

/** `text` is the plain-unicode form (used for search); `parts` drives the KaTeX-rendered display. */
export type AxiomGroup = {
  type: string
  title: string
  items: { text: string; parts?: { left: string; op: AxOp; right: string }; onDelete?: () => void }[]
}

function RowActions({ onEdit, onDelete }: { onEdit: () => void; onDelete: () => void }) {
  const { t } = useI18n()
  return (
    <div className="flex justify-end gap-1">
      <Button size="icon" variant="ghost" className="h-7 w-7" title={t("common.edit")} onClick={onEdit}>
        <Pencil className="h-3.5 w-3.5" />
      </Button>
      <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive" title={t("common.delete")} onClick={onDelete}>
        <Trash2 className="h-3.5 w-3.5" />
      </Button>
    </div>
  )
}

/**
 * Classes, properties & axioms browser: a tab per category (Classes / Object properties /
 * Data properties / Axioms) with a search box and pagination for the entity tables, instead
 * of stacking every row in one long scroll. The Axioms tab groups the class-level axioms
 * (subClassOf / disjointWith / equivalentClass) — they describe the classes listed here, so
 * they live alongside them rather than on a separate page.
 */
export default function EntitiesPanel({
  view, canWrite, initialTab = "classes",
  onAddClass, onEditClass, onDeleteClass,
  onAddProperty, onEditProperty, onDeleteProperty,
  axioms, onAddAxiom,
  selectedIri, onSelectEntity,
}: {
  view: OntologyView
  canWrite: boolean
  initialTab?: Tab
  onAddClass: () => void
  onEditClass: (c: OntologyClass) => void
  onDeleteClass: (c: OntologyClass) => void
  onAddProperty: () => void
  onEditProperty: (p: OntologyProperty, kind: "object" | "data") => void
  onDeleteProperty: (p: OntologyProperty) => void
  axioms: AxiomGroup[]
  onAddAxiom: () => void
  /** When set, class rows are clickable to drive the shared workbench selection (and highlight). */
  selectedIri?: string | null
  onSelectEntity?: (iri: string) => void
}) {
  const { t } = useI18n()
  const [tab, setTab] = useState<Tab>(initialTab)
  const [query, setQuery] = useState("")
  const [page, setPage] = useState(0)

  useEffect(() => { setPage(0) }, [tab, query])

  const labelOf = (iri: string) => view.labels[iri] ?? iri.split(/[#/]/).pop() ?? iri
  const q = query.trim().toLowerCase()

  const entities = tab === "object" ? view.object_properties : tab === "data" ? view.data_properties : view.classes
  const filteredEntities = useMemo(() => entities.filter((r) => r.label.toLowerCase().includes(q)), [entities, q])
  const flatAxioms = useMemo(
    () => axioms.flatMap((g) => g.items.map((it) => ({ type: g.type, text: it.text, parts: it.parts, onDelete: it.onDelete }))),
    [axioms],
  )
  const filteredAxioms = useMemo(() => flatAxioms.filter((a) => a.text.toLowerCase().includes(q)), [flatAxioms, q])

  const total = tab === "axioms" ? filteredAxioms.length : filteredEntities.length
  const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE))
  const p = Math.min(page, pageCount - 1)
  const rows = filteredEntities.slice(p * PAGE_SIZE, p * PAGE_SIZE + PAGE_SIZE)
  const axiomRows = filteredAxioms.slice(p * PAGE_SIZE, p * PAGE_SIZE + PAGE_SIZE)
  const colSpan = canWrite ? 4 : 3
  const axiomCount = flatAxioms.length

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Tabs value={tab} onValueChange={(v) => setTab(v as Tab)}>
          <TabsList>
            <TabsTrigger value="classes">{t("common.classes")} ({view.classes.length})</TabsTrigger>
            <TabsTrigger value="object">{t("entities.objectProperties")} ({view.object_properties.length})</TabsTrigger>
            <TabsTrigger value="data">{t("entities.dataProperties")} ({view.data_properties.length})</TabsTrigger>
            <TabsTrigger value="axioms">{t("common.axioms")} ({axiomCount})</TabsTrigger>
          </TabsList>
        </Tabs>
        <div className="flex items-center gap-2">
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={query} onChange={(e) => setQuery(e.target.value)}
              placeholder={t("common.search")} className="h-8 w-44 pl-7 text-sm"
            />
          </div>
          {canWrite && tab === "classes" && (
            <Button size="sm" variant="outline" onClick={onAddClass}><Plus className="h-3.5 w-3.5" /> {t("entities.addClass")}</Button>
          )}
          {canWrite && tab === "object" && (
            <Button size="sm" variant="outline" onClick={onAddProperty}><Plus className="h-3.5 w-3.5" /> {t("entities.addProperty")}</Button>
          )}
          {canWrite && tab === "axioms" && (
            <Button size="sm" variant="outline" disabled={view.classes.length < 2} onClick={onAddAxiom}>
              <Plus className="h-3.5 w-3.5" /> {t("entities.addAxiom")}
            </Button>
          )}
        </div>
      </div>

      {tab === "axioms" ? (
        <div className="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-28">{t("common.type")}</TableHead>
                <TableHead>{t("entities.axiom")}</TableHead>
                {canWrite && <TableHead className="w-16 text-right">{t("common.actions")}</TableHead>}
              </TableRow>
            </TableHeader>
            <TableBody>
              {axiomRows.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={canWrite ? 3 : 2} className="h-24 text-center text-muted-foreground">
                    {query ? t("entities.noMatches") : t("entities.noAxioms")}
                  </TableCell>
                </TableRow>
              ) : (
                axiomRows.map((a, i) => (
                  <TableRow key={`${a.type}-${a.text}-${i}`}>
                    <TableCell><Badge variant="secondary" className="text-[10px]">{a.type}</Badge></TableCell>
                    <TableCell>{a.parts ? <AxiomTeX {...a.parts} /> : <span className="font-medium">{a.text}</span>}</TableCell>
                    {canWrite && (
                      <TableCell className="text-right">
                        {a.onDelete && (
                          <Button size="icon" variant="ghost" className="h-7 w-7 text-muted-foreground hover:text-destructive" onClick={a.onDelete}>
                            <Trash2 className="h-3.5 w-3.5" />
                          </Button>
                        )}
                      </TableCell>
                    )}
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>
      ) : (
        <div className="rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t("entities.label")}</TableHead>
                {tab === "classes" ? (
                  <>
                    <TableHead>{t("entities.superclasses")}</TableHead>
                    <TableHead>{t("common.description")}</TableHead>
                  </>
                ) : (
                  <>
                    <TableHead>{t("entities.domain")}</TableHead>
                    <TableHead>{t("entities.range")}</TableHead>
                  </>
                )}
                {canWrite && <TableHead className="w-24 text-right">{t("common.actions")}</TableHead>}
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={colSpan} className="h-24 text-center text-muted-foreground">
                    {query ? t("entities.noMatches") : t("entities.empty")}
                  </TableCell>
                </TableRow>
              ) : tab === "classes" ? (
                (rows as OntologyClass[]).map((c) => (
                  <TableRow
                    key={c.iri}
                    onClick={onSelectEntity ? () => onSelectEntity(c.iri) : undefined}
                    data-state={selectedIri === c.iri ? "selected" : undefined}
                    className={onSelectEntity ? "cursor-pointer" : undefined}
                  >
                    <TableCell className="font-medium">{c.label}</TableCell>
                    <TableCell className="text-muted-foreground">{c.superclasses.map(labelOf).join(", ") || "—"}</TableCell>
                    <TableCell className="max-w-sm truncate text-muted-foreground">{c.comment || "—"}</TableCell>
                    {canWrite && (
                      <TableCell className="text-right" onClick={(e) => e.stopPropagation()}>
                        <RowActions onEdit={() => onEditClass(c)} onDelete={() => onDeleteClass(c)} />
                      </TableCell>
                    )}
                  </TableRow>
                ))
              ) : (
                (rows as OntologyProperty[]).map((prop) => (
                  <TableRow key={prop.iri}>
                    <TableCell className="font-medium">{prop.label}</TableCell>
                    <TableCell className="text-muted-foreground">{prop.domain_label ?? "—"}</TableCell>
                    <TableCell className="text-muted-foreground">{prop.range_label ? prop.range_label.replace(/^xsd:/, "") : "—"}</TableCell>
                    {canWrite && (
                      <TableCell className="text-right">
                        <RowActions
                          onEdit={() => onEditProperty(prop, tab === "object" ? "object" : "data")}
                          onDelete={() => onDeleteProperty(prop)}
                        />
                      </TableCell>
                    )}
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>
      )}

      {total > PAGE_SIZE && (
        <div className="flex items-center justify-between text-xs text-muted-foreground">
          <span>{t("review.page", { start: p * PAGE_SIZE + 1, end: Math.min(total, (p + 1) * PAGE_SIZE), total })}</span>
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
    </div>
  )
}
