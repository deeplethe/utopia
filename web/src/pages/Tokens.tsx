/* 个人访问令牌页（docs/decisions/0014，0016 的 A2）。
   令牌属于人、以人的身份行事：有效权限 = 角色 ∩ scope，`kb_ids` 只收窄不授权。
   服务端三个端点早就在（发放 / 列表 / 撤销），缺的只是这一页——没有它，
   MCP 对用户就是「有 API、配不了」。
   明文只在发放那一次的响应里出现，所以这一页的重心是那一刻：把令牌和一段可复制的
   客户端配置一起端出来，人复制完点「完成」，之后列表里只剩前缀。 */
import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, KeyRound } from "lucide-react";
import { api, type TokenView } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { Chip, Loading } from "../ui";
import { toast } from "../toast";

const ymd = (iso: string) => iso.slice(0, 10);
const EXPIRY_CHOICES = [30, 90, 365, 0] as const;

function copyText(text: string) {
  navigator.clipboard
    ?.writeText(text)
    .then(() => toast.success(S.account.copied))
    .catch(() => {});
}

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
    <button
      className={`u-btn u-btn-ghost ${small ? "px-2 py-1 text-[11px]" : "px-3 py-1.5 text-xs"} flex items-center gap-1.5`}
      onClick={() => {
        copyText(text);
        setDone(true);
        setTimeout(() => setDone(false), 1500);
      }}
    >
      {done ? <Check size={12} /> : <Copy size={12} />}
      {done ? S.account.copied : S.account.copy}
    </button>
  );
}

/** 刚发出来的那一枚：明文 + 配置片段，关掉就再也看不到 */
function IssuedPanel({
  token,
  info,
  kbs,
  onDone,
}: {
  token: string;
  info: TokenView;
  kbs: { id: string; name: string }[];
  onDone: () => void;
}) {
  // 片段默认指向令牌限定的第一个库；没限定就取列表第一个
  const candidates = info.kb_ids?.length
    ? kbs.filter((k) => info.kb_ids!.includes(k.id))
    : kbs;
  const [kbId, setKbId] = useState(candidates[0]?.id ?? kbs[0]?.id ?? "");
  const kb = kbs.find((k) => k.id === kbId);
  const snippet = kb ? mcpSnippet(kb.id, kb.name, token) : "";
  return (
    <div className="glass rounded-2xl p-5 mb-4 border border-[var(--u-warn)]/40">
      <div className="text-[13px] font-medium text-neutral-100">{S.account.issuedTitle}</div>
      <p className="mt-1 text-xs text-neutral-500">{S.account.issuedHint}</p>
      <div className="mt-3 flex items-center gap-2">
        <code className="flex-1 min-w-0 truncate rounded-lg bg-black/40 px-3 py-2 font-mono text-[12px] text-neutral-200">
          {token}
        </code>
        <CopyButton text={token} />
      </div>

      <div className="mt-5 flex items-baseline justify-between gap-3 flex-wrap">
        <div className="text-[13px] font-medium text-neutral-200">{S.account.mcpTitle}</div>
        {candidates.length > 1 && (
          <label className="flex items-center gap-2 text-xs text-neutral-500">
            {S.account.mcpBase}
            <select
              className="input-dark px-2 py-1 text-xs"
              value={kbId}
              onChange={(e) => setKbId(e.target.value)}
            >
              {candidates.map((k) => (
                <option key={k.id} value={k.id}>
                  {k.name}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>
      <p className="mt-1 text-xs text-neutral-500">{S.account.mcpHint}</p>
      <div className="mt-2 relative">
        <pre className="rounded-lg bg-black/40 px-3 py-2 font-mono text-[11.5px] text-neutral-300 overflow-x-auto u-scroll">
          {snippet}
        </pre>
        <div className="absolute top-1.5 right-1.5">
          <CopyButton text={snippet} small />
        </div>
      </div>

      <div className="mt-4 flex justify-end">
        <button className="u-btn u-btn-primary px-3.5 py-1.5 text-xs" onClick={onDone}>
          {S.account.tokenDone}
        </button>
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
    <div className={`glass rounded-xl px-4 py-3 ${revoked ? "opacity-55" : ""}`}>
      <div className="flex items-center gap-2 flex-wrap">
        <KeyRound size={13} className="text-neutral-500 shrink-0" />
        <span className="text-sm font-medium text-neutral-100">{t.name}</span>
        <code className="font-mono text-[11px] text-neutral-500">{t.token_prefix}…</code>
        <Chip tone={t.scope === "write" ? "warn" : "neutral"}>
          {t.scope === "write" ? S.account.scopeWrite : S.account.scopeRead}
        </Chip>
        {revoked && <Chip tone="danger">{S.account.revokedOn(ymd(t.revoked_at!))}</Chip>}
        {!revoked && (
          <div className="ml-auto flex gap-2">
            {arm ? (
              <button
                className="u-btn u-btn-ghost px-3 py-1.5 text-xs text-[var(--u-danger)]"
                disabled={busy}
                onClick={onRevoke}
                onBlur={() => setArm(false)}
              >
                {S.account.revokeConfirm}
              </button>
            ) : (
              <button
                className="u-btn u-btn-ghost px-3 py-1.5 text-xs"
                disabled={busy}
                onClick={() => setArm(true)}
              >
                {S.account.revoke}
              </button>
            )}
          </div>
        )}
      </div>
      <div className="mt-1.5 text-[11.5px] text-neutral-500 flex gap-x-3 gap-y-0.5 flex-wrap u-num">
        <span title={t.kb_ids?.length ? bases : undefined}>
          {t.kb_ids?.length ? S.account.nBases(t.kb_ids.length) : S.account.allBases}
        </span>
        <span>·</span>
        <span>
          {t.last_used_at ? S.account.lastUsed(ymd(t.last_used_at)) : S.account.neverUsed}
        </span>
        <span>·</span>
        <span>{t.expires_at ? S.account.expiresOn(ymd(t.expires_at)) : S.account.noExpiry}</span>
        <span>·</span>
        <span>{S.account.createdOn(ymd(t.created_at))}</span>
      </div>
    </div>
  );
}

export function Tokens() {
  const queryClient = useQueryClient();
  const { kbs } = useKb();
  const list = useQuery({ queryKey: ["tokens"], queryFn: api.tokens });

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

  const field = (label: string, node: React.ReactNode, hint?: string) => (
    <div className="mb-4">
      <div className="mb-1 text-[11px] font-medium text-neutral-500">{label}</div>
      {node}
      {hint && <p className="mt-1 text-[11px] text-neutral-600">{hint}</p>}
    </div>
  );

  return (
    <div className="max-w-2xl p-8">
      <h1 className="u-title text-lg">{S.account.tokensTitle}</h1>
      <p className="mt-1 mb-6 text-xs text-neutral-500 max-w-lg">{S.account.tokensHint}</p>

      {issued && (
        <IssuedPanel
          token={issued.token}
          info={issued.info}
          kbs={kbs}
          onDone={() => setIssued(null)}
        />
      )}

      <div className="glass rounded-2xl p-5 mb-6">
        <h2 className="text-[13px] font-medium text-neutral-200 mb-4">{S.account.newToken}</h2>
        {field(
          S.account.tokenName,
          <input
            className="input-dark w-full px-3 py-2 text-sm"
            placeholder={S.account.tokenNamePlaceholder}
            value={name}
            maxLength={64}
            onChange={(e) => setName(e.target.value)}
          />,
        )}
        {field(
          S.account.tokenScope,
          <div className="flex gap-2">
            {(["read", "write"] as const).map((s) => (
              <button
                key={s}
                className={`u-btn px-3 py-1.5 text-xs ${scope === s ? "u-btn-primary" : "u-btn-ghost"}`}
                onClick={() => setScope(s)}
              >
                {s === "read" ? S.account.scopeRead : S.account.scopeWrite}
              </button>
            ))}
          </div>,
          S.account.scopeHint,
        )}
        {field(
          S.account.tokenKbs,
          <div className="flex gap-2 flex-wrap">
            {kbs.map((k) => {
              const on = picked.has(k.id);
              return (
                <button
                  key={k.id}
                  className={`u-btn px-2.5 py-1 text-xs ${on ? "u-btn-primary" : "u-btn-ghost"}`}
                  onClick={() => {
                    const next = new Set(picked);
                    if (on) next.delete(k.id);
                    else next.add(k.id);
                    setPicked(next);
                  }}
                >
                  {k.name}
                </button>
              );
            })}
          </div>,
          S.account.kbsAllHint,
        )}
        {field(
          S.account.tokenExpires,
          <div className="flex gap-2">
            {EXPIRY_CHOICES.map((d) => (
              <button
                key={d}
                className={`u-btn px-2.5 py-1 text-xs u-num ${days === d ? "u-btn-primary" : "u-btn-ghost"}`}
                onClick={() => setDays(d)}
              >
                {d === 0 ? S.account.expiresNever : S.account.expiresDays(d)}
              </button>
            ))}
          </div>,
        )}
        <div className="flex justify-end">
          <button
            className="u-btn u-btn-primary px-3.5 py-1.5 text-xs"
            disabled={!name.trim() || issue.isPending}
            onClick={() => issue.mutate()}
          >
            {S.account.issueToken}
          </button>
        </div>
      </div>

      <h2 className="text-[13px] font-medium text-neutral-200 mb-3">{S.account.yourTokens}</h2>
      {rows.length === 0 ? (
        <div className="glass rounded-xl p-8 text-center text-sm text-neutral-500">
          {S.account.noTokens}
        </div>
      ) : (
        <div className="space-y-2">
          {rows.map((t) => (
            <TokenRow
              key={t.id}
              t={t}
              kbName={kbName}
              busy={revoke.isPending && revoke.variables === t.id}
              onRevoke={() => revoke.mutate(t.id)}
            />
          ))}
        </div>
      )}
    </div>
  );
}
