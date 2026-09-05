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
//
// **不是什么都画。** 导入的包动辄几百上千个类（schema.org 一份就九百多），
// 全摊开是一团毛球。大本体只画库用到的类（有实例的及其祖先），左栏点到的
// 类随时补进画布；没有「显示全部」——九百个类摊开谁也读不出结构，那张画
// 唯一说明的是包有多大，药丸上的数字就把这句话说完了。取景规则在
// schemaScope，与投影（buildSchemaGraph）分开，两个都是纯函数。
//
// **画布上没有自己的搜索框。** 找一个类或关系走左栏的过滤框——同一页放两个
// 搜索等于没决定搜索属于谁。左栏选中什么，画布就把它带到眼前（bringIntoView）。
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
import { Maximize2, X, ZoomIn, ZoomOut } from "lucide-react";
import type { EntityTypeView, RelationTypeView } from "../api";
import { S } from "../i18n";
import {
  Pill,
  Row,
  ToolButton,
  ToolDivider,
  ToolTower,
  Tooltip,
} from "../ui";
import { usePopoverFlip } from "../ui/popoverFlip";

/* ============ 边的三种语义，与三种视觉语汇的映射 ============
   关系边与 /graph 的边同一个灰（那边是应用户要求改成纯灰的，这边不另起
   一套）。两个大类之间常有几十条关系并排扇开，任何有彩度的颜色一叠就是
   一团；灰的叠起来只是深一点。继承边反过来：亮、细——它是骨架，条数少
   （每个类一条），亮一点才能从关系的灰网里透出来，细一点又不会抢戏。
   粉色是 --u-danger，ClassForm 里互斥与父类冲突的提示文字用的就是它。

   暗态的 RGB 得自己压向背景色，不能只调低 alpha：sigma 的边着色器在
   预乘混合(ONE, ONE_MINUS_SRC_ALPHA)下不预乘 RGB，alpha 单独降不会让边
   看起来变暗（Graph.tsx 的 EDGE_DIM 处有同一条注释）。这里的边不需要
   动画淡入淡出，所以不必再搬一套 lerp/parseRgba，几个状态各写一个
   现成的颜色字面量就够了。 */
const EDGE_SUBCLASS = "rgba(235,235,235,0.55)";
const EDGE_SUBCLASS_FOCUS = "rgba(255,255,255,0.95)";
// 与 Graph.tsx 的 EDGE_COLOR 同一个灰，RGB 再压一档：这里的线粗一倍
// （MIN_EDGE_THICKNESS），同一个色值会显得更亮
const EDGE_RELATION = "rgba(128,128,128,0.3)";
const EDGE_RELATION_FOCUS = "rgba(255,255,255,0.6)";
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
/** 同一对类、同一个方向上超过这么多条关系，就并成一条带计数的边。schema.org
 *  里 Person→Organization 有二十几条（worksFor、memberOf、affiliation……），
 *  二十几条弧扇开就是那团毛球，而且哪条也点不中；一条边写着「23 relations」
 *  说的是同一件事。点它选中 domain 那个类，面板的属性页把这些关系一条条列出来 */
const BUNDLE_ABOVE = 3;
/** sigma 边渲染的最小厚度（像素），默认 1.7——同一个理由，全局兜底,
 *  免得缩小到某个层级时任何边都变得难点 */
const MIN_EDGE_THICKNESS = 3;
/** 节点大小按层级深度走：根最大，每往下一层小一档，到底不再缩。区间与
 *  /graph 的节点（5–13）同一档，两张图并排看是同一个引擎画的。以前按连接数
 *  走，结果 Thing 和它的每个子类都顶到同一个上限，层级在图上读不出来 */
const NODE_SIZE_ROOT = 13;
const NODE_SIZE_STEP = 2;
const NODE_SIZE_MIN = 6;

/** 并排偏移的步长；同一对类之间的关系边超过一条时用它扇开 */
const RELATION_CURVATURE_STEP = 0.22;
/** 自环（domain === range，如 married_to: Person→Person）没有「偏移」可言——
 *  给一个固定起始弯曲，否则退化成一个看不见的点 */
const SELF_LOOP_BASE_CURVATURE = 1;

/** 类数不超过这个数就全画——几十个类的手工本体，藏起一部分只会让人找不到
 *  自己刚建的类。超过它（导入的包动辄几百上千）才按「库用到了什么」取景 */
const FULL_VIEW_MAX_CLASSES = 60;

export interface SchemaScope {
  /** 画到画布上的类；null 表示全画 */
  drawn: ReadonlySet<string> | null;
  /** 取景依据：all 全画；in-use 有实例的类及其祖先；top 一个实例都没有,
   *  画层级最上面两层 */
  basis: "all" | "in-use" | "top";
  /** 没画的类数 */
  hidden: number;
}

/** 默认取景。本体小就全画；大了只画库用到的类——有实例的，连同祖先，好让
 *  继承链是完整的；一个实例都没有（刚建的库）就退到层级顶上两层，schema.org
 *  是 Thing 和它的十来个直接子类，正是你手画时会画的那张地图。
 *  revealed 是用户在左栏点名要看的类，同样连着祖先补进来。 */
export function schemaScope(
  entityTypes: EntityTypeView[],
  revealed: ReadonlySet<string>,
): SchemaScope {
  if (entityTypes.length <= FULL_VIEW_MAX_CLASSES) {
    return { drawn: null, basis: "all", hidden: 0 };
  }
  const byId = new Map(entityTypes.map((t) => [t.id, t]));
  const drawn = new Set<string>();
  // 已经在集合里就不再往上走——父类互指成环也走不死
  const addWithAncestors = (id: string) => {
    const t = byId.get(id);
    if (!t || drawn.has(id)) return;
    drawn.add(id);
    for (const parentId of t.parents) addWithAncestors(parentId);
  };
  for (const t of entityTypes) if (t.usage > 0) addWithAncestors(t.id);
  const basis: SchemaScope["basis"] = drawn.size > 0 ? "in-use" : "top";
  if (basis === "top") {
    // 根：没有一个父类指向真实存在的类（坏引用与自指都不算父类，
    // buildSchemaGraph 画继承边时是同一条判定）
    const rootIds = new Set(
      entityTypes
        .filter((t) => !t.parents.some((p) => p !== t.id && byId.has(p)))
        .map((t) => t.id),
    );
    for (const t of entityTypes) {
      if (rootIds.has(t.id) || t.parents.some((p) => rootIds.has(p)))
        drawn.add(t.id);
    }
  }
  for (const id of revealed) addWithAncestors(id);
  return { drawn, basis, hidden: entityTypes.length - drawn.size };
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
/** 每个类在层级里的深度：根 0，往下每层 +1；多重继承取最浅的一条。
 *  按整个本体算，不按画布上那部分——取景之外的祖先也算层数，一个类被
 *  左栏点开时该多小就多小。父类互指成环时按走到过的截断 */
function classDepths(entityTypes: EntityTypeView[]): Map<string, number> {
  const byId = new Map(entityTypes.map((t) => [t.id, t]));
  const depths = new Map<string, number>();
  const walking = new Set<string>();
  const depthOf = (id: string): number => {
    const known = depths.get(id);
    if (known !== undefined) return known;
    const t = byId.get(id);
    if (!t || walking.has(id)) return 0;
    walking.add(id);
    let depth = 0;
    for (const parentId of t.parents) {
      if (parentId === id || !byId.has(parentId)) continue;
      const d = depthOf(parentId) + 1;
      if (depth === 0 || d < depth) depth = d;
    }
    walking.delete(id);
    depths.set(id, depth);
    return depth;
  };
  for (const t of entityTypes) depthOf(t.id);
  return depths;
}

export function buildSchemaGraph(
  entityTypes: EntityTypeView[],
  relationTypes: RelationTypeView[],
  /** 取景（见 schemaScope）：只有这些类进画布；null 全画 */
  drawn: ReadonlySet<string> | null = null,
): SchemaGraphResult {
  const graph = new Graphology({ multi: true });
  const byId = new Map(entityTypes.map((t) => [t.id, t]));
  const depths = classDepths(entityTypes);

  for (const t of entityTypes) {
    if (drawn && !drawn.has(t.id)) continue;
    graph.addNode(t.id, {
      label: t.label,
      size: Math.max(
        NODE_SIZE_MIN,
        NODE_SIZE_ROOT - NODE_SIZE_STEP * (depths.get(t.id) ?? 0),
      ),
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
  // primary_parent：多重继承必须在图上看得见，primary_parent 只管左栏那棵树。
  // 取景之外的类不在图里，连着它们的边也就不画——hasNode 同时挡住坏引用
  for (const t of entityTypes) {
    if (!graph.hasNode(t.id)) continue;
    for (const parentId of t.parents) {
      if (parentId === t.id || !graph.hasNode(parentId)) continue;
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
    if (!graph.hasNode(t.id)) continue;
    for (const otherId of t.disjoint) {
      if (otherId === t.id || !graph.hasNode(otherId)) continue;
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
  const pairs = new Map<
    string,
    { d: string; rg: string; rels: RelationTypeView[] }
  >();
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
    // 端点在取景之外的那部分不画，也不算「未限定」——它是本体说过的话，只是
    // 眼下没摊在画布上；左栏的属性页照样列着它，药丸上的数也说了画面不全
    const drawnDomains = domains.filter((id) => graph.hasNode(id));
    const drawnRanges = ranges.filter((id) => graph.hasNode(id));
    for (const d of drawnDomains) {
      for (const rg of drawnRanges) {
        const key = `${d}|${rg}`;
        const pair = pairs.get(key);
        if (pair) pair.rels.push(r);
        else pairs.set(key, { d, rg, rels: [r] });
      }
    }
  }

  // 先按有向类对归堆再画：少的各画各的，多的并成一条带计数的边（见 BUNDLE_ABOVE）。
  // relationIds 两种边都带——选中一条关系时，含着它的那条边要亮
  for (const { d, rg, rels } of pairs.values()) {
    if (rels.length <= BUNDLE_ABOVE) {
      for (const r of rels) {
        graph.addEdgeWithKey(`rel:${r.id}:${d}:${rg}`, d, rg, {
          kind: RELATION_KIND,
          // type 由 layOutParallelRelations 按最终弯曲度决定（直线还是弧线）
          relationIds: [r.id],
          label: r.label,
          size: RELATION_EDGE_SIZE,
        });
      }
      continue;
    }
    graph.addEdgeWithKey(`bundle:${d}:${rg}`, d, rg, {
      kind: RELATION_KIND,
      relationIds: rels.map((r) => r.id),
      label: S.ontology.schemaBundle(rels.length),
      // 并得越多越粗一点，但封顶——粗细是「有多少」的余光提示，不是柱状图
      size: RELATION_EDGE_SIZE + Math.min(2, Math.log2(rels.length) * 0.6),
    });
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

type Point = { x: number; y: number };

/** 黄金角：同一个锚点旁边接连落下的几个新节点，按它转着摆开，不叠在一起 */
const GOLDEN_ANGLE = 2.399963;
const SEED_RADIUS = 40;

/** 确定性初始布局 + 一次性同步收敛的 ForceAtlas2。**不起动画 worker**——
 *  本体的类数量级比实例图小得多（几十到大几百，不是几千），一次性跑够步数
 *  比维护一个 worker 的生命周期简单，也不会在用户只是点了一下选中时被
 *  误重启（依赖数组只挂 entityTypes/relationTypes 与取景，选中状态在别处）。
 *  circular 的起始顺序取自节点插入顺序（即 entityTypes 数组顺序），
 *  本体不变时顺序不变，因此这套布局是可重复的——不是精确到像素的稳定，
 *  但同一份本体两次渲染出来的样子不会天差地别。
 *
 *  **有旧坐标就增量。** 大半节点上一次已经画过（搜索揭开一个类、编辑加了
 *  一个类），它们从原位出发，只给新节点在已定位的邻居旁边找落点，再少跑
 *  几十步力收一收——在画面旁边多出一个点，不是整张图重新洗牌。手动摆过的
 *  节点这时钉住（FA2 认 fixed 属性）。新节点占了大半（比如刚导入一个包）
 *  就从头来。 */
function layoutSchemaGraph(
  graph: Graphology,
  known: ReadonlyMap<string, Point>,
  pinned: ReadonlySet<string>,
): void {
  if (graph.order === 0) return;
  let placed = 0;
  graph.forEachNode((node) => {
    const pos = known.get(node);
    if (!pos) return;
    graph.mergeNodeAttributes(node, { x: pos.x, y: pos.y });
    placed += 1;
  });
  const incremental = placed * 2 >= graph.order;
  if (!incremental) {
    circular.assign(graph, { scale: 260 });
  } else if (seedFreshNodes(graph, known) === 0) {
    return; // 没有新节点：旧坐标就是终局，不再跑力
  }
  if (graph.size === 0) return; // 只有孤立节点：摆好就是终局，没有力可跑
  const settings = {
    ...forceAtlas2.inferSettings(graph),
    gravity: 0.55,
    scalingRatio: 28,
    outboundAttractionDistribution: true,
  };
  const fixed = incremental
    ? [...pinned].filter((id) => graph.hasNode(id))
    : [];
  for (const id of fixed) graph.setNodeAttribute(id, "fixed", true);
  forceAtlas2.assign(graph, { iterations: incremental ? 80 : 200, settings });
  for (const id of fixed) graph.removeNodeAttribute(id, "fixed");
}

/** 增量布局里给没有旧坐标的节点找落点：挨着一个已经定位的邻居——子类通常
 *  只连着父类，于是就是父类旁边；邻居也是新的就等下一轮，直到没人可靠为止；
 *  实在孤立的落在原点附近。返回新节点数。 */
function seedFreshNodes(
  graph: Graphology,
  known: ReadonlyMap<string, Point>,
): number {
  const pending = new Set(graph.filterNodes((node) => !known.has(node)));
  const total = pending.size;
  let seeded = 0;
  const drop = (node: string, ax: number, ay: number) => {
    const angle = seeded * GOLDEN_ANGLE;
    seeded += 1;
    graph.mergeNodeAttributes(node, {
      x: ax + SEED_RADIUS * Math.cos(angle),
      y: ay + SEED_RADIUS * Math.sin(angle),
    });
    pending.delete(node);
  };
  let progressed = true;
  while (progressed && pending.size > 0) {
    progressed = false;
    for (const node of [...pending]) {
      const anchor = graph.findNeighbor(node, (nb) => !pending.has(nb));
      if (anchor === undefined) continue;
      drop(
        node,
        graph.getNodeAttribute(anchor, "x") as number,
        graph.getNodeAttribute(anchor, "y") as number,
      );
      progressed = true;
    }
  }
  for (const node of [...pending]) drop(node, 0, 0);
  return total;
}

/** 相机推过去时最多放大到这个比例；已经比它更近就保持——从左栏挨个点类
 *  看下去时，画面不该每点一下就重新缩放 */
const FOCUS_MAX_RATIO = 0.5;
/** 视口四边留的边距：贴着边的节点也算看不见（右侧还有停靠的面板压着） */
const IN_VIEW_MARGIN = 48;

function nodePosition(sigma: Sigma, id: string): Point {
  const graph = sigma.getGraph();
  return {
    x: graph.getNodeAttribute(id, "x") as number,
    y: graph.getNodeAttribute(id, "y") as number,
  };
}

/** 节点此刻是否在视口里（按上一次渲染的相机算） */
function nodeInView(sigma: Sigma, id: string): boolean {
  const { x, y } = sigma.graphToViewport(nodePosition(sigma, id));
  const { width, height } = sigma.getDimensions();
  return (
    x >= IN_VIEW_MARGIN &&
    x <= width - IN_VIEW_MARGIN &&
    y >= IN_VIEW_MARGIN &&
    y <= height - IN_VIEW_MARGIN
  );
}

/** 相机推到一个节点上。sigma 的相机坐标是归一化到 [0,1] 的「框内」坐标，
 *  不是图坐标——直接喂图坐标（x 动辄几百）相机会飞出画面几万像素，画布
 *  一片空白。sigma 没有公开的图→框内换算，绕一趟视口：graphToViewport 用的
 *  是上一次渲染的矩阵，与 viewportToFramedGraph 用同一台相机，一来一回把
 *  相机抵消掉，剩下的就是框内坐标 */
function focusNode(sigma: Sigma, id: string): void {
  if (!sigma.getGraph().hasNode(id)) return;
  const framed = sigma.viewportToFramedGraph(
    sigma.graphToViewport(nodePosition(sigma, id)),
  );
  const ratio = Math.min(sigma.getCamera().ratio, FOCUS_MAX_RATIO);
  sigma
    .getCamera()
    .animate({ x: framed.x, y: framed.y, ratio }, { duration: 300 });
}

export type SchemaSelection =
  | { kind: "class"; id: string }
  | { kind: "relation"; id: string }
  | null;

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

  // 取景：大本体只画库用到的类，左栏点到的类补进来（见 schemaScope）
  const [revealed, setRevealed] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const scope = useMemo(
    () => schemaScope(entityTypes, revealed),
    [entityTypes, revealed],
  );
  const scopeRef = useRef(scope);
  scopeRef.current = scope;
  /** 把这些类（连同祖先）拉进画布。已经在画布上的不算；什么都没补时不动
   *  状态，免得白白重建一次图 */
  const reveal = (ids: string[]) => {
    const drawn = scopeRef.current.drawn;
    if (!drawn) return;
    const missing = ids.filter((id) => entityById.has(id) && !drawn.has(id));
    if (missing.length === 0) return;
    setRevealed((prev) =>
      missing.every((id) => prev.has(id)) ? prev : new Set([...prev, ...missing]),
    );
  };

  // 本体没变就不重建图——依赖数组只看 entityTypes/relationTypes 的引用与
  // 取景，选中/悬停都是别的状态，不会触发这里
  const schema = useMemo(
    () => buildSchemaGraph(entityTypes, objectRelations, scope.drawn),
    [entityTypes, objectRelations, scope.drawn],
  );

  /** 把一个类带到眼前：还在取景之外就先揭开、重建之后再对焦；画着但在视口
   *  外就把相机推过去；已经在视口里就只高亮，画面不动 */
  const bringIntoView = (id: string) => {
    if (!entityById.has(id)) return;
    const g = graphRef.current;
    const sigma = sigmaRef.current;
    if (g && sigma && g.hasNode(id)) {
      if (!nodeInView(sigma, id)) focusNode(sigma, id);
      return;
    }
    reveal([id]);
    pendingFocusRef.current = id;
  };

  const selectedRef = useRef<SchemaSelection>(null);
  const hoverRef = useRef<string | null>(null);
  const hoverEdgeRef = useRef<string | null>(null);
  useEffect(() => {
    selectedRef.current = selected;
    // 左栏选中什么，画布就把它带到眼前；选中一条关系则补上它的两端，
    // 相机看它的第一个 domain
    if (selected?.kind === "class") bringIntoView(selected.id);
    else if (selected?.kind === "relation") {
      const rel = relationById.get(selected.id);
      if (rel) {
        reveal([...rel.domains, ...rel.ranges]);
        if (rel.domains[0]) bringIntoView(rel.domains[0]);
      }
    }
    sigmaRef.current?.refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected]);

  const containerRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLCanvasElement>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  const graphRef = useRef<Graphology | null>(null);
  // 手动拖过的节点位置，按 id 存——**跨重建存活**：这个 ref 不在 [schema]
  // 依赖里，本体一编辑（新建/改一个类或关系都会让 entityTypes/relationTypes
  // 换引用，图整个重建）也不会清空。没有这个，摆好的布局会在下一次保存后
  // 被自动布局悄悄冲掉，「挪节点求清楚」就成了白费。只留在这次会话里，
  // 不写回后端——这是渲染层的手感，不是本体数据，同一个道理 /graph 的拖拽
  // 也没有持久化
  const draggedPositionsRef = useRef<Map<string, Point>>(new Map());
  /** 上一次画完时每个节点的坐标（含拖过的）。重建时先把它们摆回原位，只给
   *  新节点找落点——见 layoutSchemaGraph 的增量说明 */
  const positionsRef = useRef<Map<string, Point>>(new Map());
  /** 左栏点中了还没画出来的类：等它进了画布再对焦 */
  const pendingFocusRef = useRef<string | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;
    const g = schema.graph;
    layoutSchemaGraph(
      g,
      positionsRef.current,
      new Set(draggedPositionsRef.current.keys()),
    );
    // 拖过的节点摆回去——从头算的布局不知道用户已经动过手，重新算一遍位置
    // 之后，把记下来的坐标原样盖回去（增量布局里它们是钉住的，盖回去无妨）
    for (const [id, pos] of draggedPositionsRef.current) {
      if (!g.hasNode(id)) continue;
      g.setNodeAttribute(id, "x", pos.x);
      g.setNodeAttribute(id, "y", pos.y);
    }
    const positions = new Map<string, Point>();
    g.forEachNode((node, attrs) =>
      positions.set(node, { x: attrs.x as number, y: attrs.y as number }),
    );
    positionsRef.current = positions;
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
      // 与 /graph 的边标签同一个灰；只有关系边挂标签，有字的就是关系边
      edgeLabelColor: { color: "#a3a3a3" },
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
        const relIds = attrs.relationIds as string[] | undefined;
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
              ? (relIds?.includes(sel.id) ?? false)
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
      const ids = g.getEdgeAttribute(edge, "relationIds") as string[] | undefined;
      if (!ids?.length) return;
      // 单独一条：选中它；并起来的一捆：选中 domain 那个类，属性页里一条条看
      if (ids.length === 1) onSelect({ kind: "relation", id: ids[0] });
      else onSelect({ kind: "class", id: g.source(edge) });
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

    // 拖节点。**没有活的力模拟要喂**——与 /graph 不同，这里的布局是一次性
    // 算完就定住的，拖完往哪放就在哪，不会被力模拟拽回去，这正是「手动摆
    // 布局求清楚」要的效果。按下只记候选：位移超过阈值才升格成拖拽，
    // 否则一次纯点击也会被当成拖了 0 像素的拖拽,鼠标松手时机跟点选打架
    let dragCandidate: string | null = null;
    let downPoint: { x: number; y: number } | null = null;
    let dragged: string | null = null;
    sigma.on("downNode", (e) => {
      dragCandidate = e.node;
      downPoint = { x: e.event.x, y: e.event.y };
    });
    sigma.getMouseCaptor().on("mousemovebody", (e) => {
      if (!dragCandidate) return;
      if (!dragged) {
        if (!downPoint || Math.hypot(e.x - downPoint.x, e.y - downPoint.y) < 4)
          return;
        dragged = dragCandidate;
        if (containerRef.current) containerRef.current.style.cursor = "grabbing";
        // 冻住此刻的包围盒：拖着拖着节点飞出画面边缘时，相机不该跟着自动
        // 缩放去「适应」新的包围盒——那样一拖节点全图就跟着抖
        if (!sigma.getCustomBBox()) sigma.setCustomBBox(sigma.getBBox());
      }
      const pos = sigma.viewportToGraph(e);
      g.setNodeAttribute(dragged, "x", pos.x);
      g.setNodeAttribute(dragged, "y", pos.y);
      // 边拖边记：万一中途出岔子（组件卸载、切换本体）也不丢这一手
      draggedPositionsRef.current.set(dragged, pos);
      positionsRef.current.set(dragged, pos);
      e.preventSigmaDefault();
      e.original.preventDefault();
      e.original.stopPropagation();
    });
    const endDrag = () => {
      dragCandidate = null;
      downPoint = null;
      if (!dragged) return;
      dragged = null;
      if (containerRef.current)
        containerRef.current.style.cursor = hoverRef.current ? "grab" : "";
      // 拖完把冻结的包围盒解开——「归位」要看得见刚挪过去的新位置，
      // 不能还按拖拽开始前的旧范围来适应
      sigma.setCustomBBox(null);
    };
    sigma.getMouseCaptor().on("mouseup", endDrag);
    // 悬停在节点上方给个「可以抓」的提示——发现得靠猜的交互等于没有
    sigma.on("enterNode", () => {
      if (containerRef.current && !dragged)
        containerRef.current.style.cursor = "grab";
    });
    sigma.on("leaveNode", () => {
      if (containerRef.current && !dragged) containerRef.current.style.cursor = "";
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
    // 左栏点中时还没画出来的那个类，现在在了：对焦。节点的框内坐标要
    // 等 sigma 处理完一轮才有，挂在第一次渲染之后
    const pending = pendingFocusRef.current;
    if (pending && g.hasNode(pending)) {
      pendingFocusRef.current = null;
      sigma.once("afterRender", () => focusNode(sigma, pending));
    }
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

  const unscopedPop = usePopoverFlip<HTMLButtonElement, HTMLDivElement>("top left");
  const empty = entityTypes.length === 0;

  return (
    <div className="h-full relative">
      {/* 顶部悬浮条：图例 + 取景 + 未限定关系入口。没有搜索框——找东西走左栏 */}
      <div className="absolute top-3 left-3 right-3 z-10 flex items-start gap-2 pointer-events-none">
        <div className="pointer-events-auto flex flex-wrap gap-2 pt-1">
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
              className="glass rounded-full px-3 py-1 text-fine flex items-center gap-2 text-ink-2"
            >
              <span className="h-0.5 w-3 rounded-full" style={{ background: color }} />
              {label}
            </span>
          ))}

          {/* 没画的有多少：只是说明，与图例同一副样子，不可点——想看哪个类
              去左栏点它 */}
          {scope.hidden > 0 && (
            <Tooltip
              content={
                scope.basis === "top"
                  ? S.ontology.schemaScopeTopHint
                  : S.ontology.schemaScopeInUseHint
              }
            >
              <span className="glass rounded-full px-3 py-1 text-fine flex items-center text-ink-2">
                {S.ontology.schemaMoreClasses(scope.hidden)}
              </span>
            </Tooltip>
          )}

          {schema.unscoped.length > 0 && (
            <div className="relative" ref={unscopedPop.rootRef}>
              <Pill
                ref={unscopedPop.anchorRef}
                active={unscopedPop.open}
                aria-expanded={unscopedPop.open}
                onClick={() =>
                  unscopedPop.open ? unscopedPop.close() : unscopedPop.setOpen(true)
                }
              >
                {S.ontology.schemaUnscoped(schema.unscoped.length)}
              </Pill>
              {unscopedPop.open && (
                <div
                  ref={unscopedPop.panelRef}
                  className="u-menu-glass absolute left-0 top-0 z-50 w-64 overflow-hidden rounded-xl p-2 shadow-2xl"
                >
                  <Pill className="mb-2 w-full" onClick={() => unscopedPop.close()}>
                    {S.ontology.schemaUnscoped(schema.unscoped.length)}
                    <X size={11} className="ml-auto text-ink-3" />
                  </Pill>
                  <p className="px-2 pb-2 text-fine leading-relaxed text-ink-3">
                    {S.ontology.schemaUnscopedHint}
                  </p>
                  <div className="flex max-h-64 flex-col overflow-y-auto">
                    {schema.unscoped.map((r) => (
                      <Row
                        key={r.id}
                        className="text-small"
                        onClick={() => {
                          onSelect({ kind: "relation", id: r.id });
                          unscopedPop.close();
                        }}
                      >
                        {r.label}
                      </Row>
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
        <ToolTower>
          <ToolButton
            label={S.ontology.schemaZoomIn}
            icon={<ZoomIn size={15} />}
            onClick={() => sigmaRef.current?.getCamera().animatedZoom({ duration: 220 })}
          />
          <ToolButton
            label={S.ontology.schemaZoomOut}
            icon={<ZoomOut size={15} />}
            onClick={() => sigmaRef.current?.getCamera().animatedUnzoom({ duration: 220 })}
          />
          <ToolDivider />
          <ToolButton
            label={S.ontology.schemaFitView}
            icon={<Maximize2 size={15} />}
            onClick={() => sigmaRef.current?.getCamera().animatedReset({ duration: 300 })}
          />
        </ToolTower>
      </div>

      {empty && (
        <div className="absolute inset-0 grid place-items-center pointer-events-none">
          <div className="text-center text-body text-ink-3 max-w-xs">
            {S.ontology.schemaEmpty}
          </div>
        </div>
      )}
    </div>
  );
}
