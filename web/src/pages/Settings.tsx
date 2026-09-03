import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { Lock, Plus, X } from "lucide-react";
import { api } from "../api";
import { LANG_NAMES, S } from "../i18n";
import { useKb } from "../kb";
import { toast } from "../toast";
import { SearchSelect } from "../ui";
import { Members } from "./Members";

/** 一个源授权给了哪些工作区（0014）。
 *
 * **授权与挂载是两层**：这里说「这个源可以给谁用」，KB 管理员再在授权过的
 * 集合里挑挂不挂。从前没有这一层——可挂载列表返回全部署每一个源，于是任何
 * 库的管理员都能把任意生产库挂进自己库。
 *
 * 两层都是多对多：一个源可授权给多个工作区，一个工作区可拿到多个源。 */
function SourceGrants({ sourceId }: { sourceId: string }) {
  const queryClient = useQueryClient();
  const [picked, setPicked] = useState("");
  const [notice, setNotice] = useState<string | null>(null);

  const grants = useQuery({
    queryKey: ["dataSourceGrants", sourceId],
    queryFn: () => api.dataSourceGrants(sourceId),
  });
  const workspaces = useQuery({
    queryKey: ["workspaces"],
    queryFn: api.workspaces,
  });
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["dataSourceGrants", sourceId] });

  const grant = useMutation({
    mutationFn: (wsId: string) => api.grantDataSource(sourceId, wsId),
    onSuccess: () => {
      setPicked("");
      setNotice(null);
      invalidate();
    },
    onError: (e: unknown) => toast.error((e as Error).message),
  });
  const revoke = useMutation({
    mutationFn: (wsId: string) => api.revokeDataSource(sourceId, wsId),
    // 卸了几个要说出来：收回授权会顺带断掉正在用的挂载，
    // 悄悄断比断本身更糟
    onSuccess: (r) => {
      setNotice(S.settings.datasources.grantRevoked(r.unmounted));
      invalidate();
    },
    onError: (e: unknown) => toast.error((e as Error).message),
  });

  const granted = grants.data?.workspaces ?? [];
  const grantedIds = new Set(granted.map((w) => w.id));
  const grantable = (workspaces.data ?? []).filter(
    (w) => !grantedIds.has(w.id),
  );

  return (
    <div className="border-t border-white/5 pt-3 space-y-2">
      <div className="flex items-baseline gap-2">
        <span className="text-[11px] text-neutral-500">
          {S.settings.datasources.grants}
        </span>
        {granted.length === 0 ? (
          <span className="text-[11px] text-neutral-600">
            {S.settings.datasources.grantsNone}
          </span>
        ) : (
          <div className="flex flex-wrap gap-1.5">
            {granted.map((w) => (
              <span
                key={w.id}
                className="u-chip u-chip-neutral text-[11px] flex items-center gap-1"
              >
                {w.name}
                <button
                  className="text-neutral-500 hover:text-[var(--u-danger)]"
                  title={S.settings.datasources.grantRevoke}
                  disabled={revoke.isPending}
                  onClick={() => revoke.mutate(w.id)}
                >
                  <X size={10} />
                </button>
              </span>
            ))}
          </div>
        )}
      </div>
      {grantable.length > 0 && (
        <div className="flex items-center gap-2">
          <SearchSelect
            className="flex-1"
            value={picked}
            options={grantable.map((w) => ({ value: w.id, label: w.name }))}
            onChange={(v) => {
              setPicked(v);
              if (v) grant.mutate(v);
            }}
            placeholder={S.settings.datasources.grantAdd}
          />
        </div>
      )}
      {notice && <p className="text-[11px] text-neutral-500">{notice}</p>}
    </div>
  );
}

/** 部署级配置：注册开关 + worker 并发。 */
function DeploymentAdmin() {
  const queryClient = useQueryClient();
  const dep = useQuery({
    queryKey: ["deployment"],
    queryFn: api.adminDeployment,
  });
  const [workers, setWorkers] = useState<number | null>(null);
  const shown = workers ?? dep.data?.worker_concurrency ?? 32;
  // 按模型的并发：缺省值 + 每个在用模型的覆盖
  const [modelDefault, setModelDefault] = useState<number | null>(null);
  const shownDefault =
    modelDefault ?? dep.data?.default_model_concurrency ?? 10;
  const [perModel, setPerModel] = useState<Record<string, number>>({});
  const save = useMutation({
    mutationFn: (v: {
      open: boolean;
      workers?: number;
      defaultModel?: number;
      modelLimit?: {
        base_url: string;
        model: string;
        max_concurrent: number | null;
      };
      ontologyLang?: "en" | "zh";
    }) =>
      api.saveAdminDeployment(
        v.open,
        v.workers,
        v.defaultModel,
        v.modelLimit,
        v.ontologyLang,
      ),
    onSuccess: () => {
      setPerModel({});
      queryClient.invalidateQueries({ queryKey: ["deployment"] });
    },
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
          <span className="block text-sm text-neutral-200">
            {S.settings.deployment.openReg}
          </span>
          <span className="block text-xs text-neutral-500 mt-0.5">
            {S.settings.deployment.openRegHint}
          </span>
        </span>
      </label>

      {/* 新建库的本体语言。**不是界面语言**——界面语言是每个人自己在账户菜单里选的，
          根本不经过后端（docs/decisions/0004）。说明里必须把这句讲出来 */}
      <div className="flex items-start justify-between gap-4 border-t border-white/10 pt-4">
        <div className="min-w-0">
          <span className="block text-sm text-neutral-200">
            {S.settings.deployment.ontologyLang}
          </span>
          <span className="block text-xs text-neutral-500 mt-0.5">
            {S.settings.deployment.ontologyLangHint}
          </span>
        </div>
        <div className="flex gap-1 rounded-lg bg-white/5 p-1 h-fit shrink-0">
          {(["en", "zh"] as const).map((l) => (
            <button
              key={l}
              disabled={dep.isPending || save.isPending}
              onClick={() => save.mutate({ open, ontologyLang: l })}
              className={`rounded-md px-3 py-1 text-[12px] font-medium transition-colors ${
                (dep.data?.default_ontology_lang ?? "en") === l
                  ? "bg-white/10 text-neutral-100"
                  : "text-neutral-500 hover:text-neutral-300"
              }`}
            >
              {LANG_NAMES[l]}
            </button>
          ))}
        </div>
      </div>

      <div className="flex items-start justify-between gap-4 border-t border-white/10 pt-4">
        <div className="min-w-0">
          <span className="block text-sm text-neutral-200">
            {S.settings.deployment.workers}
          </span>
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
            onChange={(e) =>
              setWorkers(Math.max(1, Math.min(32, Number(e.target.value) || 1)))
            }
          />
          <button
            className="u-btn u-btn-ghost px-3 py-1.5 text-xs"
            disabled={
              save.isPending ||
              workers === null ||
              workers === dep.data?.worker_concurrency
            }
            onClick={() => save.mutate({ open, workers: shown })}
          >
            {S.settings.deployment.workersApply}
          </button>
        </div>
      </div>
      {/* 按模型的并发才是真正的节流：约束来自供应商的速率限制，而那是按模型算的。
          上面那个 worker 并发只是外层兜底，防任务无限堆积 */}
      <div className="border-t border-white/10 pt-4">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <span className="block text-sm text-neutral-200">
              {S.settings.deployment.modelConcurrency}
            </span>
            <span className="block text-xs text-neutral-500 mt-0.5">
              {S.settings.deployment.modelConcurrencyHint}
            </span>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <span className="text-xs text-neutral-500">
              {S.settings.deployment.modelDefault}
            </span>
            <input
              type="number"
              min={1}
              max={256}
              className="input-dark u-input-plain w-16 px-2 py-1.5 text-sm u-num text-center"
              value={shownDefault}
              disabled={dep.isPending}
              onChange={(e) =>
                setModelDefault(
                  Math.max(1, Math.min(256, Number(e.target.value) || 1)),
                )
              }
            />
            <button
              className="u-btn u-btn-ghost px-3 py-1.5 text-xs"
              disabled={
                save.isPending ||
                modelDefault === null ||
                modelDefault === dep.data?.default_model_concurrency
              }
              onClick={() => save.mutate({ open, defaultModel: shownDefault })}
            >
              {S.settings.deployment.workersApply}
            </button>
          </div>
        </div>

        {!!dep.data?.models_in_use?.length && (
          <div className="mt-3 space-y-1.5">
            {dep.data.models_in_use.map((m) => {
              const cur =
                dep.data?.model_limits?.find(
                  (l) => l.base_url === m.base_url && l.model === m.model,
                )?.max_concurrent ?? null;
              const key = `${m.base_url}|${m.model}`;
              const val = perModel[key] ?? cur ?? shownDefault;
              return (
                <div key={key} className="flex items-center gap-2 text-xs">
                  <span className="u-chip u-chip-neutral !text-[10px] !px-1.5 shrink-0">
                    {m.kind}
                  </span>
                  <span className="font-mono text-neutral-300 truncate">
                    {m.model}
                  </span>
                  <span className="text-neutral-600 truncate hidden sm:inline">
                    {m.base_url}
                  </span>
                  <input
                    type="number"
                    min={1}
                    max={256}
                    className="input-dark u-input-plain ml-auto w-14 px-1.5 py-1 text-xs u-num text-center shrink-0"
                    value={val}
                    onChange={(e) =>
                      setPerModel({
                        ...perModel,
                        [key]: Math.max(
                          1,
                          Math.min(256, Number(e.target.value) || 1),
                        ),
                      })
                    }
                  />
                  <button
                    className="u-btn u-btn-ghost px-2 py-1 text-[11px] shrink-0"
                    disabled={save.isPending || perModel[key] === undefined}
                    onClick={() =>
                      save.mutate({
                        open,
                        modelLimit: {
                          base_url: m.base_url,
                          model: m.model,
                          max_concurrent: val,
                        },
                      })
                    }
                  >
                    {S.settings.deployment.workersApply}
                  </button>
                  {cur !== null && (
                    <button
                      className="u-btn u-btn-ghost px-2 py-1 text-[11px] shrink-0 text-neutral-500"
                      disabled={save.isPending}
                      title={S.settings.deployment.modelResetHint}
                      onClick={() =>
                        save.mutate({
                          open,
                          modelLimit: {
                            base_url: m.base_url,
                            model: m.model,
                            max_concurrent: null,
                          },
                        })
                      }
                    >
                      {S.settings.deployment.modelReset}
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        )}
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
                <span className="text-sm text-neutral-200 truncate">
                  {kb.name}
                </span>
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
                <span className="u-num">
                  {S.account.kbStats(doc_count, member_count)}
                </span>
              </div>
            </div>
            <button
              className="u-btn u-btn-ghost px-2.5 py-1 text-xs shrink-0"
              onClick={() => {
                setKb(kb.id);
                navigate({ to: "/kb/$kbId/settings", params: { kbId: kb.id } });
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
            queryClient.invalidateQueries({
              queryKey: ["myKbs", workspace.id],
            });
            queryClient.invalidateQueries({ queryKey: ["kbs", workspace.id] });
            // 建完直达库设置：下一步几乎总是邀人/配置
            if (id) {
              setKb(id);
              navigate({ to: "/kb/$kbId/settings", params: { kbId: id } });
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
  // schema.org 默认勾选，可反选（0009）。删掉内置类之后不选任何包的库是真的空，
  // 而空库仍然能用——但绝大多数人要的是一个已经能认出人、组织、产品的起点。
  // 一秒装完（0008 的批量插入），所以默认装得起
  const [packs, setPacks] = useState<string[]>(["schema-org"]);

  const available = useQuery({
    queryKey: ["ontologyPacks"],
    queryFn: api.ontologyPacks,
  });

  // 勾选顺序即安装顺序：第一个包的类会认领同名的种子类，
  // 后面的撞名才查得到对齐表。所以取消再勾会排到末尾——这是对的
  const toggle = (id: string) =>
    setPacks((prev) =>
      prev.includes(id) ? prev.filter((p) => p !== id) : [...prev, id],
    );

  const create = useMutation({
    mutationFn: () =>
      api.createKb(workspaceId, {
        name: name.trim(),
        description: desc.trim() || null,
        visibility: restricted ? "restricted" : "open",
        ontology_packs: packs,
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
          <button
            onClick={() => onDone()}
            className="text-neutral-500 hover:text-neutral-200"
          >
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

        <div className="mb-4">
          <div className="text-xs text-neutral-300 mb-1">
            {S.settings.kbs.packsLabel}
          </div>
          <p className="text-[11px] leading-relaxed text-neutral-500 mb-2">
            {S.settings.kbs.packsHint}
          </p>
          <div className="grid grid-cols-2 gap-1.5">
            {available.data?.packs.map((p) => {
              const on = packs.includes(p.id);
              return (
                <label
                  key={p.id}
                  // 选中态靠边框与底色，勾选框藏起来：五个并排时
                  // 一排勾选框比内容本身还抢眼
                  className={
                    "cursor-pointer rounded-lg border px-2.5 py-2 transition-colors " +
                    (on
                      ? "border-white/25 bg-white/[0.07]"
                      : "border-white/10 hover:bg-white/5")
                  }
                >
                  <input
                    type="checkbox"
                    className="sr-only"
                    checked={on}
                    onChange={() => toggle(p.id)}
                  />
                  <span className="block text-xs text-neutral-200">
                    {p.name}
                  </span>
                  <span className="block text-[11px] leading-snug text-neutral-500">
                    {p.summary}
                  </span>
                  <span className="mt-0.5 block text-[10px] text-neutral-600">
                    {S.settings.kbs.packsCount(p.classes, p.properties)}
                  </span>
                </label>
              );
            })}
          </div>
          {packs.length === 0 && (
            <p className="text-[11px] text-neutral-500 mt-1.5">
              {S.settings.kbs.packsNone}
            </p>
          )}
        </div>
        {create.isError && (
          <p className="text-xs text-rose-400 mb-2">
            {(create.error as Error).message}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <button
            className="u-btn u-btn-ghost px-3.5 py-1.5 text-xs"
            onClick={() => onDone()}
          >
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
  const list = useQuery({
    queryKey: ["dataSources"],
    queryFn: api.adminDataSources,
  });
  const [name, setName] = useState("");
  const [conn, setConn] = useState("");
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["dataSources"] });

  const create = useMutation({
    mutationFn: () =>
      api.adminCreateDataSource({
        name: name.trim(),
        conn_string: conn.trim(),
      }),
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
          <div key={d.id} className="px-4 py-3 space-y-3">
            <div className="flex items-center gap-3">
              <div className="min-w-0 flex-1">
                <div className="text-sm text-neutral-200">
                  {d.name}
                  <span className="ml-2 text-[11px] uppercase tracking-wide text-neutral-500">
                    {d.engine}
                  </span>
                </div>
                <div className="text-xs text-neutral-500 font-mono truncate">
                  {d.summary}
                </div>
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
            <SourceGrants sourceId={d.id} />
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
        <p className="text-[11px] leading-5 text-neutral-500 font-mono whitespace-pre-line">
          {S.settings.datasources.connSchemes}
        </p>
        <div className="flex items-center gap-3">
          <button
            className="u-btn u-btn-primary px-3.5 py-1.5 text-xs"
            disabled={!name.trim() || !conn.trim() || create.isPending}
            onClick={() => create.mutate()}
          >
            {S.settings.datasources.add}
          </button>
          {create.isError && (
            <span className="text-xs text-rose-400">
              {(create.error as Error).message}
            </span>
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
  const [tab, setTab] = useState<
    "models" | "members" | "kbs" | "datasources" | "deployment"
  >(tabParam ?? "models");
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
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["settings", workspace?.id] }),
  });

  const test = useMutation({
    mutationFn: () => api.testSettings(workspace!.id),
  });

  if (!workspace)
    return <div className="p-8 text-sm text-neutral-500">{S.nav.loading}</div>;

  const set =
    (k: keyof typeof form) => (e: React.ChangeEvent<HTMLInputElement>) =>
      setForm({ ...form, [k]: e.target.value });

  const input = "input-dark w-full px-3 py-2 text-sm";
  const label = "block text-xs font-medium text-neutral-400 mb-1";

  return (
    <div className="h-full overflow-y-auto p-6">
      <div className="max-w-xl">
        <h2 className="text-lg font-bold text-neutral-100 mb-3">
          {S.settings.title}
        </h2>
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
            <p className="text-sm text-neutral-400 mb-4">
              {S.settings.modelsIntro}
            </p>

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
              <h3 className="text-sm font-bold text-neutral-200">
                {S.settings.chatModel}
              </h3>
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
                      <span className="text-[var(--u-accent)]">
                        {S.settings.keyConfigured}
                      </span>
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

              <h3 className="text-sm font-bold text-neutral-200 pt-2">
                {S.settings.embedModel}
              </h3>
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
                      <span className="text-[var(--u-accent)]">
                        {S.settings.keyConfigured}
                      </span>
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

              {save.isSuccess && (
                <p className="text-sm text-[var(--u-accent)]">
                  {S.settings.saved}
                </p>
              )}
              {save.isError && (
                <p className="text-sm text-rose-400">
                  {(save.error as Error).message}
                </p>
              )}
              {test.data && (
                <div className="text-sm space-y-1 pt-1">
                  <p
                    className={
                      test.data.chat.ok
                        ? "text-[var(--u-accent)]"
                        : "text-rose-400"
                    }
                  >
                    {S.settings.chatLabel}:{" "}
                    {test.data.chat.ok
                      ? S.settings.ok(test.data.chat.reply ?? "OK")
                      : test.data.chat.error}
                  </p>
                  <p
                    className={
                      test.data.embed.ok
                        ? "text-[var(--u-accent)]"
                        : "text-neutral-400"
                    }
                  >
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
