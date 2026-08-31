// KB 作用域层：`/kb/$kbId` 之下的一切都属于这个知识库。
//
// **为什么把库放进路径而不是查询参数**：库是容器不是筛选条件——图谱、搜索、
// 文库、本体、复核全都在它下面。路径能表达这种包含关系，而且路由器会替你兜底：
// `/kb/$kbId/search` 没有 id 根本构造不出来。`?kb=` 是可选的，标签页之间跳一下
// 就掉了，掉了还不报错——于是悄悄看的是另一个库的数据，界面一模一样。
//
// **为什么还要 localStorage**：两者回答的不是同一个问题。URL 回答"这个链接指向
// 什么"，localStorage 回答"我上次在看什么"。所以 URL 里有就以 URL 为准，
// 没有才回落到记忆（见 kb.tsx 与下面的 KbRedirect）。
import { useEffect } from "react";
import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";

import { ApiError, api } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { kbStore, wsStore } from "../wsStore";

/** 库不存在或没权限时的落地页。**不能只给一张空图**——分享链接最常见的
 *  失败就是对方没权限，而空白的图谱看起来像"这个库是空的"，是另一回事。 */
function KbNoAccess({ status }: { status: number }) {
  return (
    <div className="grid h-full place-items-center px-6">
      <div className="max-w-sm text-center">
        <h2 className="text-[15px] text-neutral-200">
          {status === 404 ? S.kbScope.missingTitle : S.kbScope.deniedTitle}
        </h2>
        <p className="mt-2 text-xs leading-relaxed text-neutral-500">
          {status === 404 ? S.kbScope.missingBody : S.kbScope.deniedBody}
        </p>
        <Link
          to="/account/kbs"
          className="u-btn u-btn-ghost mt-4 inline-block px-3 py-1.5 text-xs"
        >
          {S.kbScope.myKbs}
        </Link>
      </div>
    </div>
  );
}

export function KbScope() {
  const { kbId } = useParams({ from: "/app/kb/$kbId" });
  // **直接问后端，而不是在当前工作区的列表里找**：链接可能指向另一个工作区
  // 里的库，那时列表里没有它，但用户其实有权限——照列表判断会错杀
  const kb = useQuery({
    queryKey: ["kbOne", kbId],
    queryFn: () => api.kbDetail(kbId),
    retry: false,
  });

  // 打开哪个库，"上次看的"就跟到哪个库；工作区也一并对齐，
  // 否则顶栏的切换器显示的还是上一个工作区
  useEffect(() => {
    if (!kb.data) return;
    kbStore.set(kb.data.id);
    wsStore.set(kb.data.workspace_id);
  }, [kb.data]);

  if (kb.isError) {
    const status = kb.error instanceof ApiError ? kb.error.status : 500;
    return <KbNoAccess status={status} />;
  }
  // 加载中什么都不画：这一层只是个作用域，闪一个 spinner 反而像页面在跳
  if (!kb.data) return null;
  return <Outlet />;
}

/** 旧路径（`/graph` 这类不带库的地址）的接管：解析出该去哪个库再跳过去。
 *
 *  **不能在 beforeLoad 里直接重定向**——那时候库列表还没取回来，
 *  localStorage 里也可能什么都没有（新设备、清过缓存）。所以做成一个组件，
 *  等 useKb 把库解析出来再走。 */
export function KbRedirect({
  page,
}: {
  page:
    | "graph"
    | "search"
    | "chat"
    | "library"
    | "ontology"
    | "mappings"
    | "review"
    | "settings";
}) {
  const { kb } = useKb();
  const navigate = useNavigate();
  useEffect(() => {
    if (!kb) return;
    navigate({
      to: `/kb/$kbId/${page}`,
      params: { kbId: kb.id },
      replace: true,
    });
  }, [kb, page, navigate]);
  return null;
}
