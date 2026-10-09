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
  Plus,
  Search,
  Settings2,
  TriangleAlert,
  Users,
} from "lucide-react";
import { api, type AuditEvent } from "../api";
import { LANG_NAMES, S } from "../i18n";
import { toast } from "../toast";
import {
  Button,
  Checkbox,
  DangerConfirm,
  Dropdown,
  Input,
  LinkButton,
  Loading,
  localDateTime,
  Pager,
  RAIL_CLS,
  type RowTone,
  Row,
  SearchSelect,
  Segmented,
  SettingsCard,
  PageHeader,
  Dialog,
  Field,
  Table,
  TBody,
  Td,
  Th,
  THead,
  Tr,
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

type Section = "general" | "members" | "activity" | "danger";

/** 一张设置卡的保存：**只送自己那几项**。PATCH 本来就是部分更新，把整张表
    一起送过去，等于用改名字的那一下覆盖别人刚改的开关。四张卡各自一个
    mutation，于是"在存""存好了"也各是各的。 */
function useKbPatch(kbId: string, onError: (m: string | null) => void) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (patch: Record<string, unknown>) => api.updateKb(kbId, patch),
    onSuccess: () => {
      onError(null);
      queryClient.invalidateQueries({ queryKey: ["kbOne", kbId] });
      queryClient.invalidateQueries({ queryKey: ["kbs"] });
    },
    onError: (e) => onError((e as Error).message),
  });
}

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
  // **默认关**，与上面那个相反：推理往账本里写事实，而声明可能是错的
  const [materialize, setMaterialize] = useState(false);
  // 类型消解自动跑（0016 C2）：只自动落地子树内精化的那一档
  const [autoResolve, setAutoResolve] = useState(false);
  // 治理（0025）：**缺省关**——它会合并实体，还没在哪个库上量过；打开就开始，关掉就停
  const [governance, setGovernance] = useState(false);
  const [inferMins, setInferMins] = useState(60);
  const [ontoLang, setOntoLang] = useState<"en" | "zh">("en");
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (kb.data) {
      setName(kb.data.name);
      setDesc(kb.data.description ?? "");
      setVisibility(kb.data.visibility);
      setMaterialize(kb.data.materialize_inferences);
      setAutoResolve(kb.data.auto_type_resolution);
      setGovernance(kb.data.governance);
      setInferMins(kb.data.inference_interval_minutes);
      setOntoLang(kb.data.ontology_lang);
    }
  }, [kb.data]);

  // 四张卡，四个保存：改名字那一下不该把下面的开关一起送上去
  const saveIdentity = useKbPatch(kbId!, setError);
  const saveVisibility = useKbPatch(kbId!, setError);
  const saveAutomation = useKbPatch(kbId!, setError);
  const saveLang = useKbPatch(kbId!, setError);

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
      <div className="p-8 text-body text-danger">
        {(kb.error as Error).message}
      </div>
    );

  const lbl = "block text-small font-medium text-ink-2 mb-1";

  /* 每张卡自己的"改过没有"：按钮亮不亮说的是这张卡里的东西动没动，
     所以拿本地状态跟服务端最新的那份逐项比，而不是记一个全局的脏位 */
  const identityDirty =
    name.trim() !== kb.data.name ||
    desc.trim() !== (kb.data.description ?? "");
  const visibilityDirty = visibility !== kb.data.visibility;
  const automationDirty =
    materialize !== kb.data.materialize_inferences ||
    autoResolve !== kb.data.auto_type_resolution ||
    governance !== kb.data.governance ||
    inferMins !== kb.data.inference_interval_minutes;
  const langDirty = ontoLang !== kb.data.ontology_lang;

  const isDefault = kb.data.is_default;
  const sections: {
    key: Section;
    label: string;
    Icon: typeof Settings2;
    tone?: RowTone;
  }[] = [
    { key: "general", label: S.kbset.general, Icon: Settings2 },
    { key: "members", label: S.kbset.members, Icon: Users },
    { key: "activity", label: S.kbset.activity, Icon: HistoryIcon },
    // 默认库不可删除：danger 节整个不出现。入口只把图标染成警示色，文字与别的
    // 节一样：这一条只是去往危险区，真正删库的那个按钮才是危险色
    ...(isDefault
      ? []
      : [
          {
            key: "danger" as Section,
            label: S.kbset.danger,
            Icon: TriangleAlert,
            tone: "warn" as const,
          },
        ]),
  ];

  return (
    <div className="h-full flex">
      {/* 分节导航：未来的抽取设置/保留策略/令牌等在此扩展 */}
      <aside className={`${RAIL_CLS} u-rail-list px-2 py-3`}>
        {sections.map(({ key, label, Icon, tone }) => (
          <Row
            key={key}
            density="nav"
            active={section === key}
            icon={<Icon size={14} className={tone === "warn" ? "text-warn" : undefined} />}
            onClick={() => setSection(key)}
          >
            {label}
          </Row>
        ))}
      </aside>

      <main className="flex-1 min-w-0 overflow-y-auto u-scroll px-8 py-6">
        {/* 设置是读一列字段，不是铺一张桌子：内容居中限宽，行长不随窗口拉长 */}
        <div className="mx-auto w-full max-w-4xl">
          {/* 不缀库名：顶栏切换器已标明当前库 */}
          <PageHeader title={S.kbset.title} />

          {section === "general" && (
            /* Vercel 设置页那种排法：一张卡是一个保存单位。从前这一节是一张
               长表加末尾一个保存——改个名字要连着四个开关一起送上去，而
               "存好了"也说不清存的是哪一件 */
            <div className="space-y-4">
              <SettingsCard
                title={S.kbset.cardIdentity}
                note={S.kbset.cardIdentityNote}
                action={
                  <>
                    {saveIdentity.isSuccess && !identityDirty && (
                      <span className="text-small text-ink-2">{S.kbset.saved}</span>
                    )}
                    <Button variant="secondary" size="sm"
                      disabled={!name.trim() || !identityDirty || saveIdentity.isPending}
                      onClick={() =>
                        saveIdentity.mutate({
                          name: name.trim(),
                          // 空串才是"清空描述"：null 在服务端是 COALESCE 的
                          // "这项不改"，把描述删干净会悄悄地什么都没发生
                          description: desc.trim(),
                        })
                      }
                    >
                      {S.kbset.save}
                    </Button>
                  </>
                }
              >
                <div className="space-y-3">
                  <div>
                    <label className={lbl}>{S.settings.kbs.name}</label>
                    <Input className="w-full"
                      value={name}
                      onChange={(e) => setName(e.target.value)}
                    />
                  </div>
                  <div>
                    <label className={lbl}>{S.settings.kbs.description}</label>
                    <Input className="w-full"
                      value={desc}
                      onChange={(e) => setDesc(e.target.value)}
                    />
                  </div>
                </div>
              </SettingsCard>

              {/* 默认库的可见性是锁死的：卡还在（这个库确实有可见性这件事），
                  但底栏没有保存——没有可改的东西就不摆一个按钮 */}
              <SettingsCard
                title={S.settings.kbs.visibility}
                hint={isDefault ? S.kbset.defaultOpenNote : S.kbset.cardVisibilityNote}
                note={isDefault ? undefined : S.kbset.cardVisibilityFoot}
                action={
                  isDefault ? undefined : (
                    <>
                      {saveVisibility.isSuccess && !visibilityDirty && (
                        <span className="text-small text-ink-2">{S.kbset.saved}</span>
                      )}
                      <Button variant="secondary" size="sm"
                        disabled={!visibilityDirty || saveVisibility.isPending}
                        onClick={() => saveVisibility.mutate({ visibility })}
                      >
                        {S.kbset.save}
                      </Button>
                    </>
                  )
                }
              >
                {isDefault ? (
                  <div className="flex w-fit items-center gap-2 rounded-control border border-line px-3 py-2 text-fine text-ink-2">
                    <Lock size={11} className="shrink-0 text-ink-2" />
                    {S.kbset.defaultOpenLabel}
                  </div>
                ) : (
                  <Segmented
                    size="sm"
                    className="w-fit"
                    value={visibility}
                    onChange={setVisibility}
                    options={(
                      [
                        ["open", "Open"],
                        ["restricted", S.settings.kbs.visRestricted],
                      ] as const
                    ).map(([v, label]) => ({ value: v, label, title: label }))}
                  />
                )}
              </SettingsCard>

              <SettingsCard
                title={S.kbset.cardAutomation}
                note={S.kbset.cardAutomationNote}
                action={
                  <>
                    {saveAutomation.isSuccess && !automationDirty && (
                      <span className="text-small text-ink-2">{S.kbset.saved}</span>
                    )}
                    <Button variant="secondary" size="sm"
                      disabled={!automationDirty || saveAutomation.isPending}
                      onClick={() =>
                        saveAutomation.mutate({
                          materialize_inferences: materialize,
                          auto_type_resolution: autoResolve,
                          governance,
                          inference_interval_minutes: inferMins,
                        })
                      }
                    >
                      {S.kbset.save}
                    </Button>
                  </>
                }
              >
                <div className="space-y-3">
                  {/* 物化推理：**默认关**。它动的是账本——按公理往图里写事实，而声明可能是错的 */}
                  <Checkbox
                    checked={materialize}
                    onChange={(v) => setMaterialize(v)}
                    label={S.kbset.materialize}
                    hint={S.kbset.materializeNote}
                  />
                  {/* 重推间隔。**只在开着的时候露出来**——关着时它不影响任何事，
                      摆在那里只会让人以为设了就会推 */}
                  {materialize && (
                    <div className="pl-6 flex flex-wrap items-center gap-2">
                      <label className="text-small text-ink-2">
                        {S.kbset.inferEvery}
                      </label>
                      <Input size="sm" className="w-24 u-num"
                        type="number"
                        min={5}
                        max={10080}
                        value={inferMins}
                        onChange={(e) => setInferMins(Number(e.target.value))}
                      />
                      <span className="text-small text-ink-2">{S.kbset.minutes}</span>
                      {kb.data.last_inference_at && (
                        <span className="text-fine text-ink-2">
                          {S.kbset.lastInference(
                            localDateTime(kb.data.last_inference_at),
                          )}
                        </span>
                      )}
                    </div>
                  )}
                  {/* 类型消解自动跑：抽完排一轮，只自动改子树内精化的那一档，跨轴的仍留给人 */}
                  <Checkbox
                    checked={autoResolve}
                    onChange={(v) => setAutoResolve(v)}
                    label={S.kbset.autoResolveTypes}
                    hint={S.kbset.autoResolveTypesNote}
                  />
                  {/* 治理（0025）：agent 按先进先出过等人的重复对，先读台账里人的先例再裁。
                      说明里要讲清三件事：读的是这个库的人的决定、合并要有先例撑着、
                      关掉队列就停 */}
                  <Checkbox
                    checked={governance}
                    onChange={(v) => setGovernance(v)}
                    label={S.kbset.governance}
                    hint={S.kbset.governanceNote}
                  />
                </div>
              </SettingsCard>

              {/* 语料语言。**不是界面语言**——类描述逐字进抽取提示词，
                  读者是正在读这些文档的模型，所以它跟文档走不跟读者走 */}
              <SettingsCard
                title={S.kbset.ontologyLang}
                hint={S.kbset.ontologyLangNote}
                action={
                  <>
                    {saveLang.isSuccess && !langDirty && (
                      <span className="text-small text-ink-2">{S.kbset.saved}</span>
                    )}
                    <Button variant="secondary" size="sm"
                      disabled={!langDirty || saveLang.isPending}
                      onClick={() => saveLang.mutate({ ontology_lang: ontoLang })}
                    >
                      {S.kbset.save}
                    </Button>
                  </>
                }
              >
                <Segmented
                  size="sm"
                  className="w-fit"
                  value={ontoLang}
                  onChange={setOntoLang}
                  options={(["en", "zh"] as const).map((l) => ({
                    value: l,
                    label: LANG_NAMES[l],
                  }))}
                />
              </SettingsCard>

              {/* 失败的任务（#216）：有才露出来。这张卡没有"保存"——它不是设置，
                  是一个动作：把这个库里全部 failed 放回队列 */}
              {failedJobs.data && failedJobs.data.failed > 0 && (
                <SettingsCard
                  title={S.kbset.cardJobs}
                  note={S.kbset.failedJobs(failedJobs.data.failed)}
                  action={
                    <Button variant="secondary" size="sm"
                      disabled={requeue.isPending}
                      onClick={() => requeue.mutate()}
                    >
                      {S.kbset.requeue}
                    </Button>
                  }
                />
              )}
            </div>
          )}

          {section === "members" && (
            <KbMembers kbId={kbId} isOpen={kb.data.visibility === "open"} />
          )}

          {section === "activity" && <KbActivity kbId={kbId} />}

          {section === "danger" && (
            <div className="glass rounded-panel px-6 py-4 flex items-center justify-between gap-4">
              <div className="min-w-0">
                <div className="text-body font-medium text-ink">
                  {S.kbset.deleteRowTitle}
                </div>
                <div className="mt-1 text-small text-ink-2">
                  {S.kbset.deleteRowHint}
                </div>
              </div>
              <Button variant="secondary" size="sm" className="shrink-0"
                style={{
                  background: "var(--u-danger-solid)",
                  color: "var(--u-on-danger)",
                }}
                onClick={() => setConfirmingDelete(true)}
              >
                {S.kbset.deleteRowBtn}
              </Button>
            </div>
          )}

          {error && <p className="text-body text-danger">{error}</p>}

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
    <div className="space-y-3">
      {/* 筛这份台账的控件在卡外面（DESIGN.md 6）：它们不是台账的内容，
          而且筛空了的时候那张卡要能变成空态，不能把改筛选的唯一办法一起带走 */}
      <div className="flex flex-wrap items-center gap-2">
        <Dropdown
          size="sm"
          className="w-48"
          value={action}
          onChange={(v) => reset(() => setAction(v))}
          options={[
            { value: "", label: S.kbset.auditAllActions },
            ...actions.map((a) => ({ value: a, label: a })),
          ]}
        />
        {/* 日期框**要自己说宽度**：`Input` 不给宽度时是 w-full，一行里放两个，
            每个都占满，于是各自换行——同一行的下拉给了 w-48 才没事。 */}
        <Input size="sm" className="u-num w-36"
          type="date"
          value={since}
          title={S.kbset.auditSince}
          onChange={(e) => reset(() => setSince(e.target.value))}
        />
        <span className="text-small text-ink-2">→</span>
        <Input size="sm" className="u-num w-36"
          type="date"
          value={until}
          title={S.kbset.auditUntil}
          onChange={(e) => reset(() => setUntil(e.target.value))}
        />
        {filtered && (
          <Button variant="secondary" size="sm"
            onClick={() =>
              reset(() => {
                setAction("");
                setSince("");
                setUntil("");
              })
            }
          >
            {S.kbset.auditClear}
          </Button>
        )}
        <span className="ml-auto u-num text-fine text-ink-2">
          {S.kbset.auditTotal(total)}
        </span>
      </div>
      <SettingsCard title={S.kbset.activity} hint={S.kbset.activityHint}>
      {audit.isPending ? (
        <p className="text-small text-ink-2">{S.nav.loading}</p>
      ) : events.length === 0 ? (
        <p className="text-small text-ink-2">{S.kbset.activityEmpty}</p>
      ) : (
        <div className="divide-y divide-line">
          {events.map((e) => (
            <div
              key={e.id}
              className="flex items-baseline gap-3 py-2 text-body first:pt-0"
            >
              <span className="u-num shrink-0 text-fine text-ink-2">
                {localDateTime(e.created_at)}
              </span>
              <span className="min-w-0 truncate">
                <span className="text-ink">
                  {e.actor_name ??
                    (e.actor_id
                      ? S.kbset.deletedUser
                      : e.action.startsWith("review.")
                        ? S.kbset.adjudicator
                        : S.kbset.engine)}
                </span>{" "}
                <span className="text-ink-2">
                  {S.kbset.auditActions[e.action] ?? e.action}
                </span>
                {auditDetailName(e) && (
                  <span className="text-ink-2">
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
      </SettingsCard>
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
  const [adding, setAdding] = useState(false);
  const [filter, setFilter] = useState("");
  const [role, setRole] = useState("all");
  const [editingRole, setEditingRole] = useState<string | null>(null);
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
  const q = filter.trim().toLowerCase();
  const shown = listed
    .filter((m) => role === "all" || m.role === role)
    .filter(
      (m) =>
        !q ||
        m.display_name.toLowerCase().includes(q) ||
        m.email.toLowerCase().includes(q),
    );

  if (members.isError) return null;

  return (
    <div className="space-y-3">
      <p className="text-body text-ink-2">
        {isOpen ? S.kbset.membersHintOpen : S.kbset.membersHintRestricted}
      </p>

      {/* 筛名单的东西与加人的按钮都在表格**外面**（DESIGN.md 6）：它们不是名单
          的内容，而且筛空了的时候那张表要能变成空态，不能把改筛选和加人的
          唯一入口一起带走。这一排与部署那边的用户管理一模一样 */}
      <div className="flex flex-wrap items-center gap-2">
        <Input
          icon={<Search size={13} />}
          className="w-64"
          placeholder={S.settings.searchUsers}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <Dropdown
          className="w-32"
          value={role}
          onChange={setRole}
          options={[
            { value: "all", label: S.members.filterAllRoles },
            ...rolesFor(isOpen),
          ]}
        />
        <div className="ml-auto">
          <Button variant="primary" size="sm" onClick={() => setAdding(true)}>
            <Plus size={12} />
            {S.kbset.addMemberTitle}
          </Button>
        </div>
      </div>

      {/* 一张表，每一列宽度定死：角色和动作都在自己那一列，不随名字长短漂移 */}
      <div className="glass overflow-hidden rounded-panel">
        <Table>
          <THead>
            <Tr>
              <Th>{S.members.userLabel}</Th>
              <Th>{S.members.roleLabel}</Th>
              <Th />
            </Tr>
          </THead>
          <TBody>
            {shown.map((m) => (
              <Tr key={m.user_id}>
                <Td>
                  <div className="truncate text-body text-ink">{m.display_name}</div>
                  <div className="truncate text-small text-ink-2">{m.email}</div>
                </Td>
                <Td>
                  {/* 静态文字，点一下才变成下拉：一列下拉框会把一张只读的名单
                      看成一张待填的表，而改角色是偶尔为之 */}
                  {editingRole === m.user_id ? (
                    <Dropdown
                      size="sm"
                      className="w-24"
                      value={m.role}
                      onChange={(r) => {
                        setEditingRole(null);
                        if (r !== m.role) setMember.mutate({ userId: m.user_id, role: r });
                      }}
                      options={rolesFor(isOpen)}
                    />
                  ) : (
                    <LinkButton onClick={() => setEditingRole(m.user_id)}>
                      {S.kbset.roles[m.role as keyof typeof S.kbset.roles] ?? m.role}
                    </LinkButton>
                  )}
                </Td>
                <Td className="whitespace-nowrap text-right">
                  <LinkButton tone="danger" onClick={() => remove.mutate(m.user_id)}>
                    {S.kbset.remove}
                  </LinkButton>
                </Td>
              </Tr>
            ))}
          </TBody>
        </Table>
        {/* 一个人都没有，与「筛没了」是两回事：前者该去加人，后者该改筛选 */}
        {shown.length === 0 && (
          <p className="px-4 py-6 text-body text-ink-2">
            {listed.length === 0
              ? isOpen
                ? S.kbset.noWriters
                : S.kbset.noMembers
              : S.ui.noMatches}
          </p>
        )}
      </div>

    {/* 把一个已有账号加进这个库。**picker 在弹窗里也仍然常驻**：没人可加时
        它自己会说（SearchSelect 有 noMatches 空态），控件消失读作"坏了" */}
    <Dialog
      open={adding}
      onOpenChange={setAdding}
      title={S.kbset.addMemberTitle}
      closeLabel={S.ui.close}
      footer={
        <>
          <Button variant="secondary" size="sm" onClick={() => setAdding(false)}>
            {S.members.cancel}
          </Button>
          <Button variant="primary" size="sm"
            disabled={!addUserId || setMember.isPending}
            onClick={() => {
              setMember.mutate({ userId: addUserId, role: addRole });
              setAdding(false);
            }}
          >
            {S.members.add}
          </Button>
        </>
      }
    >
      <div className="grid grid-cols-2 gap-3">
        <Field label={S.members.userLabel} className="mb-0">
          <SearchSelect
            className="w-full"
            value={addUserId}
            onChange={setAddUserId}
            placeholder={S.kbset.addMember}
            options={addable.map((u) => ({
              value: u.id,
              label: u.display_name,
              hint: u.email,
            }))}
          />
        </Field>
        <Field label={S.members.roleLabel} className="mb-0">
          <Dropdown
            className="w-full"
            value={addRole}
            onChange={setAddRole}
            options={rolesFor(isOpen)}
          />
        </Field>
      </div>
    </Dialog>
    </div>
  );
}
