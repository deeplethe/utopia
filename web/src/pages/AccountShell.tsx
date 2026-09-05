/* 账户层壳：Profile / Administration 的宿主。
   与 KB 无关，所以没有 KB 切换器、没有 tab 导航——只有字标、返回、用户菜单。 */
import { useQuery } from "@tanstack/react-query";
import { Link, Outlet, useNavigate } from "@tanstack/react-router";
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
  GithubMark,
  RAIL_CLS,
  rowClass,
  SectionMark,
} from "../ui";
import { ServerDown } from "./ServerDown";
import { AlertBell } from "./AlertBell";
import { UserMenu } from "./UserMenu";

export function AccountShell() {
  const navigate = useNavigate();
  const me = useQuery({ queryKey: ["me"], queryFn: api.me });
  const health = useQuery({ queryKey: ["health"], queryFn: api.health, staleTime: Infinity });
  // 标题：`Utopia | Persona`——账户区整体一个名字，不逐页细分
  usePageTitle(S.app.name, S.account.titleTag);

  if (me.isPending) {
    return (
      <div className="min-h-screen flex items-center justify-center text-ink-3 text-body">
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
      <header className="glass-strong relative z-40 border-x-0 border-t-0 h-14 shrink-0 flex items-center px-6">
        <SectionMark text={S.account.brand} title={S.docs.backTitle} />
        <div className="ml-auto flex items-center gap-2">
          <Link
            to="/"
            className="u-navlink"
          >
            {S.account.backToApp}
          </Link>
          <a
            href={S.login.githubUrl}
            target="_blank"
            rel="noreferrer"
            title="GitHub"
            className="u-pill"
          >
            <GithubMark size={13} />
            {health.data && <span className="u-num text-fine">v{health.data.version}</span>}
          </a>
          {/* 告警角标跟着人走，不跟着页面走：在账户页里库照样在跑、照样会出事 */}
          <AlertBell />
          <div className="ml-2">
            <UserMenu user={me.data} />
          </div>
        </div>
      </header>

      <div className="flex-1 min-h-0 flex">
        {/* 账户导航栏（仅两项，管理员多一项） */}
        <aside className={`${RAIL_CLS} p-3 space-y-1`}>
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
            <Link to="/admin" className={rail} activeProps={{ className: railActive }}>
              <ShieldCheck size={14} />
              {S.account.administration}
            </Link>
          )}
        </aside>
        <main className="flex-1 min-w-0 overflow-y-auto u-scroll">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
