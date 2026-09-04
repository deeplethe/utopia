// 本体的模式图：类是节点，subClassOf / 关系 / 互斥是三种边。
// 画的是 TBox（本体本身的结构），不是 /graph 画的那张 ABox（实例与事实）——
// 两者共用 graphology/sigma 这套工具链与视觉语汇（见 graphVisuals.ts），
// 但语义不共享，也不共用查询：本体数据只有一份，由 Ontology.tsx 的 useQuery
// 取，这里只接数据 + 回调。
//
// **选中态是受控的**：这个组件只管画布本身（节点/边/搜索/图例/缩放），
// 不知道编辑表单长什么样——选中一个类或关系之后，实际的 ClassForm /
// PropertyForm 由 Ontology.tsx 在画布右侧停靠渲染。早先这里自己内嵌过一份
// （selected 是内部 state，检查器也在这个文件里），代价是本体页原有的那套
// 「点左栏类名 → 出表单」路径和这里各画一遍，长得还不一样。现在两条路径
// 落到同一个 sel 状态、同一份表单组件，这个文件只剩「画」和「选中了什么」。
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
  drawHoverCard,
  drawPillLabel,
  drawWorldGrid,
  mix,
  MUTED_SHELL,
  NODE_BORDER_BASE,
  NODE_CORE_BASE,
  NODE_CORE_MIX,
  NODE_SHELL_BASE,
  NODE_TINT_MIX,
  RING_HOVER_MIX,
  RING_SELECT_MIX,
  TRANSPARENT,
} from "./graphVisuals";
import { Maximize2, Network, Search, X, ZoomIn, ZoomOut } from "lucide-react";
import type { EntityTypeView, RelationTypeView } from "../api";
import { S } from "../i18n";
import { cn } from "../ui";
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
const EDGE_SUBCLASS = "rgba(195,195,195,0.4)";
const EDGE_SUBCLASS_FOCUS = "rgba(235,235,235,0.95)";
const EDGE_RELATION = "rgba(196,165,255,0.55)"; // --u-violet
const EDGE_RELATION_FOCUS = "rgba(214,193,255,0.95)";
const EDGE_DISJOINT = "rgba(255,157,175,0.45)"; // --u-danger
const EDGE_DISJOINT_FOCUS = "rgba(255,157,175,0.9)";
const EDGE_DIM = "rgba(48,48,48,0.4)";

/** 三种边各自的语义——驱动颜色/暗淡/可点选，与「用哪个 sigma 程序画」分开管 */
const SUBCLASS_KIND = "subclass";
const RELATION_KIND = "relation";
const DISJOINT_KIND = "disjoint";

/** sigma 的渲染派发键。**故意与上面的语义分开**：关系边有直的也有弯的
 *  （只有一条就是直的，平行才弯），但两种都是「relation 语义」；早先把
 *  这两件事焊成一个字段，直的关系边全都退回默认类型、边看不见——
 *  这正是本体模式图第一版里"有些线不可见"的根因 */
const EDGE_TYPE_ARROW = "arrow"; // 直线 + 箭头（EdgeArrowProgram）
const EDGE_TYPE_CURVED_ARROW = "curvedArrow"; // 弧线 + 箭头（EdgeCurvedArrowProgram）
const EDGE_TYPE_LINE = "line"; // 直线，无箭头（EdgeLineProgram）——互斥专用

/** 结构边细、关系边粗一档——「语义关系比结构性信息更显眼」不能只靠颜色说,
 *  粗细上也要有一档差。这个粗细同时也是点选判定的命中带宽——sigma 的边拾取
 *  用的就是渲染出来的这条几何体（WebGL 拾取缓冲，不是另一套「点击容差」），
 *  细到 minEdgeThickness 的默认 1.7px 时，关系边在真实鼠标操作下几乎点不中。
 *  调粗关系边，视觉突出与「点得中」是同一个改动 */
const SUBCLASS_EDGE_SIZE = 0.9;
const DISJOINT_EDGE_SIZE = 0.8;
const RELATION_EDGE_SIZE = 2;
/** sigma 边渲染的最小厚度（像素），默认 1.7——同一个理由，全局兜底,
 *  免得缩小到某个层级时任何边都变得难点 */
const MIN_EDGE_THICKNESS = 3;
/** 节点基准大小；连接越多（继承 + 关系 + 互斥合计）越大 */
const BASE_NODE_SIZE = 9;

/** 并排偏移的步长；同一对类之间的关系边超过一条时用它扇开 */
const RELATION_CURVATURE_STEP = 0.22;
/** 自环（domain === range，如 married_to: Person→Person）没有「偏移」可言——
 *  给一个固定起始弯曲，否则退化成一个看不见的点 */
const SELF_LOOP_BASE_CURVATURE = 1;

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
      // 与 /graph 同一套四层壳配方（深壳 + 14% 类型 tint，核心 50% tint，
      // 钢灰描边微 tint）——模式图与实例图看着是同一个引擎画的
      color: mix(NODE_CORE_BASE, t.color, NODE_CORE_MIX),
      shellColor: mix(NODE_SHELL_BASE, t.color, NODE_TINT_MIX),
      borderColor: mix(NODE_BORDER_BASE, t.color, 0.3),
      ringColor: TRANSPARENT,
      typeColor: t.color,
      typeLabel: t.key,
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
        kind: SUBCLASS_KIND,
        type: EDGE_TYPE_ARROW,
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
        kind: DISJOINT_KIND,
        type: EDGE_TYPE_LINE,
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
          kind: RELATION_KIND,
          // type 由 layOutParallelRelations 按最终弯曲度决定（直线还是弧线）
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
 *  不折进画布，所以少了那一整步。
 *
 *  顺带决定每条关系边的渲染类型：**独苗走直线**（EDGE_TYPE_ARROW），
 *  只有真的平行/自环时才切到弧线程序——弧线程序在零弯曲度下也能画，
 *  但没必要为大多数只有一条的关系边多背一层曲线计算 */
function layOutParallelRelations(graph: Graphology): void {
  const groups = new Map<string, string[]>();
  graph.forEachEdge((edge, attrs, source, target) => {
    if (attrs.kind !== RELATION_KIND) return;
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
        graph.mergeEdgeAttributes(edge, {
          curvature: SELF_LOOP_BASE_CURVATURE + i * RELATION_CURVATURE_STEP,
          type: EDGE_TYPE_CURVED_ARROW,
        });
        return;
      }
      const offset = n === 1 ? 0 : i - (n - 1) / 2;
      // sigma 的弯曲度相对这条边自己的 source→target 而言，符号得按谁小谁大
      // 归一化，否则方向相反的两条边会各自以为自己是独苗，扇到同一侧叠回去
      const sign = source < target ? 1 : -1;
      const curvature = offset === 0 ? 0 : sign * offset * RELATION_CURVATURE_STEP;
      graph.mergeEdgeAttributes(edge, {
        curvature,
        type: curvature === 0 ? EDGE_TYPE_ARROW : EDGE_TYPE_CURVED_ARROW,
      });
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
  selected,
  onSelect,
}: {
  entityTypes: EntityTypeView[];
  /** 全量关系（含 attribute）：图只画 kind === "relation"，attribute 在这里
   *  单纯被忽略——它们的宾语是字面值，不是类，不进类图，也不用在这个文件里
   *  另外筛出来，展示 attribute 是 Ontology.tsx 停靠面板的事 */
  relationTypes: RelationTypeView[];
  /** 受控选中态：与 Ontology.tsx 左栏共用同一个 `sel`，点画布上的节点/边
   *  和点左栏的类名走的是同一条状态,右侧停靠的表单也就自然是同一份 */
  selected: SchemaSelection;
  onSelect: (sel: SchemaSelection) => void;
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

  const selectedRef = useRef<SchemaSelection>(null);
  const hoverRef = useRef<string | null>(null);
  const hoverEdgeRef = useRef<string | null>(null);
  useEffect(() => {
    selectedRef.current = selected;
    sigmaRef.current?.refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected]);

  const containerRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLCanvasElement>(null);
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
        // 与 /graph 同一套「状态环 → 描边 → 深色壳 → 微彩核心」四层解剖
        circle: createNodeBorderProgram({
          borders: [
            { size: { value: 0.1 }, color: { attribute: "ringColor" } },
            { size: { value: 0.07 }, color: { attribute: "borderColor" } },
            { size: { value: 0.3 }, color: { attribute: "shellColor" } },
            { size: { fill: true }, color: { attribute: "color" } },
          ],
        }),
        square: NodeSquareShellProgram,
      },
      defaultEdgeType: EDGE_TYPE_ARROW,
      edgeProgramClasses: {
        [EDGE_TYPE_ARROW]: EdgeArrowProgram,
        [EDGE_TYPE_LINE]: EdgeLineProgram,
        [EDGE_TYPE_CURVED_ARROW]: EdgeCurvedArrowProgram,
      },
      enableEdgeEvents: true,
      minEdgeThickness: MIN_EDGE_THICKNESS,
      labelFont: '"Geist", "Inter", "Noto Sans SC", sans-serif',
      labelSize: 11,
      labelColor: { color: "#e5e5e5" },
      labelRenderedSizeThreshold: 6,
      labelDensity: 0.7,
      labelGridCellSize: 140,
      minCameraRatio: 0.05,
      maxCameraRatio: 6,
      edgeLabelSize: 10,
      // 关系边的同一个紫——看到画布上有字，就知道这是一条关系边而不是继承/互斥
      edgeLabelColor: { color: "#c4a5ff" },
      edgeLabelFont: '"Geist", "Inter", sans-serif',
      defaultDrawNodeLabel: drawPillLabel,
      defaultDrawNodeHover: drawHoverCard,
      nodeReducer: (node, attrs) => {
        const res = { ...attrs };
        const base = attrs.size as number;
        const sel = selectedRef.current;
        const hov = hoverRef.current;
        // 环取节点自己的类型色，不取已经混过壳色的 color——见 graphVisuals
        // 里 RING_*_MIX 的说明
        const ownColor = (attrs.typeColor as string) ?? NODE_CORE_BASE;
        const muteNode = () => {
          res.size = base * 0.55;
          res.color = mix(MUTED_SHELL, NODE_CORE_BASE, 0.3);
          res.shellColor = MUTED_SHELL;
          res.borderColor = TRANSPARENT;
          res.ringColor = TRANSPARENT;
          res.label = "";
          res.zIndex = 0;
        };
        if (hov === node) {
          res.size = Math.max(base * 1.08, 10.4);
          res.ringColor = mix(ownColor, "#ffffff", RING_HOVER_MIX);
          // 悬浮卡接管标签展示；label 本身保留（悬浮卡靠它渲染标题）
          res.hideBaseLabel = true;
          res.zIndex = 4;
          return res;
        }
        if (sel?.kind === "class" && g.hasNode(sel.id)) {
          if (node === sel.id) {
            res.size = Math.max(base * 1.02, 9.2);
            res.ringColor = mix(ownColor, "#ffffff", RING_SELECT_MIX);
            res.forceLabel = true;
            res.zIndex = 3;
            return res;
          }
          if (g.areNeighbors(sel.id, node)) {
            res.zIndex = 2;
            return res;
          }
          muteNode();
          return res;
        }
        if (sel?.kind === "relation") {
          const rel = relationById.get(sel.id);
          if (rel) {
            const endpoints = new Set([...rel.domains, ...rel.ranges]);
            if (endpoints.has(node)) {
              res.ringColor = mix(ownColor, "#ffffff", RING_SELECT_MIX);
              res.zIndex = 2;
              return res;
            }
            muteNode();
            return res;
          }
        }
        return res;
      },
      edgeReducer: (edge, attrs) => {
        const res = { ...attrs };
        const kind = attrs.kind as string;
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

    sigma.on("clickNode", ({ node }) => onSelect({ kind: "class", id: node }));
    sigma.on("clickEdge", ({ edge }) => {
      const relId = g.getEdgeAttribute(edge, "relationId") as string | undefined;
      if (relId) onSelect({ kind: "relation", id: relId });
    });
    sigma.on("clickStage", () => onSelect(null));
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

    // 世界坐标网格：随相机变动/容器尺寸变动重绘，与 /graph 同一张背景
    const renderGrid = () => {
      if (gridRef.current) drawWorldGrid(gridRef.current, sigma);
    };
    sigma.getCamera().on("updated", renderGrid);
    sigma.on("resize", renderGrid);
    renderGrid();

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
      onSelect({ kind: "class", id: hit.id });
      focusClass(hit.id);
    } else {
      onSelect({ kind: "relation", id: hit.id });
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
                          onSelect({ kind: "relation", id: r.id });
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

      {/* 画布：世界坐标网格层（随相机动）垫在 sigma WebGL 层下，与 /graph 同款 */}
      <div className="absolute inset-0">
        <canvas ref={gridRef} className="absolute inset-0 h-full w-full" />
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
    </div>
  );
}
