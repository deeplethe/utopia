// 当前工作区/知识库上下文：均可切换且 localStorage 记忆；工作区无 KB 时自动创建 "General"。
import { useSyncExternalStore } from "react";
import { useParams } from "@tanstack/react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Kb, type Workspace } from "./api";
import { kbStore, wsStore } from "./wsStore";

/** 当前路径里的知识库 id。**页面都在 /kb/$kbId 之下，所以直接从路径取**——
 *  不必等库列表加载完，链接里写的是谁就是谁。不在作用域内（账户页等）时
 *  回落到记忆里的那个。 */
export function useKbId(): string {
  const params = useParams({ strict: false }) as { kbId?: string };
  const { kb } = useKb();
  return params.kbId ?? kb?.id ?? "";
}

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
  /* **URL 里有就以 URL 为准**：两者回答的不是同一个问题——地址栏说的是
     "这个链接指向什么"，localStorage 说的是"我上次在看什么"。
     别人分享的链接必须赢过我自己的记忆，否则打开看到的是我的库、
     数据不同而界面一模一样 */
  const routeParams = useParams({ strict: false }) as { kbId?: string };
  const wantedKbId = routeParams.kbId ?? selectedKbId;
  const kb = kbList.find((k) => k.id === wantedKbId) ?? kbList[0] ?? null;

  return {
    kb,
    kbs: kbList,
    workspace: ws,
    workspaces: list,
    setWorkspace: wsStore.set,
    setKb: kbStore.set,
  };
}
