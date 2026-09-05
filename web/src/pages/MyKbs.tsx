/* 账户层"我的知识库"：可访问的库 + 我的角色 + 加入信息 + 概览统计。
   只读全景——建库是管理动作，入口在 System settings › Knowledge bases。 */
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Lock } from "lucide-react";
import { api, type MyKb } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import {
  Button,
  Chip,
  Loading,
} from "../ui";

const ymd = (iso: string) => iso.slice(0, 10);

function joinInfo(row: MyKb): string {
  if (row.my_role === "owner") return S.account.deploymentAdmin;
  if (row.joined_at) {
    return row.added_by_name
      ? S.account.addedBy(row.added_by_name, ymd(row.joined_at))
      : S.account.joinedOn(ymd(row.joined_at));
  }
  return S.account.openToEveryone;
}

export function MyKbs() {
  const navigate = useNavigate();
  const { workspace, setKb } = useKb();

  const mine = useQuery({
    queryKey: ["myKbs", workspace?.id],
    queryFn: () => api.myKbs(workspace!.id),
    enabled: !!workspace,
  });

  if (!workspace || mine.isPending) return <Loading>{S.nav.loading}</Loading>;

  const rows = mine.data?.kbs ?? [];
  const openKb = (id: string) => {
    setKb(id);
    navigate({ to: "/kb/$kbId/graph", params: { kbId: id } });
  };

  return (
    <div className="max-w-2xl p-8">
      <div className="mb-6">
        <h1 className="u-title text-title">{S.account.kbsTitle}</h1>
      </div>

      <div className="space-y-3">
        {rows.map((row) => {
          const canManage = row.my_role === "admin" || row.my_role === "owner";
          return (
            <div key={row.kb.id} className="glass glass-hover rounded-xl px-6 py-4">
              <div className="flex items-start gap-3">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-title font-medium text-ink truncate">
                      {row.kb.name}
                    </span>
                    {row.kb.visibility === "restricted" ? (
                      <span className="flex items-center gap-1 text-fine text-ink-3">
                        <Lock size={10} />
                        {S.account.kbRestricted}
                      </span>
                    ) : (
                      <span className="text-fine text-ink-3">{S.account.kbOpen}</span>
                    )}
                  </div>
                  {row.kb.description && (
                    <p className="mt-1 text-small text-ink-3 truncate">
                      {row.kb.description}
                    </p>
                  )}
                  <p className="mt-2 text-fine text-ink-3">
                    <span className="u-num">{S.account.kbStats(row.doc_count, row.member_count)}</span>
                    <span className="mx-2 text-ink-3">·</span>
                    {joinInfo(row)}
                  </p>
                </div>
                <div className="shrink-0 flex items-center gap-2">
                  {row.my_role && (
                    <Chip tone={canManage ? "info" : "neutral"}>
                      {S.account.roleNames[row.my_role] ?? row.my_role}
                    </Chip>
                  )}
                  <Button variant="secondary" size="sm"
                    onClick={() => openKb(row.kb.id)}
                  >
                    {S.account.openKb}
                  </Button>
                  {canManage && (
                    <Button variant="secondary" size="sm"
                      onClick={() => {
                        setKb(row.kb.id);
                        navigate({ to: "/kb/$kbId/settings", params: { kbId: row.kb.id } });
                      }}
                    >
                      {S.account.kbSettingsBtn}
                    </Button>
                  )}
                </div>
              </div>
            </div>
          );
        })}
      </div>

    </div>
  );
}
