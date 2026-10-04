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
import { onThemeChange } from "../theme";
import { EdgeArrowProgram, EdgeLineProgram } from "sigma/rendering";
import EdgeCurveProgram, { EdgeCurvedArrowProgram } from "@sigma/edge-curve";
import {
  drawWorldGrid,
  mix,
  NODE_BORDER_BASE,
  NODE_CORE_BASE,
  NODE_CORE_MIX,
  NODE_SHELL_BASE,
  NODE_TINT_MIX,
  RING_SELECT_MIX,
  TRANSPARENT,
  EDGE_SUBCLASS,
  EDGE_SUBCLASS_FOCUS,
  EDGE_RELATION,
  EDGE_RELATION_FOCUS,
  EDGE_DISJOINT,
  EDGE_DISJOINT_FOCUS,
  EDGE_RULE,
  EDGE_RULE_FOCUS,
  EDGE_SCHEMA_DIM as EDGE_DIM,
  LEGEND_SUBCLASS,
  LEGEND_RELATION,
  LEGEND_DISJOINT,
  LEGEND_RULE,
  INK,
  refreshPalette,
  CANVAS_TEXT,
  CANVAS_TEXT_2,
} from "./graphVisuals";
// 画布那台机器是两页共用的（#496）：构造选项、状态表、相机、拖拽都在那边，
// 这个文件只管把本体投影成一张图、说清楚每个节点是什么颜色
import {
  attachDrag,
  focusNode,
  syncGraph,
  deferToHoverLayer,
  hoveredNode,
  mutedNode,
  neighborNode,
  nodeInView,
  NODE_TYPE_SHELL,
  drawLast,
  NODE_TYPE_SQUARE,
  ownColorOf,
  selectedNode,
  sigmaOptions,
  withTopLayer,
} from "./graphCanvas";
import { Maximize2, ZoomIn, ZoomOut } from "lucide-react";
import type { BusinessRule, EntityTypeView, RelationTypeView } from "../api";
import { S } from "../i18n";
import {
  Button,
  CanvasLoading,
  cn,
  Pill,
  Row,
  ToolButton,
  ToolDivider,
  ToolTower,
  Tooltip,
} from "../ui";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";

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

/** 三种边各自的语义——驱动颜色/暗淡/可点选，与「用哪个 sigma 程序画」分开管 */
const SUBCLASS_KIND = "subclass";
const RELATION_KIND = "relation";
const DISJOINT_KIND = "disjoint";
const RULE_KIND = "rule";

/** sigma 的渲染派发键。**故意与上面的语义分开**：关系边有直的也有弯的
 *  （只有一条就是直的，平行才弯），但两种都是「relation 语义」；早先把
 *  这两件事焊成一个字段，直的关系边全都退回默认类型、边看不见——
 *  这正是本体模式图第一版里"有些线不可见"的根因 */
const EDGE_TYPE_ARROW = "arrow"; // 直线 + 箭头（EdgeArrowProgram）
const EDGE_TYPE_CURVED_ARROW = "curvedArrow"; // 弧线 + 箭头（EdgeCurvedArrowProgram）
const EDGE_TYPE_LINE = "line"; // 直线，无箭头（EdgeLineProgram）——互斥专用
const EDGE_TYPE_CURVED_LINE = "curvedLine"; // 弧线，无箭头（EdgeCurveProgram）——互斥与别的边共用一对时


/** 结构边细、关系边粗一档——「语义关系比结构性信息更显眼」不能只靠颜色说,
 *  粗细上也要有一档差。这个粗细同时也是点选判定的命中带宽——sigma 的边拾取
 *  用的就是渲染出来的这条几何体（WebGL 拾取缓冲，不是另一套「点击容差」），
 *  细到 minEdgeThickness 的默认 1.7px 时，关系边在真实鼠标操作下几乎点不中。
 *  调粗关系边，视觉突出与「点得中」是同一个改动 */
const SUBCLASS_EDGE_SIZE = 0.9;
const DISJOINT_EDGE_SIZE = 0.8;
/** 与关系边同粗（2）。**粗细就是点选的命中带宽**——sigma 用渲染出来的几何体
 *  做拾取，细线在真实鼠标操作下几乎点不中（关系边当初调粗就是为这个）。
 *  实测 1.6 时反复点不中那条弧 */
const RULE_EDGE_SIZE = 2;
const RELATION_EDGE_SIZE = 2;
/** 同一对类、同一个方向上超过这么多条关系，就并成一条带计数的边。schema.org
 *  里 Person→Organization 有二十几条（worksFor、memberOf、affiliation……），
 *  二十几条弧扇开就是那团毛球，而且哪条也点不中；一条边写着「23 relations」
 *  说的是同一件事。点它选中 domain 那个类，面板的属性页把这些关系一条条列出来 */
const BUNDLE_ABOVE = 3;
/** sigma 边渲染的最小厚度（像素），默认 1.7——同一个理由，全局兜底，
 *  免得缩小到某个层级时任何边都变得难点。
 *
 *  **从 3 降到 2**（#497）：3 把这张图上刻意分出来的粗细一起压平了——继承边
 *  写的是 0.9、关系边是 2，下限一兜，两种都按 3 画，于是「继承细而亮、关系
 *  粗而灰」这条区分在画面上根本不存在，整张图只剩一个重量。而且模式图成了
 *  图谱页的一档之后，两档之间切换看得见这一跳：同一块画布，边不该换重量。
 *  2 与关系边自己的尺寸对齐，继承边重新细得下去，也仍然点得中 */
const MIN_EDGE_THICKNESS = 2;
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

/** 画多少个类的可选档位，与实例图的 `NODE_BUDGETS` 同构（那边是 150/300/600/1000）。
 *
 *  **「在用的类」还不够**。一个装了 schema.org 的库里，121 个类有实例，其中
 *  一百多个只有一两个——把它们全摆上去，Person（172 个）与某个只出现过一次的
 *  ImageObject 一样大一样显眼，读者看到的是一团毛球而不是这个库的形状。
 *  默认 30 已经盖到"有五个以上实例"那一档，长尾折进「+N classes」里，
 *  想看具体哪个走搜索——它会把那个类连着祖先补进画布。 */
export const SCHEMA_BUDGETS: number[] = [30, 60, 120, 240];

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
  budget: number = SCHEMA_BUDGETS[0],
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
  /* 用得最多的那几个，按档位取；**祖先不占额度**——它们是为了让继承链完整，
     不是自己要出场。所以画出来的数会比档位多一点，"+N classes" 报的是实数 */
  const inUse = entityTypes
    .filter((t) => t.usage > 0)
    .sort((a, b) => b.usage - a.usage || a.label.localeCompare(b.label))
    .slice(0, budget);
  for (const t of inUse) addWithAncestors(t.id);
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
  /** 业务规则：**一条得出类的规则就是两个类之间的一条边**——主类 → 结论类，
   *  条件写在规则里。得出属性值的那种没有目标节点，不画（它在规则表里） */
  rules: BusinessRule[] = [],
): SchemaGraphResult {
  // 壳色、边色烤进图属性，构图前先把调色板读成当前主题的值（0038）
  refreshPalette();
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
      type: t.shape === "square" ? NODE_TYPE_SQUARE : NODE_TYPE_SHELL,
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

  /* 规则边。**关掉的规则也画**，只是暗一档——「这条推理现在停着」本身是
     读图的人要知道的事；从图上消失会让人以为从来没有过这条规则 */
  for (const r of rules) {
    if (r.conclusion !== "typing" || !r.conclude_type_id) continue;
    if (!graph.hasNode(r.subject_type_id) || !graph.hasNode(r.conclude_type_id))
      continue;
    if (r.subject_type_id === r.conclude_type_id) continue;
    graph.addEdgeWithKey(`rule:${r.id}`, r.subject_type_id, r.conclude_type_id, {
      kind: RULE_KIND,
      ruleId: r.id,
      type: EDGE_TYPE_CURVED_ARROW,
      curvature: 0.35,
      label: r.name,
      size: RULE_EDGE_SIZE,
      enabled: r.enabled,
    });
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

  layOutParallelEdges(graph);
  return { graph, unscoped };
}

/** 同一对类之间的多条边各自扇到一条独立的弧上，不叠成一条谁也点不中的线。
 *  **不分种类**：继承、互斥、关系三种边都可能落在同一对类上（Unit 既是
 *  Organization 的子类又与它互斥；一条关系的主宾恰好是父子），只给关系边扇开
 *  的话，剩下两种照旧叠在直线上，标签也叠在一起。算法与 Graph.tsx 的
 *  layOutParallelEdges 同一个思路（按无向对分组，围绕直线对称铺开）；这里的边
 *  不需要先合并逆关系——本体里 inverse_of 只在关系检查器里说明，不折进画布。
 *
 *  顺带决定每条边的渲染程序：**独苗走直线**，只有真的平行/自环时才切到弧线
 *  程序；有箭头的（继承、关系）用带箭头的弧，互斥用不带箭头的弧 */
function layOutParallelEdges(graph: Graphology): void {
  const groups = new Map<string, string[]>();
  graph.forEachEdge((edge, _attrs, source, target) => {
    const key =
      source === target ? `loop:${source}` : [source, target].sort().join("|");
    const list = groups.get(key);
    if (list) list.push(edge);
    else groups.set(key, [edge]);
  });
  const typeFor = (kind: string, curved: boolean) =>
    kind === DISJOINT_KIND
      ? curved
        ? EDGE_TYPE_CURVED_LINE
        : EDGE_TYPE_LINE
      : curved
        ? EDGE_TYPE_CURVED_ARROW
        : EDGE_TYPE_ARROW;
  for (const edges of groups.values()) {
    const n = edges.length;
    edges.forEach((edge, i) => {
      const [source, target] = graph.extremities(edge);
      const kind = graph.getEdgeAttribute(edge, "kind") as string;
      if (source === target) {
        graph.mergeEdgeAttributes(edge, {
          curvature: SELF_LOOP_BASE_CURVATURE + i * RELATION_CURVATURE_STEP,
          type: typeFor(kind, true),
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
        type: typeFor(kind, curvature !== 0),
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
    // 起始圆的半径也与实例图同一个数（Graph.tsx 里是 300）：两张图同一个
    // 引擎、同一组力、同一个起点，剩下的差别才都是数据本身带来的
    circular.assign(graph, { scale: 300 });
  } else if (seedFreshNodes(graph, known) === 0) {
    return; // 没有新节点：旧坐标就是终局，不再跑力
  }
  if (graph.size === 0) return; // 只有孤立节点：摆好就是终局，没有力可跑
  /* 与实例图同一组力（Graph.tsx 里那组是拿真实的图调出来的）。
     从前这里是 gravity 0.55 / 200 步：往中心拉的力高了六成，而 FA2 是渐进
     展开的，固定步数一停就停在还没舒展开的那一刻——两件事叠起来，同一个
     引擎画出来的两张图，一张舒展一张抱团 */
  const settings = {
    ...forceAtlas2.inferSettings(graph),
    gravity: 0.35,
    scalingRatio: 22,
    outboundAttractionDistribution: true,
  };
  const fixed = incremental
    ? [...pinned].filter((id) => graph.hasNode(id))
    : [];
  for (const id of fixed) graph.setNodeAttribute(id, "fixed", true);
  // 步数也加够：200 步在几百个类上还没散开。同步跑 600 步在这个规模上
  // 是几十毫秒的事，换 worker 的复杂度不值得
  forceAtlas2.assign(graph, { iterations: incremental ? 150 : 600, settings });
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


export type SchemaSelection =
  | { kind: "class"; id: string }
  | { kind: "relation"; id: string }
  | { kind: "rule"; id: string }
  | null;

export function OntologySchemaGraph({
  entityTypes,
  relationTypes,
  rules = [],
  selected,
  onSelect,
  loading = false,
  onEmptyAction,
}: {
  /** 一个类都没有时那句话下面的按钮：带人去看文档说了什么、本体还缺什么 */
  onEmptyAction?: () => void;
  entityTypes: EntityTypeView[];
  /** 业务规则：画成主类 → 结论类的一条紫弧，点它打开规则那一页 */
  rules?: BusinessRule[];
  /** 全量关系（含 attribute）：图只画 kind === "relation"，attribute 在这里
   *  单纯被忽略——它们的宾语是字面值，不是类，不进类图，也不用在这个文件里
   *  另外筛出来，展示 attribute 是 Ontology.tsx 停靠面板的事 */
  relationTypes: RelationTypeView[];
  /** 受控选中态：与 Ontology.tsx 左栏共用同一个 `sel`，点画布上的节点/边
   *  和点左栏的类名走的是同一条状态,右侧停靠的表单也就自然是同一份 */
  selected: SchemaSelection;
  onSelect: (sel: SchemaSelection) => void;
  /** 本体还没到。网格、缩放塔、静态图例照常画，中间摆一个转圈——
   *  **与"真的没有类"分开**：那句话是结论，这个圈是过程 */
  loading?: boolean;
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

  // 取景：大本体只画用得最多的那几个类，左栏点到的类补进来（见 schemaScope）
  // 主题一变，模式图要重构（壳色烤在属性里）
  const [themeTick, setThemeTick] = useState(0);
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
    () => buildSchemaGraph(entityTypes, objectRelations, scope.drawn, rules),
    [entityTypes, objectRelations, scope.drawn, rules, themeTick]);

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
  /** 选中了、但那个类还没画进图。**画面先按兵不动**，等它进来再切。
   *
   * 不这么做的话中间会多出一帧「什么都没选中」：reducer 里那道守卫是
   * `sel.kind === "class" && g.hasNode(sel.id)`，类还没揭进来时 `hasNode` 为假，
   * 于是「选中了但还没画」被降级成「没选中」，整张图**全亮一帧**再暗回去。
   * 面板同时弹出会把画布挤窄、触发一次重绘，正好把这一帧顶到眼前，看着就是
   * 闪一下。 */
  const pendingSelectRef = useRef<SchemaSelection>(null);

  /** 这条关系在图上**有没有落点**。
   *
   * 没有主语也没有宾语的属性（图例里那批 "Unscoped properties"，一个 schema.org
   * 库里有一百八十多条）连不到任何类，也就画不出边。选中它时两个 reducer 会
   * 各自走"跟选中无关的一律压暗"那条路，结果是整张图暗下去、一个亮点都没有——
   * 读起来像"选中了但坏了"。
   *
   * 图上没它可指的时候，**画面就不该动**：细节在右边面板里，那里写着
   * Subject / Object 都是 Any type，已经把话说清楚了。 */
  const relationHasFootingRef = useRef<(id: string) => boolean>(() => true);
  relationHasFootingRef.current = (id: string) => {
    const rel = relationById.get(id);
    if (!rel) return false;
    return rel.domains.length > 0 || rel.ranges.length > 0;
  };

  useEffect(() => {
    const live = graphRef.current;
    const notDrawnYet =
      selected?.kind === "class" && !!live && !live.hasNode(selected.id);
    if (notDrawnYet) pendingSelectRef.current = selected;
    else {
      pendingSelectRef.current = null;
      selectedRef.current = selected;
    }
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
  /* 渲染器只建一次（见下面第二个 effect），闭包也就只取一次值。这两样是会变的，
     所以放进 ref 每次渲染刷新——不这么做就得让 effect 依赖它们，那又回到
     "一变就重建"。 */
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;
  const relationByIdRef = useRef(relationById);
  relationByIdRef.current = relationById;

  /* 本体或取景变了：把新算的图**搬进正在渲染的那一张**，不换渲染器。
     从前这里是 `sigmaRef.current?.kill()` 加 `new Sigma`——左栏点一个还没画出来
     的类就会走一遍：GPU 缓冲、相机、悬停与选中态全丢，再重来一次首帧。 */
  useEffect(() => {
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
    /** 图**只有一个实例**：第一次记下来，之后每次都是把新的搬进它 */
    const live = graphRef.current;
    if (live) syncGraph(live, g);
    else graphRef.current = g;
    // 选中的东西可能在新图里已经不存在了（比如删除了当前选中的类）——
    // 交给渲染时的存在性检查处理，这里不主动清空：多数情况下（编辑保存后
    // 刷新）选中的东西还在,清空只会让面板无缘无故地闪一下关掉再开

    const existing = sigmaRef.current;
    if (existing) {
      // 左栏点中时还没画出来的那个类，现在在了：对焦。节点的框内坐标要等
      // sigma 处理完一轮才有，所以挂在下一次渲染之后。
      // **先挂钩子再 refresh**：反过来的话，这一帧可能已经渲染完了，
      // 钩子挂上去就再也等不到它要等的那次渲染
      const pending = pendingFocusRef.current;
      if (pending && graphRef.current?.hasNode(pending)) {
        pendingFocusRef.current = null;
        existing.once("afterRender", () => focusNode(existing, pending));
      }
      /* 等的那个类进图了，这才把选中态交给画布：**同一帧完成切换**，
         中间不经过「什么都没选中」 */
      const held = pendingSelectRef.current;
      if (held?.kind === "class" && graphRef.current?.hasNode(held.id)) {
        pendingSelectRef.current = null;
        selectedRef.current = held;
      }
      existing.refresh();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [schema]);

  /* 渲染器**只建一次**。图是同一个实例，内容变化由上面那个 effect 搬进去，
     所以这里没有任何会变的依赖——相机、悬停、选中、GPU 缓冲也就一直活着。 */
  useEffect(() => {
    const g = graphRef.current;
    if (!containerRef.current || !g) return;
    // 画布颜色从令牌读（0038）：建实例前读一次，切主题后再读一次并重画
    refreshPalette();
    const sigma = new Sigma(g, containerRef.current, {
      ...sigmaOptions({
        defaultEdgeType: EDGE_TYPE_ARROW,
        // 每个程序配一份「最上层」的（见 `withTopLayer`）：高亮的边整批最后画
        edgeProgramClasses: withTopLayer({
          [EDGE_TYPE_ARROW]: EdgeArrowProgram,
          [EDGE_TYPE_LINE]: EdgeLineProgram,
          [EDGE_TYPE_CURVED_ARROW]: EdgeCurvedArrowProgram,
          [EDGE_TYPE_CURVED_LINE]: EdgeCurveProgram,
        }),
        minEdgeThickness: MIN_EDGE_THICKNESS,
        // 几十上百个类，缩到 0.05 就看得见全貌；实例图动辄上千，那边缩得更远
        minCameraRatio: 0.05,
        maxCameraRatio: 6,
      }),
      nodeReducer: (node, attrs) => {
        const res = { ...attrs };
        const base = attrs.size as number;
        const sel = selectedRef.current;
        const hov = hoverRef.current;
        const selClass =
          sel?.kind === "class" && g.hasNode(sel.id) ? sel.id : null;
        /* **选中压过指到**，与实例图同一条：指针落到自己选中的那个类上，读到的
           还该是选中那一副（反色底牌 + 加粗的名字）。指到别的类照常出 hover */
        if (selClass === node) {
          const picked = selectedNode(res, attrs, base);
          // 选中的这一个同时被指着：名字改由高亮层画，标签层让开
          return hov === node ? deferToHoverLayer(picked) : picked;
        }
        if (hov === node) return hoveredNode(res, attrs, base);
        if (selClass) {
          // 邻居不收小：几十个类的图，收了显得瘫（实例图那边收到 0.76）
          if (g.areNeighbors(selClass, node)) return neighborNode(res, base);
          return mutedNode(res, base);
        }
        if (sel?.kind === "relation") {
          if (!relationHasFootingRef.current(sel.id)) return res;
          const rel = relationByIdRef.current.get(sel.id);
          if (rel) {
            // 选中一条关系，亮的是它的两端——类之间没有「邻居」可言，
            // 这条关系的 domain 与 range 就是它连着的
            const endpoints = new Set([...rel.domains, ...rel.ranges]);
            if (endpoints.has(node)) {
              res.ringColor = mix(ownColorOf(attrs), INK, RING_SELECT_MIX);
              return neighborNode(res, base);
            }
            return mutedNode(res, base);
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
              : kind === RULE_KIND
                ? EDGE_RULE
                : EDGE_RELATION;
        const focus =
          kind === SUBCLASS_KIND
            ? EDGE_SUBCLASS_FOCUS
            : kind === DISJOINT_KIND
              ? EDGE_DISJOINT_FOCUS
              : kind === RULE_KIND
                ? EDGE_RULE_FOCUS
                : EDGE_RELATION_FOCUS;
        res.color = base;
        // 关掉的规则：画着，但压到暗一档——「这条推理停着」要看得见，
        // 从图上消失会让人以为从来没有过它
        if (kind === RULE_KIND && attrs.enabled === false) res.color = EDGE_DIM;
        // 结构性的边（继承/互斥）不挂标签；关系边挂——但一大张图上,全部常显
        // 会变成一堵读不动的字墙，交给 renderEdgeLabels 按缩放开关（见下方
        // updateEdgeLabels）,选中的那条在检查器里说得明明白白，不用画布保证
        // 规则边挂名字：那是它唯一说得出自己是谁的地方（条件在规则表里）
        if (kind !== RELATION_KIND && kind !== RULE_KIND) res.label = "";

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
              : sel?.kind === "rule"
                ? attrs.ruleId === sel.id
                : false;

        if (selHit || hoverHit || hoverEdgeHit) {
          res.color = focus;
          res.size = Math.max((attrs.size as number) ?? 1, 1) * 1.5;
          res.zIndex = 3;
          // 换到最后画的那一批：光有 zIndex 压不住别的程序里的边
          res.type = drawLast(String(res.type ?? EDGE_TYPE_ARROW));
          return res;
        }
        if (
          sel &&
          !(
            sel.kind === "relation" &&
            !relationHasFootingRef.current(sel.id)
          )
        ) {
          // 选中了什么但这条边跟它无关：压到背景色附近去
          res.color = EDGE_DIM;
          res.label = "";
          return res;
        }
        return res;
      },
    });

    sigma.on("clickNode", ({ node }) =>
      onSelectRef.current({ kind: "class", id: node }),
    );
    sigma.on("clickEdge", ({ edge }) => {
      const ruleId = g.getEdgeAttribute(edge, "ruleId") as string | undefined;
      if (ruleId) {
        onSelectRef.current({ kind: "rule", id: ruleId });
        return;
      }
      const ids = g.getEdgeAttribute(edge, "relationIds") as string[] | undefined;
      if (!ids?.length) return;
      // 单独一条：选中它；并起来的一捆：选中 domain 那个类，属性页里一条条看
      if (ids.length === 1)
        onSelectRef.current({ kind: "relation", id: ids[0] });
      else onSelectRef.current({ kind: "class", id: g.source(edge) });
    });
    sigma.on("clickStage", () => onSelectRef.current(null));
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
    // 布局求清楚」要的效果。阈值、包围盒冻结、光标那一套在 attachDrag 里
    attachDrag(sigma, {
      container: () => containerRef.current,
      hovering: () => !!hoverRef.current,
      // 边拖边记：万一中途出岔子（组件卸载、切换本体）也不丢这一手
      onMove: (node, pos) => {
        draggedPositionsRef.current.set(node, pos);
        positionsRef.current.set(node, pos);
      },
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
      // 调试句柄（仅 dev）：与 /graph 的 __g/__sigma 同一套，
      // 两张图的疏密、reducer 输出可以在无头环境里直接对比
      (window as unknown as Record<string, unknown>).__sg = g;
      (window as unknown as Record<string, unknown>).__ssigma = sigma;
    }
    if (import.meta.env.DEV) {
      // 调试句柄（仅 dev），与 Graph.tsx 同一个约定
      (window as unknown as Record<string, unknown>).__schemaGraph = g;
      (window as unknown as Record<string, unknown>).__schemaSigma = sigma;
    }
    const offTheme = onThemeChange(() => {
      refreshPalette();
      // 标签色是建实例时按当时的调色板定死的（sigmaOptions），实例不重建就得改设置
      sigma.setSetting("labelColor", { color: CANVAS_TEXT });
      sigma.setSetting("edgeLabelColor", { color: CANVAS_TEXT_2 });
      setThemeTick((t) => t + 1);
      sigma.refresh();
      // 世界网格只在相机动时重画：这里补一笔，不然它停在上一套墨色
      if (gridRef.current) drawWorldGrid(gridRef.current, sigma);
    });
    return () => {
      offTheme();

      sigma.kill();
      sigmaRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const [unscopedOpen, setUnscopedOpen] = useState(false);
  const empty = entityTypes.length === 0;

  return (
    <div className="h-full relative">
      {/* 图例 + 取景 + 未限定关系入口。没有搜索框——找东西走左栏 */}
      {/* 图例这一排与右侧停靠面板**顶对齐**（都从 top-3 起）：从前面板压在
          top-14，右半边空出一条 56px 的带子，看着像没对齐。
          面板开着时这一排的右缘让出面板那一列（见 .u-canvas-chrome-docked），
          否则窄窗口下图例会钻到面板底下 */}
      <div
        className={cn(
          "absolute left-3 right-3 top-3 z-10 flex items-start gap-2 pointer-events-none",
          (selected?.kind === "class" || selected?.kind === "relation") &&
            "u-canvas-chrome-docked",
        )}
      >
        <div className="pointer-events-auto flex flex-wrap gap-2">
          {/* 静态图例：几种边各自的说法，不是可切换的过滤器——本体的边远比
              实例图少，藏一种边省下的空间不值得多一层交互。
              规则那一条只在真有规则时出现：没有规则的库不该看到一个解释
              不存在的东西的图例 */}
          {(
            [
              [S.ontology.schemaLegendInheritance, LEGEND_SUBCLASS],
              [S.ontology.schemaLegendRelation, LEGEND_RELATION],
              [S.ontology.schemaLegendDisjoint, LEGEND_DISJOINT],
              ...(rules.length
                ? ([[S.ontology.schemaLegendRule, LEGEND_RULE]] as const)
                : []),
            ] as const
          ).map(([label, color]) => (
            <span
              key={label}
              /* **就用药丸那一个类**。与旁边那枚可点的 Pill 同高、同内距、
                 同圆角——规矩上 4 给 chip、6 给控件，可这一排的盒子高度内距
                 字号全一样，只有"能不能点"不同；一样大的盒子摆一排却两种圆角，
                 读出来是没对齐，不是有含义。`is-static` 收掉悬停时的提亮：
                 不可点的东西给反馈是在骗手 */
              className="u-pill is-static gap-2"
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
              <span className="u-pill is-static">
                {S.ontology.schemaMoreClasses(scope.hidden)}
              </span>
            </Tooltip>
          )}

          {schema.unscoped.length > 0 && (
            <Popover open={unscopedOpen} onOpenChange={setUnscopedOpen}>
              <PopoverTrigger asChild>
                <Pill active={unscopedOpen}>
                  {S.ontology.schemaUnscoped(schema.unscoped.length)}
                </Pill>
              </PopoverTrigger>
              <PopoverContent align="start" className="w-72 overflow-hidden p-0">
                  {/* 标题行只说这是什么；关闭归 Esc、外点与胶囊本身 */}
                  <div className="flex items-center gap-3 border-b border-line px-4 py-3">
                    <span className="min-w-0 flex-1 truncate text-body font-medium text-ink">
                      {S.ontology.schemaUnscoped(schema.unscoped.length)}
                    </span>
                  </div>
                  <p className="border-b border-line px-4 py-2 text-fine leading-relaxed text-ink-2">
                    {S.ontology.schemaUnscopedHint}
                  </p>
                  <div className="u-scroll flex max-h-64 flex-col overflow-y-auto p-2">
                    {schema.unscoped.map((r) => (
                      <Row
                        key={r.id}
                        className="text-small"
                        onClick={() => {
                          onSelect({ kind: "relation", id: r.id });
                          setUnscopedOpen(false);
                        }}
                      >
                        {r.label}
                      </Row>
                    ))}
                  </div>
              </PopoverContent>
            </Popover>
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

      {/* 本体还在路上：**这块地方大半已经可以画了**。世界坐标网格、左下的缩放塔、
          三条静态图例，都跟本体取没取回来无关；缺的只是节点。所以转圈落在画布
          中间，而不是把整块换成一个转圈——后者等于把已经就绪的东西一起藏起来。
          与"真的没有类"分开：那句话是结论，这个圈是过程，长得一样就读错了 */}
      {loading ? (
        <CanvasLoading />
      ) : empty ? (
        <div className="absolute inset-0 grid place-items-center pointer-events-none">
          <div className="text-center text-body text-ink-2 max-w-sm">
            {S.ontology.schemaEmpty}
            {onEmptyAction && (
              <div className="mt-3 pointer-events-auto">
                <Button size="sm" variant="secondary" onClick={onEmptyAction}>
                  {S.ontology.schemaEmptyAction}
                </Button>
              </div>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}
