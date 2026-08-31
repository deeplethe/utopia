import {
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
} from "@tanstack/react-router";
import { Account } from "./pages/Account";
import { AccountShell } from "./pages/AccountShell";
import { Chat } from "./pages/Chat";
import { DocViewer } from "./pages/DocViewer";
import { DocsPage } from "./pages/Docs";
import { Graph } from "./pages/Graph";
import { Library } from "./pages/Library";
import { Login } from "./pages/Login";
import { Privacy, Terms } from "./pages/Legal";
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

const chatRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/chat",
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
  getParentRoute: () => appRoute,
  path: "/search",
  component: Search,
});

const graphRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/graph",
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
  getParentRoute: () => appRoute,
  path: "/doc/$docId",
  validateSearch: (search: Record<string, unknown>): { chunk?: string } => ({
    chunk: typeof search.chunk === "string" ? search.chunk : undefined,
  }),
  component: DocViewer,
});

const libraryRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/library",
  validateSearch: (search: Record<string, unknown>): { src?: string } => ({
    src: typeof search.src === "string" ? search.src : undefined,
  }),
  component: Library,
});

const ontologyRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/ontology",
  component: Ontology,
});

const mappingsRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/mappings",
  component: Mappings,
});

const reviewRoute = createRoute({
  getParentRoute: () => appRoute,
  path: "/review",
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
  getParentRoute: () => appRoute,
  path: "/kb-settings",
  validateSearch: (search: Record<string, unknown>): { kb?: string } => ({
    kb: typeof search.kb === "string" ? search.kb : undefined,
  }),
  component: KbSettings,
});

const routeTree = rootRoute.addChildren([
  loginRoute,
  privacyRoute,
  termsRoute,
  settingsRoute,
  docsIndexRoute,
  docsRoute,
  accountShellRoute.addChildren([accountRoute, myKbsRoute, adminRoute]),
  appRoute.addChildren([
    indexRoute,
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
