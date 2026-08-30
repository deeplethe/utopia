/* 知识库设置：左栏分节（General / Members / Danger zone），为未来设置项立骨架
   （抽取设置、保留策略、库级令牌…）。访问控制由 API 端执行（库 admin 起步）。
   分节互斥渲染也根治了下拉弹层被后续玻璃卡（backdrop-filter 自成 stacking
   context）遮蔽的层级 bug。 */
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import {
  Database,
  History as HistoryIcon,
  Lock,
  Plus,
  Settings2,
  TriangleAlert,
  Users,
} from "lucide-react";
import { api, type AuditEvent } from "../api";
import { LANG_NAMES, S } from "../i18n";
import { useKb } from "../kb";
import {
  DangerConfirm,
  Dropdown,
  Loading,
  RAIL_CLS,
  SearchSelect,
} from "../ui";

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

type Section = "general" | "members" | "data" | "activity" | "danger";

export function KbSettings() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { kb: currentKb } = useKb();
  const { kb: kbParam } = useSearch({ from: "/app/kb-settings" });
  const kbId = kbParam ?? currentKb?.id;

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
      navigate({ to: "/library", search: {} });
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
    { key: "data", label: S.kbset.data, Icon: Database },
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
          {section === "data" && <KbDataSources kbId={kbId} />}

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

function KbActivity({ kbId }: { kbId: string }) {
  const audit = useQuery({
    queryKey: ["kbAudit", kbId],
    queryFn: () => api.kbAudit(kbId),
  });
  const events = audit.data?.events ?? [];

  return (
    <div className="glass rounded-xl p-4">
      <p className="text-xs text-neutral-500 mb-3">{S.kbset.activityHint}</p>
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
                {e.created_at.slice(0, 16).replace("T", " ")}
              </span>
              <span className="min-w-0 truncate">
                <span className="text-neutral-200">
                  {e.actor_name ?? S.kbset.deletedUser}
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
    </div>
  );
}

/** 知识库层数据源挂载：从系统已注册的源里挑选;挂载即摄取 schema。
    注册新连接是部署级动作：管理员从这里深链去 Deployment settings 的 Data sources。 */
function KbDataSources({ kbId }: { kbId: string }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const me = useQuery({ queryKey: ["me"], queryFn: api.me });
  const mounted = useQuery({
    queryKey: ["kbDataSources", kbId],
    queryFn: () => api.kbDataSources(kbId),
  });
  const available = useQuery({
    queryKey: ["kbDataSourcesAvail", kbId],
    queryFn: () => api.kbDataSourcesAvailable(kbId),
  });
  const [picked, setPicked] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["kbDataSources", kbId] });

  const mount = useMutation({
    mutationFn: (dsId: string) => api.mountDataSource(kbId, dsId),
    onSuccess: (r) => {
      setPicked("");
      setNotice(S.kbset.dataSchemaSynced(r.schema_tables));
      invalidate();
    },
  });
  const unmount = useMutation({
    mutationFn: (dsId: string) => api.unmountDataSource(kbId, dsId),
    onSettled: invalidate,
  });
  const sync = useMutation({
    mutationFn: (dsId: string) => api.syncDataSourceSchema(kbId, dsId),
    onSuccess: (r) => setNotice(S.kbset.dataSchemaSynced(r.schema_tables)),
  });
  const explore = useMutation({
    mutationFn: () => api.exploreMappings(kbId),
    onSuccess: () => setNotice(S.kbset.dataExploreQueued),
  });

  const mountedIds = new Set(
    (mounted.data?.data_sources ?? []).map((d) => d.id),
  );
  const mountable = (available.data?.data_sources ?? []).filter(
    (d) => !mountedIds.has(d.id),
  );

  return (
    <div className="space-y-4">
      <p className="text-xs text-neutral-500">{S.kbset.dataHint}</p>

      <div className="glass rounded-xl divide-y divide-white/5">
        {(mounted.data?.data_sources ?? []).map((d) => (
          <div key={d.id} className="px-4 py-3 flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <div className="text-sm text-neutral-200">{d.name}</div>
              <div className="text-xs text-neutral-500 font-mono truncate">
                {d.summary}
              </div>
            </div>
            <button
              className="u-btn u-btn-ghost px-2.5 py-1 text-xs shrink-0"
              disabled={sync.isPending}
              onClick={() => sync.mutate(d.id)}
            >
              {S.kbset.dataSyncSchema}
            </button>
            <button
              className="text-xs text-neutral-500 hover:text-[var(--u-danger)] shrink-0"
              disabled={unmount.isPending}
              onClick={() => unmount.mutate(d.id)}
            >
              {S.kbset.dataUnmount}
            </button>
          </div>
        ))}
        {mounted.data?.data_sources.length === 0 && (
          <p className="px-4 py-6 text-sm text-neutral-500">
            {S.kbset.dataNone}
          </p>
        )}
      </div>

      {mountable.length > 0 && (
        <div className="flex items-center gap-2">
          <SearchSelect
            className="flex-1"
            value={picked}
            options={mountable.map((d) => ({
              value: d.id,
              label: d.name,
              hint: d.summary,
            }))}
            onChange={setPicked}
            placeholder={S.kbset.dataMount + "…"}
          />
          <button
            className="u-btn u-btn-primary px-3.5 py-1.5 text-xs shrink-0"
            disabled={!picked || mount.isPending}
            onClick={() => mount.mutate(picked)}
          >
            {S.kbset.dataMount}
          </button>
        </div>
      )}
      {/* 连接注册在部署层：管理员给直达入口，其他人指路找管理员 */}
      {me.data?.is_admin ? (
        <button
          className="flex items-center gap-1.5 text-xs text-neutral-500 hover:text-neutral-300 transition-colors"
          onClick={() =>
            navigate({ to: "/admin", search: { tab: "datasources" } })
          }
        >
          <Plus size={12} />
          {S.kbset.dataNewConn}
        </button>
      ) : (
        mountable.length === 0 &&
        available.data &&
        (mounted.data?.data_sources.length ?? 0) === 0 && (
          <p className="text-xs text-neutral-600">
            {S.kbset.dataNoneAvailable}
          </p>
        )
      )}
      {(mounted.data?.data_sources.length ?? 0) > 0 && (
        <div className="glass rounded-xl px-4 py-3 flex items-center gap-3">
          <p className="text-xs text-neutral-500 flex-1">
            {S.kbset.dataExploreHint}
          </p>
          <button
            className="u-btn u-btn-ghost px-2.5 py-1 text-xs shrink-0"
            disabled={explore.isPending}
            onClick={() => explore.mutate()}
          >
            {S.kbset.dataExplore}
          </button>
        </div>
      )}
      {notice && <p className="text-xs text-[var(--u-ok)]">{notice}</p>}
      {(mount.isError || sync.isError || explore.isError) && (
        <p className="text-xs text-rose-400">
          {((mount.error ?? sync.error ?? explore.error) as Error).message}
        </p>
      )}
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
            onClick={() =>
              setMember.mutate({ userId: addUserId, role: addRole })
            }
          >
            {S.members.add}
          </button>
      </div>
    </div>
  );
}
