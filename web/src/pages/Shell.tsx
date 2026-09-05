import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Link,
  Outlet,
  useNavigate,
  useRouterState,
} from "@tanstack/react-router";
import {
  Database,
  Layers,
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
import { useKb, useKbId } from "../kb";
import { Dropdown, GithubMark, Wordmark } from "../ui";
import { AlertBell } from "./AlertBell";
import { UserMenu } from "./UserMenu";
import { ServerDown } from "./ServerDown";
import { useAlertEvents } from "../useAlertEvents";
import { useKbEvents } from "../useKbEvents";
import { usePageTitle } from "../useTitle";

const TABS = [
  // 图谱是门面，排第一；两种查询方式（Search/Ask）随后
  { to: "/kb/$kbId/graph", label: S.nav.graph, Icon: Waypoints },
  { to: "/kb/$kbId/search", label: S.nav.search, Icon: SearchIcon },
  { to: "/kb/$kbId/chat", label: S.nav.ask, Icon: MessagesSquare },
  { to: "/kb/$kbId/library", label: S.nav.library, Icon: LibraryIcon },
  { to: "/kb/$kbId/review", label: S.review.title, Icon: ListChecks },
  { to: "/kb/$kbId/ontology", label: S.ontology.title, Icon: Shapes },
  // 本体说「世界上有什么」，数据映射说「这个数在库里怎么算」——挨着放
  { to: "/kb/$kbId/mappings", label: S.mapping.title, Icon: Database },
  // 库设置与其它 tab 同为"当前知识库作用域"，并列于内容导航
  { to: "/kb/$kbId/settings", label: S.nav.settings, Icon: SettingsIcon },
] as const;

export function Shell() {
  const navigate = useNavigate();
  const kbId = useKbId();

  const me = useQuery({ queryKey: ["me"], queryFn: api.me });
  const health = useQuery({
    queryKey: ["health"],
    queryFn: api.health,
    staleTime: Infinity,
  });
  const { kb, kbs, setKb } = useKb();
  // 标题跟随当前 tab：`Graph · Utopia`；文档查看页归入 Library
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const tabLabel =
    TABS.find((t) => pathname.startsWith(t.to))?.label ??
    (pathname.startsWith("/doc/") ? S.nav.library : undefined);
  usePageTitle(S.app.name, tabLabel);
  // 全局唯一的 KB 事件流连接：文档/审核状态实时刷新（替轮询）
  useKbEvents(kb?.id);
  // 告警流是全局的：角标跨库，而系统级告警根本没有库
  useAlertEvents();

  // 未登录就去登录页。**副作用要在 effect 里**，理由见下面 401 那一支
  const unauthorized =
    me.isError && me.error instanceof ApiError && me.error.status === 401;
  useEffect(() => {
    if (unauthorized) navigate({ to: "/login" });
  }, [unauthorized, navigate]);

  if (me.isPending) {
    return (
      <div className="min-h-screen flex items-center justify-center text-ink-3 text-body">
        {S.nav.loading}
      </div>
    );
  }

  if (me.isError) {
    // **跳转在 effect 里做，不在渲染里。** 渲染期间调 `navigate` 是在别人渲染
    // 的过程中改路由器的状态，React 会常驻一条「Cannot update a component
    // while rendering a different component」的警告。今天不出错，但它是
    // 「渲染顺序依赖」的味道——改布局时最容易在这种地方变成真 bug
    if (me.error instanceof ApiError && me.error.status === 401) {
      return null;
    }
    return <ServerDown />;
  }

  return (
    <div className="h-screen flex flex-col overflow-hidden u-arrive">
      {/* 顶栏：品牌 + 工作区 + 用户（Vercel 式） */}
      {/* z-40：backdrop-filter 使顶栏与 tab 条各自成 stacking context，
          不提权则后者按 DOM 序盖住顶栏内的弹出面板 */}
      {/* 左内距 32px：字标的左缘落在下面第一个标签的图标上（nav px-4 + 标签
          px-4）。字标与切换器之间 gap-4：切换器的图标正好落在第二个标签的图标上
          （英文界面下的巧合，字标一换字号就得重量）——两行同一套节奏 */}
      <header className="glass-strong relative z-40 border-x-0 border-t-0 h-14 shrink-0 flex items-center gap-4 px-8">
        {/* 字标：逐字母淡入，hover 浮出 ↗，点击去官网 */}
        <Wordmark className="text-display" />
        {/* 知识库切换器紧跟字标，中间不画斜杠——它不是面包屑的第二级，就是
            「现在在哪个库」。Workspace 已从概念层折叠为部署级隐形管道
            （settings/members 仍经它走 API，如 organizations 之于单租户）。
            左边的图标与账户页左栏「Knowledge bases」那一项同一个，说明这一串字
            是库名；中号字与下面的标签同一个字号；箭头贴着名字，不顶到一个固定
            宽度的右边去。
            纯切换器：建库是管理动作，入口在 System settings › Knowledge bases */}
        <Dropdown
          bare
          className="max-w-64"
          icon={<Layers size={13} />}
          menuLabel={S.nav.kbLabel}
          value={kb?.id ?? ""}
          onChange={setKb}
          options={kbs.map((k) => ({ value: k.id, label: k.name }))}
        />
        {/* 三组：项目入口 / 告警 / 身份。**组间 gap-3，组内 gap-2**——
            间距由结构表达，而不是给某一个元素补一次性的 ml。
            此前用户菜单挂着一个 ml-2（当初它紧挨 GitHub 胶囊时调的），
            铃铛插进两者之间以后就成了左 6px 右 12px */}
        <div className="ml-auto flex items-center gap-3">
          {/* 项目入口：Docs + [GitHub·版本] 胶囊（版本取自后端 health，与部署一致）。
              版本并入 GitHub 胶囊：两个等高元素，视觉平衡。
              这两个是一对，所以彼此贴得比组间近 */}
          <div className="flex items-center gap-2">
            <Link
              to="/docs"
              className="u-navlink"
            >
              {S.nav.docs}
            </Link>
            <a
              href={S.login.githubUrl}
              target="_blank"
              rel="noreferrer"
              title="GitHub"
              className="u-pill"
            >
              <GithubMark size={13} />
              {health.data && (
                <span className="u-num text-fine">
                  v{health.data.version}
                </span>
              )}
            </a>
          </div>
          {/* 告警角标：跨库的未读数。失败此前只留在日志与 jobs.last_error 里，
              界面上一份文档也不会变颜色（0005） */}
          <AlertBell />
          {/* 用户菜单：个人信息 / 系统管理（仅管理员）/ 登出 */}
          <UserMenu user={me.data} />
        </div>
      </header>

      {/* Tab 导航条：图标 + 文字，激活态下划线（Vercel 式） */}
      <nav className="glass-strong border-x-0 border-t-0 shrink-0 flex items-stretch gap-1 px-4">
        {TABS.map(({ to, label, Icon }) => (
          <Link
            key={to}
            to={to}
            params={{ kbId }}
            className="u-tab"
            activeProps={{ className: "u-tab is-active" }}
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
