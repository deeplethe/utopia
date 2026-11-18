import { Button, Chip, MenuSelect } from "../ui";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
/* 用户菜单：顶栏右侧的头像胶囊 + 弹出面板（个人信息 / 系统管理 / 登出）。
   Shell（KB 工作区）与 AccountShell（账户层）共用。 */
import { useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import {
  Languages,
  Layers,
  Monitor,
  Moon,
  LogOut,
  ShieldCheck,
  Sun,
  UserRound,
  SunMoon,
} from "lucide-react";
import { api, type User } from "../api";
import { LANGS, LANG_NAMES, S, lang, setLang } from "../i18n";
import { clearSessionState } from "../authSession";

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

import { getTheme, setTheme, type Theme } from "../theme";

// 三档的顺序就是菜单里的顺序：本色在前，跟系统在最后
const THEMES: Theme[] = ["dark", "light", "system"];
/** 三档主题各自的符号：月亮 / 太阳 / 一块屏（跟着系统走的那一档，说的是"这台机器"） */
const THEME_ICONS: Record<Theme, ReactNode> = {
  dark: <Moon size={13} />,
  light: <Sun size={13} />,
  system: <Monitor size={13} />,
};

/** 菜单里的一行：与顶栏另外两个面板的行同一副（px-4 py-2，正文号，图标 gap 3） */
const ITEM = "gap-3 rounded-none px-4 py-2 text-body";

export function UserMenu({ user }: { user: User }) {
  const [theme, setThemeState] = useState<Theme>(() => getTheme());
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const go = (to: string) => {
    setOpen(false);
    navigate({ to });
  };

  const logout = async () => {
    await api.logout();
    clearSessionState(queryClient);
    navigate({ to: "/login" });
  };

  // 行通到面板边缘（与 Dropdown 同语汇）：容器不留内衬，高度由行自身撑

  return (
    /* 顶栏的用户菜单。**是菜单不是面板**，所以用 DropdownMenu 而不是 Popover：
       它要的是「一列动作」加上二级菜单。语言与主题收进二级——语言以后会有好几种，
       主题有三档，全都平铺在一级里，这张菜单会越长越长，而它们都属于「偏好」，
       不是与个人资料、管理并列的动作。 */
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className="u-avatar-btn">
          {/* 24：胶囊是 32 高（顶栏所有控件同一个高度），头像两边各留 4 */}
          <Avatar name={user.display_name} size={24} />
          <span className="text-body text-ink-2">{user.display_name}</span>
        </Button>
      </DropdownMenuTrigger>
      {/* **行的节奏跟顶栏另外两个面板一样**（px-4 py-2 text-body，整行出血）：
          shadcn 菜单项默认是 px-2 py-1.5，挨着库切换器和告警面板一看就更挤。
          容器去掉 p-1，让行顶到边——悬停归行，与全站一致。 */}
      <DropdownMenuContent align="end" className="w-64 p-0">
        {/* 身份头：不是一个可点的项，只说"你是谁" */}
        <div className="flex items-center gap-3 border-b border-line px-4 py-3">
          <Avatar name={user.display_name} size={32} />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate text-body font-medium text-ink">
                {user.display_name}
              </span>
              {user.is_admin && (
                <Chip tone="neutral" className="text-fine">
                  {S.account.adminChip}
                </Chip>
              )}
            </div>
            <div className="truncate text-fine text-ink-2">{user.email}</div>
          </div>
        </div>

        <div className="py-1">
          <DropdownMenuItem className={ITEM} onSelect={() => go("/account")}>
            <UserRound size={13} />
            {S.account.profile}
          </DropdownMenuItem>
          {/* 人人可看：全部可见库 + 我在每个库的身份 */}
          <DropdownMenuItem className={ITEM} onSelect={() => go("/account/kbs")}>
            <Layers size={13} />
            {S.account.kbsNav}
          </DropdownMenuItem>
          {user.is_admin && (
            <DropdownMenuItem className={ITEM} onSelect={() => go("/admin")}>
              <ShieldCheck size={13} />
              {S.account.administration}
            </DropdownMenuItem>
          )}
        </div>

        {/* 语言与主题：**在值那一头弹出来**，不是把选项摊在下面，也不是往右
            甩一张子菜单。这一行读起来是「语言：English」——要改的是冒号后面
            那一格，弹层就该出现在那一格上，像填空。
            用 shadcn 的 Select：它自己带 portal 与层叠，弹出时盖在菜单之上，
            选完两层一起收。整行都是触发器，点哪儿都能开。 */}
        <div className="border-t border-line py-1">
          <MenuSelect
            icon={<Languages size={13} />}
            label={S.account.language}
            value={lang}
            onChange={(v) => setLang(v as (typeof LANGS)[number])}
            options={LANGS.map((l) => ({ value: l, label: LANG_NAMES[l] }))}
          />
          <MenuSelect
            icon={<SunMoon size={13} />}
            label={S.account.theme}
            value={theme}
            onChange={(v) => {
              setTheme(v as Theme);
              setThemeState(v as Theme);
            }}
            options={THEMES.map((t) => ({
              value: t,
              label: S.account.themeNames[t],
              icon: THEME_ICONS[t],
            }))}
          />
        </div>

        <div className="border-t border-line py-1">
          <DropdownMenuItem className={ITEM} variant="destructive" onSelect={logout}>
            <LogOut size={13} />
            {S.nav.signOut}
          </DropdownMenuItem>
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
