/* 单点登录（0056）：谁的哪个身份提供方 subject 绑定到了哪个账号。
   绑定必须由管理员在这里显式建立——首次通过 SSO 登录绝不会自动开一个新账号,
   见 crates/utopia-server/src/api/oidc_routes.rs 的模块说明。 */
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { S } from "../i18n";
import {
  Button,
  Dialog,
  Field,
  Input,
  LinkButton,
  SearchSelect,
  Table,
  TBody,
  Td,
  Th,
  THead,
  Tr,
} from "../ui";

function LinkIdentityDialog({
  open,
  onOpenChange,
  users,
  onLinked,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  users: { id: string; label: string }[];
  onLinked: () => void;
}) {
  const [userId, setUserId] = useState("");
  const [subject, setSubject] = useState("");
  const link = useMutation({
    mutationFn: () => api.oidcLink(userId, subject.trim()),
    onSuccess: () => {
      setUserId("");
      setSubject("");
      onLinked();
    },
  });
  const ready = !!userId && !!subject.trim();

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={S.settings.sso.linkTitle}
      description={S.settings.sso.linkHint}
      closeLabel={S.ui.close}
      footer={
        <>
          <Button variant="secondary" size="sm" onClick={() => onOpenChange(false)}>
            {S.members.cancel}
          </Button>
          <Button variant="primary" size="sm"
            disabled={!ready || link.isPending}
            onClick={() => link.mutate()}
          >
            {S.settings.sso.link}
          </Button>
        </>
      }
    >
      <div className="space-y-3">
        <Field label={S.settings.sso.pickUser} className="mb-0">
          <SearchSelect
            className="w-full"
            value={userId}
            options={users.map((u) => ({ value: u.id, label: u.label }))}
            onChange={setUserId}
            placeholder={S.settings.sso.pickUser}
          />
        </Field>
        <Field
          label={S.settings.sso.subject}
          hint={S.settings.sso.subjectHint}
          className="mb-0"
        >
          <Input
            className="w-full font-mono"
            value={subject}
            onChange={(e) => setSubject(e.target.value)}
          />
        </Field>
        {link.isError && (
          <p className="text-small text-danger">{(link.error as Error).message}</p>
        )}
      </div>
    </Dialog>
  );
}

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
  const users = useQuery({
    queryKey: ["org-users"],
    queryFn: api.orgUsers,
    enabled: !!status.data?.enabled,
  });
  const [linking, setLinking] = useState(false);
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["oidc-identities"] });
  const unlink = useMutation({
    mutationFn: (userId: string) => api.oidcUnlink(userId),
    onSuccess: invalidate,
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
  // 已绑定的账号不再出现在选人下拉里——一个 subject 只认一个账号（PRIMARY KEY），
  // 但反过来一个账号也只留一条绑定更清楚,免得下拉里同一个人出现好几次
  const linkedIds = new Set(rows.map((r) => r.user_id));
  const linkable = (users.data ?? [])
    .filter((u) => !linkedIds.has(u.id))
    .map((u) => ({ id: u.id, label: `${u.display_name} · ${u.email}` }));

  return (
    <div className="space-y-4">
      <div className="flex items-start gap-4">
        <p className="min-w-0 flex-1 text-small text-ink-2">{S.settings.sso.hint}</p>
        <Button variant="primary" size="sm" className="shrink-0"
          onClick={() => setLinking(true)}
        >
          {S.settings.sso.link}
        </Button>
      </div>

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
                      disabled={unlink.isPending}
                      onClick={() => unlink.mutate(r.user_id)}
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

      <LinkIdentityDialog
        open={linking}
        onOpenChange={setLinking}
        users={linkable}
        onLinked={() => {
          setLinking(false);
          invalidate();
        }}
      />
    </div>
  );
}
