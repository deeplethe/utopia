/* 用户菜单：顶栏右侧的头像胶囊 + 弹出面板（个人信息 / 系统管理 / 登出）。
   Shell（KB 工作区）与 AccountShell（账户层）共用。 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { BookMarked, LogOut, ShieldCheck, UserRound } from "lucide-react";
import { api, type User } from "../api";
import { S } from "../i18n";

/** 首字母头像：中性灰底（chrome 零色偏），拉丁取词首两枚，CJK 取前两字。 */
export function Avatar({ name, size = 24 }: { name: string; size?: number }) {
  const trimmed = name.trim();
  const words = trimmed.split(/\s+/).filter(Boolean);
  const initials =
    words.length >= 2
      ? (words[0][0] + words[1][0]).toUpperCase()
      : [...trimmed].slice(0, 2).join("").toUpperCase();
  return (
    <span
      className="inline-grid place-items-center rounded-full bg-white/[0.09] border border-white/10 text-neutral-200 select-none shrink-0"
      style={{ width: size, height: size, fontSize: Math.round(size * 0.38) }}
    >
      {initials}
    </span>
  );
}

export function UserMenu({ user }: { user: User }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const chipRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  // 原地变形（FLIP）：面板盖在胶囊原位，首帧压到胶囊的真实边界
  //（右上角对齐，圆角 999px），下一帧过渡到面板全形——胶囊"长成"面板
  useLayoutEffect(() => {
    if (!open) return;
    const panel = panelRef.current;
    const chip = chipRef.current;
    if (!panel || !chip) return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const c = chip.getBoundingClientRect();
    const p = panel.getBoundingClientRect();
    if (p.width < 1 || p.height < 1) return;
    panel.style.transformOrigin = "top right";
    panel.style.transform = `scale(${c.width / p.width}, ${c.height / p.height})`;
    panel.style.borderRadius = "999px";
    panel.style.opacity = "0.35";
    const raf = requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        panel.style.transition =
          "transform 0.26s cubic-bezier(0.16,1,0.3,1), border-radius 0.26s cubic-bezier(0.16,1,0.3,1), opacity 0.18s ease";
        panel.style.transform = "scale(1, 1)";
        panel.style.borderRadius = "12px";
        panel.style.opacity = "1";
      }),
    );
    return () => cancelAnimationFrame(raf);
  }, [open]);

  // 反向变形收回：面板缩回胶囊边界后再卸载（导航等即时场景直接关）
  const closingRef = useRef(false);
  const close = () => {
    const panel = panelRef.current;
    const chip = chipRef.current;
    if (closingRef.current) return;
    if (!panel || !chip || window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      setOpen(false);
      return;
    }
    closingRef.current = true;
    const c = chip.getBoundingClientRect();
    // offsetWidth/Height：布局尺寸，不受当前 transform 影响
    panel.style.transition =
      "transform 0.2s cubic-bezier(0.5,0,0.9,0.4), border-radius 0.2s cubic-bezier(0.5,0,0.9,0.4), opacity 0.16s ease";
    panel.style.transform = `scale(${c.width / panel.offsetWidth}, ${c.height / panel.offsetHeight})`;
    panel.style.borderRadius = "999px";
    panel.style.opacity = "0.3";
    window.setTimeout(() => {
      closingRef.current = false;
      setOpen(false);
    }, 190);
  };

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) close();
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

  const go = (to: string) => {
    setOpen(false);
    navigate({ to });
  };

  const logout = async () => {
    await api.logout();
    queryClient.clear();
    navigate({ to: "/login" });
  };

  // 行通到面板边缘（与 Dropdown 同语汇）：容器不留内衬，高度由行自身撑
  const item =
    "w-full flex items-center gap-2.5 px-3.5 py-2.5 text-[13px] text-neutral-300 hover:bg-white/[0.06] hover:text-white transition-colors";

  return (
    <div ref={rootRef} className="relative">
      <button
        ref={chipRef}
        onClick={() => setOpen((v) => !v)}
        className={`flex items-center gap-2 rounded-full py-1 pl-1 pr-3 transition-[background-color,opacity] duration-150 ${
          open ? "opacity-0" : "hover:bg-white/[0.06]"
        }`}
      >
        <Avatar name={user.display_name} size={24} />
        <span className="text-sm text-neutral-300">{user.display_name}</span>
      </button>

      {open && (
        <div
          ref={panelRef}
          className="u-menu-glass absolute right-0 top-0 w-64 rounded-xl shadow-2xl z-50 overflow-hidden"
        >
          {/* 身份头：再点一下缩回胶囊 */}
          <div
            onClick={close}
            className="flex items-center gap-3 px-3.5 py-3 border-b border-white/10 cursor-pointer hover:bg-white/[0.04] transition-colors"
          >
            <Avatar name={user.display_name} size={32} />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="truncate text-[13px] font-medium text-neutral-100">
                  {user.display_name}
                </span>
                {user.is_admin && (
                  <span className="u-chip u-chip-neutral !text-[10px] !px-1.5">
                    {S.account.adminChip}
                  </span>
                )}
              </div>
              <div className="truncate text-[11px] text-neutral-500">{user.email}</div>
            </div>
          </div>

          <div>
            <button onClick={() => go("/account")} className={item}>
              <UserRound size={13} className="text-neutral-500" />
              {S.account.profile}
            </button>
            {/* 人人可看：全部可见库 + 我在每个库的身份 */}
            <button onClick={() => go("/account/kbs")} className={item}>
              <BookMarked size={13} className="text-neutral-500" />
              {S.account.kbsNav}
            </button>
            {user.is_admin && (
              <button onClick={() => go("/admin")} className={item}>
                <ShieldCheck size={13} className="text-neutral-500" />
                {S.account.administration}
              </button>
            )}
          </div>

          <div className="border-t border-white/10">
            <button onClick={logout} className={`${item} !text-[var(--u-danger)]`}>
              <LogOut size={13} />
              {S.nav.signOut}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
