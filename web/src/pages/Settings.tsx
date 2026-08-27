import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { Lock, Plus, X } from "lucide-react";
import { api } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { Members } from "./Members";

/** 部署级配置：注册开关 + worker 并发。 */
function DeploymentAdmin() {
  const queryClient = useQueryClient();
  const dep = useQuery({ queryKey: ["deployment"], queryFn: api.adminDeployment });
  const [workers, setWorkers] = useState<number | null>(null);
  const shown = workers ?? dep.data?.worker_concurrency ?? 4;
  const save = useMutation({
    mutationFn: (v: { open: boolean; workers?: number }) =>
      api.saveAdminDeployment(v.open, v.workers),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["deployment"] }),
  });
  const open = dep.data?.open_registration ?? true;

  return (
    <div className="glass rounded-xl p-4 space-y-4">
      <label className="flex items-start gap-3 cursor-pointer">
        <input
          type="checkbox"
          className="mt-0.5"
          checked={open}
          disabled={dep.isPending || save.isPending}
          onChange={(e) => save.mutate({ open: e.target.checked })}
        />
        <span>
          <span className="block text-sm text-neutral-200">{S.settings.deployment.openReg}</span>
          <span className="block text-xs text-neutral-500 mt-0.5">
            {S.settings.deployment.openRegHint}
          </span>
        </span>
      </label>

      <div className="flex items-start justify-between gap-4 border-t border-white/10 pt-4">
        <div className="min-w-0">
          <span className="block text-sm text-neutral-200">{S.settings.deployment.workers}</span>
          <span className="block text-xs text-neutral-500 mt-0.5">
            {S.settings.deployment.workersHint}
          </span>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <input
            type="number"
            min={1}
            max={32}
            className="input-dark u-input-plain w-16 px-2 py-1.5 text-sm u-num text-center"
            value={shown}
            disabled={dep.isPending}
            onChange={(e) => setWorkers(Math.max(1, Math.min(32, Number(e.target.value) || 1)))}
          />
          <button
            className="u-btn u-btn-ghost px-3 py-1.5 text-xs"
            disabled={save.isPending || workers === null || workers === dep.data?.worker_concurrency}
            onClick={() => save.mutate({ open, workers: shown })}
          >
            {S.settings.deployment.workersApply}
          </button>
        </div>
      </div>
      {save.isError && (
        <p className="text-xs text-rose-400">{(save.error as Error).message}</p>
      )}
    </div>
  );
}

/** 知识库管理（部署层）：全部库总览 + 新建（建库是管理动作，切换器只切换）。 */
function KbsAdmin() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { workspace, setKb } = useKb();
  const [creating, setCreating] = useState(false);
  const list = useQuery({
    queryKey: ["myKbs", workspace?.id],
    queryFn: () => api.myKbs(workspace!.id),
    enabled: !!workspace,
  });
  const rows = list.data?.kbs ?? [];

  return (
    <div className="space-y-4">
      <p className="text-xs text-neutral-500">{S.settings.kbs.hint}</p>

      <div className="glass rounded-xl divide-y divide-white/5">
        {rows.map(({ kb, doc_count, member_count }) => (
          <div key={kb.id} className="px-4 py-3 flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="text-sm text-neutral-200 truncate">{kb.name}</span>
                {kb.is_default && (
                  <span className="u-chip u-chip-neutral !text-[10px]">
                    {S.settings.kbs.defaultChip}
                  </span>
                )}
                {kb.visibility === "restricted" && (
                  <span className="flex items-center gap-1 text-[10.5px] text-neutral-500">
                    <Lock size={10} />
                    {S.settings.kbs.visRestricted}
                  </span>
                )}
              </div>
              <div className="mt-0.5 text-xs text-neutral-500">
                <span className="u-num">{S.account.kbStats(doc_count, member_count)}</span>
              </div>
            </div>
            <button
              className="u-btn u-btn-ghost px-2.5 py-1 text-xs shrink-0"
              onClick={() => {
                setKb(kb.id);
                navigate({ to: "/kb-settings", search: { kb: kb.id } });
              }}
            >
              {S.settings.kbs.openSettings}
            </button>
          </div>
        ))}
        {!list.isPending && rows.length === 0 && (
          <p className="px-4 py-6 text-sm text-neutral-500">—</p>
        )}
      </div>

      <button
        onClick={() => setCreating(true)}
        className="u-btn u-btn-primary px-3.5 py-1.5 text-xs flex items-center gap-1.5"
      >
        <Plus size={12} />
        {S.settings.kbs.newKb}
      </button>

      {creating && workspace && (
        <NewKbModal
          workspaceId={workspace.id}
          onDone={(id) => {
            setCreating(false);
            queryClient.invalidateQueries({ queryKey: ["myKbs", workspace.id] });
            queryClient.invalidateQueries({ queryKey: ["kbs", workspace.id] });
            // 建完直达库设置：下一步几乎总是邀人/配置
            if (id) {
              setKb(id);
              navigate({ to: "/kb-settings", search: { kb: id } });
            }
          }}
        />
      )}
    </div>
  );
}

/** 新建知识库弹窗（管理员）：缺省 restricted，不污染全员切换器。 */
function NewKbModal({
  workspaceId,
  onDone,
}: {
  workspaceId: string;
  onDone: (id?: string) => void;
}) {
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [restricted, setRestricted] = useState(true);

  const create = useMutation({
    mutationFn: () =>
      api.createKb(workspaceId, {
        name: name.trim(),
        description: desc.trim() || null,
        visibility: restricted ? "restricted" : "open",
      }),
    onSuccess: (kb) => onDone(kb.id),
  });

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/60 backdrop-blur-sm"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onDone();
      }}
    >
      <div className="glass-strong w-[24rem] max-w-[calc(100vw-2rem)] rounded-2xl shadow-2xl p-5">
        <div className="flex items-center justify-between mb-4">
          <h2 className="u-title text-[15px]">{S.settings.kbs.newKb}</h2>
          <button onClick={() => onDone()} className="text-neutral-500 hover:text-neutral-200">
            <X size={15} />
          </button>
        </div>
        <input
          autoFocus
          className="input-dark w-full px-3 py-2 text-sm mb-2"
          placeholder={S.settings.kbs.name}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="input-dark w-full px-3 py-2 text-sm mb-3"
          placeholder={S.settings.kbs.description}
          value={desc}
          onChange={(e) => setDesc(e.target.value)}
        />
        <label className="flex items-center gap-2 text-xs text-neutral-400 mb-4">
          <input
            type="checkbox"
            checked={restricted}
            onChange={(e) => setRestricted(e.target.checked)}
          />
          {S.settings.kbs.visRestricted}
        </label>
        {create.isError && (
          <p className="text-xs text-rose-400 mb-2">{(create.error as Error).message}</p>
        )}
        <div className="flex justify-end gap-2">
          <button className="u-btn u-btn-ghost px-3.5 py-1.5 text-xs" onClick={() => onDone()}>
            {S.library.cancel}
          </button>
          <button
            className="u-btn u-btn-primary px-3.5 py-1.5 text-xs"
            disabled={!name.trim() || create.isPending}
            onClick={() => create.mutate()}
          >
            {S.settings.kbs.create}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 系统层数据源注册（问数）：凭据只进不出，列表只显示 host:port/db 摘要。 */
function DataSourcesAdmin() {
  const queryClient = useQueryClient();
  const list = useQuery({ queryKey: ["dataSources"], queryFn: api.adminDataSources });
  const [name, setName] = useState("");
  const [conn, setConn] = useState("");
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["dataSources"] });

  const create = useMutation({
    mutationFn: () => api.adminCreateDataSource({ name: name.trim(), conn_string: conn.trim() }),
    onSuccess: () => {
      setName("");
      setConn("");
      invalidate();
    },
  });
  const remove = useMutation({
    mutationFn: (id: string) => api.adminDeleteDataSource(id),
    onSettled: invalidate,
  });
  const test = useMutation({
    mutationFn: (id: string) => api.adminTestDataSource(id),
    onSettled: invalidate,
  });

  return (
    <div className="space-y-4">
      <p className="text-xs text-neutral-500">{S.settings.datasources.hint}</p>

      <div className="glass rounded-xl divide-y divide-white/5">
        {(list.data?.data_sources ?? []).map((d) => (
          <div key={d.id} className="px-4 py-3 flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <div className="text-sm text-neutral-200">{d.name}</div>
              <div className="text-xs text-neutral-500 font-mono truncate">{d.summary}</div>
            </div>
            <span
              className={`u-chip shrink-0 ${
                d.last_test_ok === true
                  ? "u-chip-success"
                  : d.last_test_ok === false
                    ? "u-chip-danger"
                    : "u-chip-neutral"
              }`}
            >
              {d.last_test_ok === true
                ? S.settings.datasources.testOk
                : d.last_test_ok === false
                  ? S.settings.datasources.testFail
                  : S.settings.datasources.neverTested}
            </span>
            <button
              className="u-btn u-btn-ghost px-2.5 py-1 text-xs shrink-0"
              disabled={test.isPending}
              onClick={() => test.mutate(d.id)}
            >
              {S.settings.datasources.test}
            </button>
            <button
              className="text-xs text-neutral-500 hover:text-[var(--u-danger)] shrink-0"
              disabled={remove.isPending}
              onClick={() => remove.mutate(d.id)}
            >
              {S.settings.datasources.remove}
            </button>
          </div>
        ))}
        {list.data?.data_sources.length === 0 && (
          <p className="px-4 py-6 text-sm text-neutral-500">—</p>
        )}
      </div>

      <div className="glass rounded-xl p-4 space-y-2">
        <input
          className="input-dark w-full px-3 py-2 text-sm"
          placeholder={S.settings.datasources.name}
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          className="input-dark w-full px-3 py-2 text-sm font-mono u-placeholder-sans"
          placeholder={S.settings.datasources.connString}
          value={conn}
          onChange={(e) => setConn(e.target.value)}
        />
        <div className="flex items-center gap-3">
          <button
            className="u-btn u-btn-primary px-3.5 py-1.5 text-xs"
            disabled={!name.trim() || !conn.trim() || create.isPending}
            onClick={() => create.mutate()}
          >
            {S.settings.datasources.add}
          </button>
          {create.isError && (
            <span className="text-xs text-rose-400">{(create.error as Error).message}</span>
          )}
        </div>
      </div>
    </div>
  );
}

const PRESETS: Record<
  string,
  { chat: string; embed: string; chatModel: string; embedModel: string }
> = {
  DeepSeek: {
    chat: "https://api.deepseek.com/v1",
    chatModel: "deepseek-chat",
    embed: "",
    embedModel: "",
  },
  SiliconFlow: {
    chat: "https://api.siliconflow.cn/v1",
    chatModel: "deepseek-ai/DeepSeek-V3",
    embed: "https://api.siliconflow.cn/v1",
    embedModel: "BAAI/bge-m3",
  },
  "Qwen (DashScope)": {
    chat: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    chatModel: "qwen-plus",
    embed: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    embedModel: "text-embedding-v3",
  },
  Ollama: {
    chat: "http://localhost:11434/v1",
    chatModel: "qwen2.5:7b",
    embed: "http://localhost:11434/v1",
    embedModel: "bge-m3",
  },
  OpenAI: {
    chat: "https://api.openai.com/v1",
    chatModel: "gpt-4o-mini",
    embed: "https://api.openai.com/v1",
    embedModel: "text-embedding-3-small",
  },
};

export function Settings() {
  const { workspace } = useKb();
  const { tab: tabParam } = useSearch({ from: "/account/admin" });
  const [tab, setTab] = useState<"models" | "members" | "kbs" | "datasources" | "deployment">(
    tabParam ?? "models",
  );
  const queryClient = useQueryClient();
  const settings = useQuery({
    queryKey: ["settings", workspace?.id],
    queryFn: () => api.settings(workspace!.id),
    enabled: !!workspace,
  });

  const [form, setForm] = useState({
    chat_base_url: "",
    chat_api_key: "",
    chat_model: "",
    embed_base_url: "",
    embed_api_key: "",
    embed_model: "",
  });

  useEffect(() => {
    if (settings.data) {
      setForm((f) => ({
        ...f,
        chat_base_url: settings.data.chat_base_url ?? "",
        chat_model: settings.data.chat_model ?? "",
        embed_base_url: settings.data.embed_base_url ?? "",
        embed_model: settings.data.embed_model ?? "",
      }));
    }
  }, [settings.data]);

  const save = useMutation({
    mutationFn: () => api.saveSettings(workspace!.id, form),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["settings", workspace?.id] }),
  });

  const test = useMutation({ mutationFn: () => api.testSettings(workspace!.id) });

  if (!workspace) return <div className="p-8 text-sm text-neutral-500">{S.nav.loading}</div>;

  const set = (k: keyof typeof form) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setForm({ ...form, [k]: e.target.value });

  const input =
    "input-dark w-full px-3 py-2 text-sm";
  const label = "block text-xs font-medium text-neutral-400 mb-1";

  return (
    <div className="h-full overflow-y-auto p-6">
      <div className="max-w-xl">
        <h2 className="text-lg font-bold text-neutral-100 mb-3">{S.settings.title}</h2>
        <div className="flex gap-1 mb-5 bg-white/5 rounded-lg p-1 w-fit">
          {(
            [
              ["models", S.settings.tabModels],
              ["members", S.settings.tabMembers],
              ["kbs", S.settings.tabKbs],
              ["datasources", S.settings.datasources.tab],
              ["deployment", S.settings.tabDeployment],
            ] as const
          ).map(([key, tabLabel]) => (
            <button
              key={key}
              onClick={() => setTab(key)}
              className={`rounded-md px-4 py-1.5 text-sm font-medium transition-colors ${
                tab === key
                  ? "bg-white/10 text-neutral-100"
                  : "text-neutral-500 hover:text-neutral-300"
              }`}
            >
              {tabLabel}
            </button>
          ))}
        </div>

        {tab === "members" && <Members workspaceId={workspace.id} />}
        {tab === "kbs" && <KbsAdmin />}
        {tab === "datasources" && <DataSourcesAdmin />}
        {tab === "deployment" && <DeploymentAdmin />}

        {tab === "models" && (
          <>
            <p className="text-sm text-neutral-400 mb-4">{S.settings.modelsIntro}</p>

            <div className="mb-5 flex flex-wrap gap-2">
              {Object.entries(PRESETS).map(([name, p]) => (
                <button
                  key={name}
                  onClick={() =>
                    setForm({
                      ...form,
                      chat_base_url: p.chat,
                      chat_model: p.chatModel,
                      embed_base_url: p.embed,
                      embed_model: p.embedModel,
                    })
                  }
                  className="rounded-full border border-white/15 bg-white/5 px-3 py-1 text-xs text-neutral-300 hover:border-[rgba(var(--u-accent-deep),0.5)] hover:text-[var(--u-accent)]"
                >
                  {name}
                </button>
              ))}
            </div>

            <div className="glass rounded-xl p-5 space-y-4">
              <h3 className="text-sm font-bold text-neutral-200">{S.settings.chatModel}</h3>
              <div>
                <label className={label}>{S.settings.baseUrl}</label>
                <input
                  className={input}
                  placeholder="https://api.deepseek.com/v1"
                  value={form.chat_base_url}
                  onChange={set("chat_base_url")}
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className={label}>{S.settings.model}</label>
                  <input
                    className={input}
                    placeholder="deepseek-chat"
                    value={form.chat_model}
                    onChange={set("chat_model")}
                  />
                </div>
                <div>
                  <label className={label}>
                    {S.settings.apiKey}{" "}
                    {settings.data?.has_chat_key && (
                      <span className="text-[var(--u-accent)]">{S.settings.keyConfigured}</span>
                    )}
                  </label>
                  <input
                    className={input}
                    type="password"
                    placeholder="sk-…"
                    value={form.chat_api_key}
                    onChange={set("chat_api_key")}
                  />
                </div>
              </div>

              <h3 className="text-sm font-bold text-neutral-200 pt-2">{S.settings.embedModel}</h3>
              <div>
                <label className={label}>{S.settings.baseUrl}</label>
                <input
                  className={input}
                  placeholder="http://localhost:11434/v1"
                  value={form.embed_base_url}
                  onChange={set("embed_base_url")}
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className={label}>{S.settings.model}</label>
                  <input
                    className={input}
                    placeholder="bge-m3"
                    value={form.embed_model}
                    onChange={set("embed_model")}
                  />
                </div>
                <div>
                  <label className={label}>
                    {S.settings.apiKey}{" "}
                    {settings.data?.has_embed_key && (
                      <span className="text-[var(--u-accent)]">{S.settings.keyConfigured}</span>
                    )}
                  </label>
                  <input
                    className={input}
                    type="password"
                    value={form.embed_api_key}
                    onChange={set("embed_api_key")}
                  />
                </div>
              </div>

              <div className="flex gap-2 pt-2">
                <button
                  onClick={() => save.mutate()}
                  disabled={save.isPending}
                  className="u-btn u-btn-primary px-4 py-2 text-sm"
                >
                  {save.isPending ? S.settings.saving : S.settings.save}
                </button>
                <button
                  onClick={() => test.mutate()}
                  disabled={test.isPending}
                  className="u-btn u-btn-ghost px-4 py-2 text-sm"
                >
                  {test.isPending ? S.settings.testing : S.settings.test}
                </button>
              </div>

              {save.isSuccess && <p className="text-sm text-[var(--u-accent)]">{S.settings.saved}</p>}
              {save.isError && (
                <p className="text-sm text-rose-400">{(save.error as Error).message}</p>
              )}
              {test.data && (
                <div className="text-sm space-y-1 pt-1">
                  <p className={test.data.chat.ok ? "text-[var(--u-accent)]" : "text-rose-400"}>
                    {S.settings.chatLabel}:{" "}
                    {test.data.chat.ok
                      ? S.settings.ok(test.data.chat.reply ?? "OK")
                      : test.data.chat.error}
                  </p>
                  <p className={test.data.embed.ok ? "text-[var(--u-accent)]" : "text-neutral-400"}>
                    {S.settings.embedLabel}:{" "}
                    {test.data.embed.ok
                      ? S.settings.okDim(test.data.embed.dim ?? 0)
                      : test.data.embed.error}
                  </p>
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
