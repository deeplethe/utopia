/* 登录页背景：巨构变换（Canvas 2D，零依赖）。
   四种巨型形态——星球（球面）→ 环形都市（环面）→ 城市平原（起伏网格）→
   波动巨碑（竖直巨墙）。结构持续缓慢自转（运动本身很便宜）；形态切换不做
   逐点插值变形（那才是卡顿来源），而是整体淡出 → 换形态 → 淡入，浮现时
   带一点由小到大的生长感。透视投影、结构大于视口（巨物应当出画）。
   性能：形态几何只在切换时重算进 TypedArray；点按亮度分桶、每桶一次 fill；
   DPR 封顶 1.5。纯中性白灰；prefers-reduced-motion 时静止单帧。 */
import { useEffect, useRef } from "react";

const U = 48; // 经向点数
const V = 26; // 纬向点数
const N = U * V;
const HOLD_MS = 7000; // 完全可见的停留
const FADE_MS = 1400; // 淡出 / 淡入各自时长
const ROT_SPEED = 0.000024; // 弧度/毫秒（约 260s 一周，巨物应当迟缓）
const BUCKETS = 12; // 点亮度分桶数

type Vec3 = [number, number, number];

/** 形态族：每个函数把 (u,v)∈[0,1) 映射到 ~[-1,1]³ */
const FORMS: ((u: number, v: number) => Vec3)[] = [
  // 星球：球面
  (u, v) => {
    const lon = u * Math.PI * 2;
    const lat = (v - 0.5) * Math.PI * 0.92;
    return [Math.cos(lat) * Math.cos(lon), Math.sin(lat) * 0.95, Math.cos(lat) * Math.sin(lon)];
  },
  // 环形都市：环面
  (u, v) => {
    const a = u * Math.PI * 2;
    const b = v * Math.PI * 2;
    const R = 0.74;
    const r = 0.32;
    return [
      (R + r * Math.cos(b)) * Math.cos(a),
      r * Math.sin(b) * 1.05,
      (R + r * Math.cos(b)) * Math.sin(a),
    ];
  },
  // 城市平原：起伏网格
  (u, v) => {
    const x = (u - 0.5) * 2.5;
    const z = (v - 0.5) * 2.5;
    const y = Math.sin(x * 2.3) * 0.14 + Math.cos(z * 2.1 + x * 1.2) * 0.12 - 0.15;
    return [x, y, z];
  },
  // 波动巨碑：竖直巨墙
  (u, v) => {
    const x = (u - 0.5) * 2.3;
    const y = (v - 0.5) * 1.5;
    const z = Math.sin(x * 2.8 + y * 1.6) * 0.22;
    return [x, y, z];
  },
];

function smoothstep(t: number): number {
  return t * t * (3 - 2 * t);
}

export function LoginScene({ leaving }: { leaving?: boolean }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    // 点阵参数坐标
    const pu = new Float32Array(N);
    const pv = new Float32Array(N);
    for (let j = 0; j < V; j++)
      for (let i = 0; i < U; i++) {
        const k = j * U + i;
        pu[k] = i / U;
        pv[k] = j / (V - 1);
      }
    // 网格邻接边，扁平存储
    const edgeIdx: number[] = [];
    for (let j = 0; j < V; j++)
      for (let i = 0; i < U; i++) {
        const a = j * U + i;
        if (i < U - 1) edgeIdx.push(a, a + 1);
        if (j < V - 1) edgeIdx.push(a, a + U);
      }
    const edges = new Int32Array(edgeIdx);
    // u 首尾回绕边：只在 u 方向闭合的形态（球面/环面）上绘制——
    // 摊平形态（平原/巨碑）画它会横穿整个画面
    const wrapIdx: number[] = [];
    for (let j = 0; j < V; j++) wrapIdx.push(j * U + U - 1, j * U);
    const wrapEdges = new Int32Array(wrapIdx);
    const U_CLOSED = [true, true, false, false]; // 与 FORMS 一一对应

    // 当前形态几何：只在切换时重算，帧内零分配
    const fx = new Float32Array(N), fy = new Float32Array(N), fz = new Float32Array(N);
    let cachedForm = -1;
    const fillForm = (idx: number) => {
      const map = FORMS[idx];
      for (let k = 0; k < N; k++) {
        const P = map(pu[k], pv[k]);
        fx[k] = P[0];
        fy[k] = P[1];
        fz[k] = P[2];
      }
      cachedForm = idx;
    };

    const px = new Float32Array(N);
    const py = new Float32Array(N);
    const pa = new Float32Array(N); // 深度→亮度 0..1

    // 亮度分桶：预生成样式串，帧内无字符串拼接
    const bucketStyle: string[] = [];
    for (let b = 0; b < BUCKETS; b++)
      bucketStyle.push(`rgba(255,255,255,${(0.1 + (b / (BUCKETS - 1)) * 0.34).toFixed(3)})`);
    const buckets: number[][] = Array.from({ length: BUCKETS }, () => []);

    // 原地闪烁：弱化为氛围层（主秀是下面的光脉冲）
    const TWINKLES = 40;
    const twIdx = new Int32Array(TWINKLES);
    const twPhase = new Float32Array(TWINKLES);
    const twSpeed = new Float32Array(TWINKLES);
    for (let n = 0; n < TWINKLES; n++) {
      twIdx[n] = (n * 1013 + 389) % N; // 确定性伪随机散布
      twPhase[n] = ((n * 7919) % 628) / 100; // 0..2π
      twSpeed[n] = 0.0008 + ((n * 271) % 100) / 100 * 0.0016; // 弧度/毫秒
    }

    // 光脉冲：从一个顶点沿边亮向另一个顶点，拖渐隐尾迹（信号在巨构上传导）
    const PULSES = 12;
    const TRAIL_MAX = 5; // 尾迹保留的节点数
    const TRAIL_LEN = 3.2; // 尾迹可见长度（边数）
    type Pulse = {
      trail: number[]; // 已过节点，旧→新
      next: number; // 正在亮向的节点
      t: number; // 当前边上的进度 0..1
      speed: number; // 边/毫秒
      edgesLeft: number;
      fade: number; // 出生/消亡包络
      dying: boolean;
      delay: number; // 重生倒计时（毫秒）
    };
    const nbuf: number[] = [];
    const neighborsOf = (k: number) => {
      nbuf.length = 0;
      const i = k % U;
      const j = (k / U) | 0;
      if (i > 0) nbuf.push(k - 1);
      if (i < U - 1) nbuf.push(k + 1);
      if (j > 0) nbuf.push(k - U);
      if (j < V - 1) nbuf.push(k + U);
    };
    const spawnPulse = (p: Pulse, first: boolean) => {
      const k = (Math.random() * N) | 0;
      neighborsOf(k);
      p.trail = [k];
      p.next = nbuf[(Math.random() * nbuf.length) | 0];
      p.t = 0;
      p.speed = (1.6 + Math.random() * 1.8) / 1000;
      p.edgesLeft = 5 + ((Math.random() * 8) | 0);
      p.fade = 0;
      p.dying = false;
      p.delay = first ? Math.random() * 4000 : 600 + Math.random() * 3500;
    };
    const pulses: Pulse[] = [];
    for (let n = 0; n < PULSES; n++) {
      const p = {} as Pulse;
      spawnPulse(p, true);
      pulses.push(p);
    }

    let raf = 0;
    let w = 0;
    let h = 0;
    const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
    const resize = () => {
      w = canvas.clientWidth;
      h = canvas.clientHeight;
      canvas.width = Math.max(1, Math.round(w * dpr));
      canvas.height = Math.max(1, Math.round(h * dpr));
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

    let lastNow = -1;
    const draw = (now: number) => {
      const dt = lastNow < 0 ? 16 : Math.min(50, now - lastNow);
      lastNow = now;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);

      // 推进光脉冲（与场景可见性无关，淡出期间也在走）
      for (const p of pulses) {
        if (p.delay > 0) {
          p.delay -= dt;
          continue;
        }
        if (p.dying) {
          p.fade -= dt / 350;
          if (p.fade <= 0) spawnPulse(p, false);
          continue;
        }
        p.fade = Math.min(1, p.fade + dt / 350);
        p.t += p.speed * dt;
        while (p.t >= 1) {
          p.t -= 1;
          const prev = p.trail[p.trail.length - 1];
          const cur = p.next;
          p.trail.push(cur);
          if (p.trail.length > TRAIL_MAX) p.trail.shift();
          if (--p.edgesLeft <= 0) {
            p.dying = true;
            break;
          }
          // 选下一条边：偏好直行（信号感），不走回头路
          const straight = cur + (cur - prev);
          neighborsOf(cur);
          if (nbuf.includes(straight) && Math.random() < 0.72) {
            p.next = straight;
          } else {
            let pick = prev;
            let seen = 0;
            for (const cand of nbuf) {
              if (cand === prev) continue;
              seen++;
              if (Math.random() < 1 / seen) pick = cand;
            }
            p.next = pick;
          }
        }
      }

      // 时间轴：淡入 → 停留 → 淡出，边界处（不可见时）切换形态
      const cycle = HOLD_MS + FADE_MS * 2;
      const tt = now % (FORMS.length * cycle);
      const slot = Math.floor(tt / cycle);
      const tc = tt - slot * cycle;
      if (slot !== cachedForm) fillForm(slot);
      let vis: number;
      if (tc < FADE_MS) vis = smoothstep(tc / FADE_MS);
      else if (tc < FADE_MS + HOLD_MS) vis = 1;
      else vis = 1 - smoothstep((tc - FADE_MS - HOLD_MS) / FADE_MS);
      if (vis <= 0.004) {
        if (!reduced) raf = requestAnimationFrame(draw);
        return;
      }

      const ry = now * ROT_SPEED;
      const sinY = Math.sin(ry);
      const cosY = Math.cos(ry);
      const sinX = Math.sin(0.4);
      const cosX = Math.cos(0.4);

      // 巨物尺度：大于视口；浮现时带一点生长感
      const scale = Math.max(w, h) * 0.62 * (0.96 + 0.04 * vis);
      const cx = w * 0.5;
      const cy = h * 0.66;

      for (let k = 0; k < N; k++) {
        let x = fx[k];
        let y = fy[k];
        let z = fz[k];
        const x1 = x * cosY + z * sinY;
        const z1 = -x * sinY + z * cosY;
        const y1 = y * cosX - z1 * sinX;
        const z2 = y * sinX + z1 * cosX;
        x = x1;
        y = y1;
        z = z2;
        const persp = 2.6 / (2.6 - z * 0.9);
        px[k] = cx + x * scale * persp;
        py[k] = cy - y * scale * persp;
        pa[k] = Math.min(1, Math.max(0, (z + 1.15) / 2.1)); // 近处亮
      }

      // 整体可见度只动 globalAlpha 这一个旋钮
      ctx.globalAlpha = vis;

      // 边：极淡的白，一次 stroke
      ctx.lineWidth = 1;
      ctx.strokeStyle = "rgba(255,255,255,0.045)";
      ctx.beginPath();
      for (let e = 0; e < edges.length; e += 2) {
        const a = edges[e];
        const b = edges[e + 1];
        ctx.moveTo(px[a], py[a]);
        ctx.lineTo(px[b], py[b]);
      }
      if (U_CLOSED[slot]) {
        for (let e = 0; e < wrapEdges.length; e += 2) {
          const a = wrapEdges[e];
          const b = wrapEdges[e + 1];
          ctx.moveTo(px[a], py[a]);
          ctx.lineTo(px[b], py[b]);
        }
      }
      ctx.stroke();

      // 点：按亮度分桶，每桶一次 fill（矩形，1~2px 下与圆不可分辨）
      for (let b = 0; b < BUCKETS; b++) buckets[b].length = 0;
      for (let k = 0; k < N; k++) {
        const b = Math.min(BUCKETS - 1, (pa[k] * BUCKETS) | 0);
        buckets[b].push(k);
      }
      for (let b = 0; b < BUCKETS; b++) {
        const list = buckets[b];
        if (!list.length) continue;
        ctx.fillStyle = bucketStyle[b];
        ctx.beginPath();
        for (let n = 0; n < list.length; n++) {
          const k = list[n];
          const r = 0.8 + pa[k] * 1.1;
          ctx.rect(px[k] - r, py[k] - r, r * 2, r * 2);
        }
        ctx.fill();
      }

      // 氛围闪烁：尖峰脉冲（sin⁶），弱化版
      ctx.fillStyle = "rgba(255,255,255,0.92)";
      for (let n = 0; n < TWINKLES; n++) {
        const s = Math.sin(now * twSpeed[n] + twPhase[n]);
        if (s <= 0) continue;
        const glint = s * s * s * s * s * s;
        if (glint < 0.02) continue;
        const k = twIdx[n];
        ctx.globalAlpha = vis * glint * (0.35 + pa[k] * 0.65) * 0.5;
        const r = 0.9 + glint * 1.3;
        ctx.beginPath();
        ctx.arc(px[k], py[k], r, 0, Math.PI * 2);
        ctx.fill();
      }

      // 光脉冲：头部亮点 + 沿走过的边渐隐的尾迹
      ctx.strokeStyle = "#ffffff";
      ctx.fillStyle = "#ffffff";
      ctx.lineWidth = 1.2;
      for (const p of pulses) {
        if (p.delay > 0 || p.fade <= 0) continue;
        const tail = p.trail;
        const cur = tail[tail.length - 1];
        const hx = px[cur] + (px[p.next] - px[cur]) * p.t;
        const hy = py[cur] + (py[p.next] - py[cur]) * p.t;
        const base = vis * p.fade * (0.35 + pa[cur] * 0.65);

        // 尾迹：先画 cur→头部 的半段，再逐段回溯，按距头部的边数渐隐
        let x2 = hx;
        let y2 = hy;
        let dist = 0; // 段中点距头部的边数
        for (let s = tail.length - 1; s >= 0; s--) {
          const k = tail[s];
          const segLen = s === tail.length - 1 ? p.t : 1;
          const a = Math.max(0, 1 - (dist + segLen / 2) / TRAIL_LEN);
          if (a <= 0.01) break;
          ctx.globalAlpha = base * a * 0.55;
          ctx.beginPath();
          ctx.moveTo(px[k], py[k]);
          ctx.lineTo(x2, y2);
          ctx.stroke();
          x2 = px[k];
          y2 = py[k];
          dist += segLen;
        }

        // 头部：光晕 + 亮核
        ctx.globalAlpha = base * 0.16;
        ctx.beginPath();
        ctx.arc(hx, hy, 3.8, 0, Math.PI * 2);
        ctx.fill();
        ctx.globalAlpha = base * 0.95;
        ctx.beginPath();
        ctx.arc(hx, hy, 1.5, 0, Math.PI * 2);
        ctx.fill();
      }
      ctx.globalAlpha = 1;

      if (!reduced) raf = requestAnimationFrame(draw);
    };

    raf = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, []);

  return (
    <canvas
      ref={canvasRef}
      className={`pointer-events-none fixed inset-0 h-full w-full ${
        leaving ? "u-scene-depart" : ""
      }`}
      aria-hidden
    />
  );
}
