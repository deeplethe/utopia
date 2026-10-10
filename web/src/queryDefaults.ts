// 服务端数据在缓存里算多久新鲜（#518）。
//
// 从前默认项只有 retry 与 refetchOnWindowFocus，staleTime 是 0：每个查询一落地就算过期，
// 每次导航回到一页都重拉一遍。而新鲜度早已经是推的、不是拉的——`useKbEvents` 订着当前库的
// 事件流，该刷的键它会失效，失效不看 staleTime。于是挂载时那次重拉对流覆盖的键是重复的，
// 对其余键多半是浪费：79 处 useQuery 只有 4 处各自设了 staleTime，`me` 在 12 处查同一个人，
// 六处 `placeholderData: (prev) => prev` 是在下游遮这个重拉带来的闪烁。
//
// 按键头分三档，写在这一处：
//
// - **一次会话内不变的**：me、health、deployment。Infinity。换人不靠过期，靠登录
//   （`Login` 成功时）与登出（`UserMenu`）清空整个缓存——会话过期后同一标签页登另一个账号
//   是唯一会露馅的路，所以登录那一清不能省。deployment 由设置页的 mutation 自己失效。
// - **事件流覆盖的**：documents、graph、review、pending、sources、mappings。当前库即时；
//   可流只订**当前**库，所以不是 Infinity：超过 30 秒后再次挂载会重取，
//   `setKb` 切库时按 `STREAM_KEYS` 主动失效目标库，断线后的 open 也补刷当前库。
//   staleTime 只决定新鲜度，不会自动轮询或触发重取。
// - **其余**：15 秒的全局默认。新鲜时来回导航不重拉；过期后再次挂载会重取。
//   要更短的键（readiness 10 秒）在自己那处写，只有比这里短的才值得单独写。
//
// `setQueryDefaults` 按键前缀匹配：`["graph"]` 管到 `["graph", kbId, …]`。
import type { QueryClient } from "@tanstack/react-query";

/** 其余键的默认新鲜期 */
export const DEFAULT_STALE_MS = 15_000;
/** 事件流覆盖的键：超过这个窗口后再次挂载会重取，切库与重连另有主动失效 */
export const STREAM_STALE_MS = 30_000;

/** 事件流会失效的键头（见 `useKbEvents`）；切库时也按这张表失效目标库 */
export const STREAM_KEYS = [
  "documents",
  "graph",
  "review",
  "pending",
  "sources",
  "mappings",
  "table-alignments",
] as const;
/** 一次会话内不变的键头；换人靠清空缓存 */
export const SESSION_KEYS = ["me", "health", "deployment"] as const;

const STALE_BY_HEAD: ReadonlyMap<string, number> = new Map<string, number>([
  ...SESSION_KEYS.map((head): [string, number] => [head, Infinity]),
  ...STREAM_KEYS.map((head): [string, number] => [head, STREAM_STALE_MS]),
]);

/** 一个键该用的新鲜期。测试用它对表；运行时由 `applyQueryDefaults` 装进 QueryClient */
export function staleTimeFor(queryKey: readonly unknown[]): number {
  const head = queryKey[0];
  if (typeof head !== "string") return DEFAULT_STALE_MS;
  return STALE_BY_HEAD.get(head) ?? DEFAULT_STALE_MS;
}

/** 把上面那张表装进 QueryClient；全局默认在 `main.tsx` 建客户端时给 */
export function applyQueryDefaults(client: QueryClient): void {
  for (const [head, staleTime] of STALE_BY_HEAD) {
    client.setQueryDefaults([head], { staleTime });
  }
}
