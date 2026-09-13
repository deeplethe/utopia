import type { ReactNode } from "react";
import { fmtObjectValue } from "../objectValue";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MutableRefObject,
} from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import Graphology from "graphology";
import { circular, circlepack } from "graphology-layout";
import forceAtlas2 from "graphology-layout-forceatlas2";
import FA2Layout from "graphology-layout-forceatlas2/worker";
import Sigma from "sigma";
import EdgeCurveProgram from "@sigma/edge-curve";
import { EdgeRectangleProgram } from "sigma/rendering";
import { onThemeChange } from "../theme";
import {
  EDGE,
  EDGE_CONTEST,
  EDGE_DERIVED,
  EDGE_DERIVED_DIM,
  EDGE_DIM,
  EDGE_FOCUS,
  EDGE_FOCUS_CONTEST,
  EDGE_FOCUS_DERIVED,
  EDGE_INFERRED,
  HOVER_MUTE,
  MUTED_SHELL,
  NODE_BORDER_BASE,
  NODE_CORE_BASE,
  NODE_CORE_MIX,
  NODE_SHELL_BASE,
  NODE_TINT_MIX,
  SCRUB_FUTURE,
  SCRUB_PAST,
  SCRUB_PLAY,
  TRANSPARENT,
  drawWorldGrid,
  lerpColor,
  mix,
  refreshPalette,
} from "./graphVisuals";
// 画布那台机器是两页共用的（#496）：构造选项、状态表、相机、拖拽都在那边，
// 这个文件只管把实例与事实投影成一张图、说清楚每个节点是什么颜色
import {
  attachDrag,
  deferToHoverLayer,
  hoveredNode,
  mutedNode,
  neighborNode,
  NODE_TYPE_SHELL,
  NODE_TYPE_SQUARE,
  selectedNode,
  sigmaOptions,
  withTopLayer,
  drawLast,
  softMutedNode,
} from "./graphCanvas";
import { EntityHistory } from "./EntityHistory";
import { EntityDialog, FactTimeDialog } from "./graphDialogs";
import { fmtTime } from "../time";
import { NextStep, nextStep, useReadiness } from "./NextStep";
import {
  ArrowLeft,
  ArrowRight,
  ChevronRight,
  CircleDashed,
  ExternalLink,
  Grape,
  Loader2,
  Maximize2,
  Orbit,
  Pause,
  Pencil,
  Play,
  Search,
  Waypoints,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import {
  api,
  type DerivedFact,
  type EntityFact,
  type Evidence,
  type GraphEdge,
  type GraphNode,
  type BlockedDerivation,
  type ProofStep,
} from "../api";
import { S } from "../i18n";
import { predicateSentence } from "../predicateText";
import {
  Button,
  CanvasLoading,
  ExpandCard,
  HOVER_ROW,
  IconButton,
  Input,
  LinkButton,
  Pill,
  REVEAL,
  ROW_VALUE,
  Row,
  Segmented,
  ToolButton,
  ToolDivider,
  ToolTower,
  cn,
  GroupLabel,
  chipLike,
  Chip,} from "../ui";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { useKb, useKbId } from "../kb";
import { toast } from "../toast";

const EDGE_CURVATURE_STEP = 0.18;

/** 一条边画在哪条弧上；`curvature === 0` = 直线。 */
interface PlacedEdge {
  edge: GraphEdge;
  curvature: number;
  /** 并进这条边的别的说法（逆关系），悬停时一并显示 */
  alsoLabels: string[];
}

/** 同一对节点之间的边，画在各自的弧上；逆关系推出来的那些先并掉。
 *
 *  **两件事，顺序有讲究：先减后分。**
 *
 *  一、`A works_at B` 与它推出的 `B employs A` 是**同一件事的两种说法**，
 *  不是两条知识。画成两条弧只是把冗余画得好看一点。所以逆关系推出来的边
 *  并进它的来源边，说法挂在那条边上。`sub_property`（`ceo_of ⊑ works_at`）
 *  不并——那是两条粒度不同的事实，各自成立。
 *
 *  二、剩下的按**无向对**分组扇开。无向是要点：一条边和它的反向边起终点相反，
 *  按有向对分组会各自成组、各自以为自己是独苗，于是又叠回同一条直线上。
 *  分组用 min/max，而落到弧上时按边自己的方向翻符号——sigma 的曲率是相对
 *  source→target 的，不翻的话反向的弧会绕到同一侧。 */
function layOutParallelEdges(edges: GraphEdge[]): {
  edges: PlacedEdge[];
  folded: number;
} {
  const pairKey = (a: string, b: string) => (a < b ? `${a} ${b}` : `${b} ${a}`);
  const push = (m: Map<string, GraphEdge[]>, k: string, e: GraphEdge) => {
    const list = m.get(k);
    if (list) list.push(e);
    else m.set(k, [e]);
  };

  // ---- 一、并掉逆关系推出来的边
  const survivors: GraphEdge[] = [];
  const inverses: GraphEdge[] = [];
  for (const e of edges) {
    if (e.derived && e.rule === "inverse") inverses.push(e);
    else survivors.push(e);
  }
  /* **按前提找来源边，不是按节点对。**
     一度写成「取那一对节点上的第一条边」，于是 `contains` 挂到了恰好也连着
     那两点的 `allied_with` 上——而 `contains` 属于 `part_of`。挂错之后
     界面上看着完全正常，是最难发现的那一种。前提是服务端算出来的，用它。 */
  const onScreen = new Map<string, GraphEdge>();
  for (const e of survivors) onScreen.set(e.id, e);

  const also = new Map<string, string[]>();
  let folded = 0;
  for (const e of inverses) {
    const host = (e.premises ?? []).map((p) => onScreen.get(p)).find(Boolean);
    if (!host) {
      // 来源边不在这一屏（时间轴筛掉了，或它自己也是推出来的而被过滤了）。
      // **那就留着它**——并进一条不存在的边等于把这条知识删了
      survivors.push(e);
      continue;
    }
    const list = also.get(host.id) ?? [];
    // 去重：传递推出来的几条 `part_of` 各有各的逆，而前提链都回到同一条边上，
    // 于是同一个说法会被挂三遍。**说法是名字，不是计数**
    const name = e.label ?? e.predicate ?? "";
    if (!list.includes(name)) list.push(name);
    also.set(host.id, list);
    folded++;
  }

  // ---- 二、剩下的按无向对扇开
  const groups = new Map<string, GraphEdge[]>();
  for (const e of survivors) push(groups, pairKey(e.source, e.target), e);

  const placed: PlacedEdge[] = [];
  for (const group of groups.values()) {
    const n = group.length;
    group.forEach((e, i) => {
      // 围绕直线对称铺开：n=1 → [0]；n=2 → [-0.5, 0.5]；n=3 → [-1, 0, 1]
      const offset = n === 1 ? 0 : i - (n - 1) / 2;
      const sign = e.source < e.target ? 1 : -1;
      placed.push({
        edge: e,
        curvature: offset === 0 ? 0 : sign * offset * EDGE_CURVATURE_STEP,
        alsoLabels: also.get(e.id) ?? [],
      });
    });
  }
  return { edges: placed, folded };
}
// 呼吸周期。动画不是为了好看，是因为静态的一个色差在几百条边里根本注意不到
const DERIVED_PULSE_MS = 2200;
// 超过这个数就只上色不动画。**写出来而不是悄悄降级**：每帧重算几千条边的颜色，
// 换来的是拖不动图，而那时候用户要的是能拖得动
const DERIVED_ANIMATE_MAX = 400;
// 开关的淡入淡出时长。**比 FADE_MS(320) 略长**：播放淡入是一批边陆续到位，
// 这个是一整批边同时进出，走慢一点才看得清「那批金线是一起退场的」
const DERIVED_TOGGLE_MS = 420;
/* 推出来的边**比事实晚一点进来**，然后整体淡入。

   试过把推导过程演出来：前提按顺序点亮、最后点亮结论。做了两版都读不懂——
   第一版前提闪一下就灭，等结论出现时前提早暗了；第二版改成整组同亮同收，
   仍然是几十组在图上此起彼伏，谁属于谁根本分不出。
   **一张几百条边的图不是讲因果链的地方**——那件事侧栏的 Derived 页
   一条一条写着，看得清楚得多。这里只需要交代一件事：这些边是后来的、
   跟别人写下的不是一回事。晚一点进来 + 自己的颜色，已经说完了。 */
const DERIVE_SETTLE_MS = 500; // 事实落位之后，隔多久轮到推出来的
const DERIVE_FADE_MS = 620; // 整体淡入的时长，比开关那一档慢，是"入场"不是"切换"
// 图例最多摆几个胶囊，其余收进「+N 个类」。**这一排是横向排布的，
// 类一多就会换行、把画布顶到下面去**；而且十几个同样的胶囊排开，
// 谁也读不出哪个重要。收起来的那些从「+N」里搜得到
const LEGEND_MAX = 6;
/* 画多少个节点的可选档位。**给档位而不是给输入框**：这个数没有「精确」可言
   ——它只影响看得清还是拖得动，用户要的是「多点/少点」，不是 237 这个数。
   最大值与后端 GRAPH_NODE_CAP_MAX 对齐；再高先垮的是拖动，不是清晰度 */
const NODE_BUDGETS: number[] = [150, 300, 600, 1000];
/* 边色全部来自调色板（graphVisuals，读的是当前主题的令牌，0038）。
   幽灵边（没落地的派生）同一个色相往 EDGE_DIM 混——sigma 的边着色器在预乘混合
   下 alpha 压不暗边，暗度必须编码进 RGB——所以是函数，切主题后重算 */
const edgeGhost = () => lerpColor(EDGE_FOCUS_CONTEST, EDGE_DIM, 0.55);
const edgeGhostFocus = () => lerpColor(EDGE_FOCUS_CONTEST, EDGE_DIM, 0.2);
const DAY_MS = 24 * 3600 * 1000;

/* 播放淡入：解析 hex / rgb / rgba（含 alpha）并线性插值 */
/** 播放中新元素的淡入时长 */
const FADE_MS = 320;

export function Graph() {
  const kbId = useKbId();
  const { kb } = useKb();
  /* 地址栏与画面**双向**同步。
     从前只有"进"这一半：`?entity=` 在挂载时读一次就再不管了——
     别人给的链接能用，而你自己看到的东西却没法分享，因为地址栏一直停在
     光秃秃的 /graph。 */
  const search = useSearch({ from: "/app/kb/$kbId/graph" });
  const navigate = useNavigate();
  const entityParam = search.entity;
  // 主题一变，图要重构（颜色烤在属性里）：见构图 effect
  const [themeTick, setThemeTick] = useState(0);
  const [focusEntity, setFocusEntity] = useState<string | null>(
    search.focus ?? entityParam ?? null,
  );
  const [selected, setSelected] = useState<string | null>(entityParam ?? null);
  const [searchInput, setSearchInput] = useState("");
  const [searchQ, setSearchQ] = useState("");
  const [hiddenTypes, setHiddenTypes] = useState<Set<string>>(new Set());
  // 推出来的边显不显示。默认显示——推理默认关着，有派生就意味着用户开过开关
  const [showDerived, setShowDerived] = useState(true);
  // 信息窗默认收起：它答的是「什么时候推的」，那是偶尔才问的问题
  /* 画布上的两块浮层（Inference、「+N 个类」）与顶栏那三个面板同一副：
     shadcn 的 Popover。从前是手写的原地展开，面板第一行还要把触发它的那个
     胶囊再画一遍当关闭键；统一之后关闭归 Esc、外点与触发器本身 */
  const [derivedOpen, setDerivedOpen] = useState(false);
  const [legendOpen, setLegendOpen] = useState(false);
  const [legendQ, setLegendQ] = useState("");
  /* 正在退场的实体。**面板不能一取消选中就卸载**——那样它是瞬间消失的。
     先留在原地演完退场，再真的移除。用 selectedRef 取当前值而不是把
     setState 写成带副作用的 updater：那种写法在 StrictMode 下会跑两遍 */
  const [exiting, setExiting] = useState<string | null>(null);
  // 打开面板时想停在哪一档、展开哪一行——幽灵边点进来时用
  const panelIntentRef = useRef<{ view: "derived"; open: string } | null>(null);
  const deselect = useCallback(() => {
    const cur = selectedRef.current;
    if (!cur) return;
    setExiting(cur);
    setSelected(null);
    window.setTimeout(() => setExiting(null), 170);
  }, []);
  /** null = 全时段；数值 = as-of 时刻(ms)。
      默认 as-of 今天：时态平台的图谱默认呈现"现在的世界"，
      已闭合的事实不该与现行事实无差别并列（All time 是显式选择） */
  /* 时间轴。URL 里带了就用它：`all` = 全时段，否则按 YYYY-MM-DD 解析
     （与数据的 day 级精度一致，也比一串毫秒好读） */
  const [timeT, setTimeT] = useState<number | null>(() => {
    if (search.at === "all") return null;
    if (search.at) {
      const t = Date.parse(search.at);
      if (!Number.isNaN(t)) return t;
    }
    return Date.now();
  });
  const [activeCount, setActiveCount] = useState(0);
  const [stabilizing, setStabilizing] = useState(false);
  /* 播放态提升到此层：reducer 需区分"播放推进"（淡入）与"手动拖动"（瞬切） */
  const [playing, setPlaying] = useState(false);

  /* 画面 → 地址栏。**replace 不是 push**：点节点是浏览不是导航，
     堆进历史会把「后退」变成逐个撤销点击。
     播放中整段跳过——每帧写一次 URL 是灾难 */
  useEffect(() => {
    if (playing) return;
    const at =
      timeT === null
        ? "all"
        : // 停在「现在」就不写。否则每次打开都在地址栏拖一串今天的日期，
          // 而那本来就是默认值
          Math.abs(timeT - Date.now()) < DAY_MS
          ? undefined
          : new Date(timeT).toISOString().slice(0, 10);
    const next = {
      entity: selected ?? undefined,
      // **与 entity 相同就不写**：点搜索结果会同时设这两个，
      // 照直写出来地址栏里就是同一串 UUID 出现两遍。
      // 只有"聚焦在 A 的邻域、却选中了 B"时它才带信息
      focus:
        focusEntity && focusEntity !== selected ? focusEntity : undefined,
      at,
    };
    if (
      next.entity === search.entity &&
      next.focus === search.focus &&
      next.at === search.at
    )
      return;
    navigate({
      to: "/kb/$kbId/graph",
      params: { kbId },
      search: next,
      replace: true,
    });
  }, [
    selected,
    focusEntity,
    timeT,
    playing,
    search.entity,
    search.focus,
    search.at,
    navigate,
  ]);

  /* 地址栏 → 画面。**这一半是给后退/前进用的**：没有它，浏览器回退
     只改地址不改画面，看起来像后退失灵。两个方向都先比较再动手，所以不会打架 */
  useEffect(() => {
    const e = search.entity ?? null;
    const f = search.focus ?? null;
    setSelected((cur) => (cur === e ? cur : e));
    setFocusEntity((cur) => (cur === f ? cur : f));
  }, [search.entity, search.focus]);
  /* 布局模式：force = FA2 斥力；circular = 圆环；pack = 按类型圆填充聚簇 */
  type LayoutMode = "force" | "circular" | "pack";
  const [layoutMode, setLayoutMode] = useState<LayoutMode>("force");
  const layoutModeRef = useRef<LayoutMode>("force");
  const layoutCtlRef = useRef<{ apply: (m: LayoutMode) => void } | null>(null);

  /* 画多少个。**进 queryKey**——不进的话调了档位不会重新取数，
     界面看着变了实际还是老数据 */
  const [nodeBudget, setNodeBudget] = useState<number>(NODE_BUDGETS[0]);

  // 空状态给谁看：管理员能自己去配模型，其他人只能去找管理员。与 Shell 共用同一份缓存
  const me = useQuery({ queryKey: ["me"], queryFn: api.me });
  // 空状态说哪一句，取决于这个库走到哪一步了（#313）
  const readiness = useReadiness(kbId);
  const step = nextStep(readiness.data, {
    kbId,
    isAdmin: !!me.data?.is_admin,
    canUpload: kb?.my_role !== "viewer",
  });
  const data = useQuery({
    queryKey: ["graph", kb?.id, focusEntity, nodeBudget],
    queryFn: () =>
      focusEntity
        ? api.graphNeighborhood(kb!.id, focusEntity)
        : api.graphOverview(kb!.id, nodeBudget),
    enabled: !!kb,
  });

  // 全图模式走全库实体搜索；子图模式只在已加载的子图内客户端过滤
  const inSubgraph = !!focusEntity;
  // 搜到的条数上限。**「加载更多」而不是翻页**：这是个下拉建议框，
  // 用户在找一个具体的实体，翻页会让他丢掉刚才扫过的那几条
  const [searchLimit, setSearchLimit] = useState(10);
  useEffect(() => setSearchLimit(10), [searchQ]);
  const candidates = useQuery({
    queryKey: ["entitySearch", kb?.id, searchQ, searchLimit],
    queryFn: () => api.searchEntities(kb!.id, searchQ, searchLimit),
    enabled: !!kb && searchQ.length > 0 && !inSubgraph,
    placeholderData: (prev) => prev,
  });
  const subgraphHits = useMemo(() => {
    if (!inSubgraph || !searchQ || !data.data) return [];
    const q = searchQ.toLowerCase();
    return data.data.nodes
      .filter(
        (n) =>
          n.name.toLowerCase().includes(q) ||
          n.disambiguator?.toLowerCase().includes(q),
      )
      .slice(0, 10);
  }, [inSubgraph, searchQ, data.data]);
  const searchHits = inSubgraph
    ? subgraphHits
    : (candidates.data?.entities ?? []);

  const containerRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLCanvasElement>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  /* 焦点 = hover 优先于选中；样式在 reducer 里统一处理 */
  const selectedRef = useRef<string | null>(null);
  const hoverRef = useRef<string | null>(null);
  /** 鼠标停在哪条边上。用来把并进它的逆关系说法亮出来 */
  const hoverEdgeRef = useRef<string | null>(null);
  /** 近到什么程度算「贴脸看」：到了就每条边都写字（见 updateEdgeLabels） */
  const deepZoomRef = useRef(false);
  const filterRef = useRef<{
    hiddenTypes: Set<string>;
    activeNodes: Set<string> | null;
    activeEdges: Set<string> | null;
    /** 推出来的边显不显示。**默认显示**——推理默认是关的，所以有派生边就意味着
     *  用户主动开过开关；但要能一键藏起来，看「只有人说过的那张图」长什么样 */
    showDerived: boolean;
  }>({
    hiddenTypes: new Set(),
    activeNodes: null,
    activeEdges: null,
    showDerived: true,
  });
  const playingRef = useRef(false);
  /* 播放淡入表：本轮新激活的节点/边 id → 激活时刻（rAF 循环驱动至到位） */
  const fadeRef = useRef<Map<string, number>>(new Map());
  const fadeRafRef = useRef(0);

  const kickFade = useCallback(() => {
    if (fadeRafRef.current) return;
    const step = () => {
      const now = performance.now();
      for (const [id, start] of fadeRef.current)
        if (now - start >= FADE_MS) fadeRef.current.delete(id);
      sigmaRef.current?.refresh();
      fadeRafRef.current = fadeRef.current.size
        ? requestAnimationFrame(step)
        : 0;
    };
    fadeRafRef.current = requestAnimationFrame(step);
  }, []);

  useEffect(() => {
    playingRef.current = playing;
    if (!playing) {
      // 停止播放：未完成的淡入直接到位
      fadeRef.current.clear();
      sigmaRef.current?.refresh();
    }
  }, [playing]);

  useEffect(() => () => cancelAnimationFrame(fadeRafRef.current), []);

  const types = useMemo(() => {
    const map = new Map<
      string,
      { label: string; color: string; shape: string; count: number }
    >();
    for (const n of data.data?.nodes ?? []) {
      // 没判出类型的归到空 key 一档（0009）。真实 key 由 IRI 派生，不可能为空，
      // 所以它撞不着任何一个类；标签走 i18n，别把 null 画到图例上
      const key = n.type_key ?? "";
      const cur = map.get(key);
      if (cur) cur.count++;
      else
        map.set(key, {
          label: n.type_label ?? S.graph.untyped,
          color: n.color,
          shape: n.shape,
          count: 1,
        });
    }
    // **按出现次数排，不是按遇到的先后**。图例只摆得下几个，那几个位置该给
    // 画面上最多的类；从前是节点到达顺序，等于随机。次数相同按标签排——
    // 否则同样的数据每次刷新顺序都在抖
    return [...map.entries()].sort(
      (a, b) => b[1].count - a[1].count || a[1].label.localeCompare(b[1].label),
    );
  }, [data.data]);

  /* 摆得下的 / 收起来的。收起来的那些仍然可以在「+N」里搜到并切换 */
  const legendShown = types.slice(0, LEGEND_MAX);
  const legendRest = types.slice(LEGEND_MAX);
  // 被收起来的类里有没有正被隐藏的。**没有这个标记就是无声过滤**——
  // 在面板里关掉一个类、把面板一收，界面上再没有任何东西说它被关了
  const hiddenInRest = legendRest.filter(([k]) => hiddenTypes.has(k)).length;

  // 有几条推出来的边。**为零时那个开关整个不出现**——一个没开推理的库不该
  // 看到一个永远切换不出任何变化的按钮
  const derivedCount = useMemo(
    () => (data.data?.edges ?? []).filter((e) => e.derived).length,
    [data.data],
  );

  /* 时间过滤：计算 T 时刻的活跃边/节点集合 */
  const recomputeActive = useCallback(
    (t: number | null) => {
      const d = data.data;
      if (!d) return;
      if (t === null) {
        filterRef.current.activeNodes = null;
        filterRef.current.activeEdges = null;
        setActiveCount(d.edges.length);
      } else {
        const prevNodes = filterRef.current.activeNodes;
        const prevEdges = filterRef.current.activeEdges;
        const edges = new Set<string>();
        const nodes = new Set<string>();
        const touched = new Set<string>();
        for (const e of d.edges) {
          // 按**读出来的**区间过滤（0022）：没起点的边从最早的证据起才亮，结束了
          // 不知哪天的到说出它的那份文档就灭。NULL 一端才是开放——这里不再把
          // 「没有起点」读成「一直都在」
          const hf = e.holds_from ? Date.parse(e.holds_from) : null;
          const ht = e.holds_to ? Date.parse(e.holds_to) : null;
          touched.add(e.source);
          touched.add(e.target);
          const active = (hf === null || hf <= t) && (ht === null || ht > t);
          if (active) {
            edges.add(e.id);
            nodes.add(e.source);
            nodes.add(e.target);
          }
        }
        // 没有任何边的孤立节点保持可见
        for (const n of d.nodes) if (!touched.has(n.id)) nodes.add(n.id);
        // 播放推进时新出现的元素淡入登场；手动拖动保持瞬时切换
        if (playingRef.current) {
          const now = performance.now();
          for (const id of edges)
            if (prevEdges && !prevEdges.has(id)) fadeRef.current.set(id, now);
          for (const id of nodes)
            if (prevNodes && !prevNodes.has(id)) fadeRef.current.set(id, now);
          if (fadeRef.current.size) kickFade();
        }
        filterRef.current.activeNodes = nodes;
        filterRef.current.activeEdges = edges;
        setActiveCount(edges.size);
      }
      sigmaRef.current?.refresh();
    },
    [data.data, kickFade],
  );

  useEffect(() => {
    filterRef.current.hiddenTypes = hiddenTypes;
    filterRef.current.showDerived = showDerived;
    sigmaRef.current?.refresh();
  }, [hiddenTypes, showDerived]);

  const deriveRafRef = useRef(0);
  /* 演完之前派生边不出现。**开关是"要不要显示"，这个是"演到了没有"**——
     两件事，混成一个会让关掉再打开时少演一遍 */
  const [derivedRevealed, setDerivedRevealed] = useState(false);
  /* reducer 是每帧跑的闭包，读 state 会读到旧值——它只认 ref */
  const derivedRevealedRef = useRef(false);
  useEffect(() => {
    derivedRevealedRef.current = derivedRevealed;
    sigmaRef.current?.refresh();
  }, [derivedRevealed]);

  const revealDerived = useCallback(() => {
    setDerivedRevealed(true);
    // 复用开关那套淡入：方向为"开"，从近背景色亮到常态
    derivedToggleRef.current = { at: performance.now(), on: true };
    const step = () => {
      const tr = derivedToggleRef.current;
      const done = !tr || performance.now() - tr.at >= DERIVE_FADE_MS;
      if (done) derivedToggleRef.current = null;
      sigmaRef.current?.refresh();
      deriveRafRef.current = done ? 0 : requestAnimationFrame(step);
    };
    cancelAnimationFrame(deriveRafRef.current);
    deriveRafRef.current = requestAnimationFrame(step);
  }, []);

  useEffect(() => () => cancelAnimationFrame(deriveRafRef.current), []);

  /* 开关的淡入淡出：{ 起始时刻, 朝哪个方向 }；null = 没有过渡在飞 */
  const derivedToggleRef = useRef<{ at: number; on: boolean } | null>(null);
  const derivedRafRef = useRef(0);
  /* 上一次的开关值。**判「是不是真的切换了」只能靠它**——effect 的依赖里
     还有 derivedCount，而「Run now 推出新边」会改 count 却没碰开关；
     只看 effect 触发就淡一次，那是一次没人要求的动画 */
  const prevShowDerived = useRef(showDerived);

  // 切换时走一段渐变，而不是瞬间消失。**得自己驱动重绘**——关掉时下面那个
  // 呼吸定时器不转了，没人推 sigma 重画，淡出就会卡在第一帧
  useEffect(() => {
    const changed = prevShowDerived.current !== showDerived;
    prevShowDerived.current = showDerived;
    // 首次挂载与「只有 count 变了」都不是切换：
    // 进页面时、以及推理跑完刷新计数时，都不该看到一段莫名其妙的淡入
    if (!changed) return;
    // 数量太多时不淡：与呼吸同一条线——每帧重算几千条边的颜色换来的是卡顿。
    // **写出来而不是悄悄降级**
    if (derivedCount > DERIVED_ANIMATE_MAX) return;

    const now = performance.now();
    const prev = derivedToggleRef.current;
    // 半途反向（用户连点两下）：从当前进度接着走，而不是从头开始——
    // 否则会看见一次亮度的跳变
    const at =
      prev && prev.on !== showDerived
        ? now - Math.max(0, DERIVED_TOGGLE_MS - (now - prev.at))
        : now;
    derivedToggleRef.current = { at, on: showDerived };

    const step = () => {
      const tr = derivedToggleRef.current;
      const done = !tr || performance.now() - tr.at >= DERIVED_TOGGLE_MS;
      if (done) derivedToggleRef.current = null;
      sigmaRef.current?.refresh();
      derivedRafRef.current = done ? 0 : requestAnimationFrame(step);
    };
    cancelAnimationFrame(derivedRafRef.current);
    derivedRafRef.current = requestAnimationFrame(step);
    // **不在这里挂清理**：清理会在依赖变化时也跑一遍，而依赖里有 derivedCount
    // ——推理恰好在这 420ms 中途跑完，动画就被掐在半路（画面停在一半亮度，
    // 要等下一次任意重绘才归位）。循环自己会终止；取消只该发生在卸载时
  }, [showDerived, derivedCount]);

  // 卸载时收掉可能在飞的那一帧
  useEffect(() => () => cancelAnimationFrame(derivedRafRef.current), []);

  /* 什么时候进场。两个入口共用一段延时：进页面、以及手动打开开关。
     **不等布局收敛**——收敛要 2.5 秒，等完人早就在看别处了。

     **顺序本身是内容**：先落位的是人写下的边，然后才轮到推出来的。
     一起出现就分不清谁在前 */
  useEffect(() => {
    if (!showDerived || !data.data) {
      if (!showDerived) setDerivedRevealed(false);
      return;
    }
    if (derivedRevealed) return;
    const t = window.setTimeout(revealDerived, DERIVE_SETTLE_MS);
    return () => window.clearTimeout(t);
  }, [showDerived, data.data, derivedRevealed, revealDerived]);

  // 派生边的呼吸。**只在有派生边、且开着显示、且数量不多时才转**——
  // 一个没开推理的库不该为这件事每两秒重画一次
  useEffect(() => {
    const n = derivedCount;
    if (!showDerived || n === 0 || n > DERIVED_ANIMATE_MAX) return;
    // 与 sigma 的重绘同频即可，不必每帧：呼吸是慢动作，30 fps 看不出差别
    const timer = setInterval(() => sigmaRef.current?.refresh(), 1000 / 30);
    return () => clearInterval(timer);
  }, [showDerived, derivedCount]);

  useEffect(() => {
    selectedRef.current = selected;
    sigmaRef.current?.refresh();
  }, [selected]);

  useEffect(() => {
    recomputeActive(timeT);
  }, [timeT, recomputeActive]);

  useEffect(() => {
    // 节点壳色、边色是**烤进图属性**的，不是渲染时才取；所以构图前先把调色板
    // 读成当前主题的值，切主题时靠 themeTick 让这里重跑一遍（0038）
    refreshPalette();
    if (!containerRef.current || !data.data) return;
    const g = new Graphology({ multi: true });
    for (const n of data.data.nodes) {
      if (!g.hasNode(n.id)) {
        g.addNode(n.id, {
          label: n.name,
          // Semantica 配方：深壳 + 14% 类型 tint，核心 50% tint，钢灰描边微 tint
          color: mix(NODE_CORE_BASE, n.color, NODE_CORE_MIX),
          shellColor: mix(NODE_SHELL_BASE, n.color, NODE_TINT_MIX),
          borderColor: mix(NODE_BORDER_BASE, n.color, 0.3),
          ringColor: TRANSPARENT,
          typeColor: n.color,
          typeLabel: n.type_label ?? S.graph.untyped,
          typeKey: n.type_key ?? "",
          type: n.shape === "square" ? NODE_TYPE_SQUARE : NODE_TYPE_SHELL,
          size: 5 + Math.min(8, Math.sqrt(Number(n.degree)) * 1.6),
        });
      }
    }
    const placed = layOutParallelEdges(
      data.data.edges.filter((e) => g.hasNode(e.source) && g.hasNode(e.target)),
    );
    for (const { edge: e, curvature, alsoLabels } of placed.edges) {
      g.addEdgeWithKey(e.id, e.source, e.target, {
        /* 争议的边标签前置 ⚠：颜色之外再给一个不靠色觉的记号。
           **谓语照本体里写的样子显示**，不再大写——界面上没有一处大写拉字距，
           而本体页列的就是「part of」这个原样 */
        label: (e.contested ? "⚠ " : "") + (e.label ?? ""),
        size: e.blocked ? 0.7 : 1,
        color: e.blocked
          ? edgeGhost()
          : e.contested
            ? EDGE_CONTEST
            : e.derived
              ? EDGE_DERIVED
              : e.inferred
                ? EDGE_INFERRED
                : EDGE,
        contested: e.contested,
        blocked: e.blocked,
        // 独一条就走直线：曲线是为了把重叠分开，没有重叠就不必弯
        type: curvature === 0 ? "line" : "curved",
        curvature,
        // 并进来的逆关系说法，悬停时连着本名一起显示
        alsoLabels,
        // reducer 每帧读它：决定要不要藏、要不要呼吸
        derived: e.derived,
      });
    }
    // 布局：先静态铺开，再用 worker 动画稳定 ~2.5s（Semantica 式 stabilizing）
    let fa2: InstanceType<typeof FA2Layout> | null = null;
    let stabilizeTimer: ReturnType<typeof setTimeout> | null = null;
    // 拖拽状态先于 fa2 声明：outputReducer 闭包引用它们
    let dragged: string | null = null;
    let dragPos: { x: number; y: number } | null = null;
    let fa2Settings: ReturnType<typeof forceAtlas2.inferSettings> | null = null;
    if (g.order > 0) {
      circular.assign(g, { scale: 300 });
      /* 试过按规模缩放（gravity 0.12–0.22 / scalingRatio 11–16 + 加大阻尼），
         拿真实的图一看就否了：散是散开了，但那种"被推开"的张力没了，
         整张图显得瘫。**这一组是既有的、刻意偏大的**——要的是节点之间
         互相顶着的感觉，不是最省力的排布 */
      const settings = {
        ...forceAtlas2.inferSettings(g),
        gravity: 0.35,
        scalingRatio: 22,
        outboundAttractionDistribution: true,
      };
      fa2Settings = settings;
      forceAtlas2.assign(g, { iterations: 60, settings });
      fa2 = new FA2Layout(g, {
        settings,
        // 关键：回写时把被拖节点钉回光标（不闪）；且提供 outputReducer 后
        // supervisor 每帧 readGraphPositions —— 光标位置持续进入力模拟
        outputReducer: (node, attr) => {
          if (dragged && node === dragged && dragPos) {
            attr.x = dragPos.x;
            attr.y = dragPos.y;
          }
          return attr;
        },
      });
      fa2.start();
      setStabilizing(true);
      stabilizeTimer = setTimeout(() => {
        fa2?.stop();
        setStabilizing(false);
      }, 2500);
    }

    // 数据重建后布局回到 force（世界重新长出来）
    setLayoutMode("force");
    layoutModeRef.current = "force";

    // 任意布局结果统一缩放到 FA2 同量级世界（±target），相机 reset 观感一致
    const rescaleWorld = (target = 300) => {
      let minX = Infinity,
        maxX = -Infinity,
        minY = Infinity,
        maxY = -Infinity;
      g.forEachNode((_n, a) => {
        minX = Math.min(minX, a.x as number);
        maxX = Math.max(maxX, a.x as number);
        minY = Math.min(minY, a.y as number);
        maxY = Math.max(maxY, a.y as number);
      });
      const span = Math.max(maxX - minX, maxY - minY) || 1;
      const k = (target * 2) / span;
      const cx = (minX + maxX) / 2;
      const cy = (minY + maxY) / 2;
      g.updateEachNodeAttributes((_n, a) => ({
        ...a,
        x: (a.x - cx) * k,
        y: (a.y - cy) * k,
      }));
    };

    // 布局切换控制（挂到 ref 供组件层按钮调用；闭包内直握 g / fa2）
    layoutCtlRef.current = {
      apply: (mode) => {
        if (g.order === 0) return;
        if (stabilizeTimer) clearTimeout(stabilizeTimer);
        fa2?.stop();
        setStabilizing(false);
        if (mode === "force") {
          forceAtlas2.assign(g, {
            iterations: 60,
            settings: fa2Settings ?? undefined,
          });
          fa2?.start();
          setStabilizing(true);
          stabilizeTimer = setTimeout(() => {
            fa2?.stop();
            setStabilizing(false);
          }, 2500);
        } else if (mode === "circular") {
          circular.assign(g, { scale: 300 });
        } else {
          // 按实体类型聚簇：同类型挤进同一个圆
          circlepack.assign(g, { hierarchyAttributes: ["typeKey"] });
          rescaleWorld(300);
        }
        sigma.setCustomBBox(null);
        sigma.refresh();
        sigma.getCamera().animatedReset({ duration: 300 });
      },
    };

    /* **此刻还连着吗**。
     *
     * `areNeighbors` 只问图上有没有这条边，不问时间轴停的这一刻它还成不成立；
     * 边那边是问的（见 `liveNow`）。两边不一致，画出来就是「节点亮着、线却
     * 没有」：悬停 Google DeepMind，Arthur Mensch 亮着，可它那条
     * former_employee_of 早就结束了，线被压回背景色——看着像凭空亮了一个。
     * 派生边关掉时同理：那条边整条不画，另一头就不该还当邻居亮着。
     *
     * **按焦点节点缓存一份**：这个判断每帧要对每个节点问一次，而枢纽点动辄
     * 几百条边，逐节点扫一遍度数太亏。时间轴每动一次 `activeEdges` 都是新的
     * Set（见 `recomputeActive`），拿它的身份当键就够，再带上边数兜住图本身
     * 被换掉的情况 */
    const liveNbr = {
      focus: "",
      edges: null as Set<string> | null,
      derived: true,
      size: -1,
      set: new Set<string>(),
    };
    const isLiveNeighbor = (focus: string, node: string) => {
      const { activeEdges, showDerived } = filterRef.current;
      if (
        liveNbr.focus !== focus ||
        liveNbr.edges !== activeEdges ||
        liveNbr.derived !== showDerived ||
        liveNbr.size !== g.size
      ) {
        const set = new Set<string>();
        g.forEachEdge(focus, (e, attrs, src, tgt) => {
          if (activeEdges && !activeEdges.has(e)) return;
          if (!showDerived && attrs.derived === true) return;
          set.add(src === focus ? tgt : src);
        });
        liveNbr.focus = focus;
        liveNbr.edges = activeEdges;
        liveNbr.derived = showDerived;
        liveNbr.size = g.size;
        liveNbr.set = set;
      }
      return liveNbr.set.has(node);
    };

    sigmaRef.current?.kill();
    // 画布颜色从令牌读（0038）：建实例前读一次，切主题后再读一次并重画
    refreshPalette();
    const sigma = new Sigma(g, containerRef.current, {
      ...sigmaOptions({
        defaultEdgeType: "line",
        /* 平行边扇成弧（见 `layOutParallelEdges`）。直线那一版把同一对节点
           之间的每条边画在同一条线段上，于是几个标签逐字符叠成乱码——实测
           一对节点之间最多压着六条 */
        // 每个程序配一份「最上层」的（见 `withTopLayer`）：高亮的边整批最后画
        edgeProgramClasses: withTopLayer({
          line: EdgeRectangleProgram,
          curved: EdgeCurveProgram,
        }),
        // 边上写的是谓词，近距离下每条画得出来的边都写（见 updateEdgeLabels）
        renderEdgeLabels: true,
        // 上千个节点，得缩得比本体页更远才看得见全貌
        minCameraRatio: 0.04,
        maxCameraRatio: 8,
      }),
      nodeReducer: (node, attrs) => {
        const f = filterRef.current;
        const res = { ...attrs };
        const base = attrs.size as number;
        if (f.hiddenTypes.has(attrs.typeKey as string)) {
          res.hidden = true;
          return res;
        }
        const hov = hoverRef.current;
        // 选中实体可能不在当前画布（侧栏跳转/邻域重载间隙）——不在则跳过聚焦压暗逻辑
        const sel =
          selectedRef.current && g.hasNode(selectedRef.current)
            ? selectedRef.current
            : null;
        /* **选中压过指到**。这两句从前是反的：指针一落到自己刚选中的那个节点上，
           它就改画 hover 那一副，选中的记号（反色底牌、加粗的名字）当场消失——
           而人把指针移过去，往往正因为那是他选的那一个。
           指到别的节点仍然照常出 hover */
        if (sel === node) {
          const picked = selectedNode(res, attrs, base);
          // 选中的这一个同时被指着：名字改由高亮层画，标签层让开
          return hov === node ? deferToHoverLayer(picked) : picked;
        }
        if (hov === node) return hoveredNode(res, attrs, base);
        if (sel) {
          // 邻居收到 0.76：上千个节点，得给选中的那一条路让地方
          if (isLiveNeighbor(sel, node)) neighborNode(res, base, 0.76);
          /* **指到谁，就把谁的邻居也留出来**。边那边悬停是压过选中的
             （`boost()` 在最前面），节点这边不跟上，选中之后再指别处就成了
             一圈亮着的边通向一圈黑着的点——指过去正是想读"它连着谁"，
             而那一问什么也答不出来 */
          else if (hov && isLiveNeighbor(hov, node))
            neighborNode(res, base, 0.76);
          else return mutedNode(res, base);
        } else if (hov && hov !== node && !isLiveNeighbor(hov, node)) {
          // **悬停也压暗其余**，只是比选中轻一档（见 HOVER_MUTE）。
          // 邻居留着：悬停要回答的正是"它连着谁"。
          // **此刻还不存在的节点直接压到底**：这个分支会提前 return，
          // 绕过下面那道时间过滤，只压一半的话它反而比不 hover 时更亮
          if (f.activeNodes && !f.activeNodes.has(node))
            return mutedNode(res, base);
          return softMutedNode(res, attrs, base);
        } else {
          // default {×0.7}
          res.size = base * 0.7;
        }
        if (f.activeNodes && !f.activeNodes.has(node))
          return mutedNode(res, base);
        // 播放淡入：从 muted 形态渐变到本帧算出的正常形态
        const fs = fadeRef.current.get(node);
        if (fs !== undefined) {
          const t = Math.min(1, (performance.now() - fs) / FADE_MS);
          res.size = (res.size as number) * (0.55 + 0.45 * t);
          res.color = lerpColor(
            MUTED_SHELL,
            String(res.color ?? NODE_CORE_BASE),
            t,
          );
          res.shellColor = lerpColor(
            MUTED_SHELL,
            String(res.shellColor ?? NODE_SHELL_BASE),
            t,
          );
          res.borderColor = lerpColor(
            "rgba(0,0,0,0)",
            String(res.borderColor ?? NODE_BORDER_BASE),
            t,
          );
          if (t < 0.7) res.label = "";
        }
        return res;
      },
      edgeReducer: (edge, attrs) => {
        const f = filterRef.current;
        const res = { ...attrs };
        const [s, t] = g.extremities(edge);
        // 近距离下每条画得出来的边都写字（见 updateEdgeLabels）
        if (deepZoomRef.current) res.forceLabel = true;
        /* 并进这条边的逆关系说法，接在本名后面：`PART OF ⁻¹ CONTAINS`。
           **只在关注它的时候显示**——常驻会把标签拉长一倍，而标签太长
           正是这次要治的毛病。
           两个触发点，因为**边只有一像素宽，精确悬停对人也很难命中**：
           鼠标压在这条边上，或者压在它两端任一个节点上。后者才是实际
           用得上的那个，前者留着是因为有时人就是要指那一条。
           放在隐藏/压暗逻辑之前：说法是显示的事，不是可见性的事 */
        const also = attrs.alsoLabels as string[] | undefined;
        if (also && also.length > 0) {
          const focused =
            edge === hoverEdgeRef.current ||
            hoverRef.current === s ||
            hoverRef.current === t ||
            selectedRef.current === s ||
            selectedRef.current === t;
          if (focused) {
            res.label = `${attrs.label} ⁻¹ ${also.join(" / ")}`;
          }
        }
        const sk = g.getNodeAttribute(s, "typeKey") as string;
        const tk = g.getNodeAttribute(t, "typeKey") as string;
        if (f.hiddenTypes.has(sk) || f.hiddenTypes.has(tk)) {
          res.hidden = true;
          return res;
        }
        // 推出来的边：先看藏不藏，再决定呼吸到哪一档。
        // **放在最前面**——藏起来的边不必再算后面那些提亮/压暗
        const isDerived = attrs.derived === true;

        if (isDerived) {
          // 还没轮到它进场：先不画。**事实先落位，推出来的后到**
          if (!derivedRevealedRef.current) {
            res.hidden = true;
            return res;
          }
          const tr = derivedToggleRef.current;
          const k = tr
            ? Math.min(1, (performance.now() - tr.at) / DERIVED_TOGGLE_MS)
            : 1;
          // 关掉了：只有「淡出尚未走完」这一种情况还留着不藏
          if (!f.showDerived) {
            if (!tr || tr.on || k >= 1) {
              res.hidden = true;
              return res;
            }
            /* 由当前颜色渐灭到近背景色。**暗度必须编码进 RGB**
               （见 EDGE_DIM 处的注释：预乘混合下 alpha 压不暗边），
               所以是往 EDGE_DIM 混而不是降 alpha。

               **起点不能一律写死成满亮的金**：这个分支在悬停/选中的压暗逻辑
               之前就 return 了，于是一条本来被压成暗色的无关派生边，
               会先跳回满亮再淡出——那一跳就是"关派生时无关的边闪一下"。
               起点得取它此刻本来的样子 */
            const selNow =
              selectedRef.current && g.hasNode(selectedRef.current)
                ? selectedRef.current
                : null;
            const hovNow = hoverRef.current;
            const focused = selNow ?? hovNow;
            const ghost = attrs.blocked === true;
            const from = !focused
              ? ghost
                ? edgeGhost()
                : EDGE_DERIVED
              : s === focused || t === focused
                ? ghost
                  ? edgeGhostFocus()
                  : EDGE_FOCUS_DERIVED
                : EDGE_DIM;
            res.color = lerpColor(from, EDGE_DIM, k);
            res.label = "";
            return res;
          }
          // 幽灵边不呼吸：它不是知识，是一条没走通的路
          const pulse = attrs.blocked === true
            ? edgeGhost()
            : lerpColor(
            EDGE_DERIVED_DIM,
            EDGE_DERIVED,
            // 三角波而不是正弦：两端各停一瞬，看起来是「呼吸」不是「闪」
            Math.abs(
              ((performance.now() % DERIVED_PULSE_MS) / DERIVED_PULSE_MS) * 2 -
                1,
            ),
          );
          // 打开：从近背景色亮起来，接上呼吸
          res.color =
            tr && tr.on && k < 1 ? lerpColor(EDGE_DIM, pulse, k) : pulse;
        }
        // hover: 只提亮关联边；selected: 提亮关联边 + 压暗其余
        const hov = hoverRef.current;
        const sel =
          selectedRef.current && g.hasNode(selectedRef.current)
            ? selectedRef.current
            : null;
        const boost = () => {
          res.color =
            attrs.blocked === true
              ? edgeGhostFocus()
              : attrs.contested === true
                ? EDGE_FOCUS_CONTEST
                : isDerived
                  ? EDGE_FOCUS_DERIVED
                  : EDGE_FOCUS;
          res.size = Math.max((attrs.size as number) * 1.42, 1.85);
          res.zIndex = 5;
          // 换到最后画的那批（见 `edgeProgramClasses`）：光有 zIndex 压不住别的程序
          res.type = drawLast(String(res.type ?? "line"));
        };
        /* **时间轴停在某一刻时，这条边此刻存不存在**。
           悬停的两条分支都会提前 return，绕过下面那道时间过滤——
           不带上它的话，一 hover，所有"还没长出来"的边会从近背景色
           跳到常态色的 45%，看起来是被点亮了。实测就是这么亮的 */
        const liveNow = !f.activeEdges || f.activeEdges.has(edge);
        if (hov && (s === hov || t === hov) && liveNow) {
          boost();
        } else if (hov && !sel) {
          // 悬停时其余的边也退下去，但**只退一半**——与节点那边同一个 HOVER_MUTE。
          // 压到底是选中才有的待遇。此刻不存在的边**本来就该是暗的**，
          // 从 EDGE_DIM 起混等于原地不动
          const from = liveNow ? String(res.color) : EDGE_DIM;
          res.color = lerpColor(from, EDGE_DIM, HOVER_MUTE);
          res.size = (attrs.size as number) * (1 - 0.4 * HOVER_MUTE);
          res.label = "";
          return res;
        } else if (sel) {
          if (s === sel || t === sel) {
            boost();
          } else {
            res.color = EDGE_DIM;
            res.size = (attrs.size as number) * 0.6;
            res.label = "";
            return res;
          }
        }
        if (f.activeEdges && !f.activeEdges.has(edge)) {
          res.color = EDGE_DIM;
          /* **压暗了就退出最上层那批**（见 `boost()`）。这一档在 `boost()`
             之后：一条挨着选中项、但此刻时间轴上还不存在的边，颜色已经被压
             回背景色，却还留着高亮时换上的置顶程序——那就成了一条画在所有
             高亮边之上的暗线，正好把它们切断 */
          res.type = attrs.type as string;
          res.label = "";
          return res;
        }
        // 播放淡入：边从近背景色渐亮到常规色（alpha 同步插值）
        const fs = fadeRef.current.get(edge);
        if (fs !== undefined) {
          const t = Math.min(1, (performance.now() - fs) / FADE_MS);
          res.color = lerpColor(EDGE_DIM, String(res.color), t);
          if (t < 0.8) res.label = "";
        }
        return res;
      },
    });
    sigma.on("clickNode", ({ node }) => setSelected(node));
    // 幽灵边点一下：打开它主语的面板，停在「推出来的」那一档、展开那一行（0017 §3）
    sigma.on("clickEdge", ({ edge }) => {
      if (g.getEdgeAttribute(edge, "blocked") !== true) return;
      const [s] = g.extremities(edge);
      panelIntentRef.current = { view: "derived", open: edge };
      setSelected(s);
    });
    sigma.on("doubleClickNode", ({ node, event }) => {
      event.preventSigmaDefault();
      setFocusEntity(node);
      setSelected(node);
    });
    const offTheme = onThemeChange(() => {
      refreshPalette();
      setThemeTick((t) => t + 1);
      sigma.refresh();
      if (gridRef.current) drawWorldGrid(gridRef.current, sigma);
    });
    sigma.on("clickStage", () => deselect());
    sigma.on("enterNode", ({ node }) => {
      hoverRef.current = node;
      sigma.refresh();
    });
    sigma.on("leaveNode", () => {
      hoverRef.current = null;
      sigma.refresh();
    });
    /* 悬停一条边，把并进来的说法亮出来。
       逆关系的边被并掉了（见 `layOutParallelEdges`），少画一条是对的，
       但那个名字不该就此消失：`part_of` 反过来叫 `contains` 是本体里
       写着的东西，人有权看见。**只在悬停时显示**——常驻会把标签拉长一倍，
       而拉长标签正是这次要治的毛病 */
    sigma.on("enterEdge", ({ edge }) => {
      hoverEdgeRef.current = edge;
      sigma.refresh();
    });
    sigma.on("leaveEdge", () => {
      hoverEdgeRef.current = null;
      sigma.refresh();
    });
    /* 边标签只在放大后出现（默认视距下太密，Semantica 同样克制）。
       **再近一档就改成"看得见的线都写字"**：sigma 挑边标签的规矩是
       「两端的节点名都在显示，才写这条边」（`edgeLabelsToDisplayFromNodes`），
       而放大之后两端常常都在视口外，于是屏幕当中那条线反倒没有说法——
       正是想看清一条关系的时候它消失了。`forceLabel` 是 sigma 留的后门，
       打上就绕开那条启发式 */
    const updateEdgeLabels = () => {
      const ratio = sigma.getCamera().ratio;
      // 边少的图（≤ 40 条）始终写谓词：标签藏起来是为了大图不挤，
      // 一个十几条边的库进来 ratio = 1，藏了只会让人以为谓词没了
      sigma.setSetting("renderEdgeLabels", ratio < 0.7 || g.size <= 40);
      const deep = ratio < 0.35;
      if (deep !== deepZoomRef.current) {
        deepZoomRef.current = deep;
        sigma.refresh({ skipIndexation: true });
      }
    };
    sigma.getCamera().on("updated", updateEdgeLabels);
    updateEdgeLabels();

    // 世界坐标网格：相机变动/容器尺寸变动时重绘
    const renderGrid = () => {
      if (gridRef.current) drawWorldGrid(gridRef.current, sigma);
    };
    sigma.getCamera().on("updated", renderGrid);
    sigma.on("resize", renderGrid);
    renderGrid();

    // 节点拖拽 + 活的力导反馈。按下只记候选：视口位移 >4px 才升格为拖拽
    //（否则纯点选也会误启 FA2）；被拖节点由 fa2 的 outputReducer 钉在光标上（见上），
    // 松手后稳定 ~1.2s 停机
    let settleTimer: ReturnType<typeof setTimeout> | null = null;
    // 阈值、包围盒冻结那一套在 attachDrag 里；这里只接三个当口。
    // `dragged` 仍留在这个闭包里——FA2 的 outputReducer 每帧读它，
    // 把被拖的那个钉回光标（见上面 fa2 的构造）
    attachDrag(sigma, {
      onStart: (node) => {
        dragged = node;
        if (settleTimer) clearTimeout(settleTimer);
        // 静态布局（circular/pack）下拖拽不唤醒力模拟——否则一碰就散架
        if (layoutModeRef.current === "force" && fa2 && !fa2.isRunning())
          fa2.start();
      },
      onMove: (_node, pos) => {
        dragPos = pos;
      },
      onEnd: () => {
        dragged = null;
        dragPos = null;
        settleTimer = setTimeout(() => fa2?.stop(), 1200);
      },
    });
    sigmaRef.current = sigma;
    if (import.meta.env.DEV) {
      // 调试句柄（仅 dev）：无头环境下检查 reducer 输出
      (window as unknown as Record<string, unknown>).__g = g;
      (window as unknown as Record<string, unknown>).__sigma = sigma;
      (window as unknown as Record<string, unknown>).__sel = selectedRef;
    }
    recomputeActive(timeT);
    return () => {
      offTheme();

      if (stabilizeTimer) clearTimeout(stabilizeTimer);
      if (settleTimer) clearTimeout(settleTimer);
      fa2?.kill();
      setStabilizing(false);
      sigma.kill();
      sigmaRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data.data, themeTick]);

  if (!kb)
    return <div className="p-8 text-body text-ink-2">{S.nav.loading}</div>;

  const empty = data.isSuccess && data.data.nodes.length === 0;
  const nodeCount = data.data?.nodes.length ?? 0;
  const edgeCount = data.data?.edges.length ?? 0;
  // 库里一共有多少。**与画上去的不是一回事**——邻域视图没有总数（它本来就只
  // 是一小片），所以缺省回落到画上去的那个数，不会显示成「共 0 个」
  const totalNodes = data.data?.total_nodes ?? nodeCount;
  const totalEdges = data.data?.total_edges ?? edgeCount;
  const capped = totalNodes > nodeCount;

  return (
    <div className="h-full relative">
      {/* 顶部悬浮条：搜索 + 图例 + 状态 */}
      <div className="absolute top-3 left-3 right-3 z-10 flex items-start gap-2 pointer-events-none">
        <div className="relative pointer-events-auto">
          {/* 与本体页左栏的过滤框同一副身材、同一个角落（见 Ontology.tsx） */}
          <Input
            icon={<Search size={12} />}
            className="w-58 u-lift"
            placeholder={
              inSubgraph ? S.graph.searchInSubgraph : S.graph.searchEntity
            }
            value={searchInput}
            onChange={(e) => {
              setSearchInput(e.target.value);
              setSearchQ(e.target.value.trim());
            }}
          />
          {searchQ && searchHits.length > 0 && (
            <div className="glass-strong absolute mt-1 w-full rounded-overlay u-lift-strong overflow-hidden">
              {searchHits.map((c) => (
                <Row
                  key={c.id}
                  trailing={c.type_label}
                  onClick={() => {
                    // 子图内命中：只选中（已在视野里）；全图搜索：跳到该实体邻域
                    if (!inSubgraph) setFocusEntity(c.id);
                    setSelected(c.id);
                    setSearchInput("");
                    setSearchQ("");
                  }}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <span
                      className="h-2.5 w-2.5 shrink-0 rounded-full"
                      style={{ background: c.color }}
                    />
                    {/* **名字最后才让**：两段都是 truncate 的话，flex 按各自
                        的宽度一起收，一个长一点的消歧词能把名字挤成「Ac…」。
                        消歧词收得快四倍、且最多占四成——它是补充，名字才是
                        这一行要读的东西 */}
                    <span className="truncate">{c.name}</span>
                    {c.disambiguator && c.disambiguator !== c.type_label && (
                      <span className="min-w-0 max-w-[40%] shrink-[4] truncate text-small text-ink-2">
                        · {c.disambiguator}
                      </span>
                    )}
                  </span>
                </Row>
              ))}
              {/* 还有更多没显示。**说清剩多少**——从前固定十条，想找的那个
                  不在这十条里的时候，界面上一点线索都没有。子图内搜索是客户端
                  过滤，没有「更多」这回事 */}
              {!inSubgraph &&
                (candidates.data?.total ?? 0) > searchHits.length && (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="w-full justify-start rounded-none border-t border-line"
                    onClick={() => setSearchLimit((n) => n + 20)}
                  >
                    {S.graph.searchMore(
                      candidates.data!.total - searchHits.length,
                    )}
                  </Button>
                )}
            </div>
          )}
        </div>
        {focusEntity && (
          <Button
            variant="secondary"
            className="glass-strong pointer-events-auto u-lift"
            onClick={() => setFocusEntity(null)}
          >
            {S.graph.backToOverview}
          </Button>
        )}

        {/* 图例（点击切换类型显隐）。**只摆前 LEGEND_MAX 个**，其余收进
            「+N 个类」——那一排横着长，类一多就换行把画布顶下去；而且十几个
            一模一样的胶囊排开，谁重要也读不出来 */}
        {/* 与搜索框顶齐：药丸和输入框都是 32 高，这里再垫 4 就矮一截 */}
        <div className="pointer-events-auto flex flex-wrap gap-2">
          {legendShown.map(([key, t]) => (
            <Pill
              key={key}
              dim={hiddenTypes.has(key)}
              onClick={() =>
                setHiddenTypes((prev) => {
                  const next = new Set(prev);
                  if (next.has(key)) next.delete(key);
                  else next.add(key);
                  return next;
                })
              }
            >
              <span
                className={`h-2 w-2 ${t.shape === "square" ? "scale-90" : "rounded-full"}`}
                style={{ background: t.color }}
              />
              <span>{t.label}</span>
            </Pill>
          ))}

          {/* chip 上的数是**全部类**，不是被收起来的那几个——
              点开看到的就是全部（搜得到任何一个），写「+3」等于承诺了另一件事 */}

          {legendRest.length > 0 && (
            <Popover open={legendOpen} onOpenChange={setLegendOpen}>
              <PopoverTrigger asChild>
              <Pill active={legendOpen} title={S.graph.legendAllHint}>
                {S.graph.legendMore(types.length)}
                {/* 收起来的类里有正被隐藏的就点一下。**不点就是无声过滤**：
                    在面板里关掉一个类、把面板一收，界面上再没有任何东西说它被关了 */}
                {hiddenInRest > 0 && (
                  <span className="h-1.5 w-1.5 rounded-full bg-ink-2" />
                )}
              </Pill>
              </PopoverTrigger>
              <PopoverContent align="start" className="w-72 overflow-hidden p-0">
                  {/* 标题行只说这是什么，不再兼任关闭键 */}
                  <div className="flex items-center gap-3 border-b border-line px-4 py-3">
                    <span className="min-w-0 flex-1 truncate text-body font-medium text-ink">
                      {S.graph.legendMore(types.length)}
                    </span>
                  </div>
                  {/* 全开 / 全关。**从顶栏那枚独立胶囊搬进来的**：它只在有隐藏时
                      才出现，于是那一排的宽度会随着你点类跳来跳去；而它要做的事
                      («把画面收窄»的反面）本就属于这份清单，不属于清单外面。
                      两个都常驻、不可用时置灰——一个会消失的出口，第二次要用时
                      得先想起它长在哪儿 */}
                  <div className="flex items-center gap-2 border-b border-line px-4 py-2">
                    <LinkButton
                      disabled={hiddenTypes.size === 0}
                      onClick={() => setHiddenTypes(new Set())}
                    >
                      {S.graph.legendShowAll(hiddenTypes.size)}
                    </LinkButton>
                    <LinkButton
                      className="ml-auto"
                      disabled={hiddenTypes.size === types.length}
                      onClick={() =>
                        setHiddenTypes(new Set(types.map(([k]) => k)))
                      }
                    >
                      {S.graph.legendHideAll}
                    </LinkButton>
                  </div>
                  {/* 查找：没有自己的框（bare）——它是面板的一段，不是面板里
                      摆的一个控件，与库切换器的查找同一个做法 */}
                  <div className="border-b border-line px-4 py-3">
                    <Input
                      bare
                      autoFocus
                      value={legendQ}
                      onChange={(e) => setLegendQ(e.target.value)}
                      placeholder={S.graph.legendSearch}
                      className="w-full text-body"
                    />
                  </div>
                  {/* **列的是全部类，不只是收起来的那些**：想找一个类的时候，
                      没人记得它是不是恰好排进了前几个 */}
                  <div className="u-scroll flex max-h-64 flex-col overflow-y-auto px-2 py-1">
                    {types
                      .filter(([, t]) =>
                        t.label.toLowerCase().includes(legendQ.toLowerCase()),
                      )
                      .map(([key, t]) => (
                        /* **一行两个按钮，不是一个按钮循环三态。**
                           单键循环的代价是：不看当前状态就不知道下一次点击
                           会发生什么，而且从「只看」回到正常必须路过「排除」
                           ——想清空却得先让画面变成另一个错的样子。
                           拆开之后每个手势含义固定 */
                        <div
                          key={key}
                          className={HOVER_ROW}
                        >
                          <Button
                            variant="ghost"
                            size="sm"
                            className="min-w-0 flex-1 justify-start px-0"
                            onClick={() =>
                              setHiddenTypes((prev) => {
                                const next = new Set(prev);
                                if (next.has(key)) next.delete(key);
                                else next.add(key);
                                return next;
                              })
                            }
                          >
                            <span
                              className={`h-2 w-2 shrink-0 ${t.shape === "square" ? "scale-90" : "rounded-full"}`}
                              style={{
                                background: t.color,
                                opacity: hiddenTypes.has(key) ? 0.35 : 1,
                              }}
                            />
                            <span
                              className={cn(
                                "truncate text-small",
                                hiddenTypes.has(key) ? "text-ink-2 line-through" : "text-ink",
                              )}
                            >
                              {t.label}
                            </span>
                          </Button>
                          {/* 「只看这个」：类一多时最想要的动作。**给显式按钮而不是
                              修饰键**——alt+点击没人猜得到，这里横向有地方 */}
                          <Button
                            variant="ghost"
                            size="sm"
                            className={cn(REVEAL, "shrink-0")}
                            onClick={() =>
                              setHiddenTypes(
                                new Set(
                                  types.map(([k]) => k).filter((k) => k !== key),
                                ),
                              )
                            }
                          >
                            {S.graph.legendOnly}
                          </Button>
                          <span className="u-num shrink-0 text-fine text-ink-2">
                            {t.count}
                          </span>
                        </div>
                      ))}
                    {types.every(
                      ([, t]) =>
                        !t.label.toLowerCase().includes(legendQ.toLowerCase()),
                    ) && (
                      <div className="px-2 py-2 text-small text-ink-2">
                        {S.graph.legendNone}
                      </div>
                    )}
                  </div>
              </PopoverContent>
            </Popover>
          )}
        </div>

        {/* 右上：能调「画多少个」+ 统计。**统计说的正是这个数**
            （「画了 150 个，共 548 个」），把调节放在它旁边，改的是谁一目了然。
            外壳保持中性——这一片是 chrome，彩色只属于数据 */}
        <div className="ml-auto flex flex-col items-end gap-1">
          <div className="flex items-start gap-2">
            <div className="pointer-events-auto flex items-center overflow-hidden rounded-control border border-line">
            <IconButton
              size="sm"
              label={S.graph.nodeBudgetLess}
              disabled={nodeBudget <= NODE_BUDGETS[0]}
              onClick={() =>
                setNodeBudget(
                  (b) => NODE_BUDGETS[Math.max(0, NODE_BUDGETS.indexOf(b) - 1)],
                )
              }
            >
              −
            </IconButton>
            {/* **画满了就别再给「多画」**：库里一共就这么多，再调高什么也不会变，
                而一个点了没反应的按钮比没有这个按钮更糟 */}
            <IconButton
              size="sm"
              label={S.graph.nodeBudgetMore}
              disabled={
                !capped || nodeBudget >= NODE_BUDGETS[NODE_BUDGETS.length - 1]
              }
              onClick={() =>
                setNodeBudget(
                  (b) =>
                    NODE_BUDGETS[
                      Math.min(NODE_BUDGETS.length - 1, NODE_BUDGETS.indexOf(b) + 1)
                    ],
                )
              }
            >
              +
            </IconButton>
          </div>
          <div className="pointer-events-none pt-1 u-num text-fine text-ink-2">
          {/* 画满上限时说清「画了多少 / 共多少」。**这个数从前是上限冒充规模**——
              一个上万实体的库右上角永远写着 150 */}
          {/* 图还没到就一个字都不写。**「0 entities · 0 facts」是个结论**，
              而此刻只是还不知道——旁边正转着圈，两句话摆在一起是自相矛盾的 */}
          {data.isPending ? null : capped ? (
            <span title={S.graph.cappedHint(nodeCount, totalNodes)}>
              {/* **事实也用「已画 / 共」的口径**：从前这里给的是库里的总数，
                  而实体给的是「画了多少 / 共多少」——同一句话里两套口径，
                  于是调档位时实体数在变、事实数纹丝不动，看着像坏了。
                  没有时间筛选时 active 恒等于已画条数，那就不说 */}
              {S.graph.statsCapped(
                nodeCount,
                totalNodes,
                edgeCount,
                totalEdges,
                timeT === null ? null : activeCount,
              )}
            </span>
          ) : (
            S.graph.stats(
              nodeCount,
              edgeCount,
              timeT === null ? null : activeCount,
            )
          )}
            </div>
          </div>
          {/* **单独一行，不做统计文字的前缀。**
              当前缀时它一出现就把整块撑宽，而这一块是靠右的——
              于是每次重新布局，左边的档位按钮都会被挤着跳一下。
              自己占一行，第一行的宽度就不再随它变 */}
          {stabilizing && (
            <div className="flex items-center gap-2 text-fine text-ink-2">
              <Loader2 size={11} className="animate-spin" />
              {S.graph.stabilizing}
            </div>
          )}
        </div>
      </div>

      {/* 画布：世界坐标网格层（随相机动）垫在 sigma WebGL 层下（全出血，时间岛悬浮其上） */}
      <div className="absolute inset-0">
        <canvas ref={gridRef} className="absolute inset-0 h-full w-full" />
        <div ref={containerRef} className="absolute inset-0" />
      </div>

      {/* 左下控件塔：推出来的边 + 布局切换 + 相机（右下归实体侧栏，底部中央归时间岛） */}
      {/* **items-start**：列内项目默认 stretch，一组展开就会把其余几组
          一起拉到同宽——那几组的字还收着，于是看着是几个莫名其妙的空白长条。
          各自按内容收放，才是「一组一组展开，不牵连别人」 */}
      <div className="absolute bottom-4 left-3 z-10 flex flex-col items-start gap-2">
        {/* 推出来的边：**自成一组，也不进类型图例。**
            图例回答「显示哪些类」，一排全是本体里的类；这个回答的是
            「显不显示推出来的边」——不是同一个问题。为零时整组不出现。

            **摆到这座塔上，是绕开一对矛盾走的**：放在顶栏图例旁边，它长得
            像第 10 个类；想靠颜色把它区分开，又撞上这文件开头那条既定原则
            ——「chrome 零色偏、彩色只属于数据」（见调色板那段注释）。
            往框架里塞一块高饱和金底，是整个界面唯一的彩色色块，扎眼且不成体系。

            这座塔本来就是「视图怎么看」的地盘（布局、缩放），
            「显不显示推出来的边」正是同一族问题。外壳保持中性，
            金色只出现在图标本身——与色点用在类胶囊上是同一个做法。 */}
        {derivedCount > 0 && (
          /* 面板现在走 Popover（Portal 到 body），不再是塔的子元素，
             所以外层这一圈只是为了跟下一座塔隔开 */
          <div className="relative">
            <ToolTower>
              <ToolButton
                role="switch"
                aria-checked={showDerived}
                active={showDerived}
                label={S.graph.viewDerived}
                title={`${S.graph.derivedEdges(derivedCount)} · ${S.graph.derivedHint}`}
                icon={<Waypoints size={15} />}
                style={
                  showDerived ? { color: EDGE_FOCUS_DERIVED } : undefined
                }
                onClick={() => setShowDerived((v) => !v)}
              />
              <ToolDivider />
              {/* 展开成一个小窗：这批边是什么时候推的、现在还推不推、手动再跑一次。
                  **与开关分成两个按钮**——「藏起来」是每天要点的，「什么时候推的」
                  是偶尔才问的，合成一个会让常用动作多一步 */}
              <Popover open={derivedOpen} onOpenChange={setDerivedOpen}>
                <PopoverTrigger asChild>
                  <ToolButton
                    active={derivedOpen}
                    label={S.graph.derivedPanel}
                    icon={
                      <span className="grid h-4 w-4 shrink-0 place-items-center leading-none">
                        ⋯
                      </span>
                    }
                  />
                </PopoverTrigger>
                {kb && (
                  <PopoverContent side="right" align="end" className="w-72 p-0">
                    <DerivedPanel kbId={kb.id} count={derivedCount} />
                  </PopoverContent>
                )}
              </Popover>
            </ToolTower>
          </div>
        )}
        <ToolTower>
          {(
            [
              { key: "force", Icon: Orbit, label: S.graph.layoutForce },
              {
                key: "circular",
                Icon: CircleDashed,
                label: S.graph.layoutCircular,
              },
              { key: "pack", Icon: Grape, label: S.graph.layoutPack },
            ] as const
          ).map(({ key, Icon, label }) => (
            <ToolButton
              key={key}
              active={layoutMode === key}
              label={label}
              icon={<Icon size={15} />}
              onClick={() => {
                setLayoutMode(key);
                layoutModeRef.current = key;
                layoutCtlRef.current?.apply(key);
              }}
            />
          ))}
        </ToolTower>
        <ToolTower>
          <ToolButton
            label={S.graph.zoomIn}
            icon={<ZoomIn size={15} />}
            onClick={() =>
              sigmaRef.current?.getCamera().animatedZoom({ duration: 220 })
            }
          />
          <ToolButton
            label={S.graph.zoomOut}
            icon={<ZoomOut size={15} />}
            onClick={() =>
              sigmaRef.current?.getCamera().animatedUnzoom({ duration: 220 })
            }
          />
          <ToolDivider />
          <ToolButton
            label={S.graph.fitView}
            icon={<Maximize2 size={15} />}
            onClick={() =>
              sigmaRef.current?.getCamera().animatedReset({ duration: 300 })
            }
          />
        </ToolTower>
      </div>

      {/* 图还没到。**左下那三座控件塔、顶上那排都已经在了**，缺的只是画布中间
          那团东西，所以这一层是盖上去的转圈，不是把整块换掉。
          与空状态分开：空状态是一句"接下来做什么"的结论，这个圈是过程，
          两者长得一样就会被读成同一件事 */}
      {data.isPending && <CanvasLoading />}

      {/* pb 把这块从几何正中抬起 40px：视觉重心比几何中心略高一点，
          正居中的短文字块看上去总是偏下 */}
      {empty && (
        <div className="absolute inset-0 grid place-items-center pb-20 pointer-events-none">
          {/* 不放标题方块：页面本身就是图谱页，tab 条上也写着，
              第三遍写"图谱"两个字不带任何信息。空状态该说的是下一步做什么——
              而"下一步"因人而异：管理员能直接去配模型，别人只能去找管理员（#267）。
              这一步现在由 readiness 统一判定（#313）：文档正在抽取时不该还让人去配模型 */}
          <div className="pointer-events-auto">
            <NextStep
              {...(step ?? {
                // 模型有了、文档也进来了、抽取跑完了，图还是空的
                line: S.steps.nothingExtracted,
                action: {
                  label: S.steps.openLibrary,
                  to: "/kb/$kbId/library",
                  params: { kbId },
                },
              })}
            />
          </div>
        </div>
      )}

      {/* 底部居中悬浮时间岛 */}
      {edgeCount > 0 && (
        <TimeScrubber
          edges={data.data!.edges}
          value={timeT}
          onChange={setTimeT}
          playing={playing}
          onPlayingChange={setPlaying}
        />
      )}

      {/* 实体侧栏。**取消选中之后还要多留 170ms**：那段时间它在演退场 */}
      {(selected || exiting) && kb && (
        <EntityPanel
          kbId={kb.id}
          entityId={(selected ?? exiting)!}
          exiting={!selected}
          intent={panelIntentRef}
          onClose={deselect}
          onNavigate={(id) => {
            // 跳转目标可能不在当前画布：同时把图 refocus 到它的邻域（与搜索选择一致）
            setFocusEntity(id);
            setSelected(id);
          }}
        />
      )}
    </div>
  );
}

/* ============ 时间轴（底部居中悬浮岛：播放 + 密度带 + 拖动） ============ */

/** 轨道 clientX → 对齐天步进的时间值（数据精度即 day，拖动求精细；播放仍按月推进求节奏）。 */
/** 轨道两端的余量，**等于轨道自己的圆角**（`rounded-control`）。
 *  柱子与播放头都缩进这么多：不缩的话，最左最右那几根正好落在圆角的弧里，
 *  看着像被切掉了一块；播放头走到头时也会贴上弧线。位置换算跟着一起缩，
 *  否则点在轨道最左边得到的值会比看到的位置偏一点。 */
const SCRUB_INSET = 10;

function scrubValueAt(
  clientX: number,
  track: HTMLDivElement | null,
  minTs: number,
  maxTs: number,
): number {
  if (!track) return maxTs;
  const rect = track.getBoundingClientRect();
  // 布局未成形（宽度 0）时避免除零产出 NaN
  if (rect.width < 1) return maxTs;
  const span = rect.width - SCRUB_INSET * 2;
  if (span < 1) return maxTs;
  const frac = Math.min(
    1,
    Math.max(0, (clientX - rect.left - SCRUB_INSET) / span),
  );
  const raw = minTs + frac * (maxTs - minTs);
  return Math.min(maxTs, minTs + Math.round((raw - minTs) / DAY_MS) * DAY_MS);
}

/** 播放/柱子的步长。**这两件事本来就该是同一个单位**——从前柱子按年、
 *  播放按天，界面上没有任何地方说得出「一格是多久」。 */
type ScrubUnit = "year" | "month" | "day";

/** 一根柱子最多画多少根。超过就把相邻的桶并起来画——**只影响画，不影响
 *  播放步长**：日单位下 15 年有五千多个桶，一根一像素也画不下，
 *  但播放仍然是一天一步。并了几个会在提示里说出来，不闷着 */
const SCRUB_MAX_BARS = 220;
/** 整条轨走完的目标时长。**与单位无关**——单位换的是颗粒度与密度，
 *  不该顺带把「等多久」也换掉：日单位若按「一天一拍」走，15 年要放二十分钟 */
const SCRUB_PLAY_MS = 18000;

function bucketStart(ts: number, unit: ScrubUnit): number {
  const d = new Date(ts);
  if (unit === "year") return Date.UTC(d.getUTCFullYear(), 0, 1);
  if (unit === "month")
    return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1);
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
}
function bucketNext(ts: number, unit: ScrubUnit): number {
  const d = new Date(ts);
  if (unit === "year") return Date.UTC(d.getUTCFullYear() + 1, 0, 1);
  if (unit === "month")
    return Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1);
  return ts + DAY_MS;
}

function TimeScrubber({
  edges,
  value,
  onChange,
  playing,
  onPlayingChange,
}: {
  edges: GraphEdge[];
  value: number | null;
  onChange: (v: number | null) => void;
  /* 播放态由 Graph 持有：渲染层要区分播放推进与手动拖动 */
  playing: boolean;
  onPlayingChange: (v: boolean) => void;
}) {
  const setPlaying = onPlayingChange;
  /* 默认年：**大多数库跨度都以年计**，一进来先给能一眼看全的那一档 */
  const [unit, setUnit] = useState<ScrubUnit>("year");
  /* 走完整条的次数。**拿它当 key**——同一个元素上重复触发同一个动画不会重播，
     换 key 让它重新挂载才会 */
  const [sweep, setSweep] = useState(0);
  /* 指针在轨道上时，已走过的那段提亮。**它回答的是"我走到哪了"**——
     不播的时候整条都是同一档灰，看不出进度停在哪；而这正是人把指针
     移上来想知道的事 */
  const [trackHover, setTrackHover] = useState(false);
  const trackRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);
  /* 拖动落点。**播放循环有自己的浮点累加器**，不读 value——否则每帧的取整
     误差会积起来。所以光改 value 是没用的，下一帧就被原样覆盖回去。
     拖动把落点放进这里，循环下一帧接手，从新位置继续走 */
  const seekRef = useRef<number | null>(null);
  const seek = (v: number) => {
    seekRef.current = v;
    onChange(v);
  };

  const { minTs, maxTs, bars, merged, trackW } = useMemo(() => {
    const now = Date.now();
    // 柱子按边**开始成立**的时刻分格（0022）：没起点的边从最早证据起算，这样
    // 滑杆的量程盖得住它亮起来的那一刻，而不是把它排除在直方图之外
    const froms = edges
      .map((e) => (e.holds_from ? Date.parse(e.holds_from) : NaN))
      .filter((t) => !Number.isNaN(t));
    const min = froms.length
      ? Math.min(...froms)
      : now - 5 * 365 * 24 * 3600 * 1000;
    // 起点对齐到单位边界：否则第一根柱子是半格，读起来像数据缺了一块
    const start = bucketStart(min, unit);

    const counts = new Map<number, number>();
    for (const t of froms) {
      const k = bucketStart(t, unit);
      counts.set(k, (counts.get(k) ?? 0) + 1);
    }
    const raw: { ts: number; n: number }[] = [];
    for (let t = start; t <= now; t = bucketNext(t, unit))
      raw.push({ ts: t, n: counts.get(t) ?? 0 });

    // 画不下就并桶。**并的是画，不是步长**
    const group = Math.max(1, Math.ceil(raw.length / SCRUB_MAX_BARS));
    const cells: { ts: number; n: number }[] = [];
    for (let i = 0; i < raw.length; i += group) {
      const slice = raw.slice(i, i + group);
      cells.push({
        ts: slice[0].ts,
        n: slice.reduce((a, b) => a + b.n, 0),
      });
    }
    const peak = Math.max(1, ...cells.map((c) => c.n));

    // 单位越大 → 桶越少 → 岛越短；越小 → 越长。**但下限要抬得够高**：
    // 岛里那排固定控件（播放键 + 单位选择器 + 两个年份 + 日期 + All time/Now）
    // 本身就要四百多像素，岛只有 320 时 flex-1 的轨道被压成 0——
    // 实测柱子一根都看不见，整条是空的。
    //
    // 抬高之后单位主要改变的是**每根柱子的粗细**：同一条轨道，
    // 年是十几根粗块，日是两百多根细线。这比整条伸缩更说明问题
    const w = Math.min(780, Math.max(660, 380 + cells.length * 2));

    return {
      minTs: start,
      maxTs: now,
      bars: cells.map((c) => ({ ts: c.ts, h: c.n / peak, n: c.n })),
      merged: group,
      trackW: w,
    };
  }, [edges, unit]);

  // 播放按日推进（数据即 day 精度），日子快速翻过；整体节奏仍 ≈ 一个月/260ms。
  // rAF 时间驱动：帧率无关，内部浮点累加避免取整漂移，值只在跨天时才下发
  useEffect(() => {
    if (!playing) return;
    // 整条走完约 SCRUB_PLAY_MS，与单位无关；单位只决定落点取整到哪一格
    const SPEED = (maxTs - minTs) / SCRUB_PLAY_MS;
    let raf = 0;
    let last = performance.now();
    let acc = value ?? minTs;
    let lastPushed = 0;
    const step = (now: number) => {
      // 有人拖过了：从落点接着走，而不是沿原来的轨迹
      if (seekRef.current !== null) {
        acc = seekRef.current;
        seekRef.current = null;
      }
      acc += (now - last) * SPEED;
      last = now;
      if (acc >= maxTs) {
        setPlaying(false);
        onChange(null);
        // 走到头了扫一道光。**这是个收尾**——播放停下、时间跳回全时段，
        // 没有交代的话看着像中途断了；一道光扫过说明"这条走完了"
        setSweep((n) => n + 1);
        return;
      }
      // **连续推进，不按桶跳。** 从前按 `bucketStart` 取整下发，年单位下
      // 一次就是一年——播放头一格一格蹦，看着像卡顿而不是在走。
      // 单位现在只管**显示**（标签精度、柱子跨度），不再管推进的步长。
      //
      // 代价是下发变密（每帧一次），而每次下发都要重算全图的现行边，
      // 所以限到 ~30fps：肉眼看不出与 60fps 的差别，重算量减半
      if (now - lastPushed >= 33) {
        lastPushed = now;
        onChange(Math.round(acc));
      }
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
    // 只随播放开关重启：acc 在循环内自持，value 帧帧变不应重建循环
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing, minTs, maxTs, unit]);

  // 展示到日：与数据的 day 级 valid_precision 对齐
  const label = (() => {
    if (value === null) return S.graph.allTime;
    const d = new Date(value);
    const mm = String(d.getUTCMonth() + 1).padStart(2, "0");
    const dd = String(d.getUTCDate()).padStart(2, "0");
    // 精度跟着单位：年单位下写出「2019-01-01」是假精确
    if (unit === "year") return `${d.getUTCFullYear()}`;
    if (unit === "month") return `${d.getUTCFullYear()}-${mm}`;
    return `${d.getUTCFullYear()}-${mm}-${dd}`;
  })();

  const minYear = bars.length
    ? new Date(bars[0].ts).getUTCFullYear()
    : undefined;
  const maxYear = bars.length
    ? new Date(bars[bars.length - 1].ts).getUTCFullYear()
    : undefined;

  return (
    /* 宽度随单位变：单位大 → 桶少 → 短；单位小 → 桶多 → 长而密。
       仍夹在视口内（calc 那一项），窄屏不会顶出去。
       实测宽度：年 320 / 月 648 / 日 760。 */
    <div
      className={`glass-strong absolute bottom-4 left-1/2 -translate-x-1/2 z-10 rounded-overlay px-3 py-2 flex items-center gap-3 u-scrub-island${playing ? " u-solid" : ""}`}
      style={{ width: `min(${trackW}px, calc(100vw - 4rem))` }}
    >
      <IconButton
        variant="secondary"
        className="shrink-0"
        label={playing ? S.graph.pause : S.graph.play}
        onClick={() => {
          // 已经在末端（`Now`）时按播放要从头来。**否则第一下等于没反应**：
          // acc 起点就是终点，循环第一帧就判定播完，只把位置清成 All time
          if (
            !playing &&
            (value === null || value >= maxTs - (maxTs - minTs) * 0.02)
          )
            onChange(minTs);
          setPlaying(!playing);
        }}
      >
        {/* 实心：播放/暂停这一对是媒体键的通用记号，空心的三角看着像
            「展开」那类折叠柄。lucide 的图标默认只描边，填色要自己给 */}
        {playing ? (
          <Pause size={13} fill="currentColor" stroke="none" />
        ) : (
          <Play size={13} fill="currentColor" stroke="none" />
        )}
      </IconButton>

      {/* 步长。**播放与柱子共用它**——从前柱子按年、播放按天，
          界面上没有一处说得出「一格是多久」 */}
      <Segmented
        size="sm"
        className="shrink-0"
        value={unit}
        onChange={setUnit}
        options={(["year", "month", "day"] as const).map((u) => ({
          value: u,
          title: S.graph.scrubUnitHint,
          label:
            u === "year"
              ? S.graph.scrubUnitYear
              : u === "month"
                ? S.graph.scrubUnitMonth
                : S.graph.scrubUnitDay,
        }))}
      />

      <span className="shrink-0 u-num text-fine text-ink-2">
        {minYear}
      </span>

      {/* 密度带轨道：内嵌浅色井 + 每年事实量柱 */}
      <div
        ref={trackRef}
        onMouseEnter={() => setTrackHover(true)}
        onMouseLeave={() => setTrackHover(false)}
        className="relative h-9 min-w-[150px] flex-1 overflow-hidden rounded-control bg-surface"
      >
        {/* 演完由 **React** 卸载，**别自己 `remove()`**。
            从前是 `onAnimationEnd={(e) => e.currentTarget.remove()}`——
            把 React 管着的节点从 DOM 里抠走，它自己并不知道。下一次扫光时
            key 变了，React 去移除"旧节点"，而那个节点已经不在父节点里，
            removeChild 抛 NotFoundError，未捕获的错误让整棵树卸载重挂：
            现象就是**连播两轮之后界面像刷新了一次** */}
        {sweep > 0 && (
          <span
            key={sweep}
            className="u-sweep"
            onAnimationEnd={() => setSweep(0)}
          />
        )}
        {/* **间隙必须随密度收**：写死 2px 时，日单位下 216 根柱子有 215 个间隙
            ≈ 430px，而轨道内宽才 ~455px——柱子被挤成 0.1px，整条看起来是空的。
            实测就是这么丢的。柱子稀疏时留 2px 好数，密了就贴在一起当密度带看 */}
        <div
          className="absolute top-1.5 bottom-1.5 flex items-end"
          style={{
            left: SCRUB_INSET,
            right: SCRUB_INSET,
            gap: bars.length > 120 ? 0 : bars.length > 40 ? 1 : 2,
          }}
        >
          {bars.map((b) => {
            // 进入即亮（桶起点为判据）：播放头脚下的柱子即已覆盖——进度条通用语义
            const past = value !== null && b.ts <= value;
            const d = new Date(b.ts);
            const stamp =
              unit === "year"
                ? `${d.getUTCFullYear()}`
                : unit === "month"
                  ? `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, "0")}`
                  : d.toISOString().slice(0, 10);
            return (
              <div
                key={b.ts}
                className="flex-1 flex items-end h-full"
                title={`${stamp} · ${b.n}${merged > 1 ? ` · ${S.graph.scrubBarMerged(merged)}` : ""}`}
              >
                <div
                  className="u-bar w-full"
                  style={{
                    height: `${Math.max(10, b.h * 100)}%`,
                    // 播放中已扫过的提亮，停止后回到常规亮度。
                    // **还没走到的压到近乎不可见**：它们本来是 0.09，
                    // 在这个底色上仍看得清，于是播放头右边跟左边一样"亮着"，
                    // 走到哪儿就看不出来了。留一点点而不是归零——
                    // 归零等于假装那段没有数据，而它只是还没到
                    background:
                      value !== null && past && (playing || trackHover)
                        ? SCRUB_PLAY
                        : value === null || past
                          ? SCRUB_PAST
                          : SCRUB_FUTURE,
                  }}
                />
              </div>
            );
          })}
        </div>
        <input
          type="range"
          className="scrubber-range"
          /* CSS 里那条 width:100% 要让开，否则左右缩进之后整条会溢出 */
          style={{ left: SCRUB_INSET, right: SCRUB_INSET, width: "auto" }}
          min={minTs}
          max={maxTs}
          step={DAY_MS}
          value={value ?? maxTs}
          /* **拖动不停播**：拖是"我要看那一段"，不是"我要停下"——
             松手之后应该从新位置继续走到底。
             （`All time` / `Now` 那两个按钮仍然停：那是明确的跳转，不是擦洗） */
          onChange={(e) => seek(Number(e.target.value))}
          // 原生 range 的拖拽手势会被页面级鼠标监听（如图上拖节点）干扰——
          // 自己用 pointer capture 驱动拖动，点击与拖拽都走同一条计算路径
          onPointerDown={(e) => {
            draggingRef.current = true;
            try {
              e.currentTarget.setPointerCapture(e.pointerId);
            } catch {
              /* 合成事件的 pointerId 可能无效，忽略 */
            }
            seek(scrubValueAt(e.clientX, trackRef.current, minTs, maxTs));
          }}
          onPointerMove={(e) => {
            if (draggingRef.current)
              seek(scrubValueAt(e.clientX, trackRef.current, minTs, maxTs));
          }}
          onPointerUp={() => {
            draggingRef.current = false;
          }}
          onPointerCancel={() => {
            draggingRef.current = false;
          }}
        />
      </div>

      <span className="shrink-0 u-num text-fine text-ink-2">
        {maxYear}
      </span>

      <div className="w-[5.6rem] shrink-0 text-center u-num text-small text-ink">
        {label}
      </div>

      <div className="h-5 w-px shrink-0 bg-surface-3" />

      {/* 双锚点分段：所处锚点高亮、点击即跳；拖在中间某天时两者皆不亮 */}
      <Segmented<"all" | "now" | "none">
        size="sm"
        className="shrink-0"
        value={value === null ? "all" : maxTs - value < DAY_MS ? "now" : "none"}
        onChange={(k) => {
          setPlaying(false);
          onChange(k === "all" ? null : maxTs);
        }}
        options={[
          { value: "all", label: S.graph.allTime },
          { value: "now", label: S.graph.nowBtn },
        ]}
      />
    </div>
  );
}

/* ============ 实体侧栏 ============ */

/* 抽出来供 vitest 测；UI 段（`EntityPanel`）内部闭包用同名 */
export function fmtInterval(f: EntityFact): string {
  // 时态 = 永恒：不画区间。
  // 写端 `Validity::under(Eternal)` 把 from/to 都抹成 null，render 时就空，
  // 等同"这条没有时间维度"。
  if (f.temporal === "eternal") return "";
  // 时态 = 事件：单点。`Validity::under(Event)` 把 from、to 都设成同一个时刻
  // （0022 / #486 写端契约）——`from ~ from` 读起来像区间错框了，事件本就一个
  // 时刻。from 为 null 时退到 to；都没有就空（抽取失败，UI 退化为不画）
  if (f.temporal === "event") {
    const moment = fmtTime(f.valid_from, f.valid_from_precision);
    if (moment) return moment;
    const at = fmtTime(f.valid_to, f.valid_to_precision);
    return at ?? "";
  }
  const from = fmtTime(f.valid_from, f.valid_from_precision);
  const to = fmtTime(f.valid_to, f.valid_to_precision);
  // **「结束了但不知哪天」绝不能显示成「至今」。** 那是这条改动要修的正脸：
  // 原文明说 "former CEO of Weta Digital"，界面却告诉读者他还在任
  const endedUnknown = !f.valid_to && f.valid_to_precision === "unknown";
  if (!from && !to && !endedUnknown) return "";
  const end = to ?? (endedUnknown ? S.graph.endedUnknown : S.graph.ongoing);
  return from ? `${from} ~ ${end}` : `~ ${end}`;
}

/** 一条推出来的事实，**证明摊开在下面**。
 *
 * 不做折叠：这一档存在的全部理由就是「这条边不是谁说的，是这么来的」，
 * 把前提藏在一次点击后面等于把理由藏起来。链最长十二条，摊开也不长。 */
/** 派生开关旁边那个小窗：**这批边是什么时候、按什么推出来的，以及现在还准不准**。
 *
 * 存在的理由是「新鲜度看不见」。派生每小时重推一次，而事实每篇文档进来都在变——
 * 一条派生边看上去和它刚推出来的时候一模一样，可它依据的前提可能三分钟前刚被撤掉。
 * 光有开关答不了「我现在看到的是什么时候的结论」。
 *
 * 手动按钮留在这里而不是别处：想重推的人正是刚看完这三行、觉得数字太旧的那个人。
 */
function DerivedPanel({ kbId, count }: { kbId: string; count: number }) {
  const kb = useQuery({
    queryKey: ["kbOne", kbId],
    queryFn: () => api.kbDetail(kbId),
  });
  /* 重跑要确认，但**确认的第二下必须落在另一个按钮上**。
     这产品的手势约定是「同一个控件连点两下 = 收回去」——开关、⋯、图例胶囊
     都是这么用的。把「再点一次就执行」压在同一个按钮上，等于让同一个手势
     在这里意外地变成了「执行」，而别处它一直是「取消」。
     所以点一下只是**问一句**，问句下面给 取消 / 跑 两个目标。

     也没有用全站的 DangerConfirm：那是红标题、可要求逐字输入的危险级，
     留给删库那类不可逆操作。重跑推理重但可重复，够不上那一档 */
  const qc = useQueryClient();
  const [armed, setArmed] = useState(false);
  const run = useMutation({
    mutationFn: () => api.runInference(kbId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["graph"] });
      qc.invalidateQueries({ queryKey: ["kbOne", kbId] });
    },
    onError: (e: Error) => toast.error(e.message),
  });

  const on = kb.data?.materialize_inferences ?? false;
  const last = kb.data?.last_inference_at;
  // 「多久以前」比一个时间戳好读——问题是「新不新」，不是「几点」
  const age = last
    ? Math.round((Date.now() - new Date(last).getTime()) / 60000)
    : null;

  /* 内容而已：定位、关闭、外点都归 Popover（与顶栏三个面板同一套）。
     从前它自己盖在触发器原位往右上长开，还得带一个关闭叉 */
  return (
    <div className="px-3 pb-3 pt-3">
      {/* items-center 而不是 baseline：标题旁边站着一个按钮和一个关闭键，
          按基线对齐会让那两个看着往上飘 */}
      <div className="flex items-center gap-2">
        <span className="text-body text-ink">
          {S.graph.derivedPanel}
        </span>
        {!armed && (
          <Button
            variant="secondary"
            size="sm"
            className="ml-auto"
            disabled={!on || run.isPending}
            title={on ? undefined : S.err.inference_off}
            onClick={() => setArmed(true)}
          >
            {run.isPending ? S.graph.derivedRunning : S.graph.derivedRun}
          </Button>
        )}
      </div>

      {/* 问句 + 两个目标。**取消排在前面**：从「跑」那一下移过来最先碰到的
          是取消，误触的代价小的那个该更近 */}
      {armed && (
        <div className="mt-2 rounded-panel bg-surface p-2">
          <p className="text-fine leading-relaxed text-ink-2">
            {S.graph.derivedRunAsk}
          </p>
          <div className="mt-2 flex gap-2">
            <Button variant="ghost" size="sm" onClick={() => setArmed(false)}>
              {S.graph.derivedRunCancel}
            </Button>
            <Button
              variant="primary"
              size="sm"
              disabled={run.isPending}
              onClick={() => {
                setArmed(false);
                run.mutate();
              }}
            >
              {S.graph.derivedRunGo}
            </Button>
          </div>
        </div>
      )}

      <dl className="mt-2 space-y-1 text-fine">
        <div className="flex justify-between gap-3">
          <dt className="text-ink-2">{S.graph.derivedCountLabel}</dt>
          <dd className="u-num text-ink">{count}</dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-ink-2">{S.graph.derivedStateLabel}</dt>
          <dd className={on ? "text-ink" : "text-warn"}>
            {on
              ? S.graph.derivedOn(kb.data!.inference_interval_minutes)
              : S.graph.derivedOff}
          </dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-ink-2">{S.graph.derivedLastLabel}</dt>
          <dd className="u-num text-ink">
            {age === null ? S.graph.derivedNever : S.graph.derivedAgo(age)}
          </dd>
        </div>
      </dl>

      {/* 上一次手动跑的结果留在这儿。**推出多少、作废多少要分开说**——
          「什么都没变」和「换掉了三十条」是两件很不一样的事 */}
      {run.data && (
        <p className="mt-2 text-fine text-ink-2">
          {run.data.inserted === 0 && run.data.invalidated === 0
            ? S.graph.derivedNoChange
            : S.graph.derivedChanged(run.data.inserted, run.data.invalidated)}
          {run.data.capped > 0 &&
            ` · ${S.graph.derivedCapped(run.data.capped)}`}
        </p>
      )}

    </div>
  );
}

/** 推出来的一条边。**行式样与 FactRow 对齐**：同样的圆角行、同样的
 *  chevron 展开、同样的 role="link" 跳转（避免按钮套按钮）。
 *
 *  从前这里是一张 `glass rounded-panel p-3` 卡片、证明常驻展开——在一列
 *  Relations/Timeline/History 的紧凑行里显得是另一个产品的东西，而且十几条
 *  推导堆起来是一面墙。证明是「问了才看」的东西，收进展开区正合适。 */
function DerivedRow({
  kbId,
  d,
  otherId,
  otherName,
  open,
  onToggle,
  onNavigate,
}: {
  kbId: string;
  d: DerivedFact;
  /** 字面值结论没有另一端实体（0021）*/
  otherId: string | null;
  otherName: string;
  open: boolean;
  onToggle: () => void;
  onNavigate: (entityId: string) => void;
}) {
  return (
    <ExpandCard
      open={open}
      onToggle={onToggle}
      header={
        <div className="flex items-center gap-2">
        {/* 业务规则的结论是字面值（一个类、一个值），另一端没有实体可跳——
            这时候画成普通文字，而不是一个点了没反应的链接（0021） */}
        {otherId ? (
          <span
            role="link"
            tabIndex={0}
            onClick={(ev) => {
              ev.stopPropagation();
              onNavigate(otherId);
            }}
            onKeyDown={(ev) => {
              if (ev.key === "Enter") {
                ev.stopPropagation();
                onNavigate(otherId);
              }
            }}
            className="u-inline-link truncate text-body text-ink"
          >
            {otherName}
          </span>
        ) : (
          <span className="truncate text-body text-ink">{otherName}</span>
        )}
        <span className="ml-auto shrink-0 pl-2 u-num text-fine text-ink-2">
          {d.premises.length}
        </span>
        </div>
      }
    >
      {/* 证明：前提按推导顺序，每条展开到原句（0002 R2）。与 EvidenceList
          同一个位置、同一种缩进——两者是同一件事的两种形态：一个给出处，一个给推理链 */}
      {open && <ProofChain kbId={kbId} d={d} />}
    </ExpandCard>
  );
}

/** 一条派生的证明链。展开时才取——证明是「问了才看」的东西。
 *
 *  每一步是一条断言前提：三元组在上，它的原句在下，原句可点进文档。
 *  前提被撤过的打标记但不藏：派生随之失效，而「当时靠的是什么」正是记录轴要答的。
 *  取不到（派生已失效）就退回列表里带来的那几行文本，不空着。 */
function ProofChain({ kbId, d }: { kbId: string; d: DerivedFact }) {
  const proof = useQuery({
    queryKey: ["proof", d.id],
    queryFn: () => api.derivedProof(kbId, d.id),
  });
  const steps = proof.data?.proof?.steps;
  return (
    <div>
      {proof.isPending && (
        <p className="text-fine text-ink-2">{S.graph.proofLoading}</p>
      )}
      {steps && <ProofSteps kbId={kbId} steps={steps} />}
      {/* 派生已失效、证明取不到：退回列表里带来的那几行文本 */}
      {!proof.isPending && !steps && (
        <ol className="space-y-1">
          {d.premises.map((p, i) => (
            <li key={i} className="text-fine text-ink-2">
              {p}
            </li>
          ))}
          {d.premises.length === 0 && (
            <li className="text-fine text-ink-2">{S.graph.derivedNoProof}</li>
          )}
        </ol>
      )}
    </div>
  );
}

/** 证明的步，落了地的与没落地的派生共用：前提是同一种东西 */
function ProofSteps({ kbId, steps }: { kbId: string; steps: ProofStep[] }) {
  return (
    <ol className="space-y-2">
      {steps.map((st) => (
        <li key={st.fact_id} className="text-fine">
          <div className="flex items-baseline gap-2 flex-wrap">
            <span className="u-num text-fine text-ink-2 shrink-0">
              {S.graph.proofStep(st.seq + 1)}
            </span>
            <span className={st.retracted ? "text-ink-2 line-through" : "text-ink-2"}>
              {st.subject}
              <span className="text-ink-2"> — {st.predicate ?? "?"} → </span>
              {st.object ?? "?"}
            </span>
            {st.retracted && (
              <Chip tone="warn" className="text-fine">{S.graph.proofRetracted}</Chip>
            )}
          </div>
          <div className="mt-1 space-y-1 pl-2">
            {st.evidence.map((ev) => (
              <Link
                key={ev.chunk_id}
                to="/kb/$kbId/doc/$docId"
                params={{ kbId, docId: ev.document_id }}
                search={{ chunk: ev.chunk_id }}
                className="u-hover-ink block text-ink-2"
              >
                <div className="line-clamp-2 italic">
                  {ev.quote ? `“${ev.quote}”` : S.graph.noQuote}
                </div>
                <div className="mt-1 text-ink-2">
                  {S.graph.sectionRef(ev.filename, ev.seq + 1)}
                  {ev.stale && (
                    <span
                      className="ml-2 u-num text-fine text-ink-2"
                      title={S.graph.staleEvidenceHint}
                    >
                      {S.graph.fromVersion(ev.doc_version)}
                    </span>
                  )}
                  {ev.document_deleted && (
                    <span
                      className="ml-2 text-fine text-contest"
                      title={S.graph.sourceDeletedHint}
                    >
                      {S.graph.sourceDeleted}
                    </span>
                  )}
                </div>
              </Link>
            ))}
            {st.evidence.length === 0 && (
              <p className="text-ink-2">{S.graph.noEvidence}</p>
            )}
          </div>
        </li>
      ))}
    </ol>
  );
}

/** 没落地的派生（0017 §3）：像 DerivedRow 一样的一行，多一句「挡住它的是谁」，
 *  展开是它的证明链——人在这里看到「引擎本可以画这条边，是什么拦住了它」 */
function BlockedRow({
  kbId,
  b,
  entityId,
  open,
  onToggle,
  onNavigate,
}: {
  kbId: string;
  b: BlockedDerivation;
  entityId: string;
  open: boolean;
  onToggle: () => void;
  onNavigate: (entityId: string) => void;
}) {
  const navigate = useNavigate();
  const out = b.subject_id === entityId;
  const otherId = out ? b.object_id : b.subject_id;
  const otherName = out ? b.object : b.subject;
  const proof = useQuery({
    queryKey: ["blocked-proof", b.violation_id],
    queryFn: () => api.blockedProof(kbId, b.violation_id),
    enabled: open,
  });
  return (
    <ExpandCard
      open={open}
      onToggle={onToggle}
      header={
        <div className="flex items-center gap-2">
        {out ? <ArrowRight size={10} className="shrink-0 text-ink-2" /> : <ArrowLeft size={10} className="shrink-0 text-ink-2" />}
        <span className="shrink-0 text-fine text-ink-2">{b.predicate}</span>
        <span
          role="link"
          tabIndex={0}
          onClick={(ev) => {
            ev.stopPropagation();
            onNavigate(otherId);
          }}
          onKeyDown={(ev) => {
            if (ev.key === "Enter") {
              ev.stopPropagation();
              onNavigate(otherId);
            }
          }}
          className="u-inline-link truncate text-body text-ink"
        >
          {otherName}
        </span>
        <span className="ml-auto shrink-0 pl-2 text-fine text-ink-2">
          {S.graph.ruleNames[b.rule] ?? b.rule}
        </span>
        </div>
      }
    >
      <div className="flex items-center gap-2 text-fine">
        <span className="truncate text-contest">
          {S.graph.blockedBy(b.against_text)}
        </span>
        <span
          role="link"
          tabIndex={0}
          onClick={() =>
            navigate({
              to: "/kb/$kbId/review",
              params: { kbId },
              search: { queue: "violations", item: b.violation_id },
            })
          }
          className="u-inline-link ml-auto shrink-0 text-ink-2"
        >
          {S.graph.blockedReview} →
        </span>
      </div>
      {open && (
        <div>
          {proof.isPending && (
            <p className="text-fine text-ink-2">{S.graph.proofLoading}</p>
          )}
          {proof.data?.steps && (
            <ProofSteps kbId={kbId} steps={proof.data.steps} />
          )}
        </div>
      )}
    </ExpandCard>
  );
}

/** 争议 chip（0017 §3）：有一条 open 的违规或冲突指着这条断言。行不压暗——
 *  它仍然活着。点它去 Review 对应那一档，并把那张卡点亮 */
function ContestedChip({
  kbId,
  c,
}: {
  kbId: string;
  c: NonNullable<EntityFact["contested"]>;
}) {
  const navigate = useNavigate();
  const queue = c.kind === "temporal_conflict" ? "conflicts" : "violations";
  return (
    <span
      role="link"
      tabIndex={0}
      onClick={(ev) => {
        ev.stopPropagation();
        navigate({
          to: "/kb/$kbId/review",
          params: { kbId },
          search: { queue, item: c.ref_id },
        });
      }}
      onKeyDown={(ev) => {
        if (ev.key === "Enter") {
          ev.stopPropagation();
          navigate({
            to: "/kb/$kbId/review",
            params: { kbId },
            search: { queue, item: c.ref_id },
          });
        }
      }}
      className={chipLike("contest", "shrink-0 cursor-pointer text-fine")}
      title={S.graph.contestedHint(c.kind, c.derived ?? null)}
    >
      {S.graph.contestedChip}
    </span>
  );
}

function EntityPanel({
  kbId,
  entityId,
  exiting,
  intent,
  onClose,
  onNavigate,
}: {
  kbId: string;
  entityId: string;
  /** 正在演退场：还挂在 DOM 上，但已经不接受点击 */
  exiting: boolean;
  /** 打开时停在哪一档、展开哪一行；读一次就清掉 */
  intent?: MutableRefObject<{ view: "derived"; open: string } | null>;
  onClose: () => void;
  onNavigate: (entityId: string) => void;
}) {
  const detail = useQuery({
    queryKey: ["entity", kbId, entityId],
    queryFn: () => api.entityDetail(kbId, entityId),
  });
  const [openFact, setOpenFact] = useState<string | null>(null);
  // 推出来的那些。**单独一个键，不掺进 facts**——混在一个列表里，用户看不出
  // 「文档里写的」和「引擎推的」的区别
  const derived = detail.data?.derived ?? [];
  // 没落地的（0017 §3）：推出来了，撞上一条断言
  const blocked = detail.data?.blocked ?? [];
  /* 按「方向 + 谓词 + 规则」分组，骨架与 Relations 的 groups 一致。
     规则挂在组上而不是每一行：它对整组都成立，逐行重复既冗余，
     那个琥珀色小字还会跟派生边抢色相 */
  const derivedGroups = useMemo(() => {
    const map = new Map<
      string,
      {
        key: string;
        direction: "in" | "out";
        predicate: string;
        rule: string;
        rows: DerivedFact[];
      }
    >();
    for (const d of derived) {
      const direction = d.subject_id === entityId ? "out" : "in";
      // 四条公理各有名字；业务规则用它自己的名字（0021）——「Gas-bearing well」
      // 比「business」有意义得多，而那个名字正是人写规则时起的。
      // **查不到就退回原始 kind 串**：对读的人没有意义，但比显示成另一条规则诚实
      const rule = d.rule_name ?? S.graph.ruleNames[d.rule] ?? d.rule;
      // 分组键用规则名而不是 kind：同一个谓词上两条业务规则各归各的
      const key = `${direction}|${d.predicate}|${d.rule_name ?? d.rule}`;
      const cur = map.get(key);
      if (cur) cur.rows.push(d);
      else map.set(key, { key, direction, predicate: d.predicate, rule, rows: [d] });
    }
    return [...map.values()];
  }, [derived, entityId]);
  // Relations = 按关系分组（查关系）；Timeline = 有效时间轴（事情何时成立）；
  // History = 记录时间轴（我们何时这么认为、又何时改了主意）
  const [view, setView] = useState<"relations" | "history" | "derived">(
    "relations",
  );
  useEffect(() => {
    const it = intent?.current;
    if (!it) return;
    intent.current = null;
    setView(it.view);
    setOpenFact(it.open);
  }, [entityId, intent]);

  const e: GraphNode | undefined = detail.data?.entity;

  // 实体修正（名字、类型）在弹窗里：面板只展示
  const [editing, setEditing] = useState(false);
  /* **同名的其他实体不在这块面板上出现。**（曾经有一条横幅，列出同名的
     每一个并给「并进来」。）两个问题：那些行只有类型名可读，同名的七个
     全写着「Organization」，界面在问"它们是同一个吗"却不给判断的依据；
     而且横幅没有高度上限，同名多几个就把 Relations / History / Derived
     挤到屏幕外。合并是 Review 那边的事——那里有并排比对。
     这块面板只回答"我正在看的这个实体是什么"。 */

  const openEdit = () => {
    if (!e) return;
    setEditing(true);
  };

  /* Relations 是一张表，不再分「现行」和「年表」两页：**从这个实体出发 / 指向这个实体**
     两节，节里一行一条——左边关系名、右边实体名，与本体页那张同一副（不再按谓词
     分二级）；按关系名、再按起点排。此刻不成立的（0022 的口径按读出来的区间判）折在
     节尾的「N past」里——Wikidata 把历史值留在同一列表里靠结束时间区分，是同一个道理 */
  const sections = useMemo(() => {
    const all = detail.data?.facts ?? [];
    const nowIso = new Date().toISOString();
    const current = (f: EntityFact) =>
      (!f.holds_from || f.holds_from <= nowIso) &&
      (!f.holds_to || f.holds_to > nowIso);
    const order = (a: EntityFact, b: EntityFact) =>
      // 按**看到的那个写法**排序，不然 `worksFor` 与 `accessTo` 的先后
      // 跟屏幕上读到的 “works for” / “access to” 对不上
      predicateSentence(a.predicate_label ?? "￿").localeCompare(
        predicateSentence(b.predicate_label ?? "￿"),
      ) ||
      ((a.valid_from ?? "9999") < (b.valid_from ?? "9999") ? -1 : 1);
    const split = (dir: "out" | "in") => {
      const mine = all.filter((f) => f.direction === dir);
      return {
        rows: mine.filter(current).sort(order),
        past: mine.filter((f) => !current(f)).sort(order),
      };
    };
    return { out: split("out"), in: split("in") };
  }, [detail.data]);

  return (
    <div
      className={`${exiting ? "u-dock-out" : "u-dock-in"} glass-strong absolute top-14 right-3 bottom-20 w-96 z-10 rounded-overlay u-lift-strong flex flex-col`}
    >
      <div className="flex items-start justify-between px-4 py-4 border-b border-line">
        <div>
          {e && (
            <>
              <div className="flex items-center gap-2">
                <span
                  className="h-2.5 w-2.5 rounded-full shrink-0"
                  style={{
                    background: e.color,
                    boxShadow: `0 0 8px ${e.color}55`,
                  }}
                />
                <span
                  className="text-title font-semibold tracking-tight text-ink"
                  style={{ fontFamily: "var(--font-display)" }}
                >
                  {e.name}
                </span>
              </div>
              {/* 消歧后缀找不到关联事实时兜底成类型标签，那就与后面的类型重复了 */}
              <div className="mt-1 text-small text-ink-2">
                {e.disambiguator && e.disambiguator !== e.type_label
                  ? `${e.disambiguator} · `
                  : ""}
                {e.type_label ?? S.graph.untyped} ·{" "}
                {detail.data?.facts.length ?? 0} {S.graph.facts}
              </div>
            </>
          )}
        </div>
        <div className="flex items-center gap-2 mt-1">
          {e && !editing && (
            <IconButton size="sm" label={S.graph.edit} onClick={openEdit}>
              <Pencil size={13} />
            </IconButton>
          )}
          <IconButton size="sm" label={S.graph.close} onClick={onClose}>
            <X size={15} />
          </IconButton>
        </div>
      </div>

      {editing && e && (
        <EntityDialog
          kbId={kbId}
          entityId={entityId}
          entity={e}
          onClose={() => setEditing(false)}
          onSaved={() => setEditing(false)}
        />
      )}


      {/* 视图切换：Relations（一张表，过去的折在组尾）| History（记录轴）| Derived */}
      {/* **左边比头部多一档**（24 而不是 16）。两个盒子本来都从 px-4 起，可
          分段控件自己还有一圈内距（p-1 加按钮的 px-2），于是「Relations」四个字
          落在 29，而标题「OpenAI」落在 35——标题前面是色点加 gap，正好差 6px，
          看着就是这一排比标题往左漏出去一截。加一档之后字落在 37，压回标题上。
          这一条只给这里：本体页那条是 `fill` 的整条，与下面正文同宽，
          它的左缘该跟正文对齐，不跟标题对齐 */}
      <div className="pl-6 pr-4 pt-3">
        <Segmented
          size="sm"
          value={view}
          onChange={setView}
          options={(["relations", "history", "derived"] as const)
            // 推出来的那一档：**没有派生就不出现**。一个没开推理的库不该看到
            // 一个永远是空的标签页。没落地的也算——那正是这一档要说的事
            .filter(
              (v) => v !== "derived" || derived.length > 0 || blocked.length > 0,
            )
            .map((v) => ({
              value: v,
              label:
                v === "relations"
                  ? S.graph.viewRelations
                  : v === "history"
                    ? S.graph.viewHistory
                    : S.graph.viewDerived,
            }))}
        />
      </div>

      <div className="u-scroll flex-1 overflow-y-auto px-2 py-2">
        {view === "relations" &&
          (["out", "in"] as const).map((dir) => {
            const { rows, past } = sections[dir];
            if (rows.length === 0 && past.length === 0) return null;
            const name = e?.name ?? "";
            return (
              <FactSection
                key={dir}
                dir={dir}
                title={dir === "out" ? S.graph.fromEntity(name) : S.graph.toEntity(name)}
                count={rows.length + past.length}
              >
                {rows.map((f) => (
                  <FactRow
                    key={f.id}
                    kbId={kbId}
                    dir={dir}
                    fact={f}
                    open={openFact === f.id}
                    onToggle={() => setOpenFact(openFact === f.id ? null : f.id)}
                    onNavigate={onNavigate}
                  />
                ))}
                {past.length > 0 && (
                  <PastFold n={past.length}>
                    {past.map((f) => (
                      <FactRow
                        key={f.id}
                        kbId={kbId}
                        dir={dir}
                        fact={f}
                        past
                        open={openFact === f.id}
                        onToggle={() => setOpenFact(openFact === f.id ? null : f.id)}
                        onNavigate={onNavigate}
                      />
                    ))}
                  </PastFold>
                )}
              </FactSection>
            );
          })}
        {view === "history" && (
          <EntityHistory kbId={kbId} entityId={entityId} />
        )}
{view === "derived" && (
          <>
            <p className="px-2 pb-2 pt-1 text-fine leading-relaxed text-ink-2">
              {S.graph.derivedHint}
            </p>
            {/* **与 Relations 同一个骨架**：方向箭头 + 谓词 + 条数的小标题，
                底下是紧凑行。规则（传递/对称）并进标题——它对整组都成立，
                挂在每一行上是重复，而且那个 `--u-warn` 琥珀色又是一处
                与派生边抢色相的地方 */}
            {derivedGroups.map((gr) => (
              <div key={gr.key} className="mb-3 last:mb-1">
                <GroupLabel
                  className="px-2 pb-1 pt-2"
                  icon={
                    gr.direction === "in" ? (
                      <ArrowLeft size={10} />
                    ) : (
                      <ArrowRight size={10} />
                    )
                  }
                  count={gr.rows.length > 1 ? gr.rows.length : undefined}
                >
                  {gr.predicate}
                  <span className="ml-2 font-normal text-ink-2">{gr.rule}</span>
                </GroupLabel>
                <div>
                  {gr.rows.map((d) => {
                    const out = d.subject_id === entityId;
                    return (
                      <DerivedRow
                        key={d.id}
                        kbId={kbId}
                        d={d}
                        otherId={out ? d.object_id : d.subject_id}
                        otherName={out ? d.object : d.subject}
                        open={openFact === d.id}
                        onToggle={() =>
                          setOpenFact(openFact === d.id ? null : d.id)
                        }
                        onNavigate={onNavigate}
                      />
                    );
                  })}
                </div>
              </div>
            ))}
            {blocked.length > 0 && (
              <div className="mb-3 last:mb-1">
                <GroupLabel className="px-2 pb-1 pt-2" tone="contest" count={blocked.length}>
                  {S.graph.blockedTitle}
                </GroupLabel>
                <p className="px-2 pb-2 text-fine leading-relaxed text-ink-2">
                  {S.graph.blockedHint}
                </p>
                {blocked.map((b) => (
                  <BlockedRow
                    key={b.violation_id}
                    kbId={kbId}
                    b={b}
                    entityId={entityId}
                    open={openFact === b.violation_id}
                    onToggle={() =>
                      setOpenFact(
                        openFact === b.violation_id ? null : b.violation_id,
                      )
                    }
                    onNavigate={onNavigate}
                  />
                ))}
              </div>
            )}
          </>
        )}
        {view !== "history" &&
          view !== "derived" &&
          detail.data?.facts.length === 0 && (
            <p className="text-body text-ink-2 p-2">{S.graph.noFacts}</p>
          )}
      </div>
    </div>
  );
}

/** 一节（从这个实体出发 / 指向这个实体）：可折叠——折叠柄占图标格，正文缩进同样的 24，
 *  于是每一行的方向箭头正好落在节标题的箭头底下（本体页 Relations 的组同一副） */
function FactSection({
  dir,
  title,
  count,
  children,
}: {
  dir: "out" | "in";
  title: string;
  count: number;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(true);
  return (
    <div className="mb-2">
      <Row
        className="mt-1"
        icon={
          <span className="flex w-4 justify-center">
            <ChevronRight size={12} className={cn("u-turn", open && "rotate-90")} />
          </span>
        }
        onClick={() => setOpen((v) => !v)}
      >
        <span className="flex items-center gap-2 text-small font-medium">
          {/* 与下面每条事实行的方向箭头**同一尺寸**（12）。它们是同一个记号、
              还特意排成一列，标题这个小 2px 就只会读成没对齐 */}
          {dir === "out" ? <ArrowRight size={12} /> : <ArrowLeft size={12} />}
          <span className="truncate">{title}</span>
          <span className="u-num">{count}</span>
        </span>
      </Row>
      {/* **箭头与标题里那个箭头排成一列**（实测都在 40.7）。一度改成 pl-8 想让
          行"挂"在标题下面，结果两个箭头差开 8px，看着就是没对齐——而它们是
          同一个方向记号（→ 出边 / ← 入边），同一个记号本来就该成列。
          层级由前面那个折叠三角表示（在 18.7），不必再靠缩进说第二遍 */}
      {open && <div className="pl-6">{children}</div>}
    </div>
  );
}

/** 已结束的留在同一节里，折起来：默认看现行的，要看来路展开它 */
function PastFold({ n, children }: { n: number; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Row
        icon={
          <span className="flex w-4 justify-center">
            <ChevronRight size={12} className={cn("u-turn", open && "rotate-90")} />
          </span>
        }
        onClick={() => setOpen((v) => !v)}
      >
        <span className="text-small">{S.graph.past(n)}</span>
      </Row>
      {open && <div className="pl-6">{children}</div>}
    </>
  );
}

/** 一条事实是一行，与本体页 Relations 的行同一副：图标格里是方向箭头，左边关系名
 *  （和 disputed 之类的标记），右边那个实体的名字（区间的小字在名字前）；整行点了
 *  跳到那个实体。指着这一行才露出「N sources」（在行下摊开证据）与铅笔（区间修正
 *  弹窗）。行首没有折叠柄——折叠柄在这块面板上只属于节与「过去」的折 */
function FactRow({
  kbId,
  dir,
  fact,
  past,
  open,
  onToggle,
  onNavigate,
}: {
  kbId: string;
  dir: "out" | "in";
  fact: EntityFact;
  /** 已结束的那些：压淡 */
  past?: boolean;
  open: boolean;
  onToggle: () => void;
  onNavigate: (entityId: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const interval = fmtInterval(fact);
  const go = fact.other_id ? () => onNavigate(fact.other_id!) : undefined;

  return (
    <div
      className={cn((fact.stale || past) && "opacity-55")}
      title={fact.stale ? S.graph.staleFactHint : undefined}
    >
      {/* **两行：上面是这条事实，下面是我们对它知道些什么。**
          从前是一行，而那一行里塞着谓词、区间、宾语、证据数、改期笔。分组的
          缩进之后只剩 249px，尾部那一组又是 shrink-0，于是唯一能收缩的谓词
          把亏空全吃了——实测一个实体的 144 行里，35 行的谓词宽度是 0，读起来
          就是「→ 2023-03-02 ~ now  Project Aurora」：说有这么条事实，就是不说
          是哪条（#500）。谓词是这一行的主语句，不该是第一个被挤掉的。
          悬停才现身的那两个动作也一起下来：`u-reveal` 只改透明度，看不见也占着位 */}
      <div
        role={go ? "link" : undefined}
        tabIndex={go ? 0 : undefined}
        onClick={go}
        onKeyDown={(ev) => {
          if (go && ev.key === "Enter") go();
        }}
        className={cn(HOVER_ROW, "items-start", go && "cursor-pointer")}
      >
        <span className="shrink-0 pt-1 text-violet">
          {dir === "out" ? <ArrowRight size={12} /> : <ArrowLeft size={12} />}
        </span>
        <span className="min-w-0 flex-1">
          {/* 第一行：谓词 + 宾语，读出来就是这条事实本身。
              **宾语紧跟着谓词**，不推到右边——主语是面板上那个实体，这一行
              是它后半句话；把宾语顶到行尾，中间隔一整行空白，两个词就不再
              读成一句了。谓词按内容占位、放不下才收，剩下的归宾语 */}
          <span className="flex items-center gap-2">
            <span
              className={cn(
                "min-w-0 truncate text-body text-ink",
                fact.predicate_label === null && "italic text-ink-2",
              )}
              // 谓词还是可能长到放不下（`publishingPrinciples`）——悬停给全名，
              // 是本体认下的关系就不必再说它是原文的说法
              title={
                fact.predicate_label
                  ? fact.inferred
                    ? `${predicateSentence(fact.predicate_label)} · ${S.graph.inferredPredicate}`
                    : predicateSentence(fact.predicate_label)
                  : undefined
              }
            >
              {/* **这一行是一句话，所以谓词拆开读**（见 predicateText.ts）：
                  库里存的是词表该有的样子 `worksFor`，摆进
                  「Li Si — ? — Meridian Systems」中间时读作 “works for”。
                  管本体的那几个界面照旧显示驼峰，那儿认的是词本身 */}
              {fact.predicate_label
                ? predicateSentence(fact.predicate_label)
                : S.graph.unknownPredicate}
            </span>
            <span className={ROW_VALUE}>
              {fact.other_name ?? fmtObjectValue(fact.object_value) ?? "?"}
            </span>
            {/* 边上的属性（0037）：`amount $4B`——跟在宾语后面，不另起一行。
                投了谁和投了多少是同一句话，拆开就读不成一句了 */}
            {fact.qualifiers?.map((q) => (
              <span key={q.qualifier_type_id} className="shrink-0 text-small text-ink-2">
                {q.label || q.key}{" "}
                <span className="text-ink">
                  {q.entity_name ?? fmtObjectValue(q.value) ?? "?"}
                </span>
              </span>
            ))}
          </span>
          {/* 第二行：何时成立、要不要留神、以及看证据与改期的入口。
              **没有日期也要说一句**——空着的时候，「原文没写日期」和
              「有日期只是我没显示」在界面上长得一模一样，而这个产品的全部
              重点就是时间。
              置信度与「区间是对账闭合的」那个 ⟲ 都撤了：一个是数字、一个是
              没人猜得出的符号，两者都只在这一行占位，说不清事。它们在证据
              那一档里用整句话说得明白（见 EvidenceList） */}
          <span className="flex items-center gap-2">
            <span
              className={cn(
                "u-num text-fine",
                interval ? "text-ink-2" : "italic text-ink-2",
              )}
            >
              {interval || S.graph.undated}
            </span>
            {fact.stale && (
              <Chip tone="neutral" className="shrink-0 text-fine">
                {S.graph.staleFactChip}
              </Chip>
            )}
            {fact.contested && <ContestedChip kbId={kbId} c={fact.contested} />}
            <span className="ml-auto flex shrink-0 items-center gap-2 pl-2">
              {fact.evidence_count > 0 && (
                <LinkButton
                  className={cn(REVEAL, "text-fine", open && "is-on")}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    onToggle();
                  }}
                >
                  {S.graph.sources(fact.evidence_count)}
                </LinkButton>
              )}
              {/* 这一档只有断言事实：派生的区间是算出来的，走 Derived 那条路径 */}
              <span
                role="button"
                tabIndex={0}
                title={S.graph.editTime}
                aria-label={S.graph.editTime}
                onClick={(ev) => {
                  ev.stopPropagation();
                  setEditing(true);
                }}
                onKeyDown={(ev) => {
                  if (ev.key === "Enter" || ev.key === " ") {
                    ev.preventDefault();
                    ev.stopPropagation();
                    setEditing(true);
                  }
                }}
                className={cn(REVEAL, "cursor-pointer rounded-cell p-1 text-ink-2")}
              >
                <Pencil size={10} />
              </span>
            </span>
          </span>
        </span>
      </div>
      {open && (
        <div className="pb-2 pl-2 pr-2">
          <EvidenceList kbId={kbId} fact={fact} />
        </div>
      )}
      {editing && (
        <FactTimeDialog kbId={kbId} fact={fact} onClose={() => setEditing(false)} />
      )}
    </div>
  );
}

/** 证据展开区（FactRow 在行下摊开）：quote + 跳原文 + 版本角标 + 置信。 */
function EvidenceList({ kbId, fact }: { kbId: string; fact: EntityFact }) {
  const evidence = useQuery({
    queryKey: ["evidence", fact.id],
    queryFn: () => api.factEvidence(kbId, fact.id),
  });
  return (
    /* 多段就滚，不把整个面板顶长。一条事实最多见过十几段证据，全摊开的话
       它下面那些事实全被挤出屏幕——而展开一条是为了读它，不是为了失去上下文 */
    <div className="u-scroll max-h-64 space-y-2 overflow-y-auto">
      {evidence.data?.evidence.map((ev: Evidence) => (
        /* **一段原文一张卡，卡本身不是链接。**
           从前整块是个 `<Link>`：想读原文，手一动就跳去了文档页；想选一句
           复制，松手也是跳走。展开这个动作要回答的是「凭什么这么说」，
           那句话就在这儿，读完了才谈得上要不要去看上下文——所以跳转收进
           末尾那个小角标，点它才走。
           底色取最低那一档，**而且没有悬停态**：整张卡不可点，给它一个高亮
           等于在骗手；会响应的只有末尾那个角标，它自己有 `u-hover-ink` */
        <div
          key={ev.chunk_id}
          className="rounded-cell bg-surface px-2 py-2 text-small text-ink-2"
        >
          {/* 原文说的谓词，只在它与事实行上显示的不同时才写出来。本体外的谓词
              事实行上已经显示原文说法（0052），相同的话再写一遍是噪声；
              一条事实有多种说法时（占 3%）这里才有话说 */}
          {ev.proposed_predicate &&
            ev.proposed_predicate !== fact.predicate_key && (
              <div className="mb-1 text-fine text-ink-2">
                {S.graph.proposedPredicate(ev.proposed_predicate)}
              </div>
            )}
          {/* **不截断**。从前是 line-clamp-2，于是「看原文」看到的是原文的
              前两行——想读全的唯一办法是跳去文档页，那就等于没有展开这一档 */}
          <div className="whitespace-pre-wrap italic text-ink">
            {ev.quote ? `“${ev.quote}”` : S.graph.noQuote}
          </div>
          <div className="mt-2 flex items-center gap-2">
            {/* 小角标：出处 + 去文档页看上下文。这是这张卡上唯一会走人的地方 */}
            <Link
              to="/kb/$kbId/doc/$docId"
              params={{ kbId, docId: ev.document_id }}
              search={{ chunk: ev.chunk_id }}
              className="u-hover-ink inline-flex min-w-0 items-center gap-1 text-fine text-ink-2"
              title={S.graph.openInDoc}
            >
              <span className="truncate">
                {S.graph.sectionRef(ev.filename, ev.seq + 1)}
              </span>
              <ExternalLink size={11} className="shrink-0" />
            </Link>
            {ev.stale && (
              <span
                className="u-num shrink-0 text-fine text-ink-2"
                title={S.graph.staleEvidenceHint}
              >
                {S.graph.fromVersion(ev.doc_version)}
              </span>
            )}
            {ev.document_deleted && (
              <span
                className="shrink-0 text-fine text-contest"
                title={S.graph.sourceDeletedHint}
              >
                {S.graph.sourceDeleted}
              </span>
            )}
          </div>
        </div>
      ))}
      {evidence.data?.evidence.length === 0 && (
        <p className="text-small text-ink-2">{S.graph.noEvidence}</p>
      )}
      {/* 这条区间不是原文写的，是引擎对账或人工裁决闭合的。**从事实行搬到这里**：
          在行上它是一个 ⟲，谁也猜不出是什么意思；证据这一档本来就在回答
          「凭什么这么说」，一句话说得明白 */}
      {fact.corrected && (
        <p className="text-fine text-ink-2">{S.graph.correctedHint}</p>
      )}
      {/* 置信度只在低到值得怀疑时说话（与 Review 低置信口径一致），常规不标 */}
      {fact.confidence < 0.75 && (
        <p className="text-fine text-warn">
          {Math.round(fact.confidence * 100)}% {S.graph.confidence}
        </p>
      )}
    </div>
  );
}
