// 当前工作区/知识库上下文：均可切换且 localStorage 记忆；工作区无 KB 时自动创建 "General"。
import { useCallback, useSyncExternalStore } from "react";
import { useLocation, useNavigate, useParams } from "@tanstack/react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, DEFAULT_ONTOLOGY_PACKS, type Kb, type Workspace } from "./api";
import { kbStore, wsStore } from "./wsStore";

/** 当前路径里的知识库 id。**页面都在 /kb/$kbId 之下，所以直接从路径取**——
 *  不必等库列表加载完，链接里写的是谁就是谁。不在作用域内（账户页等）时
 *  回落到记忆里的那个。 */
export function useKbId(): string {
  const params = useParams({ strict: false }) as { kbId?: string };
  const { kb } = useKb();
  return params.kbId ?? kb?.id ?? "";
}

/** 换库落在同一个页面上，但**不带走页面里的东西**。
 *
 *  从前是把路径里的 kbId 整个替换掉，于是 `/kb/A/chat/某会话` 变成
 *  `/kb/B/chat/某会话`——会话属于 A，页面拿着它去问 B，得到一个 404 再退回
 *  新对话。单槽的 liveAnswer 曾经把这一步掩住（它不问会话属于谁就认领），
 *  按库键控之后（#259）认领正确地失败，404 就露了出来（#261）。
 *
 *  所以只保留 kbId 后面的第一段：chat、graph、library……深一层的会话 id、
 *  文档 id 都是那个库里的东西，换库就该丢掉。文档页本身就是某一篇文档，
 *  换库后落到新库的 Library。不在 /kb 作用域下时保持原行为。 */
export function samePageInKb(pathname: string, fromKbId: string, toKbId: string): string {
  const prefix = `/kb/${fromKbId}`;
  if (!pathname.startsWith(prefix)) return pathname.replace(fromKbId, toKbId);
  const section = pathname.slice(prefix.length).split("/").filter(Boolean)[0] ?? "graph";
  return `/kb/${toKbId}/${section === "doc" ? "library" : section}`;
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
      // 静默等管理员来创建（现实中首个用户即管理员，General 总在）。
      // **带默认本体包。** 从前这里不传 packs，于是一个新部署的第一个库一个类
      // 都没有，第一批文档抽出来全是未分类实体；建库对话框里那个"默认 schema.org"
      // 只对第二个库起效。第一个库恰恰是大多数人唯一会用的那个
      try {
        const created = await api.createKb(ws!.id, {
          name: "General",
          ontology_packs: DEFAULT_ONTOLOGY_PACKS,
        });
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

  /* **换库是一次导航，不只是记一笔。**
     上面那条"URL 优先"是对的，代价是：作用域内每一页的地址里都写着 kbId，
     于是 `selectedKbId` 永远轮不到。只写 store 的话，值变了、组件也重渲染了，
     算出来的还是同一个库——顶栏那个下拉因此在 `/kb/$kbId/*` 下**整个是死的**，
     点了没反应，刷新之后才生效（首页重定向读的是记忆）。

     所以把导航并进 `setKb` 本身，而不是要求每个调用点记得配一次 `navigate`——
     漏掉的正是那两处（顶栏下拉、Chat 的范围切换器），而写对的三处都是
     "跳去某个具体页面"顺带把库带上的。忘得掉的约定就是会被忘掉的约定。

     停在当前这一页：在本体页换库，该看到另一个库的本体，而不是被送回图谱。
     地址里没有 kbId 时（账户页等）只记一笔——那里本来就不该被拽走，
     调用方自己决定跳哪去。 */
  const navigate = useNavigate();
  const pathname = useLocation({ select: (l) => l.pathname });
  const currentKbId = routeParams.kbId;
  const setKb = useCallback(
    (id: string) => {
      kbStore.set(id);
      if (currentKbId && currentKbId !== id) {
        navigate({ to: samePageInKb(pathname, currentKbId, id), replace: false });
      }
    },
    [navigate, pathname, currentKbId],
  );

  return {
    kb,
    kbs: kbList,
    workspace: ws,
    workspaces: list,
    setWorkspace: wsStore.set,
    setKb,
  };
}
