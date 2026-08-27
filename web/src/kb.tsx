// 当前工作区/知识库上下文：均可切换且 localStorage 记忆；工作区无 KB 时自动创建 "General"。
import { useSyncExternalStore } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Kb, type Workspace } from "./api";
import { kbStore, wsStore } from "./wsStore";

export function useKb(): {
  kb: Kb | null;
  kbs: Kb[];
  workspace: Workspace | null;
  workspaces: Workspace[];
  setWorkspace: (id: string) => void;
  setKb: (id: string) => void;
} {
  const queryClient = useQueryClient();
  const selectedId = useSyncExternalStore(wsStore.subscribe, wsStore.get);
  const selectedKbId = useSyncExternalStore(kbStore.subscribe, kbStore.get);

  const workspaces = useQuery({ queryKey: ["workspaces"], queryFn: api.workspaces });
  const list = workspaces.data ?? [];
  const ws = list.find((w) => w.id === selectedId) ?? list[0] ?? null;

  const kbs = useQuery({
    queryKey: ["kbs", ws?.id],
    queryFn: async () => {
      const existing = await api.kbs(ws!.id);
      if (existing.length > 0) return existing;
      // 空工作区自动创建 General——建库现在是管理员动作，非管理员会 403：
      // 静默等管理员来创建（现实中首个用户即管理员，General 总在）
      try {
        const created = await api.createKb(ws!.id, { name: "General" });
        queryClient.invalidateQueries({ queryKey: ["kbs", ws!.id] });
        return [created];
      } catch {
        return [];
      }
    },
    enabled: !!ws,
  });

  const kbList = kbs.data ?? [];
  const kb = kbList.find((k) => k.id === selectedKbId) ?? kbList[0] ?? null;

  return {
    kb,
    kbs: kbList,
    workspace: ws,
    workspaces: list,
    setWorkspace: wsStore.set,
    setKb: kbStore.set,
  };
}
