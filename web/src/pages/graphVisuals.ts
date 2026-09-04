// 节点视觉：色彩数学 + 四层壳配色 + 胶囊标签/悬浮卡 + 世界坐标网格。
// 从 Graph.tsx 抽出——这几样是「怎么画一个带类型色的节点」，不含任何实例图
// 语义（没有 derived/contested/temporal 这些概念），本体模式图原样复用，
// 图谱与模式图因此长得像同一个引擎画的，而不是各自一套视觉识别。
//
// **边的画法没有抽到这里**：/graph 的边携带派生/争议/时态这些实例图独有的
// 状态，模式图的边是 subClassOf/relation/disjoint 三种本体语义，两者的
// 「边意味着什么」根本不是同一件事，抽成共用只会把两套语义焊在一起。
import type Sigma from "sigma";

/* 画布调色板 —— 结构取自 Semantica GraphWorkspace 源码；基色已中性化：
   Semantica 原版是钢蓝系（#0B1320/#5A7A9E/#7A92AE），按"chrome 零色偏、
   彩色只属于数据"的既定原则换成同明度纯灰，类型色混入比例不变 */
export const NODE_SHELL_BASE = "#121212"; // 节点外壳深底（原 #0B1320 的中性化）
export const NODE_CORE_BASE = "#767676"; // 节点核心灰（原 #5A7A9E 的中性化）
export const NODE_BORDER_BASE = "#909090"; // 节点描边（原 #7A92AE 的中性化）
export const NODE_TINT_MIX = 0.14; // 类型色只按 14% 混入外壳（高级感的关键）
export const NODE_CORE_MIX = 0.5; // 核心向类型色的混入比例
/* 状态环取**节点自己的类型色**，不是写死的色相。往白里混而不是直接用原色：
   环画在节点自己身上，同色同亮度就看不出是个环。**悬停混得更白、选中混得
   更少**——悬停时全图不压暗，环要在一片乱线里立刻跳出来；选中时其余都
   压暗了，节点本来就孤立着，这时候环该说的是「它是谁」，所以更贴近它自己
   的颜色。 */
export const RING_HOVER_MIX = 0.7; // 悬停：偏白，为的是跳出来
export const RING_SELECT_MIX = 0.35; // 选中：偏本色，为的是认得出
export const TRANSPARENT = "rgba(0,0,0,0)";
export const MUTED_SHELL = "#151515";
/* 悬停时其余的压暗程度。**比选中轻**（选中是压到底）：悬停是随鼠标走的、
   每划过一个节点就换一次，压到底会让整张画布不停明灭 */
export const HOVER_MUTE = 0.78;
export const PILL_BG = "rgba(12,12,12,0.9)";
export const PILL_BORDER = "rgba(255,255,255,0.14)";
export const PILL_TEXT = "#ededed";

export function hexToRgb(hex: string): [number, number, number] {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex);
  if (!m) return [128, 128, 128];
  const v = parseInt(m[1], 16);
  return [(v >> 16) & 255, (v >> 8) & 255, v & 255];
}

/** c1 向 c2 按 t 比例混色 */
export function mix(c1: string, c2: string, t: number): string {
  const [r1, g1, b1] = hexToRgb(c1);
  const [r2, g2, b2] = hexToRgb(c2);
  const f = (a: number, b: number) => Math.round(a + (b - a) * t);
  return `rgb(${f(r1, r2)},${f(g1, g2)},${f(b1, b2)})`;
}

/* 胶囊标签：深色圆角底 + 柔和文字（学 Semantica 的浮签风格） */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function drawPillLabel(
  ctx: CanvasRenderingContext2D,
  data: any,
  _settings: any,
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
export function drawHoverCard(
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

/* 世界坐标网格：随相机平移/缩放（Figma/tldraw 式无限画布惯例）。
   4 倍细分 LOD：每层 alpha 随其屏幕间距连续淡入（13px 进场 → 52px 满亮 5.5%），
   粗层与细层线重合处自然叠亮，形成"大小格"层次；无任何跳变。 */
const GRID_BASE_WORLD = 24; // 基准世界格距（匹配 ~300 尺度的布局）
const GRID_FADE_IN_PX = 13;
const GRID_FULL_PX = 52;
const GRID_MAX_LEVEL_PX = 480;
const GRID_MAX_ALPHA = 0.055;

export function drawWorldGrid(canvas: HTMLCanvasElement, sigma: Sigma): void {
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
