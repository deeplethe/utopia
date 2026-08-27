import { useQuery } from "@tanstack/react-query";
import { Link, Outlet, useNavigate, useRouterState } from "@tanstack/react-router";
import {
  BookMarked,
  Library as LibraryIcon,
  ListChecks,
  MessagesSquare,
  Search as SearchIcon,
  Settings as SettingsIcon,
  Shapes,
  Waypoints,
} from "lucide-react";
import { api, ApiError } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { Dropdown, GithubMark, Wordmark } from "../ui";
import { UserMenu } from "./UserMenu";
import { ServerDown } from "./ServerDown";
import { useKbEvents } from "../useKbEvents";
import { usePageTitle } from "../useTitle";

const TABS = [
  // 图谱是门面，排第一；两种查询方式（Search/Ask）随后
  { to: "/graph", label: S.nav.graph, Icon: Waypoints },
  { to: "/search", label: S.nav.search, Icon: SearchIcon },
  { to: "/chat", label: S.nav.ask, Icon: MessagesSquare },
  { to: "/library", label: S.nav.library, Icon: LibraryIcon },
  { to: "/review", label: S.review.title, Icon: ListChecks },
  { to: "/ontology", label: S.ontology.title, Icon: Shapes },
  // 库设置与其它 tab 同为"当前知识库作用域"，并列于内容导航
  { to: "/kb-settings", label: S.nav.settings, Icon: SettingsIcon },
] as const;

export function Shell() {
  const navigate = useNavigate();

  const me = useQuery({ queryKey: ["me"], queryFn: api.me });
  const health = useQuery({ queryKey: ["health"], queryFn: api.health, staleTime: Infinity });
  const { kb, kbs, setKb } = useKb();
  // 标题跟随当前 tab：`Graph · Utopia`；文档查看页归入 Library
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const tabLabel =
    TABS.find((t) => pathname.startsWith(t.to))?.label ??
    (pathname.startsWith("/doc/") ? S.nav.library : undefined);
  usePageTitle(S.app.name, tabLabel);
  // 全局唯一的 KB 事件流连接：文档/审核状态实时刷新（替轮询）
  useKbEvents(kb?.id);

  if (me.isPending) {
    return (
      <div className="min-h-screen flex items-center justify-center text-neutral-500 text-sm">
        {S.nav.loading}
      </div>
    );
  }

  if (me.isError) {
    if (me.error instanceof ApiError && me.error.status === 401) {
      navigate({ to: "/login" });
      return null;
    }
    return <ServerDown />;
  }

  return (
    <div className="h-screen flex flex-col overflow-hidden u-arrive">
      {/* 顶栏：品牌 + 工作区 + 用户（Vercel 式） */}
      {/* z-40：backdrop-filter 使顶栏与 tab 条各自成 stacking context，
          不提权则后者按 DOM 序盖住顶栏内的弹出面板 */}
      <header className="glass-strong relative z-40 border-x-0 border-t-0 h-14 shrink-0 flex items-center gap-4 px-5">
        {/* 字标：逐字母淡入，hover 浮出 ↗，点击去官网 */}
        <Wordmark className="text-[17px]" />
        {/* 面包屑唯一一级：知识库。Workspace 已从概念层折叠为部署级隐形管道
            （settings/members 仍经它走 API，如 organizations 之于单租户）。 */}
        <span className="text-neutral-700">/</span>
        {/* 纯切换器：建库是管理动作，入口在 System settings › Knowledge bases */}
        <Dropdown
          className="w-40"
          size="sm"
          icon={<BookMarked size={12} />}
          menuLabel={S.nav.kbLabel}
          value={kb?.id ?? ""}
          onChange={setKb}
          options={kbs.map((k) => ({ value: k.id, label: k.name }))}
        />
        <div className="ml-auto flex items-center gap-1.5">
          {/* 项目入口：Docs + [GitHub·版本] 胶囊（版本取自后端 health，与部署一致）。
              版本并入 GitHub 胶囊：两个等高元素，视觉平衡 */}
          <Link
            to="/docs"
            className="px-2 py-1 rounded-lg text-[12.5px] text-neutral-500 hover:text-neutral-200 hover:bg-white/[0.05] transition-colors"
          >
            {S.nav.docs}
          </Link>
          <a
            href={S.login.githubUrl}
            target="_blank"
            rel="noreferrer"
            title="GitHub"
            className="flex items-center gap-1.5 rounded-full border border-white/10 px-2.5 py-1 text-neutral-500 hover:text-neutral-200 hover:border-white/25 transition-colors"
          >
            <GithubMark size={13} />
            {health.data && (
              <span className="u-num text-[11px]">v{health.data.version}</span>
            )}
          </a>
          {/* 用户菜单：个人信息 / 系统管理（仅管理员）/ 登出 */}
          <div className="ml-1.5">
            <UserMenu user={me.data} />
          </div>
        </div>
      </header>

      {/* Tab 导航条：图标 + 文字，激活态下划线（Vercel 式） */}
      <nav className="glass-strong border-x-0 border-t-0 shrink-0 flex items-stretch gap-1 px-4">
        {TABS.map(({ to, label, Icon }) => (
          <Link
            key={to}
            to={to}
            className="flex items-center gap-2 px-3.5 py-2.5 text-[13.5px] font-medium text-neutral-400 border-b-2 border-transparent hover:text-neutral-200"
            activeProps={{
              className:
                "flex items-center gap-2 px-3.5 py-2.5 text-[13.5px] font-medium text-white border-b-2 border-white",
            }}
          >
            <Icon size={15} strokeWidth={1.8} />
            {label}
          </Link>
        ))}
      </nav>

      <main className="flex-1 min-h-0">
        <Outlet />
      </main>
    </div>
  );
}

