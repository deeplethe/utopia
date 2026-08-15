import { useEffect, useMemo, useRef } from "react"
import Dagre from "@dagrejs/dagre"
import { Maximize2, ZoomIn, ZoomOut } from "lucide-react"
import { MultiDirectedGraph } from "graphology"
import circlepack from "graphology-layout/circlepack"
import Sigma from "sigma"
import {
  EdgeArrowProgram,
  EdgeLineProgram,
  drawDiscNodeLabel,
  type EdgeLabelDrawingFunction,
  type NodeHoverDrawingFunction,
  type NodeLabelDrawingFunction,
} from "sigma/rendering"
import { useTheme } from "next-themes"
import type { OntologyClass, OntologyView } from "@/lib/types"
import { useI18n, type Translate } from "@/lib/i18n"
import { Button } from "@/components/ui/button"

const STANDALONE_BRANCH = "__without_hierarchy__"

type GraphMode = "explore" | "full"

type Maps = {
  byIri: Map<string, OntologyClass>
  parentsOf: Map<string, string[]>
  childrenOf: Map<string, string[]>
  objByDomain: Map<string, OntologyView["object_properties"]>
  objByRange: Map<string, OntologyView["object_properties"]>
}

type NodeAttributes = {
  [key: string]: unknown
  label: string
  x: number
  y: number
  size: number
  baseSize: number
  color: string
  baseColor: string
  forceLabel: boolean
  highlighted?: boolean
  ringed?: boolean
}

type EdgeKind = "subclass" | "object" | "disjoint" | "equivalent"

type EdgeAttributes = {
  [key: string]: unknown
  label: string
  kind: EdgeKind
  color: string
  size: number
  type: "line" | "arrow"
  labelOffset?: number
  labelPosition?: number
  labelHalo?: string
}

type OntologyGraph = MultiDirectedGraph<NodeAttributes, EdgeAttributes>

const drawMinimalNodeHover: NodeHoverDrawingFunction<NodeAttributes, EdgeAttributes> = (context, data) => {
  context.save()
  context.beginPath()
  context.arc(data.x, data.y, data.size + 4, 0, Math.PI * 2)
  context.strokeStyle = data.color
  context.globalAlpha = 0.9
  context.lineWidth = 2
  context.stroke()
  context.restore()
}

const drawSpacedNodeLabel: NodeLabelDrawingFunction<NodeAttributes, EdgeAttributes> = (
  context,
  data,
  settings,
) => {
  drawDiscNodeLabel(
    context,
    data.ringed ? { ...data, x: data.x + 4 } : data,
    settings,
  )
}

const drawReadableEdgeLabel: EdgeLabelDrawingFunction<NodeAttributes, EdgeAttributes> = (
  context,
  edgeData,
  sourceData,
  targetData,
  settings,
) => {
  if (!edgeData.label) return
  const metadata = edgeData as typeof edgeData & Pick<EdgeAttributes, "labelOffset" | "labelPosition" | "labelHalo">
  const deltaX = targetData.x - sourceData.x
  const deltaY = targetData.y - sourceData.y
  const distance = Math.hypot(deltaX, deltaY)
  if (distance < sourceData.size + targetData.size + 24) return

  const fontSize = settings.edgeLabelSize
  const position = Math.max(0.25, Math.min(0.75, metadata.labelPosition ?? 0.5))
  const offset = metadata.labelOffset ?? 0
  const normalX = -deltaY / distance
  const normalY = deltaX / distance
  const x = sourceData.x + deltaX * position + normalX * offset
  const y = sourceData.y + deltaY * position + normalY * offset

  context.save()
  context.font = `${settings.edgeLabelWeight} ${fontSize}px ${settings.edgeLabelFont}`
  context.textAlign = "center"
  context.textBaseline = "middle"
  const maxWidth = Math.min(132, Math.max(88, distance * 0.9))
  const lines = edgeData.label.split("\n").slice(0, 2).map((rawLine) => {
    let line = rawLine
    while (line.length > 8 && context.measureText(line).width > maxWidth) line = `${line.slice(0, -2)}…`
    return line
  })
  const lineHeight = fontSize + 2
  context.lineJoin = "round"
  context.lineWidth = 5
  context.strokeStyle = metadata.labelHalo ?? "#09090b"
  context.fillStyle = settings.edgeLabelColor.color ?? edgeData.color
  lines.forEach((line, index) => {
    const lineY = y + (index - (lines.length - 1) / 2) * lineHeight
    context.strokeText(line, x, lineY)
    context.fillText(line, x, lineY)
  })
  context.restore()
}

type GraphModel = {
  graph: OntologyGraph
  rootCount: number
  standaloneCount: number
  additionalParentCount: number
}

type Palette = {
  node: string
  label: string
  edgeLabel: string
  edgeLabelHalo: string
  hierarchyEdge: string
  objectEdge: string
  disjointEdge: string
  equivalentEdge: string
  dimNode: string
  dimEdge: string
}

function palette(dark: boolean): Palette {
  return dark ? {
    node: "#f8fafc",
    label: "#e5e7eb",
    edgeLabel: "#cbd5e1",
    edgeLabelHalo: "#09090b",
    hierarchyEdge: "#64748b",
    objectEdge: "#22d3ee",
    disjointEdge: "#fb7185",
    equivalentEdge: "#4ade80",
    dimNode: "#334155",
    dimEdge: "#263244",
  } : {
    // sRGB equivalent of the light theme's --primary token
    // (oklch(0.52 0.105 223.128)); Sigma's WebGL color parser needs a concrete color.
    node: "#007595",
    label: "#111827",
    edgeLabel: "#334155",
    edgeLabelHalo: "#ffffff",
    hierarchyEdge: "#94a3b8",
    objectEdge: "#0891b2",
    disjointEdge: "#e11d48",
    equivalentEdge: "#16a34a",
    dimNode: "#cbd5e1",
    dimEdge: "#e2e8f0",
  }
}

function seededRandom(seed: number): () => number {
  let value = seed >>> 0
  return () => {
    value += 0x6d2b79f5
    let result = value
    result = Math.imul(result ^ result >>> 15, result | 1)
    result ^= result + Math.imul(result ^ result >>> 7, result | 61)
    return ((result ^ result >>> 14) >>> 0) / 4294967296
  }
}

function reaches(start: string, target: string, parentsOf: Map<string, string[]>): boolean {
  const stack = [start]
  const seen = new Set<string>()
  while (stack.length) {
    const current = stack.pop()!
    if (current === target) return true
    if (seen.has(current)) continue
    seen.add(current)
    stack.push(...(parentsOf.get(current) ?? []))
  }
  return false
}

function primaryHierarchy(view: OntologyView) {
  const classes = [...view.classes].sort((a, b) => a.label.localeCompare(b.label))
  const byIri = new Map(classes.map((item) => [item.iri, item]))
  const labelOf = (iri: string) => byIri.get(iri)?.label ?? iri
  const rawParents = new Map<string, string[]>()
  for (const item of classes) {
    rawParents.set(
      item.iri,
      item.superclasses
        .filter((iri) => byIri.has(iri))
        .sort((a, b) => labelOf(a).localeCompare(labelOf(b))),
    )
  }

  const parentOf = new Map<string, string>()
  let additionalParentCount = 0
  for (const item of classes) {
    const validParents = (rawParents.get(item.iri) ?? [])
      .filter((parent) => !reaches(parent, item.iri, rawParents))
    if (validParents.length) parentOf.set(item.iri, validParents[0])
    additionalParentCount += Math.max(0, validParents.length - 1)
  }

  const childrenOf = new Map<string, string[]>()
  for (const [child, parent] of parentOf) {
    const children = childrenOf.get(parent) ?? []
    children.push(child)
    childrenOf.set(parent, children)
  }
  for (const children of childrenOf.values()) {
    children.sort((a, b) => labelOf(a).localeCompare(labelOf(b)))
  }

  const standalone = new Set(classes
    .filter((item) => !parentOf.has(item.iri) && !(childrenOf.get(item.iri)?.length))
    .map((item) => item.iri))
  const roots = classes.filter((item) => !parentOf.has(item.iri) && !standalone.has(item.iri))

  const pathCache = new Map<string, string[]>()
  const pathOf = (iri: string): string[] => {
    if (pathCache.has(iri)) return pathCache.get(iri)!
    if (standalone.has(iri)) {
      const path = [STANDALONE_BRANCH, iri]
      pathCache.set(iri, path)
      return path
    }
    const path = [iri]
    let current = iri
    while (parentOf.has(current)) {
      current = parentOf.get(current)!
      path.unshift(current)
    }
    pathCache.set(iri, path)
    return path
  }

  const countCache = new Map<string, number>()
  const branchSize = (iri: string): number => {
    if (countCache.has(iri)) return countCache.get(iri)!
    const count = 1 + (childrenOf.get(iri) ?? []).reduce((sum, child) => sum + branchSize(child), 0)
    countCache.set(iri, count)
    return count
  }

  return {
    classes,
    childrenOf,
    pathOf,
    branchSize,
    roots,
    standalone,
    additionalParentCount,
  }
}

function endpoints(property: OntologyView["object_properties"][number]) {
  const domains = property.domain_members?.length
    ? property.domain_members : property.domain ? [property.domain] : []
  const ranges = property.range_members?.length
    ? property.range_members : property.range ? [property.range] : []
  return { domains, ranges }
}

function addOntologyEdges(
  graph: OntologyGraph,
  view: OntologyView,
  colors: Palette,
  include: (iri: string) => boolean,
  t: Translate,
) {
  const known = new Set(graph.nodes())
  const add = (
    key: string,
    source: string,
    target: string,
    attributes: EdgeAttributes,
  ) => {
    if (source === target || !known.has(source) || !known.has(target)) return
    graph.addDirectedEdgeWithKey(key, source, target, attributes)
  }

  view.axioms.subclass_of.forEach((relation, index) => {
    if (include(relation.sub) && include(relation.super)) {
      add(`sub-${index}`, relation.sub, relation.super, {
        label: t("workbench.isA"),
        kind: "subclass",
        color: colors.hierarchyEdge,
        size: 0.7,
        type: "arrow",
      })
    }
  })
  view.object_properties.forEach((property, index) => {
    const { domains, ranges } = endpoints(property)
    for (const domain of domains) for (const range of ranges) {
      if (include(domain) && include(range)) {
        add(`obj-${index}-${domain}-${range}`, domain, range, {
          label: property.label,
          kind: "object",
          color: colors.objectEdge,
          size: 0.9,
          type: "arrow",
        })
      }
    }
  })
  view.axioms.disjoint_with.forEach((relation, index) => {
    if (include(relation.a) && include(relation.b)) {
      add(`disjoint-${index}`, relation.a, relation.b, {
        label: t("workbench.disjointEdge"),
        kind: "disjoint",
        color: colors.disjointEdge,
        size: 0.8,
        type: "line",
      })
    }
  })
  view.axioms.equivalent_class.forEach((relation, index) => {
    if (include(relation.a) && include(relation.b)) {
      add(`equivalent-${index}`, relation.a, relation.b, {
        label: "≡",
        kind: "equivalent",
        color: colors.equivalentEdge,
        size: 0.9,
        type: "line",
      })
    }
  })
}

function buildFullGraph(view: OntologyView, colors: Palette, t: Translate): GraphModel {
  const hierarchy = primaryHierarchy(view)
  const graph = new MultiDirectedGraph<NodeAttributes, EdgeAttributes>()
  const paths = new Map(hierarchy.classes.map((item) => [item.iri, hierarchy.pathOf(item.iri)]))
  const maxDepth = Math.max(1, ...[...paths.values()].map((path) => path.length))
  const hierarchyAttributes = Array.from({ length: maxDepth }, (_, index) => `pack_${index}`)

  for (const item of hierarchy.classes) {
    const path = paths.get(item.iri)!
    const branchCount = hierarchy.branchSize(item.iri)
    const nodeSize = 5 + Math.min(7, Math.log2(branchCount + 1) * 1.4)
    const attributes: NodeAttributes = {
      label: item.label,
      x: 0,
      y: 0,
      size: nodeSize,
      baseSize: nodeSize,
      color: colors.node,
      baseColor: colors.node,
      forceLabel: branchCount > 1,
    }
    hierarchyAttributes.forEach((attribute, index) => {
      attributes[attribute] = path[index] ?? `self:${item.iri}`
    })
    graph.addNode(item.iri, attributes)
  }
  circlepack.assign(graph, {
    hierarchyAttributes,
    rng: seededRandom(view.classes.length * 2654435761),
    scale: 1,
  })
  addOntologyEdges(graph, view, colors, () => true, t)
  return {
    graph,
    rootCount: hierarchy.roots.length,
    standaloneCount: hierarchy.standalone.size,
    additionalParentCount: hierarchy.additionalParentCount,
  }
}

function neighbours(iri: string, maps: Maps): string[] {
  const result = new Set<string>()
  for (const parent of maps.parentsOf.get(iri) ?? []) result.add(parent)
  for (const child of maps.childrenOf.get(iri) ?? []) result.add(child)
  for (const property of maps.objByDomain.get(iri) ?? []) {
    for (const range of endpoints(property).ranges) if (maps.byIri.has(range)) result.add(range)
  }
  for (const property of maps.objByRange.get(iri) ?? []) {
    for (const domain of endpoints(property).domains) if (maps.byIri.has(domain)) result.add(domain)
  }
  return [...result]
}

function neighbourhood(iri: string, depth: number, maps: Maps): Set<string> {
  const seen = new Set<string>([iri])
  let frontier = [iri]
  for (let level = 0; level < depth; level++) {
    const next: string[] = []
    for (const current of frontier) {
      for (const adjacent of neighbours(current, maps)) {
        if (seen.has(adjacent)) continue
        seen.add(adjacent)
        next.push(adjacent)
      }
    }
    frontier = next
  }
  return seen
}

function buildExploreGraph(
  view: OntologyView,
  maps: Maps,
  focus: string,
  depth: number,
  colors: Palette,
  t: Translate,
): GraphModel {
  const graph = new MultiDirectedGraph<NodeAttributes, EdgeAttributes>()
  const ids = neighbourhood(focus, depth, maps)
  for (const iri of ids) {
    const item = maps.byIri.get(iri)
    if (!item) continue
    const nodeSize = iri === focus ? 13 : 9
    graph.addNode(iri, {
      label: item.label,
      x: 0,
      y: 0,
      size: nodeSize,
      baseSize: nodeSize,
      color: colors.node,
      baseColor: colors.node,
      forceLabel: true,
    })
  }
  addOntologyEdges(graph, view, colors, (iri) => ids.has(iri), t)

  const layout = new Dagre.graphlib.Graph().setDefaultEdgeLabel(() => ({}))
  layout.setGraph({ rankdir: "TB", nodesep: 52, ranksep: 68, marginx: 12, marginy: 12 })
  graph.forEachNode((iri) => layout.setNode(iri, { width: 48, height: 48 }))
  graph.forEachEdge((_, attributes, source, target) => {
    if (attributes.kind === "subclass") layout.setEdge(source, target, { weight: 6, minlen: 1 })
    else if (attributes.kind === "object") layout.setEdge(source, target, { weight: 1, minlen: 1 })
  })
  Dagre.layout(layout)
  graph.forEachNode((iri) => {
    const position = layout.node(iri)
    graph.mergeNodeAttributes(iri, { x: position?.x ?? 0, y: -(position?.y ?? 0) })
  })
  return { graph, rootCount: 0, standaloneCount: 0, additionalParentCount: 0 }
}

function connected(graph: OntologyGraph, source: string, target: string): boolean {
  return graph.hasEdge(source, target) || graph.hasEdge(target, source)
}

function cameraRatio(mode: GraphMode, nodeCount: number) {
  if (mode === "full") return 1
  if (nodeCount <= 2) return 2.2
  if (nodeCount <= 4) return 1.75
  if (nodeCount <= 8) return 1.4
  if (nodeCount <= 16) return 1.18
  return 1
}

function readableEdgeLabels(graph: OntologyGraph, colors: Palette) {
  const groups = new Map<string, { leader: string; labels: string[] }>()
  graph.forEachEdge((edge, attributes, source, target) => {
    if (attributes.kind === "subclass") return
    const key = source < target ? `${source}\u0000${target}` : `${target}\u0000${source}`
    const current = groups.get(key)
    if (!current) {
      groups.set(key, { leader: edge, labels: attributes.label ? [attributes.label] : [] })
      return
    }
    if (attributes.label && !current.labels.includes(attributes.label)) current.labels.push(attributes.label)
  })
  const result = new Map<string, { label: string; offset: number; position: number; halo: string }>()
  const placements = [
    { position: 0.34, offset: -42 },
    { position: 0.62, offset: 42 },
    { position: 0.45, offset: -60 },
    { position: 0.68, offset: 60 },
    { position: 0.30, offset: 20 },
  ]
  const orderedGroups = [...groups.entries()].sort(([left], [right]) => left.localeCompare(right))
  orderedGroups.forEach(([, { leader, labels }], index) => {
    if (!labels.length) return
    const placement = placements[index % placements.length]
    result.set(leader, {
      label: labels.length <= 2 ? labels.join("\n") : `${labels[0]}\n${labels[1]} +${labels.length - 2}`,
      offset: placement.offset,
      position: placement.position,
      halo: colors.edgeLabelHalo,
    })
  })
  return result
}

export default function SigmaOntologyGraph({
  view, maps, focus, selected, mode, depth, onSelect, onExplore,
}: {
  view: OntologyView
  maps: Maps
  focus: string | null
  selected: string | null
  mode: GraphMode
  depth: number
  onSelect: (iri: string) => void
  onExplore: (iri: string) => void
}) {
  const { t } = useI18n()
  const { resolvedTheme } = useTheme()
  const colors = useMemo(() => palette(resolvedTheme === "dark"), [resolvedTheme])
  const model = useMemo(() => (
    mode === "full"
      ? buildFullGraph(view, colors, t)
      : focus ? buildExploreGraph(view, maps, focus, depth, colors, t) : null
  ), [view, maps, focus, mode, depth, colors, t])
  const containerRef = useRef<HTMLDivElement | null>(null)
  const rendererRef = useRef<Sigma<NodeAttributes, EdgeAttributes> | null>(null)
  const selectedRef = useRef(selected)
  const lastSelectedRef = useRef(selected)
  const hoveredRef = useRef<string | null>(null)
  const hoveredEdgeRef = useRef<string | null>(null)
  const emphasizeSelectedRef = useRef(false)
  selectedRef.current = selected

  useEffect(() => {
    if (!containerRef.current || !model) return
    const graph = model.graph
    const edgeLabels = readableEdgeLabels(graph, colors)
    const renderer = new Sigma<NodeAttributes, EdgeAttributes>(graph, containerRef.current, {
      allowInvalidContainer: true,
      defaultNodeColor: colors.node,
      defaultEdgeColor: colors.hierarchyEdge,
      defaultDrawEdgeLabel: drawReadableEdgeLabel,
      defaultDrawNodeLabel: drawSpacedNodeLabel,
      defaultDrawNodeHover: drawMinimalNodeHover,
      edgeProgramClasses: { line: EdgeLineProgram, arrow: EdgeArrowProgram },
      hideEdgesOnMove: false,
      hideLabelsOnMove: false,
      itemSizesReference: "screen",
      labelColor: { color: colors.label },
      labelDensity: mode === "full" ? 0.22 : 0.82,
      labelFont: "Manrope, sans-serif",
      labelGridCellSize: mode === "full" ? 96 : 82,
      labelRenderedSizeThreshold: mode === "full" ? 6 : 3,
      labelSize: mode === "full" ? 13 : 15,
      labelWeight: "400",
      edgeLabelColor: { color: colors.edgeLabel },
      edgeLabelFont: "Manrope, sans-serif",
      edgeLabelSize: 11,
      enableEdgeEvents: true,
      minCameraRatio: 0.025,
      maxCameraRatio: 5,
      renderEdgeLabels: true,
      stagePadding: 64,
      zIndex: true,
      nodeReducer: (node, data) => {
        const selectedNode = selectedRef.current
        const isSelected = node === selectedNode
        const ringed = isSelected || node === hoveredRef.current
        const active = hoveredRef.current
          ?? (emphasizeSelectedRef.current ? selectedNode : null)
        const emphasized = isSelected || node === focus || node === active
        if (!active || !graph.hasNode(active)) {
          return emphasized ? {
            ...data,
            highlighted: isSelected,
            ringed,
            color: colors.node,
            forceLabel: true,
            size: data.baseSize * 1.22,
            zIndex: 2,
          } : { ...data }
        }
        if (node === active) {
          return {
            ...data,
            highlighted: isSelected,
            ringed,
            color: colors.node,
            forceLabel: true,
            size: data.baseSize * 1.3,
            zIndex: 3,
          }
        }
        if (connected(graph, active, node)) {
          return {
            ...data,
            highlighted: isSelected,
            ringed,
            forceLabel: true,
            size: data.baseSize * 1.12,
            zIndex: 2,
          }
        }
        if (isSelected) {
          return {
            ...data,
            highlighted: true,
            ringed: true,
            color: colors.node,
            forceLabel: true,
            size: data.baseSize * 1.22,
            zIndex: 2,
          }
        }
        return { ...data, highlighted: false, ringed: false, color: colors.dimNode, label: null, zIndex: 0 }
      },
      edgeReducer: (edge, data) => {
        const selectedNode = selectedRef.current
        const hoveredEdge = hoveredEdgeRef.current
        const active = hoveredRef.current
          ?? (emphasizeSelectedRef.current ? selectedNode : null)
        const [source, target] = graph.extremities(edge)
        const incident = Boolean(active && (source === active || target === active))
        const edgeHovered = edge === hoveredEdge
        const placement = edgeLabels.get(edge)
        if (mode === "full" && !active && data.kind !== "subclass") return { ...data, hidden: true }
        if (active && !incident) return { ...data, hidden: true }
        if (edgeHovered) {
          return {
            ...data,
            label: placement?.label ?? null,
            labelOffset: placement?.offset,
            labelPosition: placement?.position,
            labelHalo: placement?.halo,
            forceLabel: Boolean(placement),
            size: data.size * 2.4,
            zIndex: 3,
          }
        }
        if (incident) {
          return {
            ...data,
            label: placement?.label ?? null,
            labelOffset: placement?.offset,
            labelPosition: placement?.position,
            labelHalo: placement?.halo,
            forceLabel: Boolean(placement),
            size: data.size * 1.8,
            zIndex: 2,
          }
        }
        return mode === "full"
          ? { ...data, label: null, size: 0.45, color: colors.dimEdge, zIndex: 0 }
          : {
            ...data,
            label: placement?.label ?? null,
            labelOffset: placement?.offset,
            labelPosition: placement?.position,
            labelHalo: placement?.halo,
            forceLabel: Boolean(placement),
            zIndex: 1,
          }
      },
    })
    rendererRef.current = renderer
    requestAnimationFrame(() => {
      if (rendererRef.current !== renderer) return
      renderer.getCamera().setState({ ratio: cameraRatio(mode, graph.order) })
    })

    renderer.on("enterNode", ({ node }) => {
      hoveredRef.current = node
      renderer.refresh({ skipIndexation: true })
    })
    renderer.on("leaveNode", () => {
      hoveredRef.current = null
      renderer.refresh({ skipIndexation: true })
    })
    renderer.on("enterEdge", ({ edge }) => {
      hoveredEdgeRef.current = edge
      renderer.refresh({ skipIndexation: true })
    })
    renderer.on("leaveEdge", () => {
      hoveredEdgeRef.current = null
      renderer.refresh({ skipIndexation: true })
    })
    renderer.on("clickNode", ({ node }) => {
      selectedRef.current = node
      emphasizeSelectedRef.current = true
      onSelect(node)
      renderer.refresh({ skipIndexation: true })
    })
    renderer.on("doubleClickNode", ({ node, preventSigmaDefault }) => {
      preventSigmaDefault()
      onExplore(node)
    })
    renderer.on("clickStage", () => {
      hoveredRef.current = null
      hoveredEdgeRef.current = null
      emphasizeSelectedRef.current = false
      renderer.refresh({ skipIndexation: true })
    })

    return () => {
      renderer.kill()
      if (rendererRef.current === renderer) rendererRef.current = null
    }
  }, [model, mode, focus, colors, onSelect, onExplore])

  useEffect(() => {
    const previous = lastSelectedRef.current
    lastSelectedRef.current = selected
    const renderer = rendererRef.current
    if (!renderer) return
    renderer.refresh({ skipIndexation: true })
    if (!selected || selected === previous || !model?.graph.hasNode(selected)) return
    emphasizeSelectedRef.current = true
    requestAnimationFrame(() => {
      const camera = renderer.getCamera()
      if (mode !== "full") {
        void camera.animate({
          x: 0.5,
          y: 0.5,
          ratio: cameraRatio(mode, model.graph.order),
        }, { duration: 260 })
        return
      }
      const data = renderer.getNodeDisplayData(selected)
      if (!data) return
      void camera.animate({
        x: data.x,
        y: data.y,
        ratio: Math.min(camera.ratio, 0.3),
      }, { duration: 260 })
    })
  }, [selected, mode, model])

  if (!model) {
    return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">{t("workbench.selectClass")}</div>
  }

  const zoom = (direction: "in" | "out" | "reset") => {
    const camera = rendererRef.current?.getCamera()
    if (!camera) return
    if (direction === "in") void camera.animatedZoom({ duration: 180 })
    else if (direction === "out") void camera.animatedUnzoom({ duration: 180 })
    else void camera.animate({ x: 0.5, y: 0.5, ratio: cameraRatio(mode, model.graph.order) }, { duration: 220 })
  }

  return (
    <div className="relative h-full min-h-0 overflow-hidden bg-background/40">
      <div
        ref={containerRef}
        data-testid="sigma-ontology-graph"
        data-node-count={model.graph.order}
        data-edge-count={model.graph.size}
        aria-label={t("workbench.sigmaCanvas", { nodes: model.graph.order, edges: model.graph.size })}
        className="absolute inset-0"
      />

      {mode === "full" && (
        <div className="pointer-events-none absolute left-3 top-3 rounded-full border bg-background/85 px-2.5 py-1 text-[10px] text-muted-foreground shadow-sm backdrop-blur">
          {t("workbench.packStats", {
            classes: view.classes.length,
            roots: model.rootCount,
            standalone: model.standaloneCount,
            additional: model.additionalParentCount,
          })}
        </div>
      )}

      <div className="absolute right-3 top-3 flex items-center gap-1 rounded-lg border bg-background/90 p-1 shadow-sm backdrop-blur">
        <Button size="icon" variant="ghost" className="h-7 w-7" title={t("workbench.zoomIn")} onClick={() => zoom("in")}>
          <ZoomIn className="h-3.5 w-3.5" />
        </Button>
        <Button size="icon" variant="ghost" className="h-7 w-7" title={t("workbench.zoomOut")} onClick={() => zoom("out")}>
          <ZoomOut className="h-3.5 w-3.5" />
        </Button>
        <Button size="icon" variant="ghost" className="h-7 w-7" title={t("workbench.packReset")} onClick={() => zoom("reset")}>
          <Maximize2 className="h-3.5 w-3.5" />
        </Button>
      </div>

    </div>
  )
}
