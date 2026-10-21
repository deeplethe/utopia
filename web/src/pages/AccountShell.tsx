/* 账户层壳：Profile / Administration 的宿主。
   与 KB 无关，所以没有 KB 切换器、没有 tab 导航——只有字标、返回、用户菜单。 */
import { useQuery } from "@tanstack/react-query";
import { Link, Outlet, useLocation, useNavigate } from "@tanstack/react-router";
import { usePageTitle } from "../useTitle";
import {
  KeyRound,
  Layers,
  ShieldCheck,
  UserRound,
} from "lucide-react";
import { api, ApiError } from "../api";
import { S } from "../i18n";
import {
  RAIL_CLS,
  rowClass,
  SectionMark,
} from "../ui";
import { ServerDown } from "./ServerDown";
import { HeaderActions } from "./HeaderActions";
import { ADMIN_TABS } from "./Settings";
import { fetchMe } from "../authSession";

export function AccountShell() {
  const navigate = useNavigate();
  const loc = useLocation();
  const onAdmin = loc.pathname === "/admin";
  const adminTab = (loc.search as { tab?: string }).tab ?? "models";
  const me = useQuery({ queryKey: ["me"], queryFn: fetchMe });
  const health = useQuery({ queryKey: ["health"], queryFn: api.health });
  // 标题：`Utopia | Persona`——账户区整体一个名字，不逐页细分
  usePageTitle(S.app.name, S.account.titleTag);

  if (me.isPending) {
    return (
      <div className="min-h-screen flex items-center justify-center text-ink-2 text-body">
        {S.nav.loading}
      </div>
    );
  }
  if (me.isError) {
    if (me.error instanceof ApiError && me.error.status === 401) {
      navigate({ to: "/login" });
      return null;
    }
    return <ServerDown />;
  }

  const rail = rowClass(false, "nav");
  const railActive = rowClass(true, "nav");

  return (
    <div className="h-screen flex flex-col overflow-hidden u-arrive">
      {/* 顶栏与 Docs 页同构：分区字标（点击回城）+ 返回 + GitHub·版本 + 用户 */}
      {/* px-8 与 App 顶栏同一个内距：右上那一组换页时不该动 */}
      <header className="glass-strong relative z-40 border-x-0 border-t-0 h-12 shrink-0 flex items-center px-2">
        <SectionMark className="pl-3" text={S.account.brand} title={S.docs.backTitle} />
        <HeaderActions
          link={{ to: "/", label: S.account.backToApp }}
          version={health.data?.version}
          user={me.data}
        />
      </header>

      <div className="flex-1 min-h-0 flex">
        {/* 账户导航栏（仅两项，管理员多一项） */}
        <aside className={`${RAIL_CLS} u-rail-list px-2 py-3`}>
          {/* exact：/account 是 /account/kbs 的前缀，默认前缀匹配会双亮 */}
          <Link
            to="/account"
            activeOptions={{ exact: true }}
            className={rail}
            activeProps={{ className: railActive }}
          >
            <UserRound size={14} />
            {S.account.profile}
          </Link>
          <Link to="/account/kbs" className={rail} activeProps={{ className: railActive }}>
            <Layers size={14} />
            {S.account.kbsNav}
          </Link>
          <Link to="/account/tokens" className={rail} activeProps={{ className: railActive }}>
            <KeyRound size={14} />
            {S.account.tokensNav}
          </Link>
          {me.data.is_admin && (
            <>
              {/* 在管理页时父行不再反白：下面已经有一条亮着，两条一起亮反而
                  说不清人在哪儿 */}
              <Link
                to="/admin"
                className={rail}
                activeProps={onAdmin ? {} : { className: railActive }}
              >
                <ShieldCheck size={14} />
                {S.account.administration}
              </Link>
              {onAdmin &&
                ADMIN_TABS.map(({ key, label }) => (
                  <Link
                    key={key}
                    to="/admin"
                    search={{ tab: key }}
                    className={rowClass(adminTab === key, "nav", undefined, true)}
                  >
                    {label()}
                  </Link>
                ))}
            </>
          )}
        </aside>
        <main className="flex-1 min-w-0 overflow-y-auto u-scroll">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
