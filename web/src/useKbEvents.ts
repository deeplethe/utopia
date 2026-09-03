// KB 事件流订阅：收到事件只做 react-query 失效重取（事件不带业务数据，天然幂等）。
// EventSource 断线自动重连；替代 Library/Review 的轮询。
//
// **失效是合并着做的。** 一篇文档抽取时每落一条事实就发一个 graph 事件，
// 从前每个事件各失效一次，图谱页在那几秒里把 overview 重取了十几遍——
// 每次都是同一张图，最后一次才算数。所以事件只把 key 记下来，停顿一小会
// 再一次性失效：一阵事件只换来一次重取，最后那次一定包含之前所有的变化。
// 幂等性没变，只是把「每条都刷」变成「刷最后一条」。
import { useEffect } from "react";
import { useQueryClient, type QueryKey } from "@tanstack/react-query";

/** 一阵事件之间的静默期。抽取落事实的间隔远小于它，人眼看不出这点延迟 */
const SETTLE_MS = 300;

export function useKbEvents(kbId: string | undefined) {
  const queryClient = useQueryClient();

  useEffect(() => {
    if (!kbId) return;
    const pending = new Map<string, QueryKey>();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const flush = () => {
      timer = null;
      const keys = [...pending.values()];
      pending.clear();
      for (const key of keys) queryClient.invalidateQueries({ queryKey: key });
    };
    const invalidate = (...keys: QueryKey[]) => {
      for (const key of keys) pending.set(JSON.stringify(key), key);
      if (timer === null) timer = setTimeout(flush, SETTLE_MS);
    };

    const es = new EventSource(`/api/v1/kbs/${kbId}/events`);
    es.addEventListener("document", () => invalidate(["documents", kbId], ["graph"]));
    es.addEventListener("graph", () => invalidate(["graph"]));
    // 映射探索跑完发的也是 review：Pending 那一栏得跟着刷新
    es.addEventListener("review", () => invalidate(["review", kbId], ["mappings", kbId]));
    // 一句记忆抽出了等人点头的事实（0015）：对话里那张确认卡跟着长出来
    es.addEventListener("pending", () => invalidate(["pending", kbId], ["review", kbId]));
    es.addEventListener("source", () => invalidate(["sources", kbId], ["documents", kbId]));
    return () => {
      es.close();
      // 卸载时把攒着的刷掉而不是丢掉：换页回来看到的必须是新数据
      if (timer !== null) {
        clearTimeout(timer);
        flush();
      }
    };
  }, [kbId, queryClient]);
}
