// 数据映射：业务概念在数据库里对应什么、怎么算。
//
// **这一页在此之前不存在**，而它管的东西一直都在：口径由探查任务提出、在
// 「审阅」里被确认，然后沉进问数的 system prompt——人再也看不见它，也改不动。
// `mappings::revise` 连同它的留痕表从建表起就是零调用的。
//
// 审批留在同一个端点上（`review/mappings/{id}`，那里已经在写审计流水），
// 搬的是界面不是逻辑：判断一条口径对不对要看得见表结构，而那在这一页。
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Database, Plug, Plus, Search } from "lucide-react";
import { useNavigate } from "@tanstack/react-router";
import { api, type ConceptMapping } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { toast } from "../toast";
import {
  Button,
  Checkbox,
  Chip,
  Dropdown,
  EmptyState,
  ErrorText,
  Input,
  Loading,
  PageHeader,
  Pager,
  RAIL_CLS,
  Row,
  SearchSelect,
  Status,
  cn,
  type ChipTone,
} from "../ui";

const PAGE = 25;

type StatusFilter = "all" | "proposed" | "confirmed" | "rejected";
type MappingDecision = Extract<
  ConceptMapping["status"],
  "confirmed" | "rejected"
>;
const NO_PENDING_MAPPING_IDS: ReadonlySet<string> = new Set();

export const decideMappings = async (
  ids: string[],
  decide: (id: string) => Promise<unknown>,
) => {
  const outcomes = await Promise.allSettled(ids.map((id) => decide(id)));
  const failure = outcomes.find((outcome) => outcome.status === "rejected");
  if (failure?.status === "rejected") throw failure.reason;
};

export const selectedMappingIds = (
  mappings: Pick<ConceptMapping, "id" | "status">[],
  picked: ReadonlySet<string>,
  pending: ReadonlySet<string> = NO_PENDING_MAPPING_IDS,
) =>
  mappings
    .filter(
      (mapping) =>
        mapping.status === "proposed" &&
        picked.has(mapping.id) &&
        !pending.has(mapping.id),
    )
    .map((mapping) => mapping.id);

const TONE: Record<string, ChipTone> = {
  proposed: "warn",
  confirmed: "success",
  rejected: "neutral",
};

/** 「这个数怎么算」。SQL / 表达式 / 表名按这个优先级取一个——
 *  三者都在回答同一个问题，而 SQL 最具体、表名最粗 */
const howComputed = (m: ConceptMapping) =>
  m.sql ?? m.expr ?? m.table_name ?? null;

const statusLabel = (s: string) =>
  s === "proposed"
    ? S.mapping.filterProposed
    : s === "confirmed"
      ? S.mapping.filterConfirmed
      : S.mapping.filterRejected;

export function Mappings() {
  const { kb } = useKb();
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<"definitions" | "sources">("definitions");
  const [status, setStatus] = useState<StatusFilter>("all");
  const [q, setQ] = useState("");
  const [page, setPage] = useState(0);
  const [picked, setPicked] = useState<Set<string>>(() => new Set());
  const [deciding, setDeciding] = useState<Set<string>>(() => new Set());
  useEffect(() => setPicked(new Set()), [kb?.id, page, q, status, tab]);

  const data = useQuery({
    queryKey: ["mappings", kb?.id, status, q, page],
    queryFn: () =>
      api.mappings(kb!.id, {
        status: status === "all" ? undefined : status,
        q: q || undefined,
        limit: PAGE,
        offset: page * PAGE,
      }),
    enabled: !!kb,
  });

  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: ["mappings", kb?.id] });

  const batch = useMutation({
    mutationFn: ({ ids, status }: { ids: string[]; status: MappingDecision }) =>
      decideMappings(ids, (id) => api.decideMapping(kb!.id, id, status)),
    onSuccess: () => setPicked(new Set()),
    onError: (e: unknown) => toast.error((e as Error).message),
    onSettled: refresh,
  });

  if (!kb) return <Loading>{S.nav.loading}</Loading>;

  const counts = data.data?.counts;
  const selectable =
    data.data?.items.filter(
      (m) => m.status === "proposed" && !deciding.has(m.id),
    ) ?? [];
  const selectedIds = selectedMappingIds(
    data.data?.items ?? [],
    picked,
    deciding,
  );
  const individualPending = deciding.size > 0;
  const FILTERS: { key: StatusFilter; label: string; n?: number }[] = [
    { key: "all", label: S.mapping.filterAll },
    { key: "proposed", label: S.mapping.filterProposed, n: counts?.proposed },
    {
      key: "confirmed",
      label: S.mapping.filterConfirmed,
      n: counts?.confirmed,
    },
    { key: "rejected", label: S.mapping.filterRejected, n: counts?.rejected },
  ];

  return (
    /* **左栏切功能，不是标签页**：定义与数据源是两件不同的事（一个是判读，
       一个是登记连接），全站凡是这种切换都在左栏（文库、审阅、本体都是）。
       从前这里是一排按钮做的标签，选中那个还是实心主按钮——在这套语汇里
       实心说的是"这一屏最该按的那一下"，四个标签四个行动号召。
       内容区也从设置页那套居中限宽（max-w-4xl）改成内容页的铺满（px-8 py-6）。 */
    <div className="flex h-full">
      <aside className={`${RAIL_CLS} u-rail-list px-2 py-3`}>
        {(["definitions", "sources"] as const).map((t) => (
          <Row
            key={t}
            density="nav"
            active={tab === t}
            icon={t === "definitions" ? <Database size={14} /> : <Plug size={14} />}
            onClick={() => setTab(t)}
          >
            {t === "definitions" ? S.mapping.tabDefinitions : S.mapping.tabSources}
          </Row>
        ))}
      </aside>

      <div className="u-scroll flex-1 min-w-0 overflow-y-auto px-8 py-6">
        <PageHeader
          title={S.mapping.title}
          sub={S.mapping.hint}
          className="mb-4"
        />

      {tab === "sources" ? (
        <DataSources kbId={kb.id} onExplored={refresh} />
      ) : (
        <div className="space-y-4">
          {/* 筛选是下拉，不是一排按钮：四档状态是"挑一个看"，与全站其它页面的
              筛选同一副控件（Dropdown），计数跟在标签里 */}
          <div className="flex items-center gap-2 flex-wrap">
            {/* 筛选条与 Members、文库、图谱同一副身材：**带放大镜的 w-64 输入框在前**，
                下拉跟在后面。这里从前是下拉在前、输入框 flex-1——一个人从一页走到
                另一页，同样的一条工具行，控件换了顺序、搜索框还横跨整屏。 */}
            <Input
              icon={<Search size={13} />}
              className="w-64"
              placeholder={S.mapping.searchPlaceholder}
              value={q}
              onChange={(e) => {
                setQ(e.target.value);
                setPage(0);
              }}
            />
            <Dropdown
              className="w-40"
              value={status}
              onChange={(v) => {
                setStatus(v as StatusFilter);
                setPage(0);
              }}
              /* 计数写进括号里，不是用间隔点挂在后面。「Pending · 0」读起来是
                 两样并列的东西（这一页别处的 `·` 正是这个用法：口径 · 来源 ·
                 表），而这里的 0 不是第二个字段，是「Pending 有几条」——
                 括号说的就是这个从属关系。 */
              options={FILTERS.map((f) => ({
                value: f.key,
                label: f.n != null ? `${f.label} (${f.n})` : f.label,
              }))}
            />
            {/* **一条工具行**：筛选、搜索、全选、批量动作排在一起。
                从前它们各占一行，三行控件压在内容上面，读到列表要先翻过一块 */}
            {selectable.length > 0 && (
              <Checkbox
                checked={selectable.every((m) => picked.has(m.id))}
                disabled={batch.isPending || individualPending}
                onChange={(v) =>
                  setPicked(
                    v
                      ? new Set(selectable.map((m) => m.id))
                      : new Set(),
                  )
                }
                label={S.mapping.selectPage}
                className="shrink-0"
              />
            )}
            {selectedIds.length > 0 && (
              <>
                  <span className="u-num text-small text-ink-2">
                    {S.mapping.selected(selectedIds.length)}
                  </span>
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={batch.isPending || individualPending}
                    onClick={() =>
                      batch.mutate({
                        ids: selectedIds,
                        status: "confirmed",
                      })
                    }
                  >
                    {S.mapping.approve}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="text-danger"
                    disabled={batch.isPending || individualPending}
                    onClick={() =>
                      batch.mutate({
                        ids: selectedIds,
                        status: "rejected",
                      })
                    }
                  >
                    {S.mapping.reject}
                  </Button>
              </>
            )}
          </div>

          {status === "rejected" && (
            <p className="text-small text-ink-2">{S.mapping.rejectedHint}</p>
          )}
          {/* 最近一轮探索的账（#503）。**单看列表答不了「漏了多少」**——十二条
              提议对着八十列的宽表与刚好覆盖完一个小库长得一样。这条贴出来人才能
              从「等量提议」里看出覆盖范围。filtered out of view 反而更糟：搜索框
              在搜索的时候人最容易忘上一次跑了多深 */}
          {data.data?.last_run ? (
            <p className="text-small text-ink-2">
              {S.mapping.lastRun({
                tables: data.data.last_run.tables_scanned,
                columns: data.data.last_run.columns_scanned,
                returned: data.data.last_run.returned,
                accepted: data.data.last_run.accepted,
                truncated: data.data.last_run.schema_truncated,
              })}
            </p>
          ) : (
            <p className="text-small text-ink-2">{S.mapping.lastRunMissing}</p>
          )}

          {data.isPending ? (
            <Loading>{S.nav.loading}</Loading>
          ) : data.data && data.data.items.length === 0 ? (
            <div className="py-12">
              <EmptyState icon={<Database size={20} />}>
                {q || status !== "all"
                  ? S.mapping.emptyFiltered
                  : S.mapping.empty}
              </EmptyState>
            </div>
          ) : (
            /* 一个面板装多行，不是一行一张卡片（DESIGN.md 6）：这一页是
               一队待表态的口径，同构的一组，跟文库、检索、成员一副样子 */
            <div className="glass rounded-panel divide-y divide-line">
              {data.data?.items.map((m) => (
                <MappingRow
                  key={m.id}
                  kbId={kb.id}
                  mapping={m}
                  picked={selectedIds.includes(m.id)}
                  batchPending={batch.isPending}
                  onDecisionPending={(pending) =>
                    setDeciding((prev) => {
                      const next = new Set(prev);
                      if (pending) next.add(m.id);
                      else next.delete(m.id);
                      return next;
                    })
                  }
                  onPick={(on) =>
                    setPicked((prev) => {
                      const next = new Set(prev);
                      if (on) next.add(m.id);
                      else next.delete(m.id);
                      return next;
                    })
                  }
                  onChanged={refresh}
                />
              ))}
            </div>
          )}
          <Pager
            total={data.data?.total ?? 0}
            pageSize={PAGE}
            page={page}
            onPage={setPage}
          />
        </div>
      )}
      </div>
    </div>
  );
}

/** 一条口径。**未表态的才给确认/拒绝两个按钮**——已表过态的给「编辑」，
 *  因为改口径和第一次拍板是两件事：前者要留痕（revisions），后者不用。 */
function MappingRow({
  kbId,
  mapping: m,
  picked,
  batchPending,
  onDecisionPending,
  onPick,
  onChanged,
}: {
  kbId: string;
  mapping: ConceptMapping;
  picked: boolean;
  batchPending: boolean;
  onDecisionPending: (pending: boolean) => void;
  onPick: (picked: boolean) => void;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [showHistory, setShowHistory] = useState(false);

  const decide = useMutation({
    mutationFn: (s: "confirmed" | "rejected") =>
      api.decideMapping(kbId, m.id, s),
    onMutate: () => onDecisionPending(true),
    onSuccess: onChanged,
    onError: (e: unknown) => toast.error((e as Error).message),
    onSettled: () => onDecisionPending(false),
  });

  const how = howComputed(m);
  return (
    <div className={cn("px-4 py-3", picked && "u-picked")}>
      <div className="flex items-baseline gap-2 flex-wrap">
        {m.status === "proposed" && (
          <Checkbox
            className="shrink-0 self-center"
            checked={picked}
            disabled={decide.isPending || batchPending}
            onChange={(v) => onPick(v)}
            label={
              <span className="sr-only">
                {S.mapping.selectMapping(m.concept_name, m.source)}
              </span>
            }
          />
        )}
        <span className="text-body text-ink">{m.concept_name}</span>
        <span className="text-fine text-ink-2">{m.source}</span>
        {m.unit && (
          <span className="text-fine text-ink-2">[{m.unit}]</span>
        )}
        {m.derived && (
          <Chip tone="warn" className="text-fine">
            {S.mapping.derivedBadge}
          </Chip>
        )}
        <span className="flex-1" />
        <Status tone={TONE[m.status]} className="text-fine">
          {statusLabel(m.status)}
        </Status>
      </div>

      <div
        className={cn(
          "mt-1 u-num text-small break-all",
          how ? "text-ink-2" : "text-ink-2",
        )}
      >
        {how ?? S.mapping.noDefinition}
      </div>
      {m.summary && (
        <p className="mt-1 text-small text-ink-2">{m.summary}</p>
      )}

      {editing ? (
        <EditForm
          kbId={kbId}
          mapping={m}
          onDone={() => {
            setEditing(false);
            onChanged();
          }}
          onCancel={() => setEditing(false)}
        />
      ) : (
        /* 行里的动作：**拍板那一下描边，其余是 ghost，都不是实底。**十一行
           十一个实底按钮等于把一整页染成动作，而一屏最多一个 primary。
           但「不是实底」不等于「长成一句话」——从前这三个是 `LinkButton`：
           没有内距、没有 hover 底、颜色就是正文那档灰，跟它们上面那行说明
           一个样，看着不像能点。ghost 补回控件该有的两件事：一圈内距，
           和指针停上去的那块底。静止时仍然轻。
           拒绝**静止时就该是红的**：它是点了就生效的那一下，人要在按之前
           看出来它跟「编辑」不是一类，而不是划过去才发现。 */
        <div className="mt-2 -ml-2 flex items-center gap-1 flex-wrap">
          {m.status === "proposed" && (
            <>
              <Button variant="secondary" size="sm" className="ml-2"
                disabled={decide.isPending || batchPending || picked}
                onClick={() => decide.mutate("confirmed")}
              >
                {S.mapping.approve}
              </Button>
              <Button variant="ghost" size="sm" className="text-danger"
                disabled={batchPending || picked}
                onClick={() =>
                  !picked && !decide.isPending && decide.mutate("rejected")
                }
              >
                {S.mapping.reject}
              </Button>
            </>
          )}
          <Button variant="ghost" size="sm" onClick={() => setEditing(true)}>
            {S.mapping.edit}
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setShowHistory((v) => !v)}>
            {S.mapping.history}
          </Button>
        </div>
      )}

      {showHistory && <RevisionList kbId={kbId} mappingId={m.id} />}
    </div>
  );
}

function EditForm({
  kbId,
  mapping: m,
  onDone,
  onCancel,
}: {
  kbId: string;
  mapping: ConceptMapping;
  onDone: () => void;
  onCancel: () => void;
}) {
  const [table, setTable] = useState(m.table_name ?? "");
  const [expr, setExpr] = useState(m.expr ?? "");
  const [sql, setSql] = useState(m.sql ?? "");
  const [unit, setUnit] = useState(m.unit ?? "");
  const [summary, setSummary] = useState(m.summary ?? "");
  const [derived, setDerived] = useState(m.derived);

  const save = useMutation({
    mutationFn: () =>
      api.reviseMapping(kbId, m.id, {
        table_name: table,
        expr,
        sql,
        unit,
        summary,
        derived,
      }),
    onSuccess: onDone,
    onError: (e: unknown) => toast.error((e as Error).message),
  });

  const nothing = !table.trim() && !expr.trim() && !sql.trim();
  const field = (
    label: string,
    value: string,
    set: (v: string) => void,
    mono = false,
  ) => (
    <label className="block">
      <span className="text-fine text-ink-2">{label}</span>
      <Input
        size="sm"
        className={cn("mt-1 w-full", mono && "u-num")}
        value={value}
        onChange={(e) => set(e.target.value)}
      />
    </label>
  );

  return (
    <div className="mt-3 space-y-2 border-t border-line pt-3">
      <p className="text-fine text-ink-2">{S.mapping.editTitle}</p>
      <div className="grid grid-cols-2 gap-2">
        {field(S.mapping.fieldTable, table, setTable, true)}
        {field(S.mapping.fieldUnit, unit, setUnit)}
      </div>
      {field(S.mapping.fieldExpr, expr, setExpr, true)}
      {field(S.mapping.fieldSql, sql, setSql, true)}
      {field(S.mapping.fieldSummary, summary, setSummary)}
      <Checkbox
        checked={derived}
        onChange={(v) => setDerived(v)}
        label={S.mapping.fieldDerived}
      />
      <span className="hidden">
      </span>
      {nothing && (
        <p className="text-small text-danger">{S.mapping.needOne}</p>
      )}
      <div className="flex gap-2">
        <Button variant="secondary" size="sm"
          onClick={onCancel}
        >
          {S.mapping.cancel}
        </Button>
        <Button variant="primary" size="sm"
          disabled={nothing || save.isPending}
          onClick={() => save.mutate()}
        >
          {S.mapping.save}
        </Button>
      </div>
    </div>
  );
}

/** 改版历史。**留痕表从建表起就没人读过**——0006 说留它是为了答得出
 *  「上季度这个数是怎么算的」，这里是那句话的兑现处。 */
function RevisionList({
  kbId,
  mappingId,
}: {
  kbId: string;
  mappingId: string;
}) {
  const revs = useQuery({
    queryKey: ["mappingRevisions", kbId, mappingId],
    queryFn: () => api.mappingRevisions(kbId, mappingId),
  });

  if (revs.isPending) return <Loading>{S.nav.loading}</Loading>;
  // **取失败要说取失败。** `?? []` 会把一次 500 画成「还没改过」——
  // 一条改过口径的记录被说成从没改过，比报错难查得多
  if (revs.isError)
    return (
      <div className="mt-3 border-t border-line pt-3">
        <ErrorText>{(revs.error as Error).message}</ErrorText>
      </div>
    );
  const list = revs.data?.revisions ?? [];
  return (
    <div className="mt-3 border-t border-line pt-3 space-y-2">
      <p className="text-fine text-ink-2">{S.mapping.historyHint}</p>
      {list.length === 0 ? (
        <p className="text-small text-ink-2">{S.mapping.historyEmpty}</p>
      ) : (
        list.map((r) => {
          const b = r.before;
          const was = (b.sql ?? b.expr ?? b.table_name) as string | null;
          return (
            <div key={r.id} className="text-small">
              <div className="text-ink-2">
                {S.mapping.historyBy(
                  r.changed_by_name ?? S.mapping.historyUnknown,
                )}
                <span className="u-num ml-2 text-ink-2">
                  {r.changed_at.slice(0, 16).replace("T", " ")}
                </span>
              </div>
              <div className="u-num text-ink-2 break-all">
                {was ?? S.mapping.noDefinition}
              </div>
            </div>
          );
        })
      )}
    </div>
  );
}

/** 知识库层的数据源挂载。**从 KB 设置搬来的**——挂哪个库和口径怎么定，
 *  是同一件事的两半：不知道有哪些表，就判断不了口径对不对。
 *  注册新连接仍是部署级动作，管理员给直达入口，其他人指路找管理员。 */
function DataSources({
  kbId,
  onExplored,
}: {
  kbId: string;
  onExplored: () => void;
}) {
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
  // 与 notice 分开：一个是「成了」，一个是「成了一半」，配色也不同
  const [warning, setWarning] = useState<string | null>(null);
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["kbDataSources", kbId] });

  const mount = useMutation({
    mutationFn: (dsId: string) => api.mountDataSource(kbId, dsId),
    // **挂载成了、schema 没成，是两件事。** 服务端此时回的是 ok（源确实挂上了），
    // 所以不能照着 `schema_tables: 0` 说「已摄入 0 张表」——那等于说成功了。
    // 说清楚半成的是哪一半，同一件事也进了告警中心
    onSuccess: (r) => {
      setPicked("");
      setNotice(null);
      setWarning(r.schema_error ? S.mapping.schemaFailed : null);
      if (!r.schema_error) setNotice(S.mapping.schemaSynced(r.schema_tables));
      invalidate();
    },
    onError: (e: unknown) => toast.error((e as Error).message),
  });
  const unmount = useMutation({
    mutationFn: (dsId: string) => api.unmountDataSource(kbId, dsId),
    onSettled: invalidate,
  });
  const sync = useMutation({
    mutationFn: (dsId: string) => api.syncDataSourceSchema(kbId, dsId),
    onSuccess: (r) => setNotice(S.mapping.schemaSynced(r.schema_tables)),
    onError: (e: unknown) => toast.error((e as Error).message),
  });
  const explore = useMutation({
    mutationFn: () => api.exploreMappings(kbId),
    onSuccess: () => {
      setNotice(S.mapping.exploreQueued);
      onExplored();
    },
    onError: (e: unknown) => toast.error((e as Error).message),
  });

  const mountedIds = new Set(
    (mounted.data?.data_sources ?? []).map((d) => d.id),
  );
  const mountable = (available.data?.data_sources ?? []).filter(
    (d) => !mountedIds.has(d.id),
  );
  const hasMounted = (mounted.data?.data_sources.length ?? 0) > 0;

  return (
    <div className="space-y-4">
      <p className="text-small text-ink-2">{S.mapping.sourcesHint}</p>

      <div className="glass rounded-panel divide-y divide-line">
        {(mounted.data?.data_sources ?? []).map((d) => (
          <div key={d.id} className="px-4 py-3 flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <div className="text-body text-ink">{d.name}</div>
              <div className="text-small text-ink-2 u-num truncate">
                {d.summary}
              </div>
            </div>
            <Button variant="secondary" size="sm" className="shrink-0"
              disabled={sync.isPending}
              onClick={() => sync.mutate(d.id)}
            >
              {S.mapping.syncSchema}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="shrink-0 text-danger"
              disabled={unmount.isPending}
              onClick={() => unmount.mutate(d.id)}
            >
              {S.mapping.unmount}
            </Button>
          </div>
        ))}
        {!hasMounted && (
          <p className="px-4 py-6 text-body text-ink-2">
            {S.mapping.sourcesEmpty}
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
            placeholder={S.mapping.mount + "…"}
          />
          <Button variant="primary" size="sm" className="shrink-0"
            disabled={!picked || mount.isPending}
            onClick={() => mount.mutate(picked)}
          >
            {S.mapping.mount}
          </Button>
        </div>
      )}

      {me.data?.is_admin ? (
        <Button
          variant="ghost"
          size="sm"
          className="-ml-2"
          onClick={() =>
            navigate({ to: "/admin", search: { tab: "datasources" } })
          }
        >
          <Plus size={12} />
          {S.mapping.newConn}
        </Button>
      ) : (
        mountable.length === 0 &&
        available.data &&
        !hasMounted && (
          <p className="text-small text-ink-2">
            {S.mapping.sourcesNoneAvailable}
          </p>
        )
      )}

      {hasMounted && (
        <div className="glass rounded-panel px-4 py-3 flex items-center gap-3">
          <p className="text-small text-ink-2 flex-1">
            {S.mapping.exploreHint}
          </p>
          <Button variant="secondary" size="sm" className="shrink-0"
            disabled={explore.isPending}
            onClick={() => explore.mutate()}
          >
            {S.mapping.explore}
          </Button>
        </div>
      )}
      {notice && <p className="text-small text-ok">{notice}</p>}
      {warning && <p className="text-small text-warn">{warning}</p>}
    </div>
  );
}
