// 当前工作区/知识库选择：localStorage 持久化 + useSyncExternalStore 订阅。
function makeStore(key: string) {
  let current: string | null = null;
  try {
    current = typeof localStorage !== "undefined" ? localStorage.getItem(key) : null;
  } catch {
    // A denied storage read must not prevent the workspace from opening.
  }
  const listeners = new Set<() => void>();
  return {
    get: () => current,
    set: (id: string) => {
      current = id;
      try {
        localStorage.setItem(key, id);
      } catch {
        // Selection remains usable for this session when persistence is unavailable.
      }
      listeners.forEach((l) => l());
    },
    subscribe: (l: () => void) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
  };
}

export const wsStore = makeStore("utopia.ws");
export const kbStore = makeStore("utopia.kb");
