// 顶栏弹出面板的"原地变形"（FLIP）：面板盖在触发按钮原位，首帧压到按钮的
// 真实边界（圆角 999px），下一帧过渡到面板全形——按钮"长成"面板；关闭时反向缩回。
//
// **抽成一份共用**，而不是让用户菜单和告警各写一遍：这两个面板紧挨着，
// 时长或缓动差一点点，来回点两下就看得出来。
import { useEffect, useLayoutEffect, useRef, useState } from "react";

const OPEN_MS = 260;
const CLOSE_MS = 190;

function reduced(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * 返回 `{ open, setOpen, close, anchorRef, panelRef }`。
 *
 * `close()` 会先播收回动画再卸载；要立刻关（比如导航走了）直接 `setOpen(false)`。
 * 面板外点击与 Esc 已经接好，挂在 `rootRef` 上。
 */
export function usePopoverFlip<A extends HTMLElement, P extends HTMLElement>() {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const anchorRef = useRef<A>(null);
  const panelRef = useRef<P>(null);
  const closingRef = useRef(false);

  useLayoutEffect(() => {
    if (!open) return;
    const panel = panelRef.current;
    const anchor = anchorRef.current;
    if (!panel || !anchor || reduced()) return;
    const a = anchor.getBoundingClientRect();
    const p = panel.getBoundingClientRect();
    if (p.width < 1 || p.height < 1) return;
    panel.style.transformOrigin = "top right";
    panel.style.transform = `scale(${a.width / p.width}, ${a.height / p.height})`;
    panel.style.borderRadius = "999px";
    panel.style.opacity = "0.35";
    const raf = requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        panel.style.transition = `transform ${OPEN_MS}ms cubic-bezier(0.16,1,0.3,1), border-radius ${OPEN_MS}ms cubic-bezier(0.16,1,0.3,1), opacity 0.18s ease`;
        panel.style.transform = "scale(1, 1)";
        panel.style.borderRadius = "12px";
        panel.style.opacity = "1";
      }),
    );
    return () => cancelAnimationFrame(raf);
  }, [open]);

  const close = () => {
    const panel = panelRef.current;
    const anchor = anchorRef.current;
    if (closingRef.current) return;
    if (!panel || !anchor || reduced()) {
      setOpen(false);
      return;
    }
    closingRef.current = true;
    const a = anchor.getBoundingClientRect();
    // offsetWidth/Height 是布局尺寸，不受当前 transform 影响——
    // 用 getBoundingClientRect 会拿到已经缩过的值，越缩越小
    panel.style.transition = `transform ${CLOSE_MS}ms cubic-bezier(0.5,0,0.9,0.4), border-radius ${CLOSE_MS}ms cubic-bezier(0.5,0,0.9,0.4), opacity 0.16s ease`;
    panel.style.transform = `scale(${a.width / panel.offsetWidth}, ${a.height / panel.offsetHeight})`;
    panel.style.borderRadius = "999px";
    panel.style.opacity = "0.3";
    window.setTimeout(() => {
      closingRef.current = false;
      setOpen(false);
    }, CLOSE_MS);
  };

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node))
        close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  return { open, setOpen, close, rootRef, anchorRef, panelRef };
}
