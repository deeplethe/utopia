import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryObserver, type QueryKey } from "@tanstack/react-query";
import { DEFAULT_STALE_MS, STREAM_KEYS, STREAM_STALE_MS, applyQueryDefaults } from "./queryDefaults";
import { SETTLE_MS, useKbEvents } from "./useKbEvents";
import { useAlertEvents } from "./useAlertEvents";
import { HIDDEN_RELEASE_MS } from "./eventStream";

// 在 Node 环境执行 hook 的 effect，浏览器边界用可控事件源；查询与观察者仍用真实实现。
const hooks = vi.hoisted(() => ({
  effects: [] as Array<() => void | (() => void)>,
  client: undefined as unknown,
}));
vi.mock("react", async (importOriginal) => ({
  ...await importOriginal<typeof import("react")>(),
  useEffect: (effect: () => void | (() => void)) => hooks.effects.push(effect),
}));
vi.mock("@tanstack/react-query", async (importOriginal) => ({
  ...await importOriginal<typeof import("@tanstack/react-query")>(),
  useQueryClient: () => hooks.client,
}));

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  listeners = new Map<string, Set<() => void>>();
  closed = false;

  constructor(readonly url: string) {
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: () => void) {
    const listeners = this.listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.listeners.set(type, listeners);
  }

  removeEventListener(type: string, listener: () => void) {
    this.listeners.get(type)?.delete(listener);
  }

  emit(type: string) {
    for (const listener of this.listeners.get(type) ?? []) listener();
  }

  close() {
    this.closed = true;
  }
}

/** 只有可见性的假页面：node 环境里没有 `document` */
function fakePage(visible = true) {
  const page = {
    visibilityState: visible ? "visible" : "hidden",
    listeners: new Set<() => void>(),
    addEventListener(_type: string, listener: () => void) {
      page.listeners.add(listener);
    },
    removeEventListener(_type: string, listener: () => void) {
      page.listeners.delete(listener);
    },
    show(shown: boolean) {
      page.visibilityState = shown ? "visible" : "hidden";
      for (const listener of [...page.listeners]) listener();
    },
  };
  vi.stubGlobal("document", page);
  return page;
}

const cleanups: Array<() => void> = [];
const clients: QueryClient[] = [];
const unsubscribes: Array<() => void> = [];

function mount(hook: () => void) {
  hook();
  const cleanup = hooks.effects.pop()?.();
  let mounted = true;
  const unmount = () => {
    if (!mounted) return;
    mounted = false;
    cleanup?.();
  };
  cleanups.push(unmount);
  return { source: FakeEventSource.instances.at(-1)!, unmount };
}

function createClient() {
  const client = new QueryClient({
    defaultOptions: {
      queries: { staleTime: DEFAULT_STALE_MS, refetchOnWindowFocus: false, retry: false },
    },
  });
  applyQueryDefaults(client);
  hooks.client = client;
  clients.push(client);
  return client;
}

async function watch(client: QueryClient, key: QueryKey) {
  let serverData = "before disconnect";
  const fetch = vi.fn(async () => serverData);
  const observer = new QueryObserver(client, { queryKey: key, queryFn: fetch });
  unsubscribes.push(observer.subscribe(() => {}));
  // subscribe 启动的首次读取与 refetch 共享同一次请求。
  await observer.refetch();
  expect(fetch).toHaveBeenCalledTimes(1);
  return {
    fetch,
    observer,
    changeServer: () => { serverData = "after disconnect"; },
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("EventSource", FakeEventSource);
  FakeEventSource.instances = [];
  hooks.effects = [];
});

afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  for (const unsubscribe of unsubscribes.splice(0)) unsubscribe();
  for (const client of clients.splice(0)) client.clear();
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("KB stream reconnect", () => {
  it("refetches missed changes immediately for every stream key in the current KB", async () => {
    const client = createClient();
    const current = await Promise.all(STREAM_KEYS.map((head) => watch(client, [head, "kb-a", "detail"])));
    const otherKb = await watch(client, ["graph", "kb-b"]);
    const unrelated = await watch(client, ["health"]);
    const inactiveKey = ["documents", "kb-a", "unmounted"];
    client.setQueryData(inactiveKey, "cached before disconnect");
    const { source } = mount(() => useKbEvents("kb-a"));
    expect(source.url).toBe("/api/v1/kbs/kb-a/events");
    source.emit("open");
    for (const query of current) expect(query.fetch).toHaveBeenCalledTimes(1);

    source.emit("error");
    for (const query of current) query.changeServer();
    // staleTime 到期只让缓存变旧，不会自动请求；断线期间完全没有业务事件。
    await vi.advanceTimersByTimeAsync(STREAM_STALE_MS + 1);
    for (const query of current) expect(query.fetch).toHaveBeenCalledTimes(1);

    source.emit("open");
    // 重连不等普通事件的 300ms 合并窗口。
    for (const query of current) expect(query.fetch).toHaveBeenCalledTimes(2);
    await vi.waitFor(() => {
      for (const query of current) expect(query.observer.getCurrentResult().data).toBe("after disconnect");
    });
    expect(client.getQueryState(inactiveKey)?.isInvalidated).toBe(true);
    expect(otherKb.fetch).toHaveBeenCalledTimes(1);
    expect(unrelated.fetch).toHaveBeenCalledTimes(1);
  });

  it("recovers an initial failed connection and clears the recovery flag after open", async () => {
    const client = createClient();
    const query = await watch(client, ["documents", "kb-a"]);
    const { source } = mount(() => useKbEvents("kb-a"));
    source.emit("error");
    query.changeServer();
    source.emit("open");
    expect(query.fetch).toHaveBeenCalledTimes(2);
    source.emit("open");
    expect(query.fetch).toHaveBeenCalledTimes(2);
  });

  it("flushes a pending burst together with reconnect instead of scheduling another refetch", async () => {
    const client = createClient();
    const graph = await watch(client, ["graph", "kb-a"]);
    const documents = await watch(client, ["documents", "kb-a"]);
    const { source } = mount(() => useKbEvents("kb-a"));
    source.emit("open");
    source.emit("document");
    source.emit("graph");
    source.emit("error");
    source.emit("open");
    expect(graph.fetch).toHaveBeenCalledTimes(2);
    expect(documents.fetch).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(graph.fetch).toHaveBeenCalledTimes(2);
    expect(documents.fetch).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("keeps normal business events coalesced", async () => {
    const client = createClient();
    const graph = await watch(client, ["graph", "kb-a"]);
    const documents = await watch(client, ["documents", "kb-a"]);
    const { source } = mount(() => useKbEvents("kb-a"));
    source.emit("open");
    for (let i = 0; i < 12; i++) {
      source.emit("document");
      source.emit("graph");
    }
    await vi.advanceTimersByTimeAsync(SETTLE_MS - 1);
    expect(graph.fetch).toHaveBeenCalledTimes(1);
    expect(documents.fetch).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(graph.fetch).toHaveBeenCalledTimes(2);
    expect(documents.fetch).toHaveBeenCalledTimes(2);
  });

  it("ignores late callbacks from the old connection after switching KB", async () => {
    const client = createClient();
    const oldKb = await watch(client, ["graph", "kb-a"]);
    const newKb = await watch(client, ["graph", "kb-b"]);
    const old = mount(() => useKbEvents("kb-a"));
    old.source.emit("open");
    old.source.emit("error");
    const lateOpen = [...old.source.listeners.get("open") ?? []];
    const lateGraph = [...old.source.listeners.get("graph") ?? []];
    old.unmount();
    const next = mount(() => useKbEvents("kb-b"));
    next.source.emit("open");
    for (const callback of [...lateOpen, ...lateGraph]) callback();
    old.source.emit("open");
    old.source.emit("document");
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(old.source.closed).toBe(true);
    expect([...old.source.listeners.values()].every((listeners) => listeners.size === 0)).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
    expect(oldKb.fetch).toHaveBeenCalledTimes(1);
    expect(newKb.fetch).toHaveBeenCalledTimes(1);
    next.source.emit("error");
    next.source.emit("open");
    expect(oldKb.fetch).toHaveBeenCalledTimes(1);
    expect(newKb.fetch).toHaveBeenCalledTimes(2);
  });

  it("does not subscribe without a selected KB", () => {
    createClient();
    mount(() => useKbEvents(undefined));
    expect(FakeEventSource.instances).toHaveLength(0);
  });
});

describe("alert stream reconnect", () => {
  it("refreshes both badge and list after missed alerts, without refetching on the initial open", async () => {
    const client = createClient();
    const badge = await watch(client, ["alerts", "unread"]);
    const list = await watch(client, ["alerts", "list", "", 0]);
    const other = await watch(client, ["documents", "kb-a"]);
    const { source } = mount(useAlertEvents);
    expect(source.url).toBe("/api/v1/alerts/events");
    source.emit("open");
    expect(badge.fetch).toHaveBeenCalledTimes(1);
    expect(list.fetch).toHaveBeenCalledTimes(1);
    source.emit("error");
    badge.changeServer();
    list.changeServer();
    source.emit("open");
    expect(badge.fetch).toHaveBeenCalledTimes(2);
    expect(list.fetch).toHaveBeenCalledTimes(2);
    expect(other.fetch).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => {
      expect(badge.observer.getCurrentResult().data).toBe("after disconnect");
      expect(list.observer.getCurrentResult().data).toBe("after disconnect");
    });
  });

  it("also recovers when the first alert connection failed before open", async () => {
    const client = createClient();
    const query = await watch(client, ["alerts", "unread"]);
    const { source } = mount(useAlertEvents);
    source.emit("error");
    source.emit("open");
    expect(query.fetch).toHaveBeenCalledTimes(2);
    source.emit("open");
    expect(query.fetch).toHaveBeenCalledTimes(2);
  });

  it("removes listeners and ignores late alert callbacks after cleanup", async () => {
    const client = createClient();
    const query = await watch(client, ["alerts", "unread"]);
    const { source, unmount } = mount(useAlertEvents);
    source.emit("open");
    source.emit("error");
    const lateCallbacks = [...source.listeners.values()].flatMap((listeners) => [...listeners]);
    unmount();
    for (const callback of lateCallbacks) callback();
    expect(source.closed).toBe(true);
    expect([...source.listeners.values()].every((listeners) => listeners.size === 0)).toBe(true);
    expect(query.fetch).toHaveBeenCalledTimes(1);
  });
});

describe("one notification stream per page (#1028)", () => {
  it("carries alerts on the KB stream, and refreshes them when it recovers", async () => {
    const client = createClient();
    const badge = await watch(client, ["alerts", "unread"]);
    const { source } = mount(() => useKbEvents("kb-a"));
    source.emit("open");
    source.emit("alert");
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(badge.fetch).toHaveBeenCalledTimes(2);
    source.emit("error");
    source.emit("open");
    expect(badge.fetch).toHaveBeenCalledTimes(3);
  });

  it("opens no alert stream while a KB stream is open", () => {
    createClient();
    mount(() => useKbEvents("kb-a"));
    mount(() => useAlertEvents(false));
    expect(FakeEventSource.instances.map((source) => source.url)).toEqual(["/api/v1/kbs/kb-a/events"]);
  });

  it("gives up the connection after the page has been hidden a while, and catches up on return", async () => {
    const page = fakePage();
    const client = createClient();
    const documents = await watch(client, ["documents", "kb-a"]);
    const { source } = mount(() => useKbEvents("kb-a"));
    source.emit("open");

    page.show(false);
    await vi.advanceTimersByTimeAsync(HIDDEN_RELEASE_MS - 1);
    expect(source.closed).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(source.closed).toBe(true);
    expect([...source.listeners.values()].every((listeners) => listeners.size === 0)).toBe(true);

    documents.changeServer();
    page.show(true);
    const reopened = FakeEventSource.instances.at(-1)!;
    expect(reopened).not.toBe(source);
    expect(documents.fetch).toHaveBeenCalledTimes(1);
    reopened.emit("open");
    expect(documents.fetch).toHaveBeenCalledTimes(2);
  });

  it("keeps the connection through a short absence, and removes its page listener on cleanup", async () => {
    const page = fakePage();
    const client = createClient();
    const documents = await watch(client, ["documents", "kb-a"]);
    const { source, unmount } = mount(() => useKbEvents("kb-a"));
    source.emit("open");
    page.show(false);
    await vi.advanceTimersByTimeAsync(HIDDEN_RELEASE_MS - 1);
    page.show(true);
    await vi.advanceTimersByTimeAsync(HIDDEN_RELEASE_MS);
    expect(source.closed).toBe(false);
    expect(FakeEventSource.instances).toHaveLength(1);
    expect(documents.fetch).toHaveBeenCalledTimes(1);

    page.show(false);
    unmount();
    expect(page.listeners.size).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("waits for a page opened in the background to be shown before connecting", () => {
    const page = fakePage(false);
    createClient();
    mount(() => useAlertEvents());
    expect(FakeEventSource.instances).toHaveLength(0);
    page.show(true);
    expect(FakeEventSource.instances).toHaveLength(1);
  });
});

