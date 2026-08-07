import { useMemo } from "react"
import { useTheme } from "next-themes"
import Dagre from "@dagrejs/dagre"
import {
  Background, Controls, type Edge, Handle, MarkerType, type Node, type NodeProps, Position, ReactFlow,
} from "@xyflow/react"
import "@xyflow/react/dist/style.css"
import type { OntologyClass, OntologyView } from "@/lib/types"

const NODE_W = 168
const NODE_H = 44

type Maps = {
  byIri: Map<string, OntologyClass>
  parentsOf: Map<string, string[]>
  childrenOf: Map<string, string[]>
  objByDomain: Map<string, OntologyView["object_properties"]>
  objByRange: Map<string, OntologyView["object_properties"]>
}

// A class node styled with the app's own tokens (card surface, hairline border, --radius, subtle
// shadow, Manrope via inheritance) so the graph reads as part of the UI, not a stock React Flow
// diagram. The selected class gets the app's accent treatment.
function ClassNode({ data }: NodeProps) {
  const d = data as { label: string; focus?: boolean }
  return (
    <div
      style={{ width: NODE_W }}
      title={d.label}
      className={`truncate rounded-lg border px-3 py-2 text-center text-sm text-foreground shadow-sm ${
        d.focus ? "border-primary bg-primary/5 font-medium ring-1 ring-primary/30" : "border-border bg-card"
      }`}
    >
      <Handle type="target" position={Position.Top} style={{ opacity: 0 }} isConnectable={false} />
      {d.label}
      <Handle type="source" position={Position.Bottom} style={{ opacity: 0 }} isConnectable={false} />
    </div>
  )
}
const NODE_TYPES = { class: ClassNode }

type Colors = { mutedFg: string; primary: string; danger: string; success: string; border: string }
function themeColors(): Colors {
  const s = getComputedStyle(document.documentElement)
  const v = (n: string) => s.getPropertyValue(n).trim()
  return { mutedFg: v("--muted-foreground"), primary: v("--primary"), danger: v("--destructive"), success: "#16a34a", border: v("--border") }
}

function classNode(iri: string, label: string, focus: boolean): Node {
  return { id: iri, type: "class", data: { label, focus }, position: { x: 0, y: 0 } }
}
function subEdge(sub: string, sup: string, color: string): Edge {
  return {
    id: `sub-${sub}-${sup}`, source: sub, target: sup, label: "is-a", type: "smoothstep",
    style: { stroke: color }, labelStyle: { fill: color, fontSize: 10 },
    markerEnd: { type: MarkerType.ArrowClosed, color },
  }
}
function opEdge(from: string, to: string, lbl: string, id: string, color: string): Edge {
  return {
    id, source: from, target: to, label: lbl, type: "smoothstep",
    style: { stroke: color }, labelStyle: { fill: color, fontSize: 10, fontWeight: 600 },
    markerEnd: { type: MarkerType.ArrowClosed, color },
  }
}

type LEdge = { v: string; w: string; weight: number }
// is-a edges get heavy weight so they own the vertical ranking (a clean subclass tree); object
// properties are fed in with light weight only to keep nodes connected, not to bend the hierarchy
// around them. Disjoint/equivalent are cross-links and don't drive layout at all.
function layout(nodes: Node[], ledges: LEdge[], nodesep: number, ranksep: number): Node[] {
  const g = new Dagre.graphlib.Graph().setDefaultEdgeLabel(() => ({}))
  g.setGraph({ rankdir: "TB", nodesep, ranksep })
  nodes.forEach((n) => g.setNode(n.id, { width: NODE_W, height: NODE_H }))
  ledges.forEach((e) => g.setEdge(e.v, e.w, { weight: e.weight, minlen: 1 }))
  Dagre.layout(g)
  return nodes.map((n) => {
    const p = g.node(n.id)
    return { ...n, position: { x: (p?.x ?? 0) - NODE_W / 2, y: (p?.y ?? 0) - NODE_H / 2 } }
  })
}

// --------------------------------------------------------------------------- //
// Neighbourhood BFS (how many "levels" out from the selected class to show).
// --------------------------------------------------------------------------- //
function neighbours(iri: string, maps: Maps): string[] {
  const out = new Set<string>()
  for (const p of maps.parentsOf.get(iri) ?? []) out.add(p)
  for (const c of maps.childrenOf.get(iri) ?? []) out.add(c)
  for (const op of maps.objByDomain.get(iri) ?? []) if (op.range && maps.byIri.has(op.range)) out.add(op.range)
  for (const op of maps.objByRange.get(iri) ?? []) if (op.domain && maps.byIri.has(op.domain)) out.add(op.domain)
  return [...out]
}
function neighbourhood(iri: string, depth: number, maps: Maps): Set<string> {
  const seen = new Set<string>([iri])
  let frontier = [iri]
  for (let d = 0; d < depth; d++) {
    const next: string[] = []
    for (const n of frontier) for (const m of neighbours(n, maps)) if (!seen.has(m)) { seen.add(m); next.push(m) }
    frontier = next
  }
  return seen
}

// --------------------------------------------------------------------------- //
function buildExplore(center: string, depth: number, maps: Maps, col: Colors, highlight: string | null): { nodes: Node[]; edges: Edge[] } {
  const label = (iri: string) => maps.byIri.get(iri)?.label ?? iri
  const ids = neighbourhood(center, depth, maps)
  const has = (iri: string) => ids.has(iri) && maps.byIri.has(iri)

  const nodes: Node[] = [...ids].filter((iri) => maps.byIri.has(iri)).map((iri) => classNode(iri, label(iri), iri === highlight))
  const edges: Edge[] = []
  const ledges: LEdge[] = []
  for (const iri of ids)
    for (const p of maps.parentsOf.get(iri) ?? [])
      if (has(iri) && has(p)) { edges.push(subEdge(iri, p, col.mutedFg)); ledges.push({ v: iri, w: p, weight: 6 }) }
  for (const iri of ids)
    for (const op of maps.objByDomain.get(iri) ?? [])
      // skip self-loops (domain === range, e.g. 设备 配套 设备) — React Flow draws them as an ugly
      // loop out the top and back down; the property is still visible in the inspector.
      if (op.range && op.range !== iri && has(iri) && has(op.range)) { edges.push(opEdge(iri, op.range, op.label, `op-${op.iri}-${iri}`, col.primary)); ledges.push({ v: iri, w: op.range, weight: 1 }) }
  return { nodes: layout(nodes, ledges, 34, 64), edges }
}

function buildFull(view: OntologyView, selected: string | null, col: Colors): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = view.classes.map((cls) => classNode(cls.iri, cls.label, cls.iri === selected))
  const known = new Set(view.classes.map((c) => c.iri))
  const edges: Edge[] = []
  const ledges: LEdge[] = []
  for (const r of view.axioms.subclass_of)
    if (known.has(r.sub) && known.has(r.super)) { edges.push(subEdge(r.sub, r.super, col.mutedFg)); ledges.push({ v: r.sub, w: r.super, weight: 6 }) }
  for (const p of view.object_properties)
    if (p.domain && p.range && p.domain !== p.range && known.has(p.domain) && known.has(p.range)) { edges.push(opEdge(p.domain, p.range, p.label, `op-${p.iri}`, col.primary)); ledges.push({ v: p.domain, w: p.range, weight: 1 }) }
  for (const r of view.axioms.disjoint_with)
    if (known.has(r.a) && known.has(r.b))
      edges.push({ id: `dj-${r.a}-${r.b}`, source: r.a, target: r.b, label: "disjoint", type: "straight", style: { stroke: col.danger, strokeDasharray: "5 4" }, labelStyle: { fill: col.danger, fontSize: 10 } })
  for (const r of view.axioms.equivalent_class)
    if (known.has(r.a) && known.has(r.b))
      edges.push({ id: `eq-${r.a}-${r.b}`, source: r.a, target: r.b, label: "≡", type: "straight", style: { stroke: col.success, strokeDasharray: "5 4" }, labelStyle: { fill: col.success, fontSize: 12 } })
  return { nodes: layout(nodes, ledges, 45, 70), edges }
}

/** The React Flow graph: a focused neighbourhood ("explore", centred on `focus`, out to `depth`
 *  levels) or the whole ontology ("full"), dagre top-down layout, app-styled nodes. Single-click a
 *  node just selects it (highlight + inspector, graph unchanged); double-click re-centres on it. */
export default function ReactFlowGraph({
  view, maps, focus, selected, mode, depth, onSelect, onExplore,
}: {
  view: OntologyView
  maps: Maps
  focus: string | null
  selected: string | null
  mode: "explore" | "full"
  depth: number
  onSelect: (iri: string) => void
  onExplore: (iri: string) => void
}) {
  // Recompute the palette whenever the resolved theme flips so edge/label/background colours track
  // dark ↔ light live (themeColors reads the CSS custom properties off the now-current `.dark` class).
  const { resolvedTheme } = useTheme()
  const col = useMemo(themeColors, [resolvedTheme])
  const { nodes, edges } = useMemo(
    () => (mode === "full" ? buildFull(view, selected, col) : focus ? buildExplore(focus, depth, maps, col, selected) : { nodes: [], edges: [] }),
    [view, maps, focus, selected, mode, depth, col],
  )

  if (mode === "explore" && !focus)
    return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Select a class on the left to view its relations</div>

  return (
    <ReactFlow
      // key excludes `selected` so single-click (highlight only) never re-lays-out / refits.
      key={`${mode}-${mode === "explore" ? `${focus}-${depth}` : "full"}`}
      nodes={nodes} edges={edges} nodeTypes={NODE_TYPES}
      // Drive React Flow's built-in theme (Controls, edge-label backgrounds, handles) off the app
      // theme; without this it renders its stock light styling — white-on-white control icons in dark.
      colorMode={resolvedTheme === "dark" ? "dark" : "light"}
      // clamp the initial fit so Full Graph doesn't shrink to a tiny unreadable default (you'd have
      // to zoom in every time) and Explore doesn't blow a couple of nodes up huge; pan to see more.
      fitView fitViewOptions={{ minZoom: 0.55, maxZoom: 1.2 }} minZoom={0.1}
      proOptions={{ hideAttribution: true }} nodesConnectable={false}
      onNodeClick={(_, n) => onSelect(n.id)}
      onNodeDoubleClick={(_, n) => onExplore(n.id)}
    >
      <Background color={col.border} gap={18} />
      <Controls showInteractive={false} />
    </ReactFlow>
  )
}
