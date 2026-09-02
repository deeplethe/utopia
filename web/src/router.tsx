import {
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
} from "@tanstack/react-router";
import { Account } from "./pages/Account";
import { AccountShell } from "./pages/AccountShell";
import { Tokens } from "./pages/Tokens";
import { Chat } from "./pages/Chat";
import { DocViewer } from "./pages/DocViewer";
import { DocsPage } from "./pages/Docs";
import { Graph } from "./pages/Graph";
import { Library } from "./pages/Library";
import { Login } from "./pages/Login";
import { Privacy, Terms } from "./pages/Legal";
import { KbRedirect, KbScope } from "./pages/KbScope";
import { KbSettings } from "./pages/KbSettings";
import { MyKbs } from "./pages/MyKbs";
import { NotFound } from "./pages/ServerDown";
import { Ontology } from "./pages/Ontology";
import { Mappings } from "./pages/Mappings";
import { Review } from "./pages/Review";
import { Search } from "./pages/Search";
import { Settings } from "./pages/Settings";
import { Shell } from "./pages/Shell";

const rootRoute = createRootRoute();

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  component: Login,
});

// 公共法务页：登录前可达
const privacyRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/privacy",
  component: Privacy,
});

const termsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/terms",
  component: Terms,
});

const appRoute = createRoute({
  getParentRoute: () => rootRoute,
  id: "app",
  component: Shell,
});

const indexRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/",
  beforeLoad: () => {
    // 首页 = 图谱：产品的差异化门面
    throw redirect({ to: "/graph" });
  },
});

/* 知识库作用域。**库是容器不是筛选条件**——底下这些页面全都属于某一个库，
   路径表达包含关系，路由器也就替我们兜住了「忘了带库」这类错误：
   `/kb/$kbId/search` 没有 id 根本构造不出来。理由详见 pages/KbScope.tsx */
const kbRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/kb/$kbId",
  component: KbScope,
});

const chatRoute = createRoute({
  getParentRoute: () => kbRoute,
  path: "chat",
  component: Chat,
});

// 会话即路由：/chat/$conversationId 只承载 URL（刷新/分享回到同一会话），
// 渲染仍由父级 Chat 负责——父级在 /chat ↔ /chat/$id 间保持挂载，流式不断
const chatConversationRoute = createRoute({
  getParentRoute: () => chatRoute,
  path: "$conversationId",
  component: () => null,
});

const searchRoute = createRoute({
  getParentRoute: () => kbRoute,
  path: "search",
  component: Search,
});

const graphRoute = createRoute({
  getParentRoute: () => kbRoute,
  path: "graph",
  /* 图谱页的可分享状态。**三个都是"你在看什么"，不是"你怎么看"**——
     所以档位（画多少个）刻意不进 URL：那是本地观感，换台机器不该跟着走。

     - entity：选中了谁
     - focus：是否处在某个实体的邻域（与"在全图里选中"是两个画面）
     - at：时间轴停在哪一刻。**这条最不能少**——这产品的卖点就是
       "看某个时刻的世界"，不带时刻的链接把最有意思的那部分丢了 */
  validateSearch: (
    search: Record<string, unknown>,
  ): { entity?: string; focus?: string; at?: string } => ({
    entity: typeof search.entity === "string" ? search.entity : undefined,
    focus: typeof search.focus === "string" ? search.focus : undefined,
    at: typeof search.at === "string" ? search.at : undefined,
  }),
  component: Graph,
});

const docRoute = createRoute({
  getParentRoute: () => kbRoute,
  path: "doc/$docId",
  validateSearch: (search: Record<string, unknown>): { chunk?: string } => ({
    chunk: typeof search.chunk === "string" ? search.chunk : undefined,
  }),
  component: DocViewer,
});

const libraryRoute = createRoute({
  getParentRoute: () => kbRoute,
  path: "library",
  validateSearch: (search: Record<string, unknown>): { src?: string } => ({
    src: typeof search.src === "string" ? search.src : undefined,
  }),
  component: Library,
});

const ontologyRoute = createRoute({
  getParentRoute: () => kbRoute,
  path: "ontology",
  component: Ontology,
});

const mappingsRoute = createRoute({
  getParentRoute: () => kbRoute,
  path: "mappings",
  component: Mappings,
});

const reviewRoute = createRoute({
  getParentRoute: () => kbRoute,
  path: "review",
  component: Review,
});

// 内置文档：公开路由（登录前可读；私有部署离线可用）
const docsIndexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/docs",
  beforeLoad: () => {
    throw redirect({ to: "/docs/$slug", params: { slug: "ingest" } });
  },
});

const docsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/docs/$slug",
  component: DocsPage,
});

// 账户层（Profile / Administration）：与 KB 无关，用无 tab 导航的独立壳
const accountShellRoute = createRoute({
  getParentRoute: () => rootRoute,
  id: "account",
  component: AccountShell,
});

const accountRoute = createRoute({
  getParentRoute: () => accountShellRoute,
  path: "/account",
  component: Account,
});

const myKbsRoute = createRoute({
  getParentRoute: () => accountShellRoute,
  path: "/account/kbs",
  component: MyKbs,
});

// 个人令牌（0014）：给 agent 的钥匙属于人，所以在账户层，不在库里
const tokensRoute = createRoute({
  getParentRoute: () => accountShellRoute,
  path: "/account/tokens",
  component: Tokens,
});

const adminRoute = createRoute({
  getParentRoute: () => accountShellRoute,
  path: "/admin",
  // 深链指定页签（如 KB 数据节的"注册新连接"直达 Data sources）
  validateSearch: (
    search: Record<string, unknown>,
  ): { tab?: "models" | "members" | "kbs" | "datasources" | "deployment" } => ({
    tab:
      search.tab === "models" ||
      search.tab === "members" ||
      search.tab === "kbs" ||
      search.tab === "datasources" ||
      search.tab === "deployment"
        ? search.tab
        : undefined,
  }),
  component: Settings,
});

// 旧路径兼容：/settings → /admin
const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  beforeLoad: () => {
    throw redirect({ to: "/admin" });
  },
});

const kbSettingsRoute = createRoute({
  getParentRoute: () => kbRoute,
  path: "settings",
  component: KbSettings,
});

/* 旧路径兼容：`/graph` 这类不带库的地址仍然可用，解析出该去哪个库再跳。
   **不做成 beforeLoad 重定向**——那时候库列表还没取回来，localStorage 里
   也可能什么都没有（新设备、清过缓存），只能等 useKb 解析出来 */
/* **逐条写出来，不用工厂函数**：工厂里的 path 是 string，类型系统认不出
   字面量，别处 `redirect({ to: "/graph" })` 就通不过。啰嗦换类型安全 */
const legacyGraphRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/graph",
  component: () => <KbRedirect page="graph" />,
});
const legacySearchRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/search",
  component: () => <KbRedirect page="search" />,
});
const legacyChatRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/chat",
  component: () => <KbRedirect page="chat" />,
});
const legacyLibraryRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/library",
  component: () => <KbRedirect page="library" />,
});
const legacyOntologyRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/ontology",
  component: () => <KbRedirect page="ontology" />,
});
const legacyMappingsRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/mappings",
  component: () => <KbRedirect page="mappings" />,
});
const legacyReviewRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/review",
  component: () => <KbRedirect page="review" />,
});
const legacyKbSettingsRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/kb-settings",
  component: () => <KbRedirect page="settings" />,
});

const routeTree = rootRoute.addChildren([
  loginRoute,
  privacyRoute,
  termsRoute,
  settingsRoute,
  docsIndexRoute,
  docsRoute,
  accountShellRoute.addChildren([accountRoute, myKbsRoute, tokensRoute, adminRoute]),
  appRoute.addChildren([
    indexRoute,
    legacyGraphRoute,
    legacySearchRoute,
    legacyChatRoute,
    legacyLibraryRoute,
    legacyOntologyRoute,
    legacyMappingsRoute,
    legacyReviewRoute,
    legacyKbSettingsRoute,
    kbRoute.addChildren([
      chatRoute.addChildren([chatConversationRoute]),
      searchRoute,
      graphRoute,
      docRoute,
      libraryRoute,
      reviewRoute,
      ontologyRoute,
      mappingsRoute,
      kbSettingsRoute,
    ]),
  ]),
]);

export const router = createRouter({
  routeTree,
  // 迷失之城：未知路径的惩戒页
  defaultNotFoundComponent: NotFound,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
