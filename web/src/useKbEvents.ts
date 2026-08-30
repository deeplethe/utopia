// KB 事件流订阅：收到事件只做 react-query 失效重取（事件不带业务数据，天然幂等）。
// EventSource 断线自动重连；替代 Library/Review 的轮询。
import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";

export function useKbEvents(kbId: string | undefined) {
  const queryClient = useQueryClient();

  useEffect(() => {
    if (!kbId) return;
    const es = new EventSource(`/api/v1/kbs/${kbId}/events`);
    es.addEventListener("document", () => {
      queryClient.invalidateQueries({ queryKey: ["documents", kbId] });
      queryClient.invalidateQueries({ queryKey: ["graph"] });
    });
    es.addEventListener("graph", () => {
      queryClient.invalidateQueries({ queryKey: ["graph"] });
    });
    es.addEventListener("review", () => {
      queryClient.invalidateQueries({ queryKey: ["review", kbId] });
    });
    es.addEventListener("source", () => {
      queryClient.invalidateQueries({ queryKey: ["sources", kbId] });
      queryClient.invalidateQueries({ queryKey: ["documents", kbId] });
    });
    return () => es.close();
  }, [kbId, queryClient]);
}
