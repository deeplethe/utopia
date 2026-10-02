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
import { Wordmark } from "../ui";
import { KbSwitcher } from "./KbSwitcher";
import { HeaderActions } from "./HeaderActions";
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
  });
  const { kb, kbs, setKb, settled } = useKb();
  // 标题跟随当前 tab：`Graph · Utopia`；文档查看页归入 Library
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const tabLabel =
    TABS.find((t) => pathname.startsWith(t.to))?.label ??
    (pathname.startsWith("/doc/") ? S.nav.library : undefined);
  usePageTitle(S.app.name, tabLabel);
  // 全局唯一的 KB 事件流连接：文档/审核状态实时刷新（替轮询）。告警也从它来
  useKbEvents(kb?.id);
  // 没有打开着的库时才单开告警流：一页只占一条通知用的连接（#1028）。
  // 等列表取回来再判断，否则每次加载都先连上、库一到又断开
  useAlertEvents(settled && !kb);

  // 未登录就去登录页。**副作用要在 effect 里**，理由见下面 401 那一支
  const unauthorized =
    me.isError && me.error instanceof ApiError && me.error.status === 401;
  useEffect(() => {
    if (unauthorized) navigate({ to: "/login" });
  }, [unauthorized, navigate]);

  if (me.isPending) {
    return (
      <div className="min-h-screen flex items-center justify-center text-ink-2 text-body">
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
      {/* 左内距 32px：字标的左缘落在下面第一个标签的图标上（nav px-4 + 标签左 16）。
          字标与切换器之间 gap-6（切换器自己只有 px-2）：切换器的图标正好落在第二个
          标签的图标上（132；英文界面下的巧合，字标或标签一换尺寸就得重量）——
          两行同一套节奏 */}
      <header className="glass-strong relative z-40 border-x-0 border-t-0 h-12 shrink-0 flex items-center gap-6 px-2">
        {/* 字标：逐字母淡入，hover 浮出 ↗，点击去官网 */}
        <Wordmark className="u-wordmark-top pl-3" />
        {/* 知识库切换器紧跟字标，中间不画斜杠——它不是面包屑的第二级，就是
            「现在在哪个库」。Workspace 已从概念层折叠为部署级隐形管道
            （settings/members 仍经它走 API，如 organizations 之于单租户）。
            左边的图标与账户页左栏「Knowledge bases」那一项同一个，说明这一串字
            是库名；中号字与下面的标签同一个字号；箭头贴着名字，不顶到一个固定
            宽度的右边去。
            纯切换器：建库是管理动作，入口在 System settings › Knowledge bases */}
        <KbSwitcher kb={kb} kbs={kbs} onChange={setKb} />
        {/* 右上那一组三个顶栏共用一份（HeaderActions）：换页时它不该动 */}
        <HeaderActions
          link={{ to: "/docs", label: S.nav.docs }}
          version={health.data?.version}
          user={me.data}
        />
      </header>

      {/* Tab 导航条：图标 + 文字，选中的那一项是一颗填底的药丸。
          **左边一条线：盒从 8 起，内容从 20 起**——与左栏完全一样
          （栏 px-2 = 8，行 px-3 = 12，于是图标落在 20、文字落在 42）。
          顶栏的字标也按这条线：header pl-2 + 字标 pl-3 = 20。
          三处左缘从此是同一条竖线，往下看不会错位。
          py-1 而不是 py-2：药丸自己已经 32 高，外面再垫 8 上下，整条 48，
          压着下面的正文；垫 4 是 40，与顶栏 48 加起来正好一屏不占太多 */}
      <nav className="glass-strong border-x-0 border-t-0 shrink-0 flex items-center gap-1 px-2 py-1">
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
