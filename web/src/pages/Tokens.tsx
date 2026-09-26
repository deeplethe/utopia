/* 个人访问令牌页（docs/decisions/0014，0016 的 A2）。
   令牌属于人、以人的身份行事：有效权限 = 角色 ∩ scope，`kb_ids` 只收窄不授权。
   服务端三个端点早就在（发放 / 列表 / 撤销），缺的只是这一页——没有它，
   MCP 对用户就是「有 API、配不了」。
   明文只在发放那一次的响应里出现，所以这一页的重心是那一刻：把令牌和一段可复制的
   客户端配置一起端出来，人复制完点「完成」，之后列表里只剩前缀。 */
import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, KeyRound, Plus } from "lucide-react";
import { api, type TokenView } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import {
  Button,
  Chip,
  Dialog,
  Dropdown,
  Field,
  Input,
  Loading,
  MultiSearchSelect,
  PageHeader,
  RadioGroup,
  Table,
  TBody,
  Td,
  Th,
  THead,
  Tr,
  buttonLike,} from "../ui";
import { toast } from "../toast";
import { copyAndSay } from "../clipboard";

const ymd = (iso: string) => iso.slice(0, 10);
const EXPIRY_CHOICES = [30, 90, 365, 0] as const;

/** Claude Code / Claude Desktop 一族的 Streamable HTTP 写法。每个库一个端点（0014：
 *  令牌限定到库，端点也按库分），所以片段里要把库选出来 */
function mcpSnippet(kbId: string, kbName: string, token: string): string {
  const slug = kbName
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "kb";
  const cfg = {
    mcpServers: {
      [`utopia-${slug}`]: {
        type: "http",
        url: `${window.location.origin}/api/v1/kbs/${kbId}/mcp`,
        headers: { Authorization: `Bearer ${token}` },
      },
    },
  };
  return JSON.stringify(cfg, null, 2);
}

function CopyButton({ text, small }: { text: string; small?: boolean }) {
  const [done, setDone] = useState(false);
  return (
    <Button variant="secondary" size="sm"
      className={buttonLike("ghost", small ? "sm" : "md")}
      onClick={async () => {
        // 复制成了才换成「已复制」。从前先换字再复制，在 http 打开的页面上什么也没
        // 复制，按钮也照样说已复制
        if (!(await copyAndSay(text, S.account.copied))) return;
        setDone(true);
        setTimeout(() => setDone(false), 1500);
      }}
    >
      {done ? <Check size={12} /> : <Copy size={12} />}
      {done ? S.account.copied : S.account.copy}
    </Button>
  );
}

/** 刚发出来的那一枚：明文 + 配置片段，关掉就再也看不到。
    住在发令牌那个弹窗里——发放的下一秒就是"把它抄走"，中间不该有一次关窗 */
function IssuedBody({
  token,
  info,
  kbs,
}: {
  token: string;
  info: TokenView;
  kbs: { id: string; name: string }[];
}) {
  // 片段默认指向令牌限定的第一个库；没限定就取列表第一个
  const candidates = info.kb_ids?.length
    ? kbs.filter((k) => info.kb_ids!.includes(k.id))
    : kbs;
  const [kbId, setKbId] = useState(candidates[0]?.id ?? kbs[0]?.id ?? "");
  const kb = kbs.find((k) => k.id === kbId);
  const snippet = kb ? mcpSnippet(kb.id, kb.name, token) : "";
  return (
    <div>
      <div className="flex items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-cell bg-well px-3 py-2 font-mono text-small text-ink">
          {token}
        </code>
        <CopyButton text={token} />
      </div>

      <div className="mt-6 flex items-baseline justify-between gap-3 flex-wrap">
        <div className="text-body font-medium text-ink">{S.account.mcpTitle}</div>
        {candidates.length > 1 && (
          <span className="flex items-center gap-2 text-small text-ink-2">
            {S.account.mcpBase}
            <Dropdown
              size="sm"
              className="w-40"
              value={kbId}
              onChange={setKbId}
              options={candidates.map((k) => ({ value: k.id, label: k.name }))}
            />
          </span>
        )}
      </div>
      <p className="mt-1 text-small text-ink-2">{S.account.mcpHint}</p>
      <div className="mt-2 relative">
        <pre className="rounded-panel bg-well px-3 py-2 font-mono text-small text-ink-2 overflow-x-auto u-scroll">
          {snippet}
        </pre>
        <div className="absolute top-1.5 right-1.5">
          <CopyButton text={snippet} small />
        </div>
      </div>
    </div>
  );
}

function TokenRow({
  t,
  kbName,
  busy,
  onRevoke,
}: {
  t: TokenView;
  kbName: (id: string) => string;
  busy: boolean;
  onRevoke: () => void;
}) {
  // 撤销不可撤回，但代价只是重发一枚——轻确认（二次点击），不做打字解锁
  const [arm, setArm] = useState(false);
  const revoked = !!t.revoked_at;
  const bases = t.kb_ids?.length
    ? t.kb_ids.map(kbName).join(" · ")
    : S.account.allBases;
  return (
    <Tr className={revoked ? "opacity-55" : undefined}>
      <Td>
        <div className="flex items-center gap-2">
          <KeyRound size={13} className="shrink-0 text-ink-2" />
          <span className="truncate text-body text-ink">{t.name}</span>
          <code className="shrink-0 font-mono text-fine text-ink-2">
            {t.token_prefix}…
          </code>
        </div>
      </Td>
      <Td>
        <Chip tone={t.scope === "write" ? "warn" : "neutral"}>
          {t.scope === "write" ? S.account.scopeWrite : S.account.scopeRead}
        </Chip>
      </Td>
      <Td className="text-small text-ink-2" title={bases}>
        {t.kb_ids?.length ? S.account.nBases(t.kb_ids.length) : S.account.allBases}
      </Td>
      <Td className="u-num text-small text-ink-2">
        {t.last_used_at ? ymd(t.last_used_at) : S.account.neverUsed}
      </Td>
      <Td className="u-num text-small text-ink-2">
        {t.expires_at ? ymd(t.expires_at) : S.account.noExpiry}
      </Td>
      <Td className="u-num text-small text-ink-2">{ymd(t.created_at)}</Td>
      <Td className="text-right">
        {revoked ? (
          <Chip tone="danger">{S.account.revokedOn(ymd(t.revoked_at!))}</Chip>
        ) : arm ? (
          <Button variant="danger" size="sm"
            disabled={busy}
            onClick={onRevoke}
            onBlur={() => setArm(false)}
          >
            {S.account.revokeConfirm}
          </Button>
        ) : (
          <Button variant="secondary" size="sm"
            disabled={busy}
            onClick={() => setArm(true)}
          >
            {S.account.revoke}
          </Button>
        )}
      </Td>
    </Tr>
  );
}

export function Tokens() {
  const queryClient = useQueryClient();
  const { kbs } = useKb();
  const list = useQuery({ queryKey: ["tokens"], queryFn: api.tokens });

  // 发令牌是个动作，不是这一页的常驻内容：表单住在弹窗里，页面上只有名单
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [scope, setScope] = useState<"read" | "write">("read");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [days, setDays] = useState<number>(90);
  const [issued, setIssued] = useState<{ token: string; info: TokenView } | null>(null);

  const issue = useMutation({
    mutationFn: () =>
      api.issueToken({
        name: name.trim(),
        scope,
        kb_ids: picked.size ? Array.from(picked) : null,
        expires_in_days: days,
      }),
    onSuccess: (res) => {
      // 同一个窗口换一副内容：发完立刻是"把它抄走"，明文只有这一次
      setIssued(res);
      setName("");
      setPicked(new Set());
      queryClient.invalidateQueries({ queryKey: ["tokens"] });
    },
    onError: (e) => toast.error((e as Error).message),
  });
  const revoke = useMutation({
    mutationFn: (id: string) => api.revokeToken(id),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["tokens"] }),
    onError: (e) => toast.error((e as Error).message),
  });

  const kbName = useMemo(() => {
    const m = new Map(kbs.map((k) => [k.id, k.name]));
    return (id: string) => m.get(id) ?? id.slice(0, 8);
  }, [kbs]);

  if (list.isPending) return <Loading>{S.nav.loading}</Loading>;
  // 活的在前、按新到旧；撤销过的沉底但仍然列着——撤过这件事本身要看得见
  const rows = [...(list.data?.tokens ?? [])].sort((a, b) => {
    if (!!a.revoked_at !== !!b.revoked_at) return a.revoked_at ? 1 : -1;
    return b.created_at.localeCompare(a.created_at);
  });

  const close = () => {
    setCreating(false);
    setIssued(null);
  };

  return (
    <div className="px-8 py-6">
      {/* 与管理页同一副身材：内容居中限宽，行长不随窗口拉长 */}
      <div className="mx-auto w-full max-w-4xl">
        <PageHeader
          title={S.account.tokensTitle}
          sub={S.account.tokensHint}
          actions={
            <Button variant="primary" size="sm" onClick={() => setCreating(true)}>
              <Plus size={12} />
              {S.account.newToken}
            </Button>
          }
        />

        {rows.length === 0 ? (
          <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
            {S.account.noTokens}
          </div>
        ) : (
          <div className="glass rounded-panel overflow-hidden">
            <Table>
              <THead>
                <Tr>
                  <Th>{S.account.tokenName}</Th>
                  <Th>{S.account.tokenScope}</Th>
                  <Th>{S.account.tokenKbs}</Th>
                  <Th>{S.account.colLastUsed}</Th>
                  <Th>{S.account.tokenExpires}</Th>
                  <Th>{S.account.colCreated}</Th>
                  <Th />
                </Tr>
              </THead>
              <TBody>
                {rows.map((t) => (
                  <TokenRow
                    key={t.id}
                    t={t}
                    kbName={kbName}
                    busy={revoke.isPending && revoke.variables === t.id}
                    onRevoke={() => revoke.mutate(t.id)}
                  />
                ))}
              </TBody>
            </Table>
          </div>
        )}

        {/* 一个窗口两副内容：先是表单，发出来之后是那一枚明文。中间不关窗——
            关掉就再也看不到它了 */}
        <Dialog
          open={creating || !!issued}
          onOpenChange={(o) => !o && close()}
          width={issued ? "lg" : "md"}
          closeLabel={S.ui.close}
          title={issued ? S.account.issuedTitle : S.account.newToken}
          description={issued ? S.account.issuedHint : undefined}
          footer={
            issued ? (
              <Button variant="primary" size="sm" onClick={close}>
                {S.account.tokenDone}
              </Button>
            ) : (
              <>
                <Button variant="secondary" size="sm" onClick={close}>
                  {S.account.cancel}
                </Button>
                <Button variant="primary" size="sm"
                  disabled={!name.trim() || issue.isPending}
                  onClick={() => issue.mutate()}
                >
                  {S.account.issueToken}
                </Button>
              </>
            )
          }
        >
          {issued ? (
            <IssuedBody token={issued.token} info={issued.info} kbs={kbs} />
          ) : (
            <div>
              <Field label={S.account.tokenName}>
                <Input className="w-full"
                  autoFocus
                  placeholder={S.account.tokenNamePlaceholder}
                  value={name}
                  maxLength={64}
                  onChange={(e) => setName(e.target.value)}
                />
              </Field>
              {/* 范围与有效期并排：两个都是一眼扫过去就定下来的小选择，
                  各占一整行只是把这张表拉长 */}
              <div className="grid grid-cols-2 gap-4">
                {/* 二选一，且两个选项都要读得到——单选按钮，不是一排按钮 */}
                <Field label={S.account.tokenScope} hint={S.account.scopeHint}>
                  <RadioGroup
                    name="token-scope"
                    className="flex h-8 flex-row items-center gap-4"
                    value={scope}
                    onChange={(v) => setScope(v)}
                    options={[
                      { value: "read" as const, label: S.account.scopeRead },
                      { value: "write" as const, label: S.account.scopeWrite },
                    ]}
                  />
                </Field>
                <Field label={S.account.tokenExpires}>
                  <Dropdown
                    className="w-full"
                    value={String(days)}
                    onChange={(v) => setDays(Number(v))}
                    options={EXPIRY_CHOICES.map((d) => ({
                      value: String(d),
                      label: d === 0 ? S.account.expiresNever : S.account.expiresDays(d),
                    }))}
                  />
                </Field>
              </div>
              {/* 库可以很多：搜着选，选中的堆在框上面。芯片墙的高度随库数长，
                  这个不随。**不再另写一句「不选就是全部」**——控件自己就写着
                  「All bases」，同一件事说两遍只是把表单拉长 */}
              <Field label={S.account.tokenKbs} className="mb-0">
                <MultiSearchSelect
                  className="w-full"
                  values={Array.from(picked)}
                  options={kbs.map((k) => ({ value: k.id, label: k.name }))}
                  placeholder={S.account.pickBases}
                  emptyHint={S.account.allBases}
                  onToggle={(id) => {
                    const next = new Set(picked);
                    if (next.has(id)) next.delete(id);
                    else next.add(id);
                    setPicked(next);
                  }}
                />
              </Field>
            </div>
          )}
        </Dialog>
      </div>
    </div>
  );
}
