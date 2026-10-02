// KB 事件流订阅：收到事件只做 react-query 失效重取（事件不带业务数据，天然幂等）。
// EventSource 断线自动重连，重连成功后补刷漏掉的变化；替代 Library/Review 的轮询。
// 告警也从这条流来：一页只占一条通知用的连接（见 `eventStream`）。
//
// **失效是合并着做的。** 一篇文档抽取时每落一条事实就发一个 graph 事件，
// 从前每个事件各失效一次，图谱页在那几秒里把 overview 重取了十几遍——
// 每次都是同一张图，最后一次才算数。所以事件只把 key 记下来，停顿一小会
// 再一次性失效：一阵事件只换来一次重取，最后那次一定包含之前所有的变化。
// 幂等性没变，只是把「每条都刷」变成「刷最后一条」。
//
// 失效不看 staleTime：这里失效的键在 queryDefaults.ts 里放到了 30 秒，靠的就是
// 这一层把当前库的变化即时推到页面上。
import { useEffect } from "react";
import { partialMatchKey, useQueryClient, type QueryKey } from "@tanstack/react-query";
import { subscribeEvents } from "./eventStream";
import { STREAM_KEYS } from "./queryDefaults";

/** 一阵事件之间的静默期。抽取落事实的间隔远小于它，人眼看不出这点延迟 */
export const SETTLE_MS = 300;

/**
 * 把一阵失效合并成一次：`push` 只记键，静默 `settleMs` 后一起交给 `invalidate`，
 * 同一个键及其前缀只失效一次；`flush` 给卸载与重连用，攒着的刷掉而不是丢掉。
 * 抽成纯函数是为了能测；订阅那层只是把事件名映射到键。
 */
export function createInvalidationCoalescer(
  invalidate: (key: QueryKey) => void,
  settleMs: number,
) {
  const pending = new Map<string, QueryKey>();
  let timer: ReturnType<typeof setTimeout> | null = null;
  const flush = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    const keys = [...pending.values()];
    pending.clear();
    for (const key of keys) {
      // 正常 graph 事件可能已排了 ["graph"]，重连又补上 ["graph", kbId]：
      // 前缀已经覆盖后者，不能取消刚开始的请求再重取一遍。
      if (keys.some((prefix) => prefix.length < key.length && partialMatchKey(key, prefix))) continue;
      invalidate(key);
    }
  };
  const push = (...keys: QueryKey[]) => {
    for (const key of keys) pending.set(JSON.stringify(key), key);
    if (timer === null) timer = setTimeout(flush, settleMs);
  };
  return { push, flush };
}

export function useKbEvents(kbId: string | undefined) {
  const queryClient = useQueryClient();

  useEffect(() => {
    if (!kbId) return;
    const { push, flush } = createInvalidationCoalescer(
      (key) => queryClient.invalidateQueries({ queryKey: key }),
      SETTLE_MS,
    );

    const unsubscribe = subscribeEvents(
      `/api/v1/kbs/${kbId}/events`,
      {
        document: () => push(["documents", kbId], ["graph"]),
        graph: () => push(["graph"]),
        // 映射探索跑完发的也是 review：Pending 那一栏得跟着刷新
        review: () => push(["review", kbId], ["mappings", kbId]),
        // 一句记忆抽出了等人点头的事实（0015）：对话里那张确认卡跟着长出来
        pending: () => push(["pending", kbId], ["review", kbId]),
        source: () => push(["sources", kbId], ["documents", kbId]),
        // 告警搭这条流过来（#1028）：打开着一个库的页不再另开告警流
        alert: () => push(["alerts"]),
      },
      () => {
        // 事件没有回放，staleTime 到期也不会自己发请求。断线后恢复必须补刷；
        // 首次正常连接交给页面已有的读取，首次连接失败再恢复则同样可能漏事件。
        push(...STREAM_KEYS.map((head) => [head, kbId]), ["alerts"]);
        flush();
      },
    );
    return () => {
      unsubscribe();
      // 卸载时把攒着的刷掉而不是丢掉：换页回来看到的必须是新数据
      flush();
    };
  }, [kbId, queryClient]);
}
