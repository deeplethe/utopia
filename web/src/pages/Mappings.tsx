// 数据映射：业务概念在数据库里对应什么、怎么算。
//
// **这一页在此之前不存在**，而它管的东西一直都在：口径由探查任务提出、在
// 「审阅」里被确认，然后沉进问数的 system prompt——人再也看不见它，也改不动。
// `mappings::revise` 连同它的留痕表从建表起就是零调用的。
//
// 审批留在同一个端点上（`review/mappings/{id}`，那里已经在写审计流水），
// 搬的是界面不是逻辑：判断一条口径对不对要看得见表结构，而那在这一页。
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Database, History, Pencil, Plus } from "lucide-react";
import { useNavigate } from "@tanstack/react-router";
import { api, type ConceptMapping } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { toast } from "../toast";
import {
  Button,
  Checkbox,
  Chip,
  type ChipTone,
  cn,
  EmptyState,
  ErrorText,
  Input,
  LinkButton,
  Loading,
  Pager,
  PageTitle,
  SearchSelect,
} from "../ui";

const PAGE = 25;

type StatusFilter = "all" | "proposed" | "confirmed" | "rejected";

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

  if (!kb) return <Loading>{S.nav.loading}</Loading>;

  const counts = data.data?.counts;
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
    <div className="p-6 max-w-4xl mx-auto space-y-4">
      <div>
        <PageTitle>{S.mapping.title}</PageTitle>
        <p className="mt-1 text-small text-ink-3">{S.mapping.hint}</p>
      </div>

      {/* 分段控件用全站那一套（`bg-surface-3` 选中 + 静默的未选中），
          不是 `u-btn-primary`——那是主操作的实心白，用在这里每个标签都像
          一个行动号召 */}
      <div className="flex w-fit rounded-lg overflow-hidden border border-line">
        {(["definitions", "sources"] as const).map((t) => (
          <Button
            variant={tab === t ? "primary" : "ghost"}
            size="sm"
            key={t}
            onClick={() => setTab(t)}
          >
            {t === "definitions"
              ? S.mapping.tabDefinitions
              : S.mapping.tabSources}
          </Button>
        ))}
      </div>

      {tab === "sources" ? (
        <DataSources kbId={kb.id} onExplored={refresh} />
      ) : (
        <>
          <div className="flex items-center gap-2 flex-wrap">
            <div className="flex rounded-lg overflow-hidden border border-line">
              {FILTERS.map((f) => (
                <Button
                  variant={status === f.key ? "primary" : "ghost"}
                  size="sm"
                  key={f.key}
                  onClick={() => {
                    setStatus(f.key);
                    setPage(0);
                  }}
                >
                  {f.label}
                  {f.n != null && (
                    <span
                      className={cn(
                        "u-num",
                        status === f.key
                          ? "text-ink-2"
                          : "text-ink-3",
                      )}
                    >
                      {f.n}
                    </span>
                  )}
                </Button>
              ))}
            </div>
            <Input
              size="sm"
              className="flex-1 min-w-40"
              placeholder={S.mapping.searchPlaceholder}
              value={q}
              onChange={(e) => {
                setQ(e.target.value);
                setPage(0);
              }}
            />
          </div>

          {status === "rejected" && (
            <p className="text-small text-ink-3">{S.mapping.rejectedHint}</p>
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
            <div className="space-y-2">
              {data.data?.items.map((m) => (
                <MappingCard
                  key={m.id}
                  kbId={kb.id}
                  mapping={m}
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
        </>
      )}
    </div>
  );
}

/** 一条口径。**未表态的才给确认/拒绝两个按钮**——已表过态的给「编辑」，
 *  因为改口径和第一次拍板是两件事：前者要留痕（revisions），后者不用。 */
function MappingCard({
  kbId,
  mapping: m,
  onChanged,
}: {
  kbId: string;
  mapping: ConceptMapping;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [showHistory, setShowHistory] = useState(false);

  const decide = useMutation({
    mutationFn: (s: "confirmed" | "rejected") =>
      api.decideMapping(kbId, m.id, s),
    onSuccess: onChanged,
    onError: (e: unknown) => toast.error((e as Error).message),
  });

  const how = howComputed(m);
  return (
    <div className="glass rounded-xl p-3">
      <div className="flex items-baseline gap-2 flex-wrap">
        <span className="text-body text-ink">{m.concept_name}</span>
        <span className="text-fine text-ink-3">{m.source}</span>
        {m.unit && (
          <span className="text-fine text-ink-3">[{m.unit}]</span>
        )}
        {m.derived && (
          <Chip tone="warn" className="text-fine">
            {S.mapping.derivedBadge}
          </Chip>
        )}
        <span className="flex-1" />
        <Chip tone={TONE[m.status]} className="text-fine">
          {statusLabel(m.status)}
        </Chip>
      </div>

      <div
        className={cn(
          "mt-1 u-num text-small break-all",
          how ? "text-ink-2" : "text-ink-3",
        )}
      >
        {how ?? S.mapping.noDefinition}
      </div>
      {m.summary && (
        <p className="mt-1 text-small text-ink-3">{m.summary}</p>
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
        <div className="mt-2 flex items-center gap-2 flex-wrap">
          {m.status === "proposed" && (
            <>
              <Button variant="secondary" size="sm"
                disabled={decide.isPending}
                onClick={() => decide.mutate("rejected")}
              >
                {S.mapping.reject}
              </Button>
              <Button variant="primary" size="sm"
                disabled={decide.isPending}
                onClick={() => decide.mutate("confirmed")}
              >
                {S.mapping.approve}
              </Button>
            </>
          )}
          <Button variant="secondary" size="sm" className="flex items-center gap-1"
            onClick={() => setEditing(true)}
          >
            <Pencil size={11} />
            {S.mapping.edit}
          </Button>
          <Button variant="secondary" size="sm" className="flex items-center gap-1"
            onClick={() => setShowHistory((v) => !v)}
          >
            <History size={11} />
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
      <span className="text-fine text-ink-3">{label}</span>
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
      <p className="text-fine text-ink-3">{S.mapping.editTitle}</p>
      <div className="grid grid-cols-2 gap-2">
        {field(S.mapping.fieldTable, table, setTable, true)}
        {field(S.mapping.fieldUnit, unit, setUnit)}
      </div>
      {field(S.mapping.fieldExpr, expr, setExpr, true)}
      {field(S.mapping.fieldSql, sql, setSql, true)}
      {field(S.mapping.fieldSummary, summary, setSummary)}
      <Checkbox
        checked={derived}
        onChange={(e) => setDerived(e.target.checked)}
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
      <p className="text-fine text-ink-3">{S.mapping.historyHint}</p>
      {list.length === 0 ? (
        <p className="text-small text-ink-3">{S.mapping.historyEmpty}</p>
      ) : (
        list.map((r) => {
          const b = r.before;
          const was = (b.sql ?? b.expr ?? b.table_name) as string | null;
          return (
            <div key={r.id} className="text-small">
              <div className="text-ink-3">
                {S.mapping.historyBy(
                  r.changed_by_name ?? S.mapping.historyUnknown,
                )}
                <span className="u-num ml-2 text-ink-3">
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
      <p className="text-small text-ink-3">{S.mapping.sourcesHint}</p>

      <div className="glass rounded-xl divide-y divide-line">
        {(mounted.data?.data_sources ?? []).map((d) => (
          <div key={d.id} className="px-4 py-3 flex items-center gap-3">
            <div className="min-w-0 flex-1">
              <div className="text-body text-ink">{d.name}</div>
              <div className="text-small text-ink-3 u-num truncate">
                {d.summary}
              </div>
            </div>
            <Button variant="secondary" size="sm" className="shrink-0"
              disabled={sync.isPending}
              onClick={() => sync.mutate(d.id)}
            >
              {S.mapping.syncSchema}
            </Button>
            <LinkButton
              tone="danger"
              className="shrink-0"
              disabled={unmount.isPending}
              onClick={() => unmount.mutate(d.id)}
            >
              {S.mapping.unmount}
            </LinkButton>
          </div>
        ))}
        {!hasMounted && (
          <p className="px-4 py-6 text-body text-ink-3">
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
          <p className="text-small text-ink-3">
            {S.mapping.sourcesNoneAvailable}
          </p>
        )
      )}

      {hasMounted && (
        <div className="glass rounded-xl px-4 py-3 flex items-center gap-3">
          <p className="text-small text-ink-3 flex-1">
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
