// 本体的模式图：类是节点，subClassOf / 关系 / 互斥是三种边。
// 画的是 TBox（本体本身的结构），不是 /graph 画的那张 ABox（实例与事实）——
// 两者共用 graphology/sigma 这套工具链与视觉语汇，但语义不共享，也不共用查询：
// 本体数据只有一份，由 Ontology.tsx 的 useQuery 取，这里只接数据 + 回调。
import { useEffect, useMemo, useRef, useState } from "react";
import Graphology from "graphology";
import { circular } from "graphology-layout";
import forceAtlas2 from "graphology-layout-forceatlas2";
import Sigma from "sigma";
import { createNodeBorderProgram } from "@sigma/node-border";
import { EdgeArrowProgram, EdgeLineProgram } from "sigma/rendering";
import { EdgeCurvedArrowProgram } from "@sigma/edge-curve";
import { NodeSquareShellProgram } from "./squareShellProgram";
import {
  Maximize2,
  Network,
  Pencil,
  Search,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import type { EntityTypeView, RelationTypeView } from "../api";
import { S } from "../i18n";
import { Button, Chip, cn } from "../ui";
import { usePopoverFlip } from "../ui/popoverFlip";

/* ============ 边的三种语义，与三种视觉语汇的映射 ============
   紫色留给「关系」——本体页 AI 提案面板里，Property 提案的徽标（P）已经是紫的，
   这里延续同一个语义，不是另起一套颜色。粉色是 --u-danger，ClassForm 里
   互斥与父类冲突的提示文字用的就是它。灰色留给纯结构性的 subClassOf,
   让「谁是谁的对象属性」在余光里就比「谁是谁的子类」更显眼——
   语义关系是这张图要回答的主要问题，继承是骨架，互斥是附注。

   暗态的 RGB 得自己压向背景色，不能只调低 alpha：sigma 的边着色器在
   预乘混合(ONE, ONE_MINUS_SRC_ALPHA)下不预乘 RGB，alpha 单独降不会让边
   看起来变暗（Graph.tsx 的 EDGE_DIM 处有同一条注释）。这里的边不需要
   动画淡入淡出，所以不必再搬一套 lerp/parseRgba，几个状态各写一个
   现成的颜色字面量就够了。 */
const TRANSPARENT = "rgba(0,0,0,0)";
const EDGE_SUBCLASS = "rgba(195,195,195,0.4)";
const EDGE_SUBCLASS_FOCUS = "rgba(235,235,235,0.95)";
const EDGE_RELATION = "rgba(196,165,255,0.55)"; // --u-violet
const EDGE_RELATION_FOCUS = "rgba(214,193,255,0.95)";
const EDGE_DISJOINT = "rgba(255,157,175,0.45)"; // --u-danger
const EDGE_DISJOINT_FOCUS = "rgba(255,157,175,0.9)";
const EDGE_DIM = "rgba(48,48,48,0.4)";

const SUBCLASS_KIND = "subclass";
const RELATION_KIND = "relation";
const DISJOINT_KIND = "disjoint";

/** 结构边细、关系边粗一档——「语义关系比结构性信息更显眼」不能只靠颜色说,
 *  粗细上也要有一档差 */
const SUBCLASS_EDGE_SIZE = 0.9;
const DISJOINT_EDGE_SIZE = 0.8;
const RELATION_EDGE_SIZE = 1.3;
/** 节点基准大小；连接越多（继承 + 关系 + 互斥合计）越大 */
const BASE_NODE_SIZE = 9;

/** 并排偏移的步长；同一对类之间的关系边超过一条时用它扇开 */
const RELATION_CURVATURE_STEP = 0.22;
/** 自环（domain === range，如 married_to: Person→Person）没有「偏移」可言——
 *  给一个固定起始弯曲，否则退化成一个看不见的点 */
const SELF_LOOP_BASE_CURVATURE = 1;

function hexToRgb(hex: string): [number, number, number] {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex);
  if (!m) return [163, 163, 163];
  const v = parseInt(m[1], 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}
/** 悬停/选中环的颜色：节点自己的类型色往白里混，环才能在同色的节点上跳出来 */
function mixWhite(hex: string, t: number): string {
  const [r, g, b] = hexToRgb(hex);
  const f = (c: number) => Math.round(c + (255 - c) * t);
  return `rgb(${f(r)},${f(g)},${f(b)})`;
}

export interface SchemaGraphResult {
  graph: Graphology;
  /** domains 或 ranges 留空的关系——本体没把它限定在哪个类上，画不出边，
   *  但不能装作它不存在 */
  unscoped: RelationTypeView[];
}

/** 本体 → 模式图的纯函数投影。不碰 React/Sigma，方便单独验证语义是否正确：
 *  多重继承是不是都进了图、互斥有没有被对称声明重复、平行关系有没有叠成一条、
 *  未限定的关系有没有被错误地画成全连接、坏引用（父类/域/值域指向不存在的
 *  id）会不会让它崩溃。 */
export function buildSchemaGraph(
  entityTypes: EntityTypeView[],
  relationTypes: RelationTypeView[],
): SchemaGraphResult {
  const graph = new Graphology({ multi: true });
  const byId = new Map(entityTypes.map((t) => [t.id, t]));

  for (const t of entityTypes) {
    graph.addNode(t.id, {
      label: t.label,
      color: t.color,
      // 方形节点程序硬编码四通道（ring/border/shell/fill）；shell 与 fill 同色，
      // 让它读成「描边 + 实心」而不是 /graph 那种四层壳。本体类在左栏本来就是
      // 纯色的圆点/方点（ClassTree），这里延续同一套语汇，不是发明新的
      shellColor: t.color,
      borderColor: "rgba(255,255,255,0.22)",
      ringColor: TRANSPARENT,
      type: t.shape === "square" ? "square" : "circle",
      key: t.key,
    });
  }

  // subClassOf：子 → 父。方向不因为「父类画在上面看着顺眼」而倒转——
  // 那是布局要解决的事，不是边该说谎的理由。全部 parents，不是只有
  // primary_parent：多重继承必须在图上看得见，primary_parent 只管左栏那棵树
  for (const t of entityTypes) {
    for (const parentId of t.parents) {
      if (parentId === t.id || !byId.has(parentId)) continue;
      graph.addEdgeWithKey(`sub:${t.id}:${parentId}`, t.id, parentId, {
        type: SUBCLASS_KIND,
        size: SUBCLASS_EDGE_SIZE,
      });
    }
  }

  // 互斥：无向声明，两边经常互相写一遍——按排序后的 id 对去重，只画一条
  const disjointSeen = new Set<string>();
  for (const t of entityTypes) {
    for (const otherId of t.disjoint) {
      if (otherId === t.id || !byId.has(otherId)) continue;
      const pairKey = [t.id, otherId].sort().join("|");
      if (disjointSeen.has(pairKey)) continue;
      disjointSeen.add(pairKey);
      graph.addEdgeWithKey(`dis:${pairKey}`, t.id, otherId, {
        type: DISJOINT_KIND,
        size: DISJOINT_EDGE_SIZE,
      });
    }
  }

  // 关系：attribute 的宾语是字面值，不是类，这里只处理 kind === "relation"
  const unscoped: RelationTypeView[] = [];
  for (const r of relationTypes) {
    if (r.kind !== "relation") continue;
    const domains = [...new Set(r.domains)].filter((id) => byId.has(id));
    const ranges = [...new Set(r.ranges)].filter((id) => byId.has(id));
    // domains/ranges 留空 = 本体没把这条关系限定在某个类上，不是「对所有类
    // 都成立」——画成全连接等于替本体断言了一句它没说过的话。引用的类全部
    // 失效（坏数据）时退化成同一种「画不出来」，同样进这个篮子，不吞掉它
    if (
      r.domains.length === 0 ||
      r.ranges.length === 0 ||
      domains.length === 0 ||
      ranges.length === 0
    ) {
      unscoped.push(r);
      continue;
    }
    for (const d of domains) {
      for (const rg of ranges) {
        graph.addEdgeWithKey(`rel:${r.id}:${d}:${rg}`, d, rg, {
          type: RELATION_KIND,
          relationId: r.id,
          label: r.label,
          size: RELATION_EDGE_SIZE,
        });
      }
    }
  }

  // 大小按连接数走——继承、关系、互斥合起来算,连得越多的类看着越「重要」。
  // 必须等边全部建完才能算度数，所以放在这两段循环之后
  for (const t of entityTypes) {
    const degree = graph.degree(t.id);
    graph.setNodeAttribute(
      t.id,
      "size",
      BASE_NODE_SIZE + Math.min(7, Math.sqrt(degree) * 2.1),
    );
  }

  layOutParallelRelations(graph);
  return { graph, unscoped };
}

/** 同一对类之间的多条关系边（works_at / founded / owns 都连着 Person↔Organization）
 *  各自扇到一条独立的弧上，不叠成一条谁也点不中的线。算法与 Graph.tsx 的
 *  layOutParallelEdges 同一个思路（按无向对分组，围绕直线对称铺开），
 *  但这里的边不需要先合并逆关系——本体里 inverse_of 只在关系检查器里说明，
 *  不折进画布，所以少了那一整步。 */
function layOutParallelRelations(graph: Graphology): void {
  const groups = new Map<string, string[]>();
  graph.forEachEdge((edge, attrs, source, target) => {
    if (attrs.type !== RELATION_KIND) return;
    const key =
      source === target ? `loop:${source}` : [source, target].sort().join("|");
    const list = groups.get(key);
    if (list) list.push(edge);
    else groups.set(key, [edge]);
  });
  for (const edges of groups.values()) {
    const n = edges.length;
    edges.forEach((edge, i) => {
      const [source, target] = graph.extremities(edge);
      if (source === target) {
        graph.setEdgeAttribute(
          edge,
          "curvature",
          SELF_LOOP_BASE_CURVATURE + i * RELATION_CURVATURE_STEP,
        );
        return;
      }
      const offset = n === 1 ? 0 : i - (n - 1) / 2;
      // sigma 的弯曲度相对这条边自己的 source→target 而言，符号得按谁小谁大
      // 归一化，否则方向相反的两条边会各自以为自己是独苗，扇到同一侧叠回去
      const sign = source < target ? 1 : -1;
      graph.setEdgeAttribute(
        edge,
        "curvature",
        offset === 0 ? 0 : sign * offset * RELATION_CURVATURE_STEP,
      );
    });
  }
}

/** 确定性初始布局 + 一次性同步收敛的 ForceAtlas2。**不起动画 worker**——
 *  本体的类数量级比实例图小得多（几十到大几百，不是几千），一次性跑够步数
 *  比维护一个 worker 的生命周期简单，也不会在用户只是点了一下选中时被
 *  误重启（依赖数组只挂 entityTypes/relationTypes，选中状态在别处）。
 *  circular 的起始顺序取自节点插入顺序（即 entityTypes 数组顺序），
 *  本体不变时顺序不变，因此这套布局是可重复的——不是精确到像素的稳定，
 *  但同一份本体两次渲染出来的样子不会天差地别。 */
function layoutSchemaGraph(graph: Graphology): void {
  if (graph.order === 0) return;
  circular.assign(graph, { scale: 260 });
  if (graph.size === 0) return; // 只有孤立节点：圆形摆好就是终局，没有力可跑
  const settings = {
    ...forceAtlas2.inferSettings(graph),
    gravity: 0.55,
    scalingRatio: 28,
    outboundAttractionDistribution: true,
  };
  forceAtlas2.assign(graph, { iterations: 200, settings });
}

export type SchemaSelection =
  | { kind: "class"; id: string }
  | { kind: "relation"; id: string }
  | null;

interface SearchHit {
  kind: "class" | "relation";
  id: string;
  label: string;
  sub: string | null; // 类的 key，或关系的 domain → range 摘要
}

export function OntologySchemaGraph({
  entityTypes,
  relationTypes,
  onEditClass,
  onEditRelation,
}: {
  entityTypes: EntityTypeView[];
  /** 全量关系（含 attribute）：图只画 kind === "relation"，但类检查器要用
   *  attribute 列出这个类的字面值字段——单一数据源，不在这里再筛一遍就把
   *  attribute 丢了 */
  relationTypes: RelationTypeView[];
  onEditClass: (id: string) => void;
  onEditRelation: (id: string) => void;
}) {
  const entityById = useMemo(
    () => new Map(entityTypes.map((t) => [t.id, t])),
    [entityTypes],
  );
  const relationById = useMemo(
    () => new Map(relationTypes.map((r) => [r.id, r])),
    [relationTypes],
  );
  const objectRelations = useMemo(
    () => relationTypes.filter((r) => r.kind === "relation"),
    [relationTypes],
  );

  // 本体没变就不重建图——依赖数组只看 entityTypes/relationTypes 的引用，
  // 选中/悬停都是别的状态，不会触发这里
  const schema = useMemo(
    () => buildSchemaGraph(entityTypes, objectRelations),
    [entityTypes, objectRelations],
  );

  const [selected, setSelected] = useState<SchemaSelection>(null);
  const selectedRef = useRef<SchemaSelection>(null);
  const hoverRef = useRef<string | null>(null);
  const hoverEdgeRef = useRef<string | null>(null);
  useEffect(() => {
    selectedRef.current = selected;
    sigmaRef.current?.refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected]);

  const containerRef = useRef<HTMLDivElement>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  const graphRef = useRef<Graphology | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;
    const g = schema.graph;
    layoutSchemaGraph(g);
    graphRef.current = g;
    // 选中的东西可能在新图里已经不存在了（比如删除了当前选中的类）——
    // 交给渲染时的存在性检查处理，这里不主动清空：多数情况下（编辑保存后
    // 刷新）选中的东西还在,清空只会让面板无缘无故地闪一下关掉再开

    sigmaRef.current?.kill();
    const sigma = new Sigma(g, containerRef.current, {
      allowInvalidContainer: true,
      defaultNodeType: "circle",
      nodeProgramClasses: {
        circle: createNodeBorderProgram({
          borders: [
            { size: { value: 0.14 }, color: { attribute: "ringColor" } },
            { size: { value: 0.1 }, color: { attribute: "borderColor" } },
            { size: { fill: true }, color: { attribute: "color" } },
          ],
        }),
        square: NodeSquareShellProgram,
      },
      defaultEdgeType: RELATION_KIND,
      edgeProgramClasses: {
        [SUBCLASS_KIND]: EdgeArrowProgram,
        [DISJOINT_KIND]: EdgeLineProgram,
        [RELATION_KIND]: EdgeCurvedArrowProgram,
      },
      enableEdgeEvents: true,
      labelFont: '"Geist", "Inter", "Noto Sans SC", sans-serif',
      labelSize: 12,
      labelColor: { color: "#e5e5e5" },
      labelRenderedSizeThreshold: 5,
      labelDensity: 1,
      minCameraRatio: 0.05,
      maxCameraRatio: 6,
      edgeLabelSize: 10,
      // 关系边的同一个紫——看到画布上有字，就知道这是一条关系边而不是继承/互斥
      edgeLabelColor: { color: "#c4a5ff" },
      edgeLabelFont: '"Geist", "Inter", sans-serif',
      nodeReducer: (node, attrs) => {
        const res = { ...attrs };
        const base = attrs.size as number;
        const sel = selectedRef.current;
        const hov = hoverRef.current;
        const own = (attrs.color as string) ?? "#a3a3a3";
        if (hov === node) {
          res.size = base * 1.12;
          res.ringColor = mixWhite(own, 0.7);
          res.forceLabel = true;
          res.zIndex = 4;
          return res;
        }
        if (sel?.kind === "class" && g.hasNode(sel.id)) {
          if (node === sel.id) {
            res.size = base * 1.08;
            res.ringColor = mixWhite(own, 0.35);
            res.forceLabel = true;
            res.zIndex = 3;
            return res;
          }
          if (g.areNeighbors(sel.id, node)) {
            res.zIndex = 2;
            return res;
          }
          res.color = "rgba(90,90,90,0.5)";
          res.shellColor = "rgba(90,90,90,0.5)";
          res.borderColor = TRANSPARENT;
          res.label = "";
          res.zIndex = 0;
          return res;
        }
        if (sel?.kind === "relation") {
          const rel = relationById.get(sel.id);
          const endpoints = new Set([...(rel?.domains ?? []), ...(rel?.ranges ?? [])]);
          if (endpoints.has(node)) {
            res.ringColor = mixWhite(own, 0.35);
            res.zIndex = 2;
            return res;
          }
          if (rel) {
            res.color = "rgba(90,90,90,0.5)";
            res.shellColor = "rgba(90,90,90,0.5)";
            res.borderColor = TRANSPARENT;
            res.label = "";
            res.zIndex = 0;
            return res;
          }
        }
        return res;
      },
      edgeReducer: (edge, attrs) => {
        const res = { ...attrs };
        const kind = attrs.type as string;
        const relId = attrs.relationId as string | undefined;
        const base =
          kind === SUBCLASS_KIND
            ? EDGE_SUBCLASS
            : kind === DISJOINT_KIND
              ? EDGE_DISJOINT
              : EDGE_RELATION;
        const focus =
          kind === SUBCLASS_KIND
            ? EDGE_SUBCLASS_FOCUS
            : kind === DISJOINT_KIND
              ? EDGE_DISJOINT_FOCUS
              : EDGE_RELATION_FOCUS;
        res.color = base;
        // 结构性的边（继承/互斥）不挂标签；关系边挂——但一大张图上,全部常显
        // 会变成一堵读不动的字墙，交给 renderEdgeLabels 按缩放开关（见下方
        // updateEdgeLabels）,选中的那条在检查器里说得明明白白，不用画布保证
        if (kind !== RELATION_KIND) res.label = "";

        const [s, t] = g.extremities(edge);
        const hov = hoverRef.current;
        const hoverHit = hov !== null && (hov === s || hov === t);
        const hoverEdgeHit = hoverEdgeRef.current === edge;
        const sel = selectedRef.current;
        const selHit =
          sel?.kind === "class"
            ? sel.id === s || sel.id === t
            : sel?.kind === "relation"
              ? sel.id === relId
              : false;

        if (selHit || hoverHit || hoverEdgeHit) {
          res.color = focus;
          res.size = Math.max((attrs.size as number) ?? 1, 1) * 1.5;
          res.zIndex = 3;
          return res;
        }
        if (sel) {
          // 选中了什么但这条边跟它无关：压到背景色附近去
          res.color = EDGE_DIM;
          res.label = "";
          return res;
        }
        return res;
      },
    });

    sigma.on("clickNode", ({ node }) => setSelected({ kind: "class", id: node }));
    sigma.on("clickEdge", ({ edge }) => {
      const relId = g.getEdgeAttribute(edge, "relationId") as string | undefined;
      if (relId) setSelected({ kind: "relation", id: relId });
    });
    sigma.on("clickStage", () => setSelected(null));
    sigma.on("enterNode", ({ node }) => {
      hoverRef.current = node;
      sigma.refresh();
    });
    sigma.on("leaveNode", () => {
      hoverRef.current = null;
      sigma.refresh();
    });
    sigma.on("enterEdge", ({ edge }) => {
      hoverEdgeRef.current = edge;
      sigma.refresh();
    });
    sigma.on("leaveEdge", () => {
      hoverEdgeRef.current = null;
      sigma.refresh();
    });

    // 关系标签只在放大后出现——本体大起来（导入包常有几十上百个类）时,
    // 全部常显就是第 11 条要治的那堵字墙
    const updateEdgeLabels = () =>
      sigma.setSetting("renderEdgeLabels", sigma.getCamera().ratio < 1.1);
    sigma.getCamera().on("updated", updateEdgeLabels);
    updateEdgeLabels();

    sigmaRef.current = sigma;
    if (import.meta.env.DEV) {
      // 调试句柄（仅 dev），与 Graph.tsx 同一个约定
      (window as unknown as Record<string, unknown>).__schemaGraph = g;
      (window as unknown as Record<string, unknown>).__schemaSigma = sigma;
    }
    return () => {
      sigma.kill();
      sigmaRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [schema]);

  const [searchQ, setSearchQ] = useState("");
  const searchHits = useMemo<SearchHit[]>(() => {
    const q = searchQ.trim().toLowerCase();
    if (!q) return [];
    const hits: SearchHit[] = [];
    for (const t of entityTypes) {
      if (t.label.toLowerCase().includes(q) || t.key.toLowerCase().includes(q))
        hits.push({ kind: "class", id: t.id, label: t.label, sub: t.key });
    }
    for (const r of objectRelations) {
      if (!r.label.toLowerCase().includes(q) && !r.key.toLowerCase().includes(q))
        continue;
      const d = r.domains.map((id) => entityById.get(id)?.label ?? "?").join(", ");
      const rg = r.ranges.map((id) => entityById.get(id)?.label ?? "?").join(", ");
      hits.push({
        kind: "relation",
        id: r.id,
        label: r.label,
        sub: d && rg ? `${d} → ${rg}` : null,
      });
    }
    return hits.slice(0, 12);
  }, [searchQ, entityTypes, objectRelations, entityById]);

  const focusClass = (id: string) => {
    const g = graphRef.current;
    const sigma = sigmaRef.current;
    if (!g || !sigma || !g.hasNode(id)) return;
    sigma.getCamera().animate(
      { x: g.getNodeAttribute(id, "x"), y: g.getNodeAttribute(id, "y"), ratio: 0.35 },
      { duration: 300 },
    );
  };
  const pickHit = (hit: SearchHit) => {
    if (hit.kind === "class") {
      setSelected({ kind: "class", id: hit.id });
      focusClass(hit.id);
    } else {
      setSelected({ kind: "relation", id: hit.id });
      const rel = relationById.get(hit.id);
      if (rel?.domains[0]) focusClass(rel.domains[0]);
    }
    setSearchQ("");
  };

  const unscopedPop = usePopoverFlip<HTMLButtonElement, HTMLDivElement>("top left");
  const empty = entityTypes.length === 0;

  return (
    <div className="h-full relative">
      {/* 顶部悬浮条：搜索 + 图例 + 未限定关系入口 */}
      <div className="absolute top-3 left-3 right-3 z-10 flex items-start gap-2 pointer-events-none">
        <div className="relative pointer-events-auto">
          <Search
            size={12}
            className="absolute left-2.5 top-1/2 -translate-y-1/2 text-neutral-600"
          />
          <input
            className="input-dark w-60 pl-7 pr-2 py-1.5 text-sm shadow-lg"
            placeholder={S.ontology.schemaSearchPlaceholder}
            value={searchQ}
            onChange={(e) => setSearchQ(e.target.value)}
          />
          {searchQ && (
            <div className="glass-strong absolute mt-1 w-72 rounded-lg shadow-xl overflow-hidden">
              {searchHits.length === 0 && (
                <p className="px-3 py-2 text-xs text-neutral-600">{S.ui.noMatches}</p>
              )}
              {searchHits.map((hit) => (
                <button
                  key={`${hit.kind}:${hit.id}`}
                  onClick={() => pickHit(hit)}
                  className="w-full px-3 py-1.5 text-left text-sm text-neutral-200 hover:bg-white/5 flex items-center gap-2"
                >
                  {hit.kind === "class" ? (
                    <span
                      className={cn(
                        "h-2.5 w-2.5 shrink-0",
                        entityById.get(hit.id)?.shape !== "square" && "rounded-full",
                      )}
                      style={{ background: entityById.get(hit.id)?.color }}
                    />
                  ) : (
                    <Network size={11} className="shrink-0 text-[#c4a5ff]" />
                  )}
                  <span className="truncate">{hit.label}</span>
                  {hit.sub && (
                    <span className="ml-auto shrink-0 text-xs text-neutral-500 truncate max-w-[9rem]">
                      {hit.sub}
                    </span>
                  )}
                </button>
              ))}
            </div>
          )}
        </div>

        <div className="pointer-events-auto flex flex-wrap gap-1.5 pt-0.5">
          {/* 静态图例：三种边各自的说法，不是可切换的过滤器——本体的边远比
              实例图少，藏一种边省下的空间不值得多一层交互 */}
          {(
            [
              [S.ontology.schemaLegendInheritance, EDGE_SUBCLASS_FOCUS],
              [S.ontology.schemaLegendRelation, EDGE_RELATION_FOCUS],
              [S.ontology.schemaLegendDisjoint, EDGE_DISJOINT_FOCUS],
            ] as const
          ).map(([label, color]) => (
            <span
              key={label}
              className="glass rounded-full px-2.5 py-1 text-[11px] flex items-center gap-1.5 text-neutral-300"
            >
              <span className="h-0.5 w-3 rounded-full" style={{ background: color }} />
              {label}
            </span>
          ))}

          {schema.unscoped.length > 0 && (
            <div className="relative" ref={unscopedPop.rootRef}>
              <button
                ref={unscopedPop.anchorRef}
                onClick={() =>
                  unscopedPop.open ? unscopedPop.close() : unscopedPop.setOpen(true)
                }
                aria-expanded={unscopedPop.open}
                className={cn(
                  "glass rounded-full px-2.5 py-1 text-[11px] transition-colors",
                  unscopedPop.open ? "text-neutral-100" : "text-neutral-400",
                  "hover:text-neutral-100",
                )}
              >
                {S.ontology.schemaUnscoped(schema.unscoped.length)}
              </button>
              {unscopedPop.open && (
                <div
                  ref={unscopedPop.panelRef}
                  className="u-menu-glass absolute left-0 top-0 z-50 w-64 overflow-hidden rounded-xl p-2 shadow-2xl"
                >
                  <button
                    onClick={() => unscopedPop.close()}
                    className="mb-1.5 flex w-full items-center gap-1.5 rounded-full px-1.5 py-0.5 text-[11px] text-neutral-300 hover:text-neutral-100"
                  >
                    {S.ontology.schemaUnscoped(schema.unscoped.length)}
                    <X size={11} className="ml-auto text-neutral-500" />
                  </button>
                  <p className="px-1.5 pb-1.5 text-[11px] leading-relaxed text-neutral-500">
                    {S.ontology.schemaUnscopedHint}
                  </p>
                  <div className="flex max-h-64 flex-col overflow-y-auto">
                    {schema.unscoped.map((r) => (
                      <button
                        key={r.id}
                        onClick={() => {
                          setSelected({ kind: "relation", id: r.id });
                          unscopedPop.close();
                        }}
                        className="w-full truncate rounded px-1.5 py-1 text-left text-[12px] text-neutral-300 hover:bg-white/5 hover:text-white"
                      >
                        {r.label}
                      </button>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      <div className="absolute inset-0">
        <div ref={containerRef} className="absolute inset-0" />
      </div>

      {/* 左下：缩放 + 归位。不给布局切换——模式图只有一套确定性布局,
          不像实例图那样需要按类型聚簇或摊成环 */}
      <div className="absolute bottom-4 left-3 z-10 flex flex-col items-start gap-2">
        <div className="u-tower group glass-strong rounded-xl shadow-xl flex flex-col overflow-hidden">
          <button
            title={S.ontology.schemaZoomIn}
            onClick={() => sigmaRef.current?.getCamera().animatedZoom({ duration: 220 })}
            className="flex items-center p-2 text-neutral-400 hover:text-white hover:bg-white/[0.06] transition-colors"
          >
            <ZoomIn size={15} />
            <span className="u-tower-label">{S.ontology.schemaZoomIn}</span>
          </button>
          <button
            title={S.ontology.schemaZoomOut}
            onClick={() => sigmaRef.current?.getCamera().animatedUnzoom({ duration: 220 })}
            className="flex items-center p-2 text-neutral-400 hover:text-white hover:bg-white/[0.06] transition-colors"
          >
            <ZoomOut size={15} />
            <span className="u-tower-label">{S.ontology.schemaZoomOut}</span>
          </button>
          <div className="h-px bg-white/10 mx-1.5" />
          <button
            title={S.ontology.schemaFitView}
            onClick={() => sigmaRef.current?.getCamera().animatedReset({ duration: 300 })}
            className="flex items-center p-2 text-neutral-400 hover:text-white hover:bg-white/[0.06] transition-colors"
          >
            <Maximize2 size={15} />
            <span className="u-tower-label">{S.ontology.schemaFitView}</span>
          </button>
        </div>
      </div>

      {empty && (
        <div className="absolute inset-0 grid place-items-center pointer-events-none">
          <div className="text-center text-sm text-neutral-500 max-w-xs">
            {S.ontology.schemaEmpty}
          </div>
        </div>
      )}

      {selected && (
        <SchemaInspector
          selected={selected}
          entityById={entityById}
          relationById={relationById}
          relationTypes={relationTypes}
          onSelectClass={(id) => {
            setSelected({ kind: "class", id });
            focusClass(id);
          }}
          onClose={() => setSelected(null)}
          onEditClass={onEditClass}
          onEditRelation={onEditRelation}
        />
      )}
    </div>
  );
}

/* ============ 检查器：只读，导航为主，不是又一个编辑表单 ============ */

function SchemaInspector({
  selected,
  entityById,
  relationById,
  relationTypes,
  onSelectClass,
  onClose,
  onEditClass,
  onEditRelation,
}: {
  selected: NonNullable<SchemaSelection>;
  entityById: Map<string, EntityTypeView>;
  relationById: Map<string, RelationTypeView>;
  relationTypes: RelationTypeView[];
  onSelectClass: (id: string) => void;
  onClose: () => void;
  onEditClass: (id: string) => void;
  onEditRelation: (id: string) => void;
}) {
  if (selected.kind === "class") {
    const cls = entityById.get(selected.id);
    // 选中的类在刷新后没了（并发编辑/删除）——不渲染一个查无此人的面板
    if (!cls) return null;
    // 与 AttributesCard（Ontology.tsx）同一个筛法：attribute 挂在类下，
    // 检查器只读展示，编辑还是走那张卡片,不在这里另开一份
    const attributes = relationTypes.filter(
      (r) => r.kind === "attribute" && r.domains.includes(cls.id),
    );
    return (
      <ClassInspectorBody
        cls={cls}
        entityById={entityById}
        attributes={attributes}
        onSelectClass={onSelectClass}
        onClose={onClose}
        onEdit={() => onEditClass(cls.id)}
      />
    );
  }
  const rel = relationById.get(selected.id);
  if (!rel) return null;
  return (
    <RelationInspectorBody
      rel={rel}
      entityById={entityById}
      relationById={relationById}
      onClose={onClose}
      onEdit={() => onEditRelation(rel.id)}
    />
  );
}

function InspectorShell({
  swatch,
  title,
  meta,
  onEdit,
  editLabel,
  onClose,
  children,
}: {
  swatch: React.ReactNode;
  title: string;
  meta: React.ReactNode;
  onEdit: () => void;
  editLabel: string;
  onClose: () => void;
  children: React.ReactNode;
}) {
  return (
    <div className="glass-strong absolute top-14 right-3 bottom-4 w-80 z-10 rounded-xl shadow-2xl flex flex-col">
      <div className="flex items-start justify-between gap-2 px-4 py-3.5 border-b border-white/10">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            {swatch}
            <span
              className="text-[15px] font-semibold tracking-tight text-white truncate"
              style={{ fontFamily: "var(--font-display)" }}
            >
              {title}
            </span>
          </div>
          <div className="mt-1 text-xs text-neutral-500">{meta}</div>
        </div>
        <div className="flex items-center gap-1 shrink-0">
          <button
            onClick={onClose}
            className="text-neutral-500 hover:text-neutral-200"
          >
            <X size={15} />
          </button>
        </div>
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto u-scroll px-4 py-3 space-y-3.5">
        {children}
      </div>
      <div className="border-t border-white/10 px-4 py-2.5">
        <Button size="sm" variant="ghost" onClick={onEdit} className="w-full">
          <Pencil size={12} className="mr-1.5" />
          {editLabel}
        </Button>
      </div>
    </div>
  );
}

function InspectorSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="mb-1 text-[10px] font-medium uppercase tracking-[0.08em] text-neutral-600">
        {title}
      </div>
      {children}
    </div>
  );
}

function ClassInspectorBody({
  cls,
  entityById,
  attributes,
  onSelectClass,
  onClose,
  onEdit,
}: {
  cls: EntityTypeView;
  entityById: Map<string, EntityTypeView>;
  attributes: RelationTypeView[];
  onSelectClass: (id: string) => void;
  onClose: () => void;
  onEdit: () => void;
}) {
  const parents = cls.parents
    .map((id) => entityById.get(id))
    .filter((t): t is EntityTypeView => !!t);
  const disjoint = cls.disjoint
    .map((id) => entityById.get(id))
    .filter((t): t is EntityTypeView => !!t);

  const ClassChip = ({ t }: { t: EntityTypeView }) => (
    <button
      onClick={() => onSelectClass(t.id)}
      className="flex items-center gap-1.5 rounded-full bg-white/[0.08] px-2 py-0.5 text-[11px] text-neutral-200 hover:bg-white/[0.16]"
    >
      <span
        className={cn("h-1.5 w-1.5 shrink-0", t.shape !== "square" && "rounded-full")}
        style={{ background: t.color }}
      />
      {t.label}
    </button>
  );

  return (
    <InspectorShell
      swatch={
        <span
          className={cn("h-2.5 w-2.5 shrink-0", cls.shape !== "square" && "rounded-full")}
          style={{ background: cls.color, boxShadow: `0 0 8px ${cls.color}55` }}
        />
      }
      title={cls.label}
      meta={S.ontology.usage(cls.usage)}
      onEdit={onEdit}
      editLabel={S.ontology.schemaEditClass}
      onClose={onClose}
    >
      {cls.description && (
        <InspectorSection title={S.ontology.description}>
          <p className="text-[13px] leading-relaxed text-neutral-300">
            {cls.description}
          </p>
        </InspectorSection>
      )}
      <InspectorSection title={S.ontology.parent}>
        {parents.length === 0 ? (
          <p className="text-[13px] text-neutral-500">{S.ontology.noParent}</p>
        ) : (
          <div className="flex flex-wrap gap-1.5">
            {parents.map((t) => (
              <ClassChip key={t.id} t={t} />
            ))}
          </div>
        )}
      </InspectorSection>
      {disjoint.length > 0 && (
        <InspectorSection title={S.ontology.disjoint}>
          <div className="flex flex-wrap gap-1.5">
            {disjoint.map((t) => (
              <ClassChip key={t.id} t={t} />
            ))}
          </div>
        </InspectorSection>
      )}
      {attributes.length > 0 && (
        <InspectorSection title={S.ontology.attributes}>
          <div className="divide-y divide-white/[0.06]">
            {attributes.map((a) => (
              <div
                key={a.id}
                className="flex items-center gap-2 py-1 text-[13px] text-neutral-300"
              >
                <span className="truncate">{a.label}</span>
                <Chip tone="neutral" className="shrink-0">
                  {S.ontology.datatypeNames[a.datatype ?? "text"]}
                </Chip>
              </div>
            ))}
          </div>
        </InspectorSection>
      )}
    </InspectorShell>
  );
}

function RelationInspectorBody({
  rel,
  entityById,
  relationById,
  onClose,
  onEdit,
}: {
  rel: RelationTypeView;
  entityById: Map<string, EntityTypeView>;
  relationById: Map<string, RelationTypeView>;
  onClose: () => void;
  onEdit: () => void;
}) {
  const domainLabels = rel.domains.map((id) => entityById.get(id)?.label ?? id);
  const rangeLabels = rel.ranges.map((id) => entityById.get(id)?.label ?? id);
  const axioms: [boolean, string][] = [
    [rel.functional, S.ontology.functional],
    [rel.inverse_functional, S.ontology.inverseFunctional],
    [rel.is_transitive, S.ontology.transitive],
    [rel.is_symmetric, S.ontology.symmetric],
    [rel.is_asymmetric, S.ontology.asymmetric],
    [rel.is_irreflexive, S.ontology.irreflexive],
  ];
  const activeAxioms = axioms.filter(([on]) => on);
  const temporalLabel =
    rel.temporal === "event"
      ? S.ontology.temporalEvent
      : rel.temporal === "eternal"
        ? S.ontology.temporalEternal
        : S.ontology.temporalState;
  const inverseOf = rel.inverse_of ? relationById.get(rel.inverse_of) : null;
  const subPropertyOf = rel.sub_property_of
    ? relationById.get(rel.sub_property_of)
    : null;

  return (
    <InspectorShell
      swatch={<Network size={13} className="shrink-0 text-[#c4a5ff]" />}
      title={rel.label}
      meta={S.ontology.usage(rel.usage)}
      onEdit={onEdit}
      editLabel={S.ontology.schemaEditRelation}
      onClose={onClose}
    >
      {rel.description && (
        <InspectorSection title={S.ontology.description}>
          <p className="text-[13px] leading-relaxed text-neutral-300">
            {rel.description}
          </p>
        </InspectorSection>
      )}
      <div className="grid grid-cols-2 gap-3">
        <InspectorSection title={S.ontology.domainLabel}>
          <p className="text-[13px] text-neutral-300">
            {domainLabels.length > 0 ? domainLabels.join(", ") : S.ontology.anyType}
          </p>
        </InspectorSection>
        <InspectorSection title={S.ontology.rangeLabel}>
          <p className="text-[13px] text-neutral-300">
            {rangeLabels.length > 0 ? rangeLabels.join(", ") : S.ontology.anyType}
          </p>
        </InspectorSection>
      </div>
      <InspectorSection title={S.ontology.temporal}>
        <p className="text-[13px] text-neutral-300">{temporalLabel}</p>
      </InspectorSection>
      {activeAxioms.length > 0 && (
        <InspectorSection title={S.ontology.axioms}>
          <div className="flex flex-wrap gap-1.5">
            {activeAxioms.map(([, label]) => (
              <Chip key={label} tone="info">
                {label}
              </Chip>
            ))}
          </div>
        </InspectorSection>
      )}
      {inverseOf && (
        <InspectorSection title={S.ontology.inverseOf}>
          <p className="text-[13px] text-neutral-300">{inverseOf.label}</p>
        </InspectorSection>
      )}
      {subPropertyOf && (
        <InspectorSection title={S.ontology.subPropertyOf}>
          <p className="text-[13px] text-neutral-300">{subPropertyOf.label}</p>
        </InspectorSection>
      )}
    </InspectorShell>
  );
}
