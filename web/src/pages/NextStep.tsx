/* 空状态里的「下一步」（#313）。

   四个页面此前各自假设「已经就绪」：图谱页无条件让人去配模型，哪怕文档正在
   抽取；对话页照常显示问候语，哪怕模型根本没配——用户问出第一句才撞墙。
   这里把「这个库走到哪一步」收成一个判断，四个页面共用。

   **写法：状态一句话，动作在按钮上。** 不解释原理——原理属于文档，而盯着
   空页面的人要的是下一步点哪儿。 */
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { api, type Readiness } from "../api";
import { S } from "../i18n";

export function useReadiness(kbId: string | undefined) {
  return useQuery({
    queryKey: ["readiness", kbId],
    queryFn: () => api.readiness(kbId!),
    enabled: !!kbId,
    // 上传、抽取都会改它，但没必要盯着轮询：页面切回来时重取就够
    staleTime: 10_000,
  });
}

/** 一句话 + 至多一个动作。动作缺省时只剩那句话（权限不够的人不该看到按钮）。 */
export function NextStep({
  line,
  action,
}: {
  line: string;
  action?: { label: string; to: string; params?: Record<string, string>; search?: Record<string, unknown> };
}) {
  return (
    <div className="text-center text-body text-ink-3 max-w-xs">
      {line}
      {action && (
        <Link
          // 路由表是字面量联合类型，这里的目标是运行时算出来的
          to={action.to as never}
          params={action.params as never}
          search={(action.search ?? {}) as never}
          className="u-link mt-3 block"
        >
          {action.label}
        </Link>
      )}
    </div>
  );
}

/** 这个库的下一步是什么。`canUpload` 交给调用方——它知道自己的角色。 */
export function nextStep(
  r: Readiness | undefined,
  opts: { kbId: string; isAdmin: boolean; canUpload: boolean },
): { line: string; action?: { label: string; to: string; params?: Record<string, string>; search?: Record<string, unknown> } } | null {
  if (!r) return null;
  // 顺序即优先级：没有模型，上传了也抽不出东西；没有文档，谈不上处理中
  if (!r.has_chat_model) {
    return opts.isAdmin
      ? {
          line: S.steps.noModel,
          action: {
            label: S.steps.configureModel,
            to: "/admin",
            search: { tab: "models" },
          },
        }
      : { line: S.steps.noModelAsk };
  }
  if (r.documents === 0) {
    return opts.canUpload
      ? {
          line: S.steps.noDocs,
          action: {
            label: S.steps.upload,
            to: "/kb/$kbId/library",
            params: { kbId: opts.kbId },
          },
        }
      : { line: S.steps.noDocs };
  }
  if (r.processing > 0) {
    return {
      line: S.steps.processing(r.processing),
      action: {
        label: S.steps.viewProgress,
        to: "/kb/$kbId/library",
        params: { kbId: opts.kbId },
      },
    };
  }
  // 处理完了却一个失败都没修，值得说一句——否则「什么都没抽出来」会被当成产品坏了
  if (r.failed > 0 && r.entities === 0) {
    return {
      line: S.steps.someFailed(r.failed),
      action: {
        label: S.steps.openLibrary,
        to: "/kb/$kbId/library",
        params: { kbId: opts.kbId },
      },
    };
  }
  return null;
}
