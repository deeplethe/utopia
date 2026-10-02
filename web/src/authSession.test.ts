import { Children, isValidElement, type ReactNode } from "react";
import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type User } from "./api";
import { S } from "./i18n";
import { liveAnswer, type LiveHandle } from "./liveAnswer";
import { Login } from "./pages/Login";
import { UserMenu } from "./pages/UserMenu";
import { convMarks } from "./unread";
import { kbStore, wsStore } from "./wsStore";

// 不挂载界面，直接调登录页与用户菜单里真实的回调；QueryClient 和聊天的几个 store 都是真的。
const hooks = vi.hoisted(() => ({
  client: undefined as unknown,
  mutation: undefined as { onSuccess: () => void } | undefined,
  navigate: vi.fn(),
}));
vi.mock("react", async (importOriginal) => ({
  ...await importOriginal<typeof import("react")>(),
  useState: (initial: unknown) => [typeof initial === "function" ? initial() : initial, vi.fn()],
  useEffect: vi.fn(),
}));
vi.mock("@tanstack/react-query", async (importOriginal) => ({
  ...await importOriginal<typeof import("@tanstack/react-query")>(),
  useQueryClient: () => hooks.client,
  useQuery: () => ({ data: { enabled: true } }),
  useMutation: (options: typeof hooks.mutation) => {
    hooks.mutation = options;
    return { error: null, isPending: false, mutate: vi.fn() };
  },
}));
vi.mock("@tanstack/react-router", async (importOriginal) => ({
  ...await importOriginal<typeof import("@tanstack/react-router")>(),
  useNavigate: () => hooks.navigate,
}));

class MemoryStorage {
  private values = new Map<string, string>();
  get length() { return this.values.size; }
  key(index: number) { return [...this.values.keys()][index] ?? null; }
  getItem(key: string) { return this.values.get(key) ?? null; }
  setItem(key: string, value: string) { this.values.set(key, value); }
  removeItem(key: string) { this.values.delete(key); }
}

/** 元素树里按可见文字找到那个按钮或菜单项的回调 */
function action(tree: ReactNode, label: string, prop: "onSelect" | "onClick"): () => unknown {
  for (const child of Children.toArray(tree)) {
    if (!isValidElement<{ children?: ReactNode; onSelect?: () => unknown; onClick?: () => unknown }>(child)) continue;
    if (Children.toArray(child.props.children).includes(label) && child.props[prop]) return child.props[prop];
    try { return action(child.props.children, label, prop); } catch { /* 下一个兄弟 */ }
  }
  throw new Error(`Missing action: ${label}`);
}

let client: QueryClient;
const handles: LiveHandle[] = [];

/** 上一个人留在这个标签页里的：缓存、所选的库、草稿、上次会话、一场写完未读的回答、一场还在写的 */
function seedPreviousAccount() {
  client.setQueryData(["me"], { id: "previous-user" });
  wsStore.set("previous-workspace");
  kbStore.set("shared-kb");
  sessionStorage.setItem("chat:draft", "unsent question");
  sessionStorage.setItem("chat:last:shared-kb", "private-conversation");
  convMarks.view(null);
  const done = liveAnswer.begin("shared-kb", "private-conversation", [{ role: "assistant", content: "answer" }], () => {});
  const running = liveAnswer.begin("shared-kb", "still-running", [{ role: "assistant", content: "partial" }], () => {});
  handles.push(done, running);
  done.finish();
  expect(convMarks.unread().has("private-conversation")).toBe(true);
}

function expectNothingOfThePreviousAccount() {
  expect(liveAnswer.get()).toEqual([]);
  expect(client.getQueryCache().getAll()).toEqual([]);
  expect([...convMarks.unread()]).toEqual([]);
  expect(localStorage.getItem("utopia.unread")).toBeNull();
  expect(wsStore.get()).toBeNull();
  expect(kbStore.get()).toBeNull();
  expect(localStorage.getItem("utopia.ws")).toBeNull();
  expect(localStorage.getItem("utopia.kb")).toBeNull();
  expect(sessionStorage.getItem("chat:draft")).toBeNull();
  expect(sessionStorage.getItem("chat:last:shared-kb")).toBeNull();
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("localStorage", new MemoryStorage());
  vi.stubGlobal("sessionStorage", new MemoryStorage());
  vi.stubGlobal("window", { location: { search: "", href: "" }, setTimeout });
  client = new QueryClient();
  hooks.client = client;
  hooks.navigate.mockReset();
});
afterEach(() => {
  for (const handle of handles.splice(0)) handle.discard();
  for (const id of [...convMarks.unread()]) convMarks.forget(id);
  client.clear();
  vi.restoreAllMocks();
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("the next account on a tab finds nothing of the previous one", () => {
  it("after signing out", async () => {
    seedPreviousAccount();
    vi.spyOn(api, "logout").mockResolvedValue({ ok: true });
    await action(UserMenu({ user: { display_name: "Previous user" } as User }), S.nav.signOut, "onSelect")();
    expectNothingOfThePreviousAccount();
    expect(hooks.navigate).toHaveBeenCalledWith({ to: "/login" });
  });

  it("after signing in once the previous session expired", () => {
    seedPreviousAccount();
    Login();
    hooks.mutation!.onSuccess();
    expectNothingOfThePreviousAccount();
  });

  it("before the single sign-on redirect", () => {
    seedPreviousAccount();
    action(Login(), S.login.ssoButton, "onClick")();
    expectNothingOfThePreviousAccount();
    expect(window.location.href).toBe("/api/v1/auth/oidc/start");
  });
});
