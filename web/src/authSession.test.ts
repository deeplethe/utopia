import { Children, isValidElement, type ReactNode } from "react";
import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, type User } from "./api";
import { fetchMe } from "./authSession";
import { S } from "./i18n";
import { liveAnswer, type LiveHandle } from "./liveAnswer";
import { Login } from "./pages/Login";
import { UserMenu } from "./pages/UserMenu";
import { convMarks } from "./unread";
import { kbStore, wsStore } from "./wsStore";

// 不挂载界面，直接调登录页、用户菜单和 `me` 取数里真实的代码；QueryClient 和聊天的几个 store 都是真的。
const hooks = vi.hoisted(() => ({
  client: undefined as unknown,
  mutation: undefined as { onSuccess: (data: { user: { id: string } }) => void } | undefined,
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

/** 元素树里按可见文字找到那个菜单项的回调 */
function action(tree: ReactNode, label: string): () => unknown {
  for (const child of Children.toArray(tree)) {
    if (!isValidElement<{ children?: ReactNode; onSelect?: () => unknown }>(child)) continue;
    if (Children.toArray(child.props.children).includes(label) && child.props.onSelect) return child.props.onSelect;
    try { return action(child.props.children, label); } catch { /* 下一个兄弟 */ }
  }
  throw new Error(`Missing action: ${label}`);
}

let client: QueryClient;
const handles: LiveHandle[] = [];

/** 上一个人经由真实的 `me` 取数认领了这个标签页，然后留下：所选的库、草稿、上次会话、一场写完未读的回答、一场还在写的 */
async function seedPreviousAccount() {
  vi.spyOn(api, "me").mockResolvedValue({ id: "previous-user" } as User);
  client.setQueryData(["me"], await fetchMe());
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

function expectTabCleared() {
  expect(liveAnswer.get()).toEqual([]);
  expect(sessionStorage.getItem("chat:draft")).toBeNull();
  expect(sessionStorage.getItem("chat:last:shared-kb")).toBeNull();
}

function expectSharedCleared() {
  expect([...convMarks.unread()]).toEqual([]);
  expect(localStorage.getItem("utopia.unread")).toBeNull();
  expect(wsStore.get()).toBeNull();
  expect(kbStore.get()).toBeNull();
  expect(localStorage.getItem("utopia.ws")).toBeNull();
  expect(localStorage.getItem("utopia.kb")).toBeNull();
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("localStorage", new MemoryStorage());
  vi.stubGlobal("sessionStorage", new MemoryStorage());
  vi.stubGlobal("window", { location: { search: "" }, setTimeout });
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

describe("a tab keeps a person's chat state only for that person", () => {
  it("clears all of it on sign-out", async () => {
    await seedPreviousAccount();
    vi.spyOn(api, "logout").mockResolvedValue({ ok: true });
    await action(UserMenu({ user: { display_name: "Previous user" } as User }), S.nav.signOut)();
    expectTabCleared();
    expectSharedCleared();
    expect(client.getQueryCache().getAll()).toEqual([]);
    expect(sessionStorage.getItem("utopia.account")).toBeNull();
    expect(localStorage.getItem("utopia.account")).toBeNull();
    expect(hooks.navigate).toHaveBeenCalledWith({ to: "/login" });
  });

  it("clears it when another account signs in", async () => {
    await seedPreviousAccount();
    Login();
    hooks.mutation!.onSuccess({ user: { id: "next-user" } });
    expectTabCleared();
    expectSharedCleared();
  });

  it("keeps it when the same account signs back in after its session expired", async () => {
    await seedPreviousAccount();
    Login();
    hooks.mutation!.onSuccess({ user: { id: "previous-user" } });
    expect(sessionStorage.getItem("chat:draft")).toBe("unsent question");
    expect(sessionStorage.getItem("chat:last:shared-kb")).toBe("private-conversation");
    expect(liveAnswer.get()).toHaveLength(2);
    expect(kbStore.get()).toBe("shared-kb");
  });

  it("drops a tab's draft on reload after another account signed in in another tab", async () => {
    await seedPreviousAccount();
    // 另一个标签页里 B 登录：共用的那份已经归了 B
    localStorage.setItem("utopia.account", "next-user");
    kbStore.set("next-users-kb");
    vi.spyOn(api, "me").mockResolvedValue({ id: "next-user" } as User);
    await fetchMe();
    expectTabCleared();
    expect(kbStore.get()).toBe("next-users-kb");
  });
});
