/* 知识库设置：左栏分节（General / Members / Danger zone），为未来设置项立骨架
   （抽取设置、保留策略、库级令牌…）。访问控制由 API 端执行（库 admin 起步）。
   分节互斥渲染也根治了下拉弹层被后续玻璃卡（backdrop-filter 自成 stacking
   context）遮蔽的层级 bug。 */
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useParams, useNavigate } from "@tanstack/react-router";
import {
  History as HistoryIcon,
  Lock,
  Settings2,
  TriangleAlert,
  Users,
} from "lucide-react";
import { api, type AuditEvent } from "../api";
import { LANG_NAMES, S } from "../i18n";
import { toast } from "../toast";
import { DangerConfirm,
  Dropdown,
  Loading,
  Pager,
  RAIL_CLS,
  SearchSelect, localDateTime } from "../ui";

const KB_ROLES = [
  { value: "viewer", label: S.kbset.roles.viewer },
  { value: "editor", label: S.kbset.roles.editor },
  { value: "admin", label: S.kbset.roles.admin },
];

/**
 * 这个库能授予哪些角色。
 *
 * **open 库没有 viewer 可授**：`access::kb_role` 对 open 库直接给部署内每个人
 * Viewer，所以写一行 `role=viewer` 什么都没多给——一条空操作的记录，
 * 还占着成员名单一行让人以为它起了作用。列在这里的意义只剩"给写权限"。
 *
 * 历史数据里可能存着 open 库的 viewer 行，但那些行在名单里已经不显示了
 *（见 `listed`），所以这里不必为"当前值不在选项里"兜底。
 */
function rolesFor(isOpen: boolean) {
  return isOpen ? KB_ROLES.filter((r) => r.value !== "viewer") : KB_ROLES;
}

type Section = "general" | "members" | "activity" | "danger";

export function KbSettings() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  /* 库 id 来自路径。**从前是 `?kb=`**——那是这套路由改造之前唯一
     带着库走的地方，现在整片都在 /kb/$kbId 之下，它就不必自成一格了 */
  const { kbId } = useParams({ from: "/app/kb/$kbId/settings" });

  // 失败任务数与重排（#216）。查询键带库 id，重排后失效重取
  const failedJobs = useQuery({
    queryKey: ["jobs", "failed", kbId],
    queryFn: () => api.failedJobs(kbId!),
    enabled: !!kbId,
  });
  const requeue = useMutation({
    mutationFn: () => api.requeueJobs(kbId!),
    onSuccess: (r) => {
      toast.success(S.kbset.requeued(r.requeued));
      queryClient.invalidateQueries({ queryKey: ["jobs", "failed", kbId] });
    },
    onError: (e) => toast.error(String(e)),
  });
  const kb = useQuery({
    queryKey: ["kbOne", kbId],
    queryFn: () => api.kbDetail(kbId!),
    enabled: !!kbId,
  });

  const [section, setSection] = useState<Section>("general");
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [visibility, setVisibility] = useState<"open" | "restricted">("open");
  const [autoExtend, setAutoExtend] = useState(true);
  // **默认关**，与上面那个相反：推理往账本里写事实，而声明可能是错的
  const [materialize, setMaterialize] = useState(false);
  const [inferMins, setInferMins] = useState(60);
  const [ontoLang, setOntoLang] = useState<"en" | "zh">("en");
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (kb.data) {
      setName(kb.data.name);
      setDesc(kb.data.description ?? "");
      setVisibility(kb.data.visibility);
      setAutoExtend(kb.data.auto_extend_ontology);
      setMaterialize(kb.data.materialize_inferences);
      setInferMins(kb.data.inference_interval_minutes);
      setOntoLang(kb.data.ontology_lang);
    }
  }, [kb.data]);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["kbOne", kbId] });
    queryClient.invalidateQueries({ queryKey: ["kbs"] });
  };

  const save = useMutation({
    mutationFn: () =>
      api.updateKb(kbId!, {
        name: name.trim(),
        description: desc.trim() || null,
        visibility,
        auto_extend_ontology: autoExtend,
        materialize_inferences: materialize,
        inference_interval_minutes: inferMins,
        ontology_lang: ontoLang,
      }),
    onSuccess: () => {
      setError(null);
      invalidate();
    },
    onError: (e) => setError((e as Error).message),
  });

  const removeKb = useMutation({
    mutationFn: () => api.deleteKb(kbId!),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["kbs"] });
      navigate({ to: "/kb/$kbId/library", params: { kbId } });
    },
    onError: (e) => setError((e as Error).message),
  });

  if (!kbId || kb.isPending) return <Loading>{S.nav.loading}</Loading>;
  if (kb.isError)
    return (
      <div className="p-8 text-sm text-rose-400">
        {(kb.error as Error).message}
      </div>
    );

  const lbl = "block text-xs font-medium text-neutral-500 mb-1";
  const rail =
    "w-full flex items-center gap-2.5 rounded-lg px-3 py-2 text-[13px] text-left transition-colors";

  const isDefault = kb.data.is_default;
  const sections: {
    key: Section;
    label: string;
    Icon: typeof Settings2;
    danger?: boolean;
  }[] = [
    { key: "general", label: S.kbset.general, Icon: Settings2 },
    { key: "members", label: S.kbset.members, Icon: Users },
    { key: "activity", label: S.kbset.activity, Icon: HistoryIcon },
    // 默认库不可删除：danger 节整个不出现
    ...(isDefault
      ? []
      : [
          {
            key: "danger" as Section,
            label: S.kbset.danger,
            Icon: TriangleAlert,
            danger: true,
          },
        ]),
  ];

  return (
    <div className="h-full flex">
      {/* 分节导航：未来的抽取设置/保留策略/令牌等在此扩展 */}
      <aside className={`${RAIL_CLS} p-3 space-y-0.5`}>
        {sections.map(({ key, label, Icon, danger }) => (
          <button
            key={key}
            onClick={() => setSection(key)}
            className={`${rail} ${
              section === key
                ? "u-nav-active"
                : danger
                  ? "text-neutral-500 hover:bg-white/[0.05] hover:text-[var(--u-danger)]"
                  : "text-neutral-400 hover:bg-white/[0.05] hover:text-neutral-200"
            }`}
          >
            <Icon size={14} />
            {label}
          </button>
        ))}
      </aside>

      <main className="flex-1 min-w-0 overflow-y-auto u-scroll px-8 py-6">
        <div className="max-w-xl space-y-5">
          {/* 不缀库名：顶栏切换器已标明当前库 */}
          <h2 className="u-title text-lg">{S.kbset.title}</h2>

          {section === "general" && (
            <div className="glass rounded-xl p-4 space-y-3">
              <div className="grid grid-cols-2 gap-2">
                <div>
                  <label className={lbl}>{S.settings.kbs.name}</label>
                  <input
                    className="input-dark w-full px-3 py-2 text-sm"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                  />
                </div>
                <div>
                  <label className={lbl}>{S.settings.kbs.visibility}</label>
                  {isDefault ? (
                    /* 默认库锁 open：说明常驻可见（藏在 hover 里等于没解释）,详情见 grid 下方整行 */
                    <div className="flex items-center gap-1.5 rounded-lg border border-white/10 px-2.5 py-2 text-[11px] text-neutral-500 cursor-not-allowed">
                      <Lock size={11} className="shrink-0 text-neutral-600" />
                      {S.kbset.defaultOpenLabel}
                    </div>
                  ) : (
                    <div className="flex rounded-lg overflow-hidden border border-white/10">
                      {(
                        [
                          ["open", "Open"],
                          ["restricted", S.settings.kbs.visRestricted],
                        ] as const
                      ).map(([v, label]) => (
                        <button
                          key={v}
                          onClick={() => setVisibility(v)}
                          title={label}
                          className={`flex-1 px-2 py-2 text-[11px] truncate transition-colors ${
                            visibility === v
                              ? "bg-white/[0.12] text-white"
                              : "text-neutral-500 hover:bg-white/[0.05] hover:text-neutral-300"
                          }`}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              </div>
              {isDefault && (
                <p className="text-xs leading-relaxed text-neutral-500">
                  {S.kbset.defaultOpenNote}
                </p>
              )}
              <div>
                <label className={lbl}>{S.settings.kbs.description}</label>
                <input
                  className="input-dark w-full px-3 py-2 text-sm"
                  value={desc}
                  onChange={(e) => setDesc(e.target.value)}
                />
              </div>
              {/* 自动扩本体：默认开，因为新库的十个默认关系不是任何人选的。
                  说明里要讲清关掉之后失去的**只是**代劳，不是留意 */}
              <label className="flex items-start gap-2.5 pt-1 cursor-pointer">
                <input
                  type="checkbox"
                  className="mt-0.5 accent-[var(--u-accent)]"
                  checked={autoExtend}
                  onChange={(e) => setAutoExtend(e.target.checked)}
                />
                <span className="min-w-0">
                  <span className="block text-sm text-neutral-200">
                    {S.kbset.autoExtend}
                  </span>
                  <span className="block text-xs leading-relaxed text-neutral-500">
                    {S.kbset.autoExtendNote}
                  </span>
                </span>
              </label>
              {/* 物化推理：**默认关**，与上面那个相反。自动扩本体动的是词表，
                  这个动的是账本——它按公理往图里写事实，而声明可能是错的 */}
              <label className="flex items-start gap-2.5 pt-1 cursor-pointer">
                <input
                  type="checkbox"
                  className="mt-0.5 accent-[var(--u-accent)]"
                  checked={materialize}
                  onChange={(e) => setMaterialize(e.target.checked)}
                />
                <span className="min-w-0">
                  <span className="block text-sm text-neutral-200">
                    {S.kbset.materialize}
                  </span>
                  <span className="block text-xs leading-relaxed text-neutral-500">
                    {S.kbset.materializeNote}
                  </span>
                </span>
              </label>
              {/* 重推间隔。**只在开着的时候露出来**——关着时它不影响任何事，
                  摆在那里只会让人以为设了就会推 */}
              {materialize && (
                <div className="pl-6 flex items-center gap-2">
                  <label className="text-xs text-neutral-500">
                    {S.kbset.inferEvery}
                  </label>
                  <input
                    type="number"
                    min={5}
                    max={10080}
                    className="input-dark w-24 px-2 py-1 text-xs u-num"
                    value={inferMins}
                    onChange={(e) => setInferMins(Number(e.target.value))}
                  />
                  <span className="text-xs text-neutral-500">
                    {S.kbset.minutes}
                  </span>
                  {kb.data.last_inference_at && (
                    <span className="text-[11px] text-neutral-600">
                      {S.kbset.lastInference(
                        new Date(kb.data.last_inference_at).toLocaleString(),
                      )}
                    </span>
                  )}
                </div>
              )}
              {/* 失败的任务（#216）：有才露出来。「再跑一遍」把这个库里全部 failed 放回队列 */}
              {failedJobs.data && failedJobs.data.failed > 0 && (
                <div className="flex items-center gap-2">
                  <span className="text-xs text-neutral-400">
                    {S.kbset.failedJobs(failedJobs.data.failed)}
                  </span>
                  <button
                    className="u-btn u-btn-ghost px-2.5 py-1 text-xs"
                    disabled={requeue.isPending}
                    onClick={() => requeue.mutate()}
                  >
                    {S.kbset.requeue}
                  </button>
                </div>
              )}
              {/* 语料语言。**不是界面语言**——类描述逐字进抽取提示词，
                  读者是正在读这些文档的模型，所以它跟文档走不跟读者走 */}
              <div className="pt-1">
                <span className="block text-sm text-neutral-200">
                  {S.kbset.ontologyLang}
                </span>
                <span className="mt-0.5 block text-xs leading-relaxed text-neutral-500">
                  {S.kbset.ontologyLangNote}
                </span>
                <div className="mt-2 flex gap-1 rounded-lg bg-white/5 p-1 w-fit">
                  {(["en", "zh"] as const).map((l) => (
                    <button
                      key={l}
                      onClick={() => setOntoLang(l)}
                      className={`rounded-md px-3 py-1 text-[12px] font-medium transition-colors ${
                        ontoLang === l
                          ? "bg-white/10 text-neutral-100"
                          : "text-neutral-500 hover:text-neutral-300"
                      }`}
                    >
                      {LANG_NAMES[l]}
                    </button>
                  ))}
                </div>
              </div>
              <div className="flex items-center gap-3">
                <button
                  className="u-btn u-btn-primary px-3.5 py-1.5 text-xs"
                  disabled={!name.trim() || save.isPending}
                  onClick={() => save.mutate()}
                >
                  {S.kbset.save}
                </button>
                {save.isSuccess && (
                  <span className="text-xs text-neutral-400">
                    {S.kbset.saved}
                  </span>
                )}
              </div>
            </div>
          )}

          {section === "members" && (
            <KbMembers kbId={kbId} isOpen={kb.data.visibility === "open"} />
          )}

          {section === "activity" && <KbActivity kbId={kbId} />}

          {section === "danger" && (
            <div className="glass rounded-2xl px-5 py-4 flex items-center justify-between gap-4">
              <div className="min-w-0">
                <div className="text-sm font-medium text-neutral-200">
                  {S.kbset.deleteRowTitle}
                </div>
                <div className="mt-0.5 text-xs text-neutral-500">
                  {S.kbset.deleteRowHint}
                </div>
              </div>
              <button
                className="u-btn px-3.5 py-1.5 text-xs font-semibold shrink-0"
                style={{
                  background: "var(--u-danger-solid)",
                  color: "#ffffff",
                }}
                onClick={() => setConfirmingDelete(true)}
              >
                {S.kbset.deleteRowBtn}
              </button>
            </div>
          )}

          {error && <p className="text-sm text-rose-400">{error}</p>}

          {confirmingDelete && (
            <DangerConfirm
              title={S.kbset.deleteKb}
              hint={S.kbset.deleteHint(kb.data.name)}
              requireText={kb.data.name}
              confirmLabel={S.kbset.deleteBtn}
              cancelLabel={S.library.cancel}
              busy={removeKb.isPending}
              onConfirm={() => removeKb.mutate()}
              onCancel={() => setConfirmingDelete(false)}
            />
          )}
        </div>
      </main>
    </div>
  );
}

/** detail 里挑一个人类可读的名字（按 action 语义各异，逐键兜底） */
function auditDetailName(e: AuditEvent): string {
  const d = e.detail;
  const cand = [d.label, d.name, d.filename, d.key, d.role];
  const hit = cand.find((v) => typeof v === "string" && v);
  return typeof hit === "string" ? hit : "";
}

const AUDIT_PAGE = 50;

function KbActivity({ kbId }: { kbId: string }) {
  // 筛选按真实查法来：查一类动作、查一个人、查一段时间。
  // 动作前缀匹配——`entity.` 就能把 retyped / renamed 一族一起捞出来
  const [action, setAction] = useState("");
  const [since, setSince] = useState("");
  const [until, setUntil] = useState("");
  const [page, setPage] = useState(0);
  const audit = useQuery({
    queryKey: ["kbAudit", kbId, action, since, until, page],
    queryFn: () =>
      api.kbAudit(kbId, {
        action: action || undefined,
        since: since || undefined,
        until: until || undefined,
        limit: AUDIT_PAGE,
        offset: page * AUDIT_PAGE,
      }),
    placeholderData: (prev) => prev,
  });
  const events = audit.data?.events ?? [];
  const total = audit.data?.total ?? 0;
  // 下拉按这个库实际发生过的动作填，不是硬编码清单
  const actions = audit.data?.actions ?? [];
  const filtered = !!(action || since || until);

  const reset = (fn: () => void) => {
    fn();
    setPage(0);
  };

  return (
    <div className="glass rounded-xl p-4">
      <p className="text-xs text-neutral-500 mb-3">{S.kbset.activityHint}</p>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <select
          className="input-dark px-2 py-1 text-xs"
          value={action}
          onChange={(e) => reset(() => setAction(e.target.value))}
        >
          <option value="">{S.kbset.auditAllActions}</option>
          {actions.map((a) => (
            <option key={a} value={a}>
              {a}
            </option>
          ))}
        </select>
        <input
          type="date"
          className="input-dark px-2 py-1 text-xs u-num"
          value={since}
          title={S.kbset.auditSince}
          onChange={(e) => reset(() => setSince(e.target.value))}
        />
        <span className="text-xs text-neutral-600">→</span>
        <input
          type="date"
          className="input-dark px-2 py-1 text-xs u-num"
          value={until}
          title={S.kbset.auditUntil}
          onChange={(e) => reset(() => setUntil(e.target.value))}
        />
        {filtered && (
          <button
            className="u-btn u-btn-ghost px-2 py-1 text-xs"
            onClick={() =>
              reset(() => {
                setAction("");
                setSince("");
                setUntil("");
              })
            }
          >
            {S.kbset.auditClear}
          </button>
        )}
        <span className="ml-auto u-num text-[11px] text-neutral-500">
          {S.kbset.auditTotal(total)}
        </span>
      </div>
      {audit.isPending ? (
        <p className="text-xs text-neutral-600">{S.nav.loading}</p>
      ) : events.length === 0 ? (
        <p className="text-xs text-neutral-600">{S.kbset.activityEmpty}</p>
      ) : (
        <div className="space-y-0.5">
          {events.map((e) => (
            <div
              key={e.id}
              className="flex items-baseline gap-3 py-1.5 text-[13px]"
            >
              <span className="u-num shrink-0 text-[11px] text-neutral-600">
                {localDateTime(e.created_at)}
              </span>
              <span className="min-w-0 truncate">
                <span className="text-neutral-200">
                  {e.actor_name ??
                    (e.actor_id
                      ? S.kbset.deletedUser
                      : e.action.startsWith("review.")
                        ? S.kbset.adjudicator
                        : S.kbset.engine)}
                </span>{" "}
                <span className="text-neutral-500">
                  {S.kbset.auditActions[e.action] ?? e.action}
                </span>
                {auditDetailName(e) && (
                  <span className="text-neutral-300">
                    {" "}
                    “{auditDetailName(e)}”
                  </span>
                )}
              </span>
            </div>
          ))}
        </div>
      )}
      <Pager total={total} pageSize={AUDIT_PAGE} page={page} onPage={setPage} />
    </div>
  );
}

function KbMembers({ kbId, isOpen }: { kbId: string; isOpen: boolean }) {
  const queryClient = useQueryClient();
  const members = useQuery({
    queryKey: ["kbMembers", kbId],
    queryFn: () => api.kbMembers(kbId),
  });
  const orgUsers = useQuery({ queryKey: ["orgUsers"], queryFn: api.orgUsers });
  const [addUserId, setAddUserId] = useState("");
  // open 库连 viewer 这个选项都没有，默认值得跟着走
  const [addRole, setAddRole] = useState(isOpen ? "editor" : "viewer");

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["kbMembers", kbId] });

  const setMember = useMutation({
    mutationFn: ({ userId, role }: { userId: string; role: string }) =>
      api.setKbMember(kbId, userId, role),
    onSuccess: () => {
      setAddUserId("");
      invalidate();
    },
  });
  const remove = useMutation({
    mutationFn: (userId: string) => api.removeKbMember(kbId, userId),
    onSuccess: invalidate,
  });

  // open 库里 `role=viewer` 的一行**等价于没有这一行**：读权限本来人人都有，
  // 那条记录什么都没授予。所以名单里只留真正拿到写权限的人。
  //
  // **不算进 memberIds 是配套的一半**，不能只藏不放：留在里面的话，
  // 那个人会从添加选择器里消失，于是再也授不了 editor——
  // 一条本该无意义的记录反而把人锁住了
  const listed = (members.data?.members ?? []).filter(
    (m) => !isOpen || m.role !== "viewer",
  );
  const memberIds = new Set(listed.map((m) => m.user_id));
  const addable = orgUsers.data?.filter((u) => !memberIds.has(u.id)) ?? [];

  if (members.isError) return null;

  return (
    <div className="glass rounded-xl p-4">
      <p className="text-xs text-neutral-500 mb-3">
        {isOpen ? S.kbset.membersHintOpen : S.kbset.membersHintRestricted}
      </p>

      {members.data && listed.length === 0 && (
        <p className="text-xs text-neutral-600 mb-3">
          {isOpen ? S.kbset.noWriters : S.kbset.noMembers}
        </p>
      )}
      {listed.map((m) => (
        <div key={m.user_id} className="flex items-center gap-3 py-1.5">
          <div className="min-w-0 flex-1">
            <span className="text-sm text-neutral-200">{m.display_name}</span>
            <span className="ml-2 text-xs text-neutral-500">{m.email}</span>
          </div>
          <Dropdown
            size="sm"
            className="w-24"
            value={m.role}
            onChange={(role) => setMember.mutate({ userId: m.user_id, role })}
            options={rolesFor(isOpen)}
          />
          <button
            onClick={() => remove.mutate(m.user_id)}
            className="text-xs text-neutral-500 hover:text-rose-400"
          >
            {S.kbset.remove}
          </button>
        </div>
      ))}

      {/* **picker 常驻**，不按"有没有人可加"来显示或隐藏。
          一个时有时无的控件比一个空着的控件更让人困惑——不见了的第一反应是
          功能坏了，而不是"没人可加"。空列表由 SearchSelect 自己说
          （它有 noMatches 空态），这里不必再加一句话 */}
      <div className="mt-3 flex gap-2 items-center border-t border-white/5 pt-3">
        <SearchSelect
          className="flex-1"
          value={addUserId}
          onChange={setAddUserId}
          placeholder={S.kbset.addMember}
          options={addable.map((u) => ({
            value: u.id,
            label: u.display_name,
            hint: u.email,
          }))}
        />
        <Dropdown
          className="w-24"
          value={addRole}
          onChange={setAddRole}
          options={rolesFor(isOpen)}
        />
        <button
          className="u-btn u-btn-primary px-3 py-1.5 text-xs"
          disabled={!addUserId || setMember.isPending}
          onClick={() => setMember.mutate({ userId: addUserId, role: addRole })}
        >
          {S.members.add}
        </button>
      </div>
    </div>
  );
}
