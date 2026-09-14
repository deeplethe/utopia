/* 单点登录（0056）。

   **绑定由本人完成**：账户页上「绑定身份」跳到身份提供方，回来时服务端核对还是那个
   已登录的人才写绑定。管理页只列出谁绑了哪个 subject、能解绑——没有替别人绑定的入口，
   那是一条冒充任何人登录的路（见 crates/utopia-server/src/api/oidc_routes.rs 的模块说明）。 */
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { S } from "../i18n";
import { toast } from "../toast";
import {
  Button,
  DangerConfirm,
  LinkButton,
  Table,
  TBody,
  Td,
  Th,
  THead,
  Tr,
} from "../ui";

/** 三行信息展示，只读——issuer/client_id/redirect_uri 由环境变量定，这里不能改。
 *  两列网格而不是一句话拼接：三个值长短不一，右列各自 truncate 更好读 */
function ConfigSummary({
  issuer,
  clientId,
  redirectUri,
}: {
  issuer: string;
  clientId: string;
  redirectUri: string;
}) {
  const D = S.settings.sso;
  return (
    <div className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-1 text-fine text-ink-2">
      <span>{D.issuer}</span>
      <span className="truncate font-mono" title={issuer}>
        {issuer}
      </span>
      <span>{D.clientId}</span>
      <span className="truncate font-mono" title={clientId}>
        {clientId}
      </span>
      <span>{D.redirectUri}</span>
      <span className="truncate font-mono" title={redirectUri}>
        {redirectUri}
      </span>
    </div>
  );
}

export function SsoAdmin() {
  const queryClient = useQueryClient();
  const status = useQuery({ queryKey: ["oidc-status"], queryFn: api.oidcStatus });
  const identities = useQuery({
    queryKey: ["oidc-identities"],
    queryFn: api.oidcIdentities,
    enabled: !!status.data?.enabled,
  });
  const [confirming, setConfirming] = useState<{ userId: string; email: string } | null>(
    null,
  );
  const unlink = useMutation({
    mutationFn: (userId: string) => api.oidcUnlink(userId),
    onSuccess: () => {
      setConfirming(null);
      queryClient.invalidateQueries({ queryKey: ["oidc-identities"] });
    },
    onError: (e) => toast.error((e as Error).message),
  });

  if (status.isPending) return null;

  if (!status.data?.enabled) {
    return (
      <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
        {S.settings.sso.disabled}
      </div>
    );
  }

  const rows = identities.data?.identities ?? [];

  return (
    <div className="space-y-4">
      <p className="text-small text-ink-2">{S.settings.sso.hint}</p>

      {identities.data && (
        <ConfigSummary
          issuer={identities.data.issuer}
          clientId={identities.data.client_id}
          redirectUri={identities.data.redirect_uri}
        />
      )}

      {rows.length === 0 ? (
        <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
          {S.settings.sso.empty}
        </div>
      ) : (
        <div className="glass rounded-panel overflow-hidden">
          <Table>
            <THead>
              <Tr>
                <Th>{S.settings.sso.colUser}</Th>
                <Th>{S.settings.sso.colSubject}</Th>
                <Th />
              </Tr>
            </THead>
            <TBody>
              {rows.map((r) => (
                <Tr key={r.user_id}>
                  <Td className="text-ink">{r.email}</Td>
                  <Td
                    className="max-w-xs truncate font-mono text-small text-ink-2"
                    title={r.subject}
                  >
                    {r.subject}
                  </Td>
                  <Td className="whitespace-nowrap text-right">
                    <LinkButton
                      tone="danger"
                      onClick={() => setConfirming({ userId: r.user_id, email: r.email })}
                    >
                      {S.settings.sso.unlink}
                    </LinkButton>
                  </Td>
                </Tr>
              ))}
            </TBody>
          </Table>
        </div>
      )}

      {confirming && (
        <DangerConfirm
          title={S.settings.sso.unlinkTitle(confirming.email)}
          hint={S.settings.sso.unlinkHint}
          confirmLabel={S.settings.sso.unlink}
          cancelLabel={S.members.cancel}
          busy={unlink.isPending}
          onConfirm={() => unlink.mutate(confirming.userId)}
          onCancel={() => setConfirming(null)}
        />
      )}
    </div>
  );
}

/** 账户页上的一节：我自己的绑定。没配单点登录的部署上整节不出现 */
export function SsoAccount() {
  const queryClient = useQueryClient();
  const me = useQuery({ queryKey: ["oidc-me"], queryFn: api.oidcMe });
  const [confirming, setConfirming] = useState(false);
  const unlink = useMutation({
    mutationFn: api.oidcUnlinkMe,
    onSuccess: () => {
      setConfirming(false);
      queryClient.invalidateQueries({ queryKey: ["oidc-me"] });
      toast.success(S.account.sso.unlinked);
    },
    onError: (e) => toast.error((e as Error).message),
  });

  // 从身份提供方回来时带着结果（`?sso=linked` 或 `?sso_error=`）：提示一次，再把地址洗干净，
  // 免得刷新一次又提示一次
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const linked = params.get("sso") === "linked";
    const error = params.get("sso_error");
    if (!linked && !error) return;
    if (linked) toast.success(S.account.sso.linked);
    if (error) toast.error(S.login.ssoErrors[error] ?? S.login.ssoErrorOther);
    params.delete("sso");
    params.delete("sso_error");
    const rest = params.toString();
    window.history.replaceState(null, "", window.location.pathname + (rest ? `?${rest}` : ""));
  }, []);

  if (!me.data?.enabled) return null;

  return (
    <div className="glass rounded-panel p-6 mt-4">
      <div className="max-w-xl">
        <h2 className="text-body font-medium text-ink mb-2">{S.account.sso.title}</h2>
        <p className="text-small text-ink-2 mb-4">{S.account.sso.hint}</p>
        <div className="flex items-center gap-3">
          <span className="min-w-0 flex-1 truncate text-body text-ink-2">
            {me.data.linked && me.data.subject
              ? S.account.sso.linkedAs(me.data.subject)
              : S.account.sso.notLinked}
          </span>
          {me.data.linked ? (
            <Button variant="secondary" size="sm" onClick={() => setConfirming(true)}>
              {S.account.sso.unlink}
            </Button>
          ) : (
            <Button variant="primary" size="sm"
              onClick={() => {
                window.location.href = "/api/v1/auth/oidc/start?link=1";
              }}
            >
              {S.account.sso.link}
            </Button>
          )}
        </div>
      </div>
      {confirming && (
        <DangerConfirm
          title={S.account.sso.unlinkTitle}
          hint={S.account.sso.unlinkHint}
          confirmLabel={S.account.sso.unlink}
          cancelLabel={S.account.cancel}
          busy={unlink.isPending}
          onConfirm={() => unlink.mutate()}
          onCancel={() => setConfirming(false)}
        />
      )}
    </div>
  );
}
