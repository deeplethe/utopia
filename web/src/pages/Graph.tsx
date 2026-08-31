import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useSearch } from "@tanstack/react-router";
import Graphology from "graphology";
import { circular, circlepack } from "graphology-layout";
import forceAtlas2 from "graphology-layout-forceatlas2";
import FA2Layout from "graphology-layout-forceatlas2/worker";
import Sigma from "sigma";
import { createNodeBorderProgram } from "@sigma/node-border";
import { NodeSquareShellProgram } from "./squareShellProgram";
import { EntityHistory } from "./EntityHistory";
import {
  ArrowLeft,
  ArrowRight,
  ChevronRight,
  CircleDashed,
  Grape,
  Loader2,
  Maximize2,
  Orbit,
  Pause,
  Pencil,
  Play,
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
} from "../api";
import { S } from "../i18n";
import { usePopoverFlip } from "../ui/popoverFlip";
import { useKb } from "../kb";
import { toast } from "../toast";

/* 画布调色板 —— 结构取自 Semantica GraphWorkspace 源码；基色已中性化：
   Semantica 原版是钢蓝系（#0B1320/#5A7A9E/#7A92AE），按"chrome 零色偏、
   彩色只属于数据"的既定原则换成同明度纯灰，类型色混入比例不变 */
const NODE_SHELL_BASE = "#121212"; // 节点外壳深底（原 #0B1320 的中性化）
const NODE_CORE_BASE = "#767676"; // 节点核心灰（原 #5A7A9E 的中性化）
const NODE_BORDER_BASE = "#909090"; // 节点描边（原 #7A92AE 的中性化）
const NODE_TINT_MIX = 0.14; // 类型色只按 14% 混入外壳（高级感的关键）
const NODE_CORE_MIX = 0.5; // 核心向类型色的混入比例
/* 状态环取**节点自己的类型色**，不是两个写死的色相。

   换掉的直接原因是一次撞色：原来的选中金环 `#E7C57C` 就是
   `rgb(231,197,124)`，与 `EDGE_COLOR_DERIVED` 逐位相同——「这个节点被选中了」
   和「这条边是推出来的」用同一个颜色说话，而这两件事毫无关系。
   金色现在专属于「推出来的」。

   往白里混而不是直接用原色：环画在节点自己身上，同色同亮度就看不出是个环。
   **悬停混得更白、选中混得更少**——悬停时全图不压暗，环要在一片乱线里
   立刻跳出来；选中时其余都压暗了，节点本来就孤立着，这时候环该说的是
   「它是谁」，所以更贴近它自己的颜色。 */
const RING_HOVER_MIX = 0.7; // 悬停：偏白，为的是跳出来
const RING_SELECT_MIX = 0.35; // 选中：偏本色，为的是认得出
const EDGE_COLOR = "rgba(163,163,163,0.2)"; // 纯灰（应用户要求，不用钢蓝）
// 本体没认下的关系：同色更淡。名字来自原文，不该跟词表里的关系看着一样重
const EDGE_COLOR_INFERRED = "rgba(163,163,163,0.1)";
// 推出来的边（R1）。**跟上面两者说的不是一件事**：那两个说「这条边的名字从哪来」，
// 这个说「这条边根本不是谁说的，是引擎推的」。所以给它自己的色相而不是再淡一档灰——
// 用户要在余光里就分得出「文档里写的」和「推出来的」
const EDGE_COLOR_DERIVED = "rgba(231,197,124,0.42)";
const EDGE_COLOR_DERIVED_DIM = "rgba(231,197,124,0.14)";
// 呼吸周期。动画不是为了好看，是因为静态的一个色差在几百条边里根本注意不到
const DERIVED_PULSE_MS = 2200;
// 超过这个数就只上色不动画。**写出来而不是悄悄降级**：每帧重算几千条边的颜色，
// 换来的是拖不动图，而那时候用户要的是能拖得动
const DERIVED_ANIMATE_MAX = 400;
// 开关的淡入淡出时长。**比 FADE_MS(320) 略长**：播放淡入是一批边陆续到位，
// 这个是一整批边同时进出，走慢一点才看得清「那批金线是一起退场的」
const DERIVED_TOGGLE_MS = 420;
// 图例最多摆几个胶囊，其余收进「+N 个类」。**这一排是横向排布的，
// 类一多就会换行、把画布顶到下面去**；而且十几个同样的胶囊排开，
// 谁也读不出哪个重要。收起来的那些从「+N」里搜得到
const LEGEND_MAX = 6;
/* 画多少个节点的可选档位。**给档位而不是给输入框**：这个数没有「精确」可言
   ——它只影响看得清还是拖得动，用户要的是「多点/少点」，不是 237 这个数。
   最大值与后端 GRAPH_NODE_CAP_MAX 对齐；再高先垮的是拖动，不是清晰度 */
const NODE_BUDGETS: number[] = [150, 300, 600, 1000];
// 注意：sigma 边着色器在预乘混合(ONE, ONE_MINUS_SRC_ALPHA)下不预乘 RGB，
// alpha 无法压暗边——暗度必须编码进 RGB（不透明近背景色）
const EDGE_DIM = "#141414";
const EDGE_FOCUS = "rgba(255,255,255,0.55)";
// 选中/悬停时的派生边。**不能跟着走白**：选中恰恰是看得最仔细的时候，
// 而这时候「这条边是推出来的、没人写过」比任何时候都该说清楚。
// 从前一律 EDGE_FOCUS，一选中金线就变白，等于把来历抹掉了。
// 比常态的金更亮更实——它同样要表达「被选中了」
const EDGE_FOCUS_DERIVED = "rgba(255,214,140,0.95)";
const MUTED_SHELL = "#151515";
const PILL_BG = "rgba(12,12,12,0.9)";
const PILL_BORDER = "rgba(255,255,255,0.14)";
const PILL_TEXT = "#ededed";
const TRANSPARENT = "rgba(0,0,0,0)";
const DAY_MS = 24 * 3600 * 1000;

function hexToRgb(hex: string): [number, number, number] {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex);
  if (!m) return [128, 128, 128];
  const v = parseInt(m[1], 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}

/** c1 向 c2 按 t 比例混色 */
function mix(c1: string, c2: string, t: number): string {
  const [r1, g1, b1] = hexToRgb(c1);
  const [r2, g2, b2] = hexToRgb(c2);
  const f = (a: number, b: number) => Math.round(a + (b - a) * t);
  return `rgb(${f(r1, r2)},${f(g1, g2)},${f(b1, b2)})`;
}

/* 播放淡入：解析 hex / rgb / rgba（含 alpha）并线性插值 */
function parseRgba(c: string): [number, number, number, number] {
  if (c.startsWith("#")) {
    const [r, g, b] = hexToRgb(c);
    return [r, g, b, 1];
  }
  const m = c.match(
    /rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,\s/]+([\d.]+))?/,
  );
  if (!m) return [128, 128, 128, 1];
  return [+m[1], +m[2], +m[3], m[4] !== undefined ? +m[4] : 1];
}
function lerpColor(from: string, to: string, t: number): string {
  const a = parseRgba(from);
  const b = parseRgba(to);
  const f = (i: number) => a[i] + (b[i] - a[i]) * t;
  return `rgba(${Math.round(f(0))},${Math.round(f(1))},${Math.round(f(2))},${f(3).toFixed(3)})`;
}
/** 播放中新元素的淡入时长 */
const FADE_MS = 320;

/* 世界坐标网格：随相机平移/缩放（Figma/tldraw 式无限画布惯例）。
   4 倍细分 LOD：每层 alpha 随其屏幕间距连续淡入（13px 进场 → 52px 满亮 5.5%），
   粗层与细层线重合处自然叠亮，形成"大小格"层次；无任何跳变。 */
const GRID_BASE_WORLD = 24; // 基准世界格距（匹配 ~300 尺度的布局）
const GRID_FADE_IN_PX = 13;
const GRID_FULL_PX = 52;
const GRID_MAX_LEVEL_PX = 480;
const GRID_MAX_ALPHA = 0.055;

function drawWorldGrid(canvas: HTMLCanvasElement, sigma: Sigma): void {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const { width, height } = sigma.getDimensions();
  const dpr = window.devicePixelRatio || 1;
  const pw = Math.round(width * dpr);
  const ph = Math.round(height * dpr);
  if (canvas.width !== pw || canvas.height !== ph) {
    canvas.width = pw;
    canvas.height = ph;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  if (width <= 0 || height <= 0) return;

  // 世界→屏幕：两个探针点求每世界单位像素数与原点位置（无相机旋转场景）
  const p0 = sigma.graphToViewport({ x: 0, y: 0 });
  const p1 = sigma.graphToViewport({ x: 1, y: 0 });
  const ppw = p1.x - p0.x;
  if (!Number.isFinite(ppw) || ppw <= 0) return;

  // 最细可见层级：屏幕间距 ≥ 淡入阈值的最小 4 幂格距
  let spacing = GRID_BASE_WORLD;
  while (spacing * ppw < GRID_FADE_IN_PX) spacing *= 4;
  while (spacing * ppw >= GRID_FADE_IN_PX * 4) spacing /= 4;

  for (let sp = spacing; sp * ppw < GRID_MAX_LEVEL_PX; sp *= 4) {
    const ss = sp * ppw;
    const t = Math.min(
      1,
      (ss - GRID_FADE_IN_PX) / (GRID_FULL_PX - GRID_FADE_IN_PX),
    );
    if (t <= 0) continue;
    ctx.strokeStyle = `rgba(255,255,255,${(GRID_MAX_ALPHA * t).toFixed(4)})`;
    ctx.lineWidth = 1;
    ctx.beginPath();
    const startX = ((p0.x % ss) + ss) % ss;
    for (let x = startX; x <= width; x += ss) {
      const px = Math.round(x) + 0.5;
      ctx.moveTo(px, 0);
      ctx.lineTo(px, height);
    }
    const startY = ((p0.y % ss) + ss) % ss;
    for (let y = startY; y <= height; y += ss) {
      const py = Math.round(y) + 0.5;
      ctx.moveTo(0, py);
      ctx.lineTo(width, py);
    }
    ctx.stroke();
  }
}

/* 胶囊标签：深色圆角底 + 柔和文字（学 Semantica 的浮签风格） */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function drawPillLabel(
  ctx: CanvasRenderingContext2D,
  data: any,
  settings: any,
): void {
  if (!data.label) return;
  // hover 时悬浮卡（drawHoverCard）接管展示，底层 pill 隐去，避免双层标签
  if (data.hideBaseLabel) return;
  // Semantica chip: fontSize=clamp(10, size*0.25, 11), pad 6/3, radius 6, 位于节点上方，投影 blur 12
  const size = Math.max(10, Math.min(11, data.size * 0.25));
  ctx.font = `500 ${size}px Geist, Inter, "Noto Sans SC", sans-serif`;
  ctx.textBaseline = "middle";
  const padX = 6;
  const padY = 3;
  const w = ctx.measureText(data.label).width + padX * 2;
  const h = size + padY * 2;
  const x = data.x + Math.max(data.size * 0.7, 12);
  const y = data.y - Math.max(data.size * 0.9, 10) - h;
  ctx.save();
  ctx.shadowColor = "rgba(0,0,0,0.6)";
  ctx.shadowBlur = 12;
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, 6);
  ctx.fillStyle = PILL_BG;
  ctx.fill();
  ctx.shadowBlur = 0;
  ctx.strokeStyle = PILL_BORDER;
  ctx.lineWidth = 1;
  ctx.stroke();
  ctx.fillStyle = PILL_TEXT;
  ctx.fillText(data.label, x + padX, y + h / 2);
  ctx.restore();
}

/* Hover 悬浮卡（Semantica hoverCard 规格）：径向柔光 + 名称 + 类型行 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function drawHoverCard(
  ctx: CanvasRenderingContext2D,
  data: any,
  _settings: any,
): void {
  if (!data.label) return;
  ctx.save();

  // 柔光: 半径 max(size*4.8, 16), 类型色 alpha 0.18 → 0
  const glowR = Math.max(data.size * 4.8, 16);
  const [r, g, b] = hexToRgb((data.typeColor as string) ?? "#888888");
  const grad = ctx.createRadialGradient(
    data.x,
    data.y,
    0,
    data.x,
    data.y,
    glowR,
  );
  grad.addColorStop(0, `rgba(${r},${g},${b},0.18)`);
  grad.addColorStop(1, `rgba(${r},${g},${b},0)`);
  ctx.fillStyle = grad;
  ctx.beginPath();
  ctx.arc(data.x, data.y, glowR, 0, Math.PI * 2);
  ctx.fill();

  // 卡片: 标题 700/13 + 类型行 500/10 大写
  const titleSize = 13;
  const metaSize = 10;
  const padX = 10;
  const padY = 7;
  const metaGap = 5;
  const meta = String(data.typeLabel ?? "NODE").toUpperCase();
  ctx.textBaseline = "top";
  ctx.font = `700 ${titleSize}px Geist, Inter, "Noto Sans SC", sans-serif`;
  const titleW = ctx.measureText(data.label).width;
  ctx.font = `500 ${metaSize}px Geist, Inter, sans-serif`;
  const metaW = ctx.measureText(meta).width;
  const w = Math.max(titleW, metaW) + padX * 2;
  const h = padY * 2 + titleSize + metaGap + metaSize;
  const x = data.x + Math.max(data.size * 0.9, 16);
  const y = data.y - Math.max(data.size * 1.1, 16) - h;

  ctx.shadowColor = "rgba(0,0,0,0.62)";
  ctx.shadowBlur = 15;
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, 8);
  ctx.fillStyle = "rgba(12,12,12,0.94)";
  ctx.fill();
  ctx.shadowBlur = 0;
  ctx.strokeStyle = "rgba(255,255,255,0.16)";
  ctx.lineWidth = 1;
  ctx.stroke();

  ctx.fillStyle = "#f5f5f5";
  ctx.font = `700 ${titleSize}px Geist, Inter, "Noto Sans SC", sans-serif`;
  ctx.fillText(data.label, x + padX, y + padY);
  ctx.fillStyle = "rgba(255,255,255,0.5)";
  ctx.font = `500 ${metaSize}px Geist, Inter, sans-serif`;
  ctx.fillText(meta, x + padX, y + padY + titleSize + metaGap);
  ctx.restore();
}

export function Graph() {
  const { kb } = useKb();
  // 深链入口：/graph?entity=… 直接聚焦并选中该实体（证据两跳可达的反向路径）
  const { entity: entityParam } = useSearch({ from: "/app/graph" });
  const [focusEntity, setFocusEntity] = useState<string | null>(
    entityParam ?? null,
  );
  const [selected, setSelected] = useState<string | null>(entityParam ?? null);
  const [searchInput, setSearchInput] = useState("");
  const [searchQ, setSearchQ] = useState("");
  const [hiddenTypes, setHiddenTypes] = useState<Set<string>>(new Set());
  // 推出来的边显不显示。默认显示——推理默认关着，有派生就意味着用户开过开关
  const [showDerived, setShowDerived] = useState(true);
  // 信息窗默认收起：它答的是「什么时候推的」，那是偶尔才问的问题
  /* Inference 也用原地展开，与「+N 个类」、通知、用户菜单同一套。
     **贴左下角**：塔在画布左下，面板要从那个 ⋯ 按钮往右上长开 */
  const derivedPop = usePopoverFlip<HTMLButtonElement, HTMLDivElement>(
    "bottom left",
  );
  /* 「+N 个类」用与通知/用户卡片同一套原地展开：面板压到 chip 的真实边界
     （圆角 999px）再长成卡片。**贴左边，所以锚点角是 top left** */
  const legendPop = usePopoverFlip<HTMLButtonElement, HTMLDivElement>(
    "top left",
  );
  const [legendQ, setLegendQ] = useState("");
  /* 正在退场的实体。**面板不能一取消选中就卸载**——那样它是瞬间消失的。
     先留在原地演完退场，再真的移除。用 selectedRef 取当前值而不是把
     setState 写成带副作用的 updater：那种写法在 StrictMode 下会跑两遍 */
  const [exiting, setExiting] = useState<string | null>(null);
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
  const [timeT, setTimeT] = useState<number | null>(() => Date.now());
  const [activeCount, setActiveCount] = useState(0);
  const [stabilizing, setStabilizing] = useState(false);
  /* 播放态提升到此层：reducer 需区分"播放推进"（淡入）与"手动拖动"（瞬切） */
  const [playing, setPlaying] = useState(false);
  /* 布局模式：force = FA2 斥力；circular = 圆环；pack = 按类型圆填充聚簇 */
  type LayoutMode = "force" | "circular" | "pack";
  const [layoutMode, setLayoutMode] = useState<LayoutMode>("force");
  const layoutModeRef = useRef<LayoutMode>("force");
  const layoutCtlRef = useRef<{ apply: (m: LayoutMode) => void } | null>(null);

  /* 画多少个。**进 queryKey**——不进的话调了档位不会重新取数，
     界面看着变了实际还是老数据 */
  const [nodeBudget, setNodeBudget] = useState<number>(NODE_BUDGETS[0]);

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
          const vf = e.valid_from ? Date.parse(e.valid_from) : null;
          const vt = e.valid_to ? Date.parse(e.valid_to) : null;
          touched.add(e.source);
          touched.add(e.target);
          const active =
            vf === null ? true : vf <= t && (vt === null || vt > t);
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
          type: n.shape === "square" ? "square" : "shell",
          size: 5 + Math.min(8, Math.sqrt(Number(n.degree)) * 1.6),
        });
      }
    }
    for (const e of data.data.edges) {
      if (g.hasNode(e.source) && g.hasNode(e.target)) {
        g.addEdgeWithKey(e.id, e.source, e.target, {
          label: e.label?.toUpperCase() ?? "",
          size: 1,
          color: e.derived
            ? EDGE_COLOR_DERIVED
            : e.inferred
              ? EDGE_COLOR_INFERRED
              : EDGE_COLOR,
          type: "line",
          // reducer 每帧读它：决定要不要藏、要不要呼吸
          derived: e.derived,
        });
      }
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
      /* 力的大小随规模走。**原来是两个写死的常量（gravity 0.35 / scalingRatio 22），
         那是画布上限还钉在 150 个节点时调的**——现在右上角能把上限调到 1000，
         同样的力落在几百个节点上就过猛：斥力把外圈甩得很开，重力又往回拽，
         两股劲对着使，画面在收敛期一直在弹。

         三个数各管一件事：
         - scalingRatio 是**斥力**。节点多了要调小，否则外圈会被甩出视野。
         - gravity 是**往中心的拉力**。它只负责别让孤立点飘走，不该跟斥力较劲。
         - slowDown 是**阻尼**，从前完全没管（吃 inferSettings 的 1+ln(n)）。
           "力气太大"最直接的解法其实是这个：同样的力，走慢一点就不弹。

         参照：graphology 对这个规模推断出的默认值约是 gravity 0.1 / scalingRatio 10，
         我们仍略高于它——这张图要的是"散得开、看得清"，不是最紧凑的那种排布 */
      const big = g.order > 400;
      const settings = {
        ...forceAtlas2.inferSettings(g),
        gravity: big ? 0.12 : 0.22,
        scalingRatio: big ? 11 : 16,
        slowDown: (1 + Math.log(Math.max(2, g.order))) * 1.8,
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

    sigmaRef.current?.kill();
    const sigma = new Sigma(g, containerRef.current, {
      allowInvalidContainer: true,
      defaultNodeType: "shell",
      nodeProgramClasses: {
        // Semantica 节点解剖：状态环 → 描边 → 深色壳 → 微彩核心
        shell: createNodeBorderProgram({
          borders: [
            { size: { value: 0.1 }, color: { attribute: "ringColor" } },
            { size: { value: 0.07 }, color: { attribute: "borderColor" } },
            { size: { value: 0.3 }, color: { attribute: "shellColor" } },
            { size: { fill: true }, color: { attribute: "color" } },
          ],
        }),
        square: NodeSquareShellProgram,
      },
      renderEdgeLabels: true,
      defaultEdgeType: "line",
      labelFont: '"Geist", "Inter", "Noto Sans SC", sans-serif',
      labelSize: 11,
      labelColor: { color: "#e5e5e5" },
      labelRenderedSizeThreshold: 6,
      labelDensity: 0.7,
      labelGridCellSize: 140,
      minCameraRatio: 0.04,
      maxCameraRatio: 8,
      edgeLabelSize: 9,
      edgeLabelColor: { color: "#a1a1a1" },
      edgeLabelFont: '"Geist", "Inter", sans-serif',
      defaultDrawNodeLabel: drawPillLabel,
      defaultDrawNodeHover: drawHoverCard,
      nodeReducer: (node, attrs) => {
        const f = filterRef.current;
        const res = { ...attrs };
        const base = attrs.size as number;
        // 状态环取节点自己的类型色（见 RING_*_MIX 处的理由）
        const ownColor = (attrs.typeColor as string) ?? NODE_CORE_BASE;
        if (f.hiddenTypes.has(attrs.typeKey as string)) {
          res.hidden = true;
          return res;
        }
        // Semantica 状态表: muted { ×0.52, 全层压暗 }
        const muteNode = () => {
          res.size = base * 0.52;
          res.color = mix(MUTED_SHELL, NODE_CORE_BASE, 0.3);
          res.shellColor = MUTED_SHELL;
          res.borderColor = TRANSPARENT;
          res.ringColor = TRANSPARENT;
          res.label = "";
          res.zIndex = 0;
        };
        // hover 只提亮自身（不压暗全图）；压暗聚焦只属于点击选中
        if (hoverRef.current === node) {
          res.size = Math.max(base * 1.08, 10.4);
          res.ringColor = mix(ownColor, "#ffffff", RING_HOVER_MIX);
          // 悬浮卡接管标签展示；label 本身保留（悬浮卡靠它渲染标题）
          res.hideBaseLabel = true;
          res.zIndex = 4;
          return res;
        }
        // 选中实体可能不在当前画布（侧栏跳转/邻域重载间隙）——不在则跳过聚焦压暗逻辑
        const sel =
          selectedRef.current && g.hasNode(selectedRef.current)
            ? selectedRef.current
            : null;
        if (sel) {
          if (node === sel) {
            res.size = Math.max(base * 1.02, 9.2);
            res.ringColor = mix(ownColor, "#ffffff", RING_SELECT_MIX);
            res.forceLabel = true;
            res.zIndex = 3;
            return res;
          }
          if (g.areNeighbors(sel, node)) {
            // neighbor {×0.76, min 4, zIndex 2}
            res.size = Math.max(base * 0.76, 4);
            res.zIndex = 2;
          } else {
            muteNode();
            return res;
          }
        } else {
          // default {×0.7}
          res.size = base * 0.7;
        }
        if (f.activeNodes && !f.activeNodes.has(node)) {
          muteNode();
          return res;
        }
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
            // 由金渐灭到近背景色。**暗度必须编码进 RGB**（见 EDGE_DIM 处的注释：
            // 预乘混合下 alpha 压不暗边），所以是往 EDGE_DIM 混而不是降 alpha
            res.color = lerpColor(EDGE_COLOR_DERIVED, EDGE_DIM, k);
            res.label = "";
            return res;
          }
          const pulse = lerpColor(
            EDGE_COLOR_DERIVED_DIM,
            EDGE_COLOR_DERIVED,
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
          res.color = isDerived ? EDGE_FOCUS_DERIVED : EDGE_FOCUS;
          res.size = Math.max((attrs.size as number) * 1.42, 1.85);
          res.zIndex = 5;
        };
        if (hov && (s === hov || t === hov)) {
          boost();
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
    sigma.on("doubleClickNode", ({ node, event }) => {
      event.preventSigmaDefault();
      setFocusEntity(node);
      setSelected(node);
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
    // 边标签只在放大后出现（默认视距下太密，Semantica 同样克制）
    const updateEdgeLabels = () =>
      sigma.setSetting("renderEdgeLabels", sigma.getCamera().ratio < 0.7);
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
    let dragCandidate: string | null = null;
    let downPoint: { x: number; y: number } | null = null;
    sigma.on("downNode", (e) => {
      dragCandidate = e.node;
      downPoint = { x: e.event.x, y: e.event.y };
    });
    sigma.getMouseCaptor().on("mousemovebody", (e) => {
      if (!dragCandidate) return;
      if (!dragged) {
        if (!downPoint || Math.hypot(e.x - downPoint.x, e.y - downPoint.y) < 4)
          return;
        // 升格为拖拽
        dragged = dragCandidate;
        if (settleTimer) clearTimeout(settleTimer);
        // 静态布局（circular/pack）下拖拽不唤醒力模拟——否则一碰就散架
        if (layoutModeRef.current === "force" && fa2 && !fa2.isRunning())
          fa2.start();
        // 固定当前包围盒，避免拖拽时相机自动跟随缩放
        if (!sigma.getCustomBBox()) sigma.setCustomBBox(sigma.getBBox());
      }
      const pos = sigma.viewportToGraph(e);
      dragPos = pos;
      g.setNodeAttribute(dragged, "x", pos.x);
      g.setNodeAttribute(dragged, "y", pos.y);
      // 阻止相机平移
      e.preventSigmaDefault();
      e.original.preventDefault();
      e.original.stopPropagation();
    });
    const endDrag = () => {
      dragCandidate = null;
      downPoint = null;
      if (!dragged) return;
      dragged = null;
      dragPos = null;
      settleTimer = setTimeout(() => fa2?.stop(), 1200);
    };
    sigma.getMouseCaptor().on("mouseup", endDrag);
    sigmaRef.current = sigma;
    if (import.meta.env.DEV) {
      // 调试句柄（仅 dev）：无头环境下检查 reducer 输出
      (window as unknown as Record<string, unknown>).__g = g;
      (window as unknown as Record<string, unknown>).__sigma = sigma;
      (window as unknown as Record<string, unknown>).__sel = selectedRef;
    }
    recomputeActive(timeT);
    return () => {
      if (stabilizeTimer) clearTimeout(stabilizeTimer);
      if (settleTimer) clearTimeout(settleTimer);
      fa2?.kill();
      setStabilizing(false);
      sigma.kill();
      sigmaRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data.data]);

  if (!kb)
    return <div className="p-8 text-sm text-neutral-500">{S.nav.loading}</div>;

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
          <input
            className="input-dark w-60 px-3 py-1.5 text-sm shadow-lg"
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
            <div className="glass-strong absolute mt-1 w-full rounded-lg shadow-xl overflow-hidden">
              {searchHits.map((c) => (
                <button
                  key={c.id}
                  onClick={() => {
                    // 子图内命中：只选中（已在视野里）；全图搜索：跳到该实体邻域
                    if (!inSubgraph) setFocusEntity(c.id);
                    setSelected(c.id);
                    setSearchInput("");
                    setSearchQ("");
                  }}
                  className="w-full px-3 py-1.5 text-left text-sm text-neutral-200 hover:bg-white/5 flex items-center gap-2"
                >
                  <span
                    className="h-2.5 w-2.5 rounded-full shrink-0"
                    style={{ background: c.color }}
                  />
                  <span className="truncate">{c.name}</span>
                  {c.disambiguator && (
                    <span className="text-xs text-neutral-500 truncate">
                      · {c.disambiguator}
                    </span>
                  )}
                  <span className="ml-auto text-xs text-neutral-500">
                    {c.type_label}
                  </span>
                </button>
              ))}
              {/* 还有更多没显示。**说清剩多少**——从前固定十条，想找的那个
                  不在这十条里的时候，界面上一点线索都没有。子图内搜索是客户端
                  过滤，没有「更多」这回事 */}
              {!inSubgraph &&
                (candidates.data?.total ?? 0) > searchHits.length && (
                  <button
                    onClick={() => setSearchLimit((n) => n + 20)}
                    className="w-full border-t border-white/10 px-3 py-1.5 text-left text-xs text-neutral-400 hover:bg-white/5 hover:text-neutral-200"
                  >
                    {S.graph.searchMore(
                      candidates.data!.total - searchHits.length,
                    )}
                  </button>
                )}
            </div>
          )}
        </div>
        {focusEntity && (
          <button
            onClick={() => setFocusEntity(null)}
            className="u-btn u-btn-ghost glass-strong pointer-events-auto px-3 py-1.5 text-sm shadow-lg"
          >
            {S.graph.backToOverview}
          </button>
        )}

        {/* 图例（点击切换类型显隐）。**只摆前 LEGEND_MAX 个**，其余收进
            「+N 个类」——那一排横着长，类一多就换行把画布顶下去；而且十几个
            一模一样的胶囊排开，谁重要也读不出来 */}
        <div className="pointer-events-auto flex flex-wrap gap-1.5 pt-0.5">
          {legendShown.map(([key, t]) => (
            <button
              key={key}
              onClick={() =>
                setHiddenTypes((prev) => {
                  const next = new Set(prev);
                  if (next.has(key)) next.delete(key);
                  else next.add(key);
                  return next;
                })
              }
              className={`glass rounded-full px-2.5 py-1 text-[11px] flex items-center gap-1.5 transition-opacity ${
                hiddenTypes.has(key) ? "opacity-35" : ""
              }`}
            >
              <span
                className={`h-2 w-2 ${t.shape === "square" ? "" : "rounded-full"}`}
                style={{ background: t.color }}
              />
              <span className="text-neutral-300">{t.label}</span>
            </button>
          ))}

          {/* chip 上的数是**全部类**，不是被收起来的那几个——
              点开看到的就是全部（搜得到任何一个），写「+3」等于承诺了另一件事 */}
          {/* 复位。**只要存在隐藏就给一步到位的出口**——「只看」很容易把
              画面收得很窄，没有这个就得挨个点回来 */}
          {hiddenTypes.size > 0 && (
            <button
              onClick={() => setHiddenTypes(new Set())}
              className="glass rounded-full px-2.5 py-1 text-[11px] text-neutral-400 transition-colors hover:text-neutral-100"
            >
              {S.graph.legendShowAll(hiddenTypes.size)}
            </button>
          )}

          {legendRest.length > 0 && (
            <div className="relative" ref={legendPop.rootRef}>
              <button
                ref={legendPop.anchorRef}
                onClick={() =>
                  legendPop.open ? legendPop.close() : legendPop.setOpen(true)
                }
                title={S.graph.legendAllHint}
                aria-expanded={legendPop.open}
                className={`glass rounded-full px-2.5 py-1 text-[11px] flex items-center gap-1.5 transition-colors ${
                  legendPop.open ? "text-neutral-100" : "text-neutral-400"
                } hover:text-neutral-100`}
              >
                {S.graph.legendMore(types.length)}
                {/* 收起来的类里有正被隐藏的就点一下。**不点就是无声过滤**：
                    在面板里关掉一个类、把面板一收，界面上再没有任何东西说它被关了 */}
                {hiddenInRest > 0 && (
                  <span className="h-1.5 w-1.5 rounded-full bg-neutral-300" />
                )}
              </button>
              {legendPop.open && (
                <div
                  ref={legendPop.panelRef}
                  className="u-menu-glass absolute left-0 top-0 z-50 w-64 overflow-hidden rounded-xl p-2 shadow-2xl"
                >
                  {/* 面板盖在 chip 原位，所以**第一行就长成那个 chip 的样子**，
                      点它收回去——「哪儿展开的就从哪儿收回去」，
                      与通知/用户卡片的关闭键跟触发键原位重合是同一个道理 */}
                  <button
                    onClick={() => legendPop.close()}
                    className="mb-1.5 flex w-full items-center gap-1.5 rounded-full px-1.5 py-0.5 text-[11px] text-neutral-300 transition-colors hover:text-neutral-100"
                  >
                    {S.graph.legendMore(types.length)}
                    <X size={11} className="ml-auto text-neutral-500" />
                  </button>
                  <input
                    autoFocus
                    value={legendQ}
                    onChange={(e) => setLegendQ(e.target.value)}
                    placeholder={S.graph.legendSearch}
                    className="input-dark mb-1.5 w-full px-2 py-1 text-[12px]"
                  />
                  {/* **列的是全部类，不只是收起来的那些**：想找一个类的时候，
                      没人记得它是不是恰好排进了前几个 */}
                  <div className="flex max-h-64 flex-col overflow-y-auto">
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
                          className="group flex items-center gap-2 rounded px-1.5 py-1 hover:bg-white/5"
                        >
                          <button
                            onClick={() =>
                              setHiddenTypes((prev) => {
                                const next = new Set(prev);
                                if (next.has(key)) next.delete(key);
                                else next.add(key);
                                return next;
                              })
                            }
                            className="flex min-w-0 flex-1 items-center gap-2 text-left"
                          >
                            <span
                              className={`h-2 w-2 shrink-0 ${t.shape === "square" ? "" : "rounded-full"}`}
                              style={{
                                background: t.color,
                                opacity: hiddenTypes.has(key) ? 0.35 : 1,
                              }}
                            />
                            <span
                              className={`truncate text-[12px] ${
                                hiddenTypes.has(key)
                                  ? "text-neutral-500 line-through"
                                  : "text-neutral-200"
                              }`}
                            >
                              {t.label}
                            </span>
                          </button>
                          {/* 「只看这个」：类一多时最想要的动作。**给显式按钮而不是
                              修饰键**——alt+点击没人猜得到，这里横向有地方 */}
                          <button
                            onClick={() =>
                              setHiddenTypes(
                                new Set(
                                  types.map(([k]) => k).filter((k) => k !== key),
                                ),
                              )
                            }
                            className="shrink-0 rounded px-1 text-[10px] text-neutral-500 opacity-0 transition-opacity hover:text-white focus:opacity-100 group-hover:opacity-100"
                          >
                            {S.graph.legendOnly}
                          </button>
                          <span className="u-num shrink-0 text-[11px] text-neutral-500">
                            {t.count}
                          </span>
                        </div>
                      ))}
                    {types.every(
                      ([, t]) =>
                        !t.label.toLowerCase().includes(legendQ.toLowerCase()),
                    ) && (
                      <div className="px-1.5 py-2 text-[12px] text-neutral-500">
                        {S.graph.legendNone}
                      </div>
                    )}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>

        {/* 右上：能调「画多少个」+ 统计。**统计说的正是这个数**
            （「画了 150 个，共 548 个」），把调节放在它旁边，改的是谁一目了然。
            外壳保持中性——这一片是 chrome，彩色只属于数据 */}
        <div className="ml-auto flex flex-col items-end gap-1">
          <div className="flex items-start gap-2">
            <div className="pointer-events-auto flex items-center overflow-hidden rounded-md border border-white/10">
            <button
              title={S.graph.nodeBudgetLess}
              disabled={nodeBudget <= NODE_BUDGETS[0]}
              onClick={() =>
                setNodeBudget(
                  (b) => NODE_BUDGETS[Math.max(0, NODE_BUDGETS.indexOf(b) - 1)],
                )
              }
              className="px-1.5 py-[3px] text-[11px] leading-none text-neutral-400 transition-colors hover:bg-white/[0.06] hover:text-white disabled:opacity-25 disabled:hover:bg-transparent disabled:hover:text-neutral-400"
            >
              −
            </button>
            {/* **画满了就别再给「多画」**：库里一共就这么多，再调高什么也不会变，
                而一个点了没反应的按钮比没有这个按钮更糟 */}
            <button
              title={S.graph.nodeBudgetMore}
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
              className="px-1.5 py-[3px] text-[11px] leading-none text-neutral-400 transition-colors hover:bg-white/[0.06] hover:text-white disabled:opacity-25 disabled:hover:bg-transparent disabled:hover:text-neutral-400"
            >
              +
            </button>
          </div>
          <div className="pointer-events-none pt-0.5 u-num text-[11px] text-neutral-500">
          {/* 画满上限时说清「画了多少 / 共多少」。**这个数从前是上限冒充规模**——
              一个上万实体的库右上角永远写着 150 */}
          {capped ? (
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
            <div className="flex items-center gap-1.5 text-[11px] text-neutral-400">
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
          /* **两层**：外层只负责定位，内层才有 overflow-hidden。
             那个类是给按钮堆裁圆角的，可面板是同一个盒子的子元素——
             合成一层的话面板会被一起裁掉，实测只剩塔本身那 32px 宽 */
          <div className="relative" ref={derivedPop.rootRef}>
            <div className="u-tower group glass-strong rounded-xl shadow-xl flex flex-col overflow-hidden">
            <button
              onClick={() => setShowDerived((v) => !v)}
              role="switch"
              aria-checked={showDerived}
              title={`${S.graph.derivedEdges(derivedCount)} · ${S.graph.derivedHint}`}
              className={`flex items-center p-2 transition-colors ${
                showDerived
                  ? "bg-white/[0.1]"
                  : "text-neutral-500 hover:bg-white/[0.06]"
              }`}
              style={
                showDerived ? { color: "rgba(231,197,124,0.95)" } : undefined
              }
            >
              <Waypoints size={15} />
              <span className="u-tower-label">{S.graph.viewDerived}</span>
            </button>
            <div className="h-px bg-white/10 mx-1.5" />
            {/* 展开成一个小窗：这批边是什么时候推的、现在还推不推、手动再跑一次。
                **与开关分成两个按钮**——「藏起来」是每天要点的，「什么时候推的」
                是偶尔才问的，合成一个会让常用动作多一步 */}
            <button
              ref={derivedPop.anchorRef}
              onClick={() =>
                derivedPop.open ? derivedPop.close() : derivedPop.setOpen(true)
              }
              title={S.graph.derivedPanel}
              aria-expanded={derivedPop.open}
              className={`flex items-center p-2 text-[11px] leading-none transition-colors ${
                derivedPop.open
                  ? "text-white bg-white/[0.1]"
                  : "text-neutral-400 hover:text-white hover:bg-white/[0.06]"
              }`}
            >
              <span className="grid h-[15px] w-[15px] shrink-0 place-items-center leading-none">
                ⋯
              </span>
              <span className="u-tower-label">{S.graph.derivedPanel}</span>
            </button>
            </div>
            {derivedPop.open && kb && (
              <DerivedPanel
                panelRef={derivedPop.panelRef}
                kbId={kb.id}
                count={derivedCount}
                onClose={() => derivedPop.close()}
              />
            )}
          </div>
        )}
        <div className="u-tower group glass-strong rounded-xl shadow-xl flex flex-col overflow-hidden">
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
            <button
              key={key}
              title={label}
              onClick={() => {
                setLayoutMode(key);
                layoutModeRef.current = key;
                layoutCtlRef.current?.apply(key);
              }}
              className={`flex items-center p-2 transition-colors ${
                layoutMode === key
                  ? "text-white bg-white/[0.1]"
                  : "text-neutral-400 hover:text-white hover:bg-white/[0.06]"
              }`}
            >
              <Icon size={15} />
              <span className="u-tower-label">{label}</span>
            </button>
          ))}
        </div>
        <div className="u-tower group glass-strong rounded-xl shadow-xl flex flex-col overflow-hidden">
          <button
            title={S.graph.zoomIn}
            onClick={() =>
              sigmaRef.current?.getCamera().animatedZoom({ duration: 220 })
            }
            className="flex items-center p-2 text-neutral-400 hover:text-white hover:bg-white/[0.06] transition-colors"
          >
            <ZoomIn size={15} />
            <span className="u-tower-label">{S.graph.zoomIn}</span>
          </button>
          <button
            title={S.graph.zoomOut}
            onClick={() =>
              sigmaRef.current?.getCamera().animatedUnzoom({ duration: 220 })
            }
            className="flex items-center p-2 text-neutral-400 hover:text-white hover:bg-white/[0.06] transition-colors"
          >
            <ZoomOut size={15} />
            <span className="u-tower-label">{S.graph.zoomOut}</span>
          </button>
          <div className="h-px bg-white/10 mx-1.5" />
          <button
            title={S.graph.fitView}
            onClick={() =>
              sigmaRef.current?.getCamera().animatedReset({ duration: 300 })
            }
            className="flex items-center p-2 text-neutral-400 hover:text-white hover:bg-white/[0.06] transition-colors"
          >
            <Maximize2 size={15} />
            <span className="u-tower-label">{S.graph.fitView}</span>
          </button>
        </div>
      </div>

      {empty && (
        <div className="absolute inset-0 grid place-items-center pointer-events-none">
          {/* 不放标题方块：页面本身就是图谱页，tab 条上也写着，
              第三遍写"图谱"两个字不带任何信息。空状态该说的是下一步做什么 */}
          <div className="text-center text-sm text-neutral-500 max-w-xs">
            {S.graph.emptyBody}
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
  const frac = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
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
  /* 回到起点的次数。**拿它当 key**——同一个元素上重复触发同一个动画不会重播，
     换 key 让它重新挂载才会 */
  const [sweep, setSweep] = useState(0);
  const trackRef = useRef<HTMLDivElement>(null);
  const draggingRef = useRef(false);

  const { minTs, maxTs, bars, merged, trackW } = useMemo(() => {
    const now = Date.now();
    const froms = edges
      .map((e) => (e.valid_from ? Date.parse(e.valid_from) : NaN))
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
      acc += (now - last) * SPEED;
      last = now;
      if (acc >= maxTs) {
        setPlaying(false);
        onChange(null);
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
      className="glass-strong absolute bottom-4 left-1/2 -translate-x-1/2 z-10 rounded-2xl px-3 py-2 flex items-center gap-2.5 shadow-[0_12px_40px_rgba(0,0,0,0.5)] transition-[width] duration-300"
      style={{ width: `min(${trackW}px, calc(100vw - 4rem))` }}
    >
      <button
        onClick={() => {
          // 已经在末端（`Now`）时按播放要从头来。**否则第一下等于没反应**：
          // acc 起点就是终点，循环第一帧就判定播完，只把位置清成 All time
          if (
            !playing &&
            (value === null || value >= maxTs - (maxTs - minTs) * 0.02)
          ) {
            onChange(minTs);
            setSweep((s) => s + 1);
          }
          setPlaying(!playing);
        }}
        title={playing ? S.graph.pause : S.graph.play}
        className="u-btn u-btn-ghost h-8 w-8 shrink-0 grid place-items-center rounded-lg"
      >
        {playing ? <Pause size={13} /> : <Play size={13} />}
      </button>

      {/* 步长。**播放与柱子共用它**——从前柱子按年、播放按天，
          界面上没有一处说得出「一格是多久」 */}
      <div
        title={S.graph.scrubUnitHint}
        /* **与播放键同高同圆角**：那个键是 h-8 / rounded-lg，
           而这里从前是 py-[3px] 撑出来的 20px 高、rounded-md——
           并排放着两个尺寸和圆角都不一样的东西，看着不像一套 */
        className="flex h-8 shrink-0 items-center overflow-hidden rounded-lg border border-white/10"
      >
        {(["year", "month", "day"] as const).map((u) => (
          <button
            key={u}
            onClick={() => setUnit(u)}
            className={`grid h-full place-items-center px-2 text-[10px] leading-none transition-colors ${
              unit === u
                ? "bg-white/[0.08] text-neutral-100"
                : "text-neutral-500 hover:bg-white/[0.04] hover:text-neutral-300"
            }`}
          >
            {u === "year"
              ? S.graph.scrubUnitYear
              : u === "month"
                ? S.graph.scrubUnitMonth
                : S.graph.scrubUnitDay}
          </button>
        ))}
      </div>

      <span className="shrink-0 u-num text-[10px] text-neutral-600">
        {minYear}
      </span>

      {/* 密度带轨道：内嵌浅色井 + 每年事实量柱 */}
      <div
        ref={trackRef}
        className="relative h-9 min-w-[150px] flex-1 overflow-hidden rounded-lg bg-white/[0.04]"
      >
        {sweep > 0 && (
          <span
            key={sweep}
            className="u-sweep"
            onAnimationEnd={(e) => e.currentTarget.remove()}
          />
        )}
        {/* **间隙必须随密度收**：写死 2px 时，日单位下 216 根柱子有 215 个间隙
            ≈ 430px，而轨道内宽才 ~455px——柱子被挤成 0.1px，整条看起来是空的。
            实测就是这么丢的。柱子稀疏时留 2px 好数，密了就贴在一起当密度带看 */}
        <div
          className="absolute inset-x-1.5 top-1.5 bottom-1.5 flex items-end"
          style={{ gap: bars.length > 120 ? 0 : bars.length > 40 ? 1 : 2 }}
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
                  className="w-full rounded-[1px] transition-colors"
                  style={{
                    height: `${Math.max(10, b.h * 100)}%`,
                    // 播放中已扫过的提亮，停止后回到常规亮度。
                    // **还没走到的压到近乎不可见**：它们本来是 0.09，
                    // 在这个底色上仍看得清，于是播放头右边跟左边一样"亮着"，
                    // 走到哪儿就看不出来了。留一点点而不是归零——
                    // 归零等于假装那段没有数据，而它只是还没到
                    background:
                      value !== null && past && playing
                        ? "rgba(255,255,255,0.62)"
                        : value === null || past
                          ? "rgba(255,255,255,0.32)"
                          : "rgba(255,255,255,0.04)",
                  }}
                />
              </div>
            );
          })}
        </div>
        <input
          type="range"
          className="scrubber-range"
          min={minTs}
          max={maxTs}
          step={DAY_MS}
          value={value ?? maxTs}
          onChange={(e) => {
            setPlaying(false);
            onChange(Number(e.target.value));
          }}
          // 原生 range 的拖拽手势会被页面级鼠标监听（如图上拖节点）干扰——
          // 自己用 pointer capture 驱动拖动，点击与拖拽都走同一条计算路径
          onPointerDown={(e) => {
            setPlaying(false);
            draggingRef.current = true;
            try {
              e.currentTarget.setPointerCapture(e.pointerId);
            } catch {
              /* 合成事件的 pointerId 可能无效，忽略 */
            }
            onChange(scrubValueAt(e.clientX, trackRef.current, minTs, maxTs));
          }}
          onPointerMove={(e) => {
            if (draggingRef.current)
              onChange(scrubValueAt(e.clientX, trackRef.current, minTs, maxTs));
          }}
          onPointerUp={() => {
            draggingRef.current = false;
          }}
          onPointerCancel={() => {
            draggingRef.current = false;
          }}
        />
      </div>

      <span className="shrink-0 u-num text-[10px] text-neutral-600">
        {maxYear}
      </span>

      <div className="w-[5.6rem] shrink-0 text-center u-num text-xs text-neutral-200">
        {label}
      </div>

      <div className="h-5 w-px shrink-0 bg-white/10" />

      {/* 双锚点分段：所处锚点高亮、点击即跳；拖在中间某天时两者皆不亮 */}
      <div className="flex shrink-0 rounded-lg overflow-hidden border border-white/10">
        {(
          [
            {
              key: "all",
              label: S.graph.allTime,
              active: value === null,
              to: null,
            },
            {
              key: "now",
              label: S.graph.nowBtn,
              active: value !== null && maxTs - value < DAY_MS,
              to: maxTs,
            },
          ] as const
        ).map((a) => (
          <button
            key={a.key}
            onClick={() => {
              setPlaying(false);
              onChange(a.to);
            }}
            className={`px-2.5 py-1.5 text-xs transition-colors ${
              a.active
                ? "bg-white/10 text-neutral-100"
                : "text-neutral-500 hover:bg-white/[0.05] hover:text-neutral-300"
            }`}
          >
            {a.label}
          </button>
        ))}
      </div>
    </div>
  );
}

/* ============ 实体侧栏 ============ */

function fmtTime(iso: string | null, precision: string | null): string | null {
  if (!iso) return null;
  const d = new Date(iso);
  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, "0");
  const day = String(d.getUTCDate()).padStart(2, "0");
  if (precision === "year") return `${y}`;
  if (precision === "month") return `${y}-${m}`;
  return `${y}-${m}-${day}`;
}

function fmtInterval(f: EntityFact): string {
  if (f.temporal === "eternal") return "";
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
function DerivedPanel({
  panelRef,
  kbId,
  count,
  onClose,
}: {
  panelRef: React.Ref<HTMLDivElement>;
  kbId: string;
  count: number;
  onClose: () => void;
}) {
  const qc = useQueryClient();
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

  // **盖在触发器原位往右上长开**（bottom-0 left-0），而不是在旁边挂一扇窗。
  // 面与圆角跟通知/用户卡片对齐：u-menu-glass + rounded-xl
  return (
    <div
      ref={panelRef}
      className="u-menu-glass pointer-events-auto absolute bottom-0 left-0 z-50 w-72 overflow-hidden rounded-xl p-3 shadow-2xl"
    >
      {/* items-center 而不是 baseline：标题旁边站着一个按钮和一个关闭键，
          按基线对齐会让那两个看着往上飘 */}
      <div className="flex items-center gap-2">
        <span className="text-[13px] text-neutral-100">
          {S.graph.derivedPanel}
        </span>
        {!armed && (
          <button
            /* **要长得像个按钮**：从前是一段灰色幽灵文字夹在标题与 × 之间，
               读起来像第三个标题而不是一个动作。加边框 + 内距，
               与右上角那个档位加减器同一档次要控件的样子 */
            className="ml-auto rounded-md border border-white/10 px-2 py-0.5 text-[11px] text-neutral-400 transition-colors hover:border-white/20 hover:text-neutral-100"
            disabled={!on || run.isPending}
            title={on ? undefined : S.err.inference_off}
            onClick={() => setArmed(true)}
          >
            {run.isPending ? S.graph.derivedRunning : S.graph.derivedRun}
          </button>
        )}
        <button
          className={armed ? "ml-auto text-neutral-500 hover:text-neutral-200" : "text-neutral-500 hover:text-neutral-200"}
          onClick={onClose}
          aria-label={S.graph.close}
        >
          ×
        </button>
      </div>

      {/* 问句 + 两个目标。**取消排在前面**：从「跑」那一下移过来最先碰到的
          是取消，误触的代价小的那个该更近 */}
      {armed && (
        <div className="mt-2 rounded-lg bg-white/[0.04] p-2">
          <p className="text-[11px] leading-relaxed text-neutral-300">
            {S.graph.derivedRunAsk}
          </p>
          <div className="mt-1.5 flex gap-1.5">
            <button
              className="rounded px-2 py-0.5 text-[11px] text-neutral-400 transition-colors hover:bg-white/[0.06] hover:text-neutral-100"
              onClick={() => setArmed(false)}
            >
              {S.graph.derivedRunCancel}
            </button>
            <button
              className="rounded bg-white/10 px-2 py-0.5 text-[11px] text-neutral-100 transition-colors hover:bg-white/[0.16]"
              disabled={run.isPending}
              onClick={() => {
                setArmed(false);
                run.mutate();
              }}
            >
              {S.graph.derivedRunGo}
            </button>
          </div>
        </div>
      )}

      <dl className="mt-2 space-y-1 text-[11px]">
        <div className="flex justify-between gap-3">
          <dt className="text-neutral-500">{S.graph.derivedCountLabel}</dt>
          <dd className="u-num text-neutral-200">{count}</dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-neutral-500">{S.graph.derivedStateLabel}</dt>
          <dd className={on ? "text-neutral-200" : "text-[var(--u-warn)]"}>
            {on
              ? S.graph.derivedOn(kb.data!.inference_interval_minutes)
              : S.graph.derivedOff}
          </dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-neutral-500">{S.graph.derivedLastLabel}</dt>
          <dd className="u-num text-neutral-200">
            {age === null ? S.graph.derivedNever : S.graph.derivedAgo(age)}
          </dd>
        </div>
      </dl>

      {/* 上一次手动跑的结果留在这儿。**推出多少、作废多少要分开说**——
          「什么都没变」和「换掉了三十条」是两件很不一样的事 */}
      {run.data && (
        <p className="mt-2 text-[11px] text-neutral-400">
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
 *  从前这里是一张 `glass rounded-xl p-3` 卡片、证明常驻展开——在一列
 *  Relations/Timeline/History 的紧凑行里显得是另一个产品的东西，而且十几条
 *  推导堆起来是一面墙。证明是「问了才看」的东西，收进展开区正合适。 */
function DerivedRow({
  d,
  otherId,
  otherName,
  open,
  onToggle,
  onNavigate,
}: {
  d: DerivedFact;
  otherId: string;
  otherName: string;
  open: boolean;
  onToggle: () => void;
  onNavigate: (entityId: string) => void;
}) {
  return (
    <div
      className={`rounded-lg transition-colors ${open ? "bg-white/[0.05]" : "hover:bg-white/[0.04]"}`}
    >
      <button
        onClick={onToggle}
        className="w-full text-left px-2 py-1.5 flex items-center gap-1.5"
      >
        <ChevronRight
          size={11}
          className={`shrink-0 text-neutral-600 transition-transform ${open ? "rotate-90" : ""}`}
        />
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
          className="truncate text-[13px] text-neutral-200 hover:text-white hover:underline underline-offset-2 decoration-white/30"
        >
          {otherName}
        </span>
        <span className="ml-auto shrink-0 pl-2 text-[10.5px] text-neutral-600">
          {d.premises.length}
        </span>
      </button>
      {/* 证明：前提按推导顺序。**边框与 EvidenceList 同一档**——
          两者是同一件事的两种形态：一个给出处，一个给推理链 */}
      {open && (
        <div className="mx-2 mb-2 mt-0.5 border-l border-white/15 pl-2.5">
          <ol className="space-y-0.5">
            {d.premises.map((p, i) => (
              <li key={i} className="text-[11px] text-neutral-400">
                {p}
              </li>
            ))}
          </ol>
          {d.premises.length === 0 && (
            <p className="text-[11px] text-neutral-600">
              {S.graph.derivedNoProof}
            </p>
          )}
        </div>
      )}
    </div>
  );
}

function EntityPanel({
  kbId,
  entityId,
  exiting,
  onClose,
  onNavigate,
}: {
  kbId: string;
  entityId: string;
  /** 正在演退场：还挂在 DOM 上，但已经不接受点击 */
  exiting: boolean;
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
      const rule =
        d.rule === "transitive"
          ? S.graph.ruleTransitive
          : S.graph.ruleSymmetric;
      const key = `${direction}|${d.predicate}|${d.rule}`;
      const cur = map.get(key);
      if (cur) cur.rows.push(d);
      else map.set(key, { key, direction, predicate: d.predicate, rule, rows: [d] });
    }
    return [...map.values()];
  }, [derived, entityId]);
  // Relations = 按关系分组（查关系）；Timeline = 有效时间轴（事情何时成立）；
  // History = 记录时间轴（我们何时这么认为、又何时改了主意）
  const [view, setView] = useState<
    "relations" | "timeline" | "history" | "derived"
  >("relations");

  const e: GraphNode | undefined = detail.data?.entity;

  // 实体修正：抽取给的是初判，判错此前只能整库重抽
  const qc = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [draftName, setDraftName] = useState("");
  const [draftType, setDraftType] = useState("");
  // 同名的其他实体：详情接口打开就给。改名之后再用响应里的那份覆盖——
  // 改完名可能撞上一批新的同名，那时候的答案比打开时的新
  const [renamedPeers, setRenamedPeers] = useState<GraphNode[] | null>(null);
  const sameName = renamedPeers ?? detail.data?.same_name ?? [];
  const setSameName = setRenamedPeers;
  // 手动合并：把同名的那个并进**当前这个**。方向写死是有意的——
  // 用户正在看的就是他判断为「主」的那一个
  const merge = useMutation({
    mutationFn: (source: string) => api.mergeEntities(kbId, source, entityId),
    onSuccess: () => {
      toast.success(S.toast.saved);
      // 本地把并掉的那个摘掉，别等重取——它已经不存在了，留着会让人再点一次
      setSameName((prev) =>
        (prev ?? sameName).filter((p) => p.id !== merge.variables),
      );
      qc.invalidateQueries({ queryKey: ["entity", kbId, entityId] });
      qc.invalidateQueries({ queryKey: ["graph"] });
      qc.invalidateQueries({ queryKey: ["review", kbId] });
    },
    onError: (err: Error) => toast.error(err.message),
  });
  // 类型下拉要的是全量本体，不是当前视图里出现过的那几个
  const ontology = useQuery({
    queryKey: ["ontology", kbId],
    queryFn: () => api.ontology(kbId),
    enabled: editing,
  });
  const types = ontology.data?.entity_types ?? [];

  const openEdit = () => {
    if (!e) return;
    setDraftName(e.name);
    setDraftType(types.find((t) => t.key === e.type_key)?.id ?? "");
    setSameName([]);
    setEditing(true);
  };
  // 本体是异步来的：它到齐时把类型下拉对到当前类型上
  useEffect(() => {
    if (editing && !draftType && e)
      setDraftType(types.find((t) => t.key === e.type_key)?.id ?? "");
  }, [editing, draftType, e, types]);

  const save = useMutation({
    mutationFn: () => {
      const body: { type_id?: string; canonical_name?: string } = {};
      if (draftName.trim() && draftName.trim() !== e?.name)
        body.canonical_name = draftName.trim();
      const curId = types.find((t) => t.key === e?.type_key)?.id;
      if (draftType && draftType !== curId) body.type_id = draftType;
      return api.updateEntity(kbId, entityId, body);
    },
    onSuccess: (r) => {
      setEditing(false);
      setSameName(r.same_name);
      toast.success(S.graph.editSaved);
      // 改了类型/名字，图谱节点与本体计数都要跟着动
      qc.invalidateQueries({ queryKey: ["entity", kbId, entityId] });
      qc.invalidateQueries({ queryKey: ["graph", kbId] });
      qc.invalidateQueries({ queryKey: ["ontology", kbId] });
    },
    onError: (err: Error) => toast.error(err.message),
  });

  const dirty =
    !!e &&
    (draftName.trim() !== e.name ||
      draftType !== (types.find((t) => t.key === e.type_key)?.id ?? ""));

  // Relations = 当下有效的快照（as-of now）；已闭合的历史只出现在 Timeline。
  // 按「方向 + 谓词」分组：实体自身名不再逐行重复，谓词只出现在小节标题里
  const { groups, historicalCount } = useMemo(() => {
    const all = detail.data?.facts ?? [];
    const nowIso = new Date().toISOString();
    const current = all.filter(
      (f) =>
        (!f.valid_from || f.valid_from <= nowIso) &&
        (!f.valid_to || f.valid_to > nowIso),
    );
    const map = new Map<
      string,
      {
        key: string;
        label: string | null;
        inferred: boolean;
        direction: string;
        rows: EntityFact[];
      }
    >();
    for (const f of current) {
      // 谓词为空的事实归到同一组：它们的共同点就是「说不出是什么关系」
      const k = `${f.direction}:${f.predicate_key ?? ""}`;
      if (!map.has(k))
        map.set(k, {
          key: k,
          label: f.predicate_label,
          inferred: f.inferred,
          direction: f.direction,
          rows: [],
        });
      map.get(k)!.rows.push(f);
    }
    const arr = [...map.values()];
    for (const gr of arr)
      gr.rows.sort((a, b) =>
        (a.valid_from ?? "9999") < (b.valid_from ?? "9999") ? -1 : 1,
      );
    arr.sort(
      (a, b) =>
        b.rows.length - a.rows.length ||
        (a.label ?? "").localeCompare(b.label ?? ""),
    );
    return { groups: arr, historicalCount: all.length - current.length };
  }, [detail.data]);

  return (
    <div
      className={`${exiting ? "u-dock-out" : "u-dock-in"} glass-strong absolute top-14 right-3 bottom-20 w-80 z-10 rounded-xl shadow-2xl flex flex-col`}
    >
      <div className="flex items-start justify-between px-4 py-3.5 border-b border-white/10">
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
                  className="text-[15px] font-semibold tracking-tight text-white"
                  style={{ fontFamily: "var(--font-display)" }}
                >
                  {e.name}
                </span>
              </div>
              {/* 消歧后缀找不到关联事实时兜底成类型标签，那就与后面的类型重复了 */}
              <div className="mt-1 text-xs text-neutral-500">
                {e.disambiguator && e.disambiguator !== e.type_label
                  ? `${e.disambiguator} · `
                  : ""}
                {e.type_label ?? S.graph.untyped} ·{" "}
                {detail.data?.facts.length ?? 0} {S.graph.facts}
              </div>
            </>
          )}
        </div>
        <div className="flex items-center gap-1.5 mt-0.5">
          {e && !editing && (
            <button
              onClick={openEdit}
              title={S.graph.edit}
              className="text-neutral-500 hover:text-neutral-200"
            >
              <Pencil size={13} />
            </button>
          )}
          <button
            onClick={onClose}
            className="text-neutral-500 hover:text-neutral-200"
          >
            <X size={15} />
          </button>
        </div>
      </div>

      {editing && e && (
        <div className="px-4 py-3 border-b border-white/10 space-y-2.5">
          <label className="block">
            <span className="text-[10px] uppercase tracking-[0.08em] text-neutral-500">
              {S.graph.editName}
            </span>
            <input
              autoFocus
              value={draftName}
              onChange={(ev) => setDraftName(ev.target.value)}
              onKeyDown={(ev) => {
                if (ev.key === "Enter" && dirty && draftName.trim())
                  save.mutate();
                if (ev.key === "Escape") setEditing(false);
              }}
              className="mt-1 w-full bg-white/[0.04] border border-white/10 rounded px-2 py-1 text-sm text-neutral-100 focus:outline-none focus:border-white/25"
            />
          </label>
          <label className="block">
            <span className="text-[10px] uppercase tracking-[0.08em] text-neutral-500">
              {S.graph.editType}
            </span>
            <select
              value={draftType}
              onChange={(ev) => setDraftType(ev.target.value)}
              className="mt-1 w-full bg-white/[0.04] border border-white/10 rounded px-2 py-1 text-sm text-neutral-100 focus:outline-none focus:border-white/25"
            >
              {types.map((t) => (
                <option key={t.id} value={t.id} className="bg-neutral-900">
                  {t.label}
                </option>
              ))}
            </select>
          </label>
          <div className="flex items-center gap-2 pt-0.5">
            <button
              disabled={!dirty || !draftName.trim() || save.isPending}
              onClick={() => save.mutate()}
              className="u-pop px-2.5 py-1 text-xs rounded bg-white/10 text-neutral-100 hover:bg-white/15 disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {S.graph.editSave}
            </button>
            <button
              onClick={() => setEditing(false)}
              className="px-2.5 py-1 text-xs rounded text-neutral-500 hover:text-neutral-300"
            >
              {S.graph.editCancel}
            </button>
            {!draftName.trim() && (
              <span className="text-[11px] text-[var(--u-danger)]">
                {S.graph.editEmptyName}
              </span>
            )}
          </div>
        </div>
      )}

      {/* 同名不是错误——两个张伟可以并存。只提示，判定是不是同一个是人的事 */}
      {sameName.length > 0 && !editing && (
        <div className="mx-4 mt-2.5 rounded border border-white/10 bg-white/[0.03] px-2.5 py-2">
          <div className="flex items-start justify-between gap-2">
            <p className="text-[11px] text-neutral-400">
              {S.graph.sameNameNote(sameName.length)}{" "}
              <span className="text-neutral-500">{S.graph.sameNameHint}</span>
            </p>
            <button
              onClick={() => setSameName([])}
              className="text-neutral-600 hover:text-neutral-300 shrink-0"
            >
              <X size={11} />
            </button>
          </div>
          {/* 每个同名的给两个动作：去看它，或者把它并进来。
              **方向写死成「并进当前这个」**——合并有方向（源消失、事实搬到目标上），
              而当前打开的这个就是用户正在看、正在判断的那一个 */}
          <div className="mt-1.5 space-y-1">
            {sameName.map((p) => (
              <div key={p.id} className="flex items-center gap-1">
                <button
                  onClick={() => onNavigate(p.id)}
                  className="min-w-0 flex-1 truncate text-left text-[11px] px-1.5 py-0.5 rounded bg-white/[0.06] text-neutral-300 hover:bg-white/10"
                >
                  {p.type_label ?? S.graph.untyped}
                  {p.disambiguator && p.disambiguator !== p.type_label
                    ? ` · ${p.disambiguator}`
                    : ""}
                </button>
                <button
                  className="shrink-0 text-[11px] px-1.5 py-0.5 rounded text-neutral-400 hover:bg-white/10 hover:text-neutral-100"
                  disabled={merge.isPending}
                  title={S.graph.mergeIntoHint}
                  onClick={() => {
                    if (confirm(S.graph.mergeConfirm(p.name, e?.name ?? "")))
                      merge.mutate(p.id);
                  }}
                >
                  {S.graph.mergeInto}
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 视图切换：Relations（分组）| Timeline（年表） */}
      <div className="px-4 pt-2.5">
        <div className="flex rounded-lg overflow-hidden border border-white/10 w-fit">
          {(["relations", "timeline", "history", "derived"] as const)
            // 推出来的那一档：**没有派生就不出现**。一个没开推理的库不该看到
            // 一个永远是空的标签页
            .filter((v) => v !== "derived" || derived.length > 0)
            .map((v) => (
              <button
                key={v}
                onClick={() => setView(v)}
                className={`px-3 py-1 text-[11px] transition-colors ${
                  view === v
                    ? "bg-white/10 text-neutral-100"
                    : "text-neutral-500 hover:bg-white/[0.05] hover:text-neutral-300"
                }`}
              >
                {v === "relations"
                  ? S.graph.viewRelations
                  : v === "timeline"
                    ? S.graph.viewTimeline
                    : v === "history"
                      ? S.graph.viewHistory
                      : S.graph.viewDerived}
              </button>
            ))}
        </div>
      </div>

      <div className="u-scroll flex-1 overflow-y-auto px-2 py-2">
        {view === "relations" && historicalCount > 0 && (
          <button
            onClick={() => setView("timeline")}
            className="mx-2 mb-2 mt-0.5 text-[11px] text-neutral-500 hover:text-neutral-300 underline-offset-2 hover:underline"
          >
            {S.graph.historicalNote(historicalCount)}
          </button>
        )}
        {view === "relations" &&
          groups.map((gr) => (
            <div key={gr.key} className="mb-3 last:mb-1">
              <div className="flex items-center gap-1.5 px-2 pb-1 pt-1.5 text-[10px] font-medium uppercase tracking-[0.08em] text-neutral-500">
                {gr.direction === "in" ? (
                  <ArrowLeft size={10} />
                ) : (
                  <ArrowRight size={10} />
                )}
                <span
                  className={
                    gr.label === null ? "italic text-neutral-600" : undefined
                  }
                  title={
                    gr.label && gr.inferred
                      ? S.graph.inferredPredicate
                      : undefined
                  }
                >
                  {gr.label ?? S.graph.unknownPredicate}
                </span>
                {gr.rows.length > 1 && (
                  <span className="text-neutral-600">{gr.rows.length}</span>
                )}
              </div>
              <div>
                {gr.rows.map((f) => (
                  <FactRow
                    key={f.id}
                    kbId={kbId}
                    fact={f}
                    open={openFact === f.id}
                    onToggle={() =>
                      setOpenFact(openFact === f.id ? null : f.id)
                    }
                    onNavigate={onNavigate}
                  />
                ))}
              </div>
            </div>
          ))}
        {view === "timeline" && (
          <TimelineView
            kbId={kbId}
            facts={detail.data?.facts ?? []}
            openFact={openFact}
            onToggle={(id) => setOpenFact(openFact === id ? null : id)}
            onNavigate={onNavigate}
          />
        )}
        {view === "history" && (
          <EntityHistory kbId={kbId} entityId={entityId} />
        )}
{view === "derived" && (
          <>
            <p className="px-2 pb-1.5 pt-0.5 text-[11px] leading-relaxed text-neutral-500">
              {S.graph.derivedHint}
            </p>
            {/* **与 Relations 同一个骨架**：方向箭头 + 谓词 + 条数的小标题，
                底下是紧凑行。规则（传递/对称）并进标题——它对整组都成立，
                挂在每一行上是重复，而且那个 `--u-warn` 琥珀色又是一处
                与派生边抢色相的地方 */}
            {derivedGroups.map((gr) => (
              <div key={gr.key} className="mb-3 last:mb-1">
                <div className="flex items-center gap-1.5 px-2 pb-1 pt-1.5 text-[10px] font-medium uppercase tracking-[0.08em] text-neutral-500">
                  {gr.direction === "in" ? (
                    <ArrowLeft size={10} />
                  ) : (
                    <ArrowRight size={10} />
                  )}
                  <span>{gr.predicate}</span>
                  <span className="text-neutral-600">{gr.rule}</span>
                  {gr.rows.length > 1 && (
                    <span className="ml-auto text-neutral-600">
                      {gr.rows.length}
                    </span>
                  )}
                </div>
                <div>
                  {gr.rows.map((d) => {
                    const out = d.subject_id === entityId;
                    return (
                      <DerivedRow
                        key={d.id}
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
          </>
        )}
        {view !== "history" &&
          view !== "derived" &&
          detail.data?.facts.length === 0 && (
            <p className="text-sm text-neutral-500 p-2">{S.graph.noFacts}</p>
          )}
      </div>
    </div>
  );
}

/** 年表视图：带区间的事实按起点摊开成竖直时间线；无时间的沉到底部 undated。 */
function TimelineView({
  kbId,
  facts,
  openFact,
  onToggle,
  onNavigate,
}: {
  kbId: string;
  facts: EntityFact[];
  openFact: string | null;
  onToggle: (id: string) => void;
  onNavigate: (entityId: string) => void;
}) {
  const dated = facts
    .filter((f) => f.temporal !== "eternal" && (f.valid_from || f.valid_to))
    .sort((a, b) =>
      (a.valid_from ?? a.valid_to ?? "") < (b.valid_from ?? b.valid_to ?? "")
        ? -1
        : 1,
    );
  const undated = facts.filter((f) => !dated.includes(f));

  return (
    <div className="px-2 pt-1">
      <div className="relative ml-1.5 border-l border-white/15 pl-3 space-y-0.5">
        {dated.map((f) => (
          <div key={f.id} className="relative">
            <span className="absolute -left-[17.5px] top-2.5 h-2 w-2 rounded-full bg-neutral-600 ring-2 ring-[#0f0f0f]" />
            <TimelineRow
              kbId={kbId}
              fact={f}
              open={openFact === f.id}
              onToggle={() => onToggle(f.id)}
              onNavigate={onNavigate}
            />
          </div>
        ))}
        {dated.length === 0 && (
          <p className="py-2 text-xs text-neutral-500">
            {S.graph.timelineEmpty}
          </p>
        )}
      </div>
      {undated.length > 0 && (
        <div className="mt-3">
          <div className="px-2 pb-1 text-[10px] font-medium uppercase tracking-[0.08em] text-neutral-600">
            {S.graph.undated}
          </div>
          {undated.map((f) => (
            <FactRow
              key={f.id}
              kbId={kbId}
              fact={f}
              open={openFact === f.id}
              onToggle={() => onToggle(f.id)}
              onNavigate={onNavigate}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/** 年表条目：区间 + 闭合方式标记 + 开放事实的最后确认时间；点击展开证据。 */
function TimelineRow({
  kbId,
  fact,
  open,
  onToggle,
  onNavigate,
}: {
  kbId: string;
  fact: EntityFact;
  open: boolean;
  onToggle: () => void;
  onNavigate: (entityId: string) => void;
}) {
  const interval = fmtInterval(fact);
  const isOpenEnded = !fact.valid_to;
  const literal = fmtObjectValue(fact.object_value);
  return (
    <div
      className={`rounded-lg transition-colors ${open ? "bg-white/[0.05]" : "hover:bg-white/[0.04]"} ${
        fact.stale ? "opacity-55" : ""
      }`}
      title={fact.stale ? S.graph.staleFactHint : undefined}
    >
      <button onClick={onToggle} className="w-full text-left px-2 py-1.5">
        <div className="flex items-center gap-1.5 u-num text-[10.5px] text-neutral-500">
          {interval || "—"}
          {fact.corrected && (
            <span className="text-neutral-600" title={S.graph.correctedHint}>
              ⟲
            </span>
          )}
          {isOpenEnded && fact.last_evidence_time && (
            <span className="ml-auto text-neutral-600">
              {S.graph.lastConfirmed(fact.last_evidence_time.slice(0, 10))}
            </span>
          )}
        </div>
        <div className="mt-0.5 flex items-center gap-1.5 text-[13px] text-neutral-200">
          <span className="text-neutral-500 text-xs">
            {fact.direction === "in" ? "←" : "→"}{" "}
            <span
              className={
                fact.predicate_label === null
                  ? "italic text-neutral-600"
                  : undefined
              }
              title={
                fact.predicate_label && fact.inferred
                  ? S.graph.inferredPredicate
                  : undefined
              }
            >
              {fact.predicate_label ?? S.graph.unknownPredicate}
            </span>
          </span>
          {fact.other_id ? (
            <span
              role="link"
              tabIndex={0}
              onClick={(ev) => {
                ev.stopPropagation();
                onNavigate(fact.other_id!);
              }}
              onKeyDown={(ev) => {
                if (ev.key === "Enter") {
                  ev.stopPropagation();
                  onNavigate(fact.other_id!);
                }
              }}
              className="truncate hover:text-white hover:underline underline-offset-2 decoration-white/30"
            >
              {fact.other_name ?? "?"}
            </span>
          ) : (
            <span className="truncate">
              {fact.other_name ?? literal ?? "?"}
            </span>
          )}
          {fact.stale && (
            <span className="u-chip u-chip-neutral shrink-0 !text-[10px] !px-1.5">
              {S.graph.staleFactChip}
            </span>
          )}
        </div>
      </button>
      {open && <EvidenceList kbId={kbId} fact={fact} />}
    </div>
  );
}

/** 字面值宾语的显示：属性 {value,unit} / 问数映射 {summary} / 其他 JSON 兜底。 */
function fmtObjectValue(v: Record<string, unknown> | null): string | null {
  if (!v) return null;
  if (v.value !== undefined) {
    const val =
      typeof v.value === "boolean" ? (v.value ? "✓" : "✗") : String(v.value);
    return typeof v.unit === "string" && v.unit ? `${val} ${v.unit}` : val;
  }
  if (typeof v.summary === "string") return v.summary;
  return JSON.stringify(v);
}

function FactRow({
  kbId,
  fact,
  open,
  onToggle,
  onNavigate,
}: {
  kbId: string;
  fact: EntityFact;
  open: boolean;
  onToggle: () => void;
  onNavigate: (entityId: string) => void;
}) {
  const interval = fmtInterval(fact);
  // 与 Review 的低置信口径一致：只有低到需要怀疑才挂 chip，常规置信保持沉默
  const lowConfidence = fact.confidence < 0.75;

  return (
    <div
      className={`rounded-lg transition-colors ${open ? "bg-white/[0.05]" : "hover:bg-white/[0.04]"} ${
        fact.stale ? "opacity-55" : ""
      }`}
      title={fact.stale ? S.graph.staleFactHint : undefined}
    >
      <button
        onClick={onToggle}
        className="w-full text-left px-2 py-1.5 flex items-center gap-1.5"
      >
        <ChevronRight
          size={11}
          className={`shrink-0 text-neutral-600 transition-transform ${open ? "rotate-90" : ""}`}
        />
        {fact.other_id ? (
          <span
            role="link"
            tabIndex={0}
            onClick={(ev) => {
              ev.stopPropagation();
              onNavigate(fact.other_id!);
            }}
            onKeyDown={(ev) => {
              if (ev.key === "Enter") {
                ev.stopPropagation();
                onNavigate(fact.other_id!);
              }
            }}
            className="truncate text-[13px] text-neutral-200 hover:text-white hover:underline underline-offset-2 decoration-white/30"
          >
            {fact.other_name ?? "?"}
          </span>
        ) : (
          <span className="truncate text-[13px] text-neutral-200">
            {fact.other_name ?? fmtObjectValue(fact.object_value) ?? "?"}
          </span>
        )}
        {lowConfidence && (
          <span className="shrink-0 u-num u-meta-warn text-[10.5px]">
            {Math.round(fact.confidence * 100)}%
          </span>
        )}
        {fact.stale && (
          <span className="u-chip u-chip-neutral shrink-0 !text-[10px] !px-1.5">
            {S.graph.staleFactChip}
          </span>
        )}
        {interval && (
          <span className="ml-auto shrink-0 pl-2 u-num text-[10.5px] text-neutral-500">
            {interval}
          </span>
        )}
      </button>
      {open && <EvidenceList kbId={kbId} fact={fact} />}
    </div>
  );
}

/** 证据展开区（FactRow 与 TimelineRow 共用）：quote + 跳原文 + 版本角标 + 置信。 */
function EvidenceList({ kbId, fact }: { kbId: string; fact: EntityFact }) {
  const evidence = useQuery({
    queryKey: ["evidence", fact.id],
    queryFn: () => api.factEvidence(kbId, fact.id),
  });
  return (
    <div className="mx-2 mb-2 mt-0.5 space-y-2 border-l border-white/15 pl-2.5">
      {evidence.data?.evidence.map((ev: Evidence) => (
        <Link
          key={ev.chunk_id}
          to="/doc/$docId"
          params={{ docId: ev.document_id }}
          search={{ chunk: ev.chunk_id }}
          className="block text-xs text-neutral-500 hover:text-neutral-300"
        >
          {/* 原文说的谓词，只在它与事实行上显示的不同时才写出来。本体外的谓词
              事实行上已经显示原文说法（0052），相同的话再写一遍是噪声；
              一条事实有多种说法时（占 3%）这里才有话说 */}
          {ev.proposed_predicate &&
            ev.proposed_predicate !== fact.predicate_key && (
              <div className="mb-0.5 text-[11px] text-neutral-400">
                {S.graph.proposedPredicate(ev.proposed_predicate)}
              </div>
            )}
          <div className="line-clamp-2 italic">
            {ev.quote ? `“${ev.quote}”` : S.graph.noQuote}
          </div>
          <div className="mt-0.5 text-neutral-400">
            {S.graph.sectionRef(ev.filename, ev.seq + 1)}
            {ev.stale && (
              <span
                className="ml-1.5 u-num text-[10px] text-neutral-600"
                title={S.graph.staleEvidenceHint}
              >
                {S.graph.fromVersion(ev.doc_version)}
              </span>
            )}
          </div>
        </Link>
      ))}
      {evidence.data?.evidence.length === 0 && (
        <p className="text-xs text-neutral-500">{S.graph.noEvidence}</p>
      )}
      {/* 置信度只在低到值得怀疑时说话（与 Review 低置信口径一致），常规不标 */}
      {fact.confidence < 0.75 && (
        <p className="text-[10px] text-[var(--u-warn)]">
          {Math.round(fact.confidence * 100)}% {S.graph.confidence}
        </p>
      )}
    </div>
  );
}
