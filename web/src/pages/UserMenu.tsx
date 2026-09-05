import {
  Button,
  cn,
  Row,
} from "../ui";
/* 用户菜单：顶栏右侧的头像胶囊 + 弹出面板（个人信息 / 系统管理 / 登出）。
   Shell（KB 工作区）与 AccountShell（账户层）共用。 */
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import {
  BookMarked,
  Check,
  Languages,
  LogOut,
  ShieldCheck,
  UserRound,
} from "lucide-react";
import { api, type User } from "../api";
import { usePopoverFlip } from "../ui/popoverFlip";
import { LANGS, LANG_NAMES, S, lang, setLang } from "../i18n";

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
      className="inline-grid place-items-center rounded-full bg-surface-3 border border-line text-ink select-none shrink-0"
      style={{ width: size, height: size, fontSize: Math.round(size * 0.38) }}
    >
      {initials}
    </span>
  );
}

export function UserMenu({ user }: { user: User }) {
  // 原地变形（FLIP）：胶囊"长成"面板。实现共用，见 ui/popoverFlip——
  // 告警铃铛就在旁边，两处各写一遍迟早会差出一点点
  const { open, setOpen, close, rootRef, anchorRef, panelRef } =
    usePopoverFlip<HTMLButtonElement, HTMLDivElement>();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

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

  return (
    <div ref={rootRef} className="relative">
      <Button
        variant="ghost"
        ref={anchorRef}
        className={cn("u-avatar-btn", open && "is-hidden")}
        onClick={() => setOpen((v) => !v)}
      >
        <Avatar name={user.display_name} size={24} />
        <span className="text-body text-ink-2">{user.display_name}</span>
      </Button>

      {open && (
        <div
          ref={panelRef}
          className="u-menu-glass absolute right-0 top-0 w-64 rounded-xl shadow-2xl z-50 overflow-hidden"
        >
          {/* 身份头：再点一下缩回胶囊 */}
          <div
            onClick={close}
            className="u-row-shell flex cursor-pointer items-center gap-3 border-b border-line px-4 py-3"
          >
            <Avatar name={user.display_name} size={32} />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="truncate text-body font-medium text-ink">
                  {user.display_name}
                </span>
                {user.is_admin && (
                  <span className="u-chip u-chip-neutral !text-fine !px-2">
                    {S.account.adminChip}
                  </span>
                )}
              </div>
              <div className="truncate text-fine text-ink-3">
                {user.email}
              </div>
            </div>
          </div>

          <div>
            <Row density="menu" className="gap-3 px-4 py-3 text-body" onClick={() => go("/account")}>
              <UserRound size={13} className="text-ink-3" />
              {S.account.profile}
            </Row>
            {/* 人人可看：全部可见库 + 我在每个库的身份 */}
            <Row density="menu" className="gap-3 px-4 py-3 text-body" onClick={() => go("/account/kbs")}>
              <BookMarked size={13} className="text-ink-3" />
              {S.account.kbsNav}
            </Row>
            {user.is_admin && (
              <Row density="menu" className="gap-3 px-4 py-3 text-body" onClick={() => go("/admin")}>
                <ShieldCheck size={13} className="text-ink-3" />
                {S.account.administration}
              </Row>
            )}
          </div>

          {/* 界面语言：看的人自己定，不经过后端（docs/decisions/0004）。
              每个选项用**它自己的语言**写——看不懂英文的人才认得出"中文" */}
          <div className="border-t border-line">
            <div className="flex items-center gap-3 px-4 pt-3 pb-1 text-fine text-ink-3">
              <Languages size={13} className="text-ink-3" />
              {S.account.language}
            </div>
            {LANGS.map((l) => (
              <Row density="menu" className="gap-3 px-4 py-3 text-body" key={l} onClick={() => setLang(l)}>
                <span className="w-[13px] shrink-0">
                  {l === lang && (
                    <Check size={13} className="text-ink-2" />
                  )}
                </span>
                {LANG_NAMES[l]}
              </Row>
            ))}
          </div>

          <div className="border-t border-line">
            <Row density="menu" danger className="gap-3 px-4 py-3 text-body" onClick={logout}>
              <LogOut size={13} />
              {S.nav.signOut}
            </Row>
          </div>
        </div>
      )}
    </div>
  );
}
