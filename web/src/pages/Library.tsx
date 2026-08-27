import { useCallback, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useSearch } from "@tanstack/react-router";
import {
  BookOpen,
  History as HistoryIcon,
  KeyRound,
  RefreshCw,
  Search,
  Settings as SettingsIcon,
  Upload,
  X,
} from "lucide-react";
import { api, type Doc, type SourceView } from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { toast } from "../toast";
import { Chip, type ChipTone, DangerConfirm, Loading, Pager, pageSlice } from "../ui";
import {
  KIND_ICON,
  SOURCE_ICONS,
  sourceIcon,
  SourcesRail,
  SYNC_DOT,
  SYNCING_KINDS,
  type LibrarySelection,
} from "./SourcesRail";

const PAGE_SIZE = 15;

const STATUS_TONE: Record<string, ChipTone> = {
  pending: "neutral",
  parsing: "warn",
  indexing: "warn",
  embedding: "warn",
  ready: "info",
  failed: "danger",
};

const GRAPH_TONE: Record<string, ChipTone> = {
  queued: "neutral",
  extracting: "warn",
  done: "violet",
  failed: "danger",
};

/** 调度器产出的值：interval 与 cron 互斥。 */
interface ScheduleValue {
  sync_interval_minutes: number | null;
  sync_cron: string | null;
}

const pad2 = (n: number | string) => String(n).padStart(2, "0");

/** 同步日程的人话展示（advanced 自定义表达式按原样显示）。 */
function scheduleLabel(s: SourceView): string {
  if (s.sync_cron) {
    const daily = s.sync_cron.match(/^(\d{1,2}) (\d{1,2}) \* \* \*$/);
    if (daily) return S.library.schedule.dailyAt(`${pad2(daily[2])}:${pad2(daily[1])}`);
    const weekly = s.sync_cron.match(/^(\d{1,2}) (\d{1,2}) \* \* ([A-Za-z,-]+)$/);
    if (weekly) return `${weekly[3]} ${pad2(weekly[2])}:${pad2(weekly[1])}`;
    return s.sync_cron;
  }
  if (s.sync_interval_minutes) return S.library.intervalEvery(s.sync_interval_minutes);
  return S.library.intervalManual;
}

type ScheduleMode = "manual" | "interval" | "daily" | "weekly" | "advanced";

/** 已存日程 → 选择器初始状态（编辑模式回显；识别不了的 cron 落到 Advanced）。 */
function scheduleToPickerState(initial?: ScheduleValue) {
  const base = {
    mode: "manual" as ScheduleMode,
    every: 30,
    unit: "minutes" as "minutes" | "hours",
    time: "09:00",
    days: new Set([0]),
    cron: "",
  };
  if (!initial) return base;
  if (initial.sync_cron) {
    const daily = initial.sync_cron.match(/^(\d{1,2}) (\d{1,2}) \* \* \*$/);
    if (daily)
      return { ...base, mode: "daily" as ScheduleMode, time: `${pad2(daily[2])}:${pad2(daily[1])}` };
    const weekly = initial.sync_cron.match(/^(\d{1,2}) (\d{1,2}) \* \* ([A-Za-z,]+)$/);
    if (weekly) {
      const idx = weekly[3]
        .split(",")
        .map((n) => (S.library.schedule.daysShort as readonly string[]).indexOf(n))
        .filter((i) => i >= 0);
      if (idx.length)
        return {
          ...base,
          mode: "weekly" as ScheduleMode,
          time: `${pad2(weekly[2])}:${pad2(weekly[1])}`,
          days: new Set(idx),
        };
    }
    return { ...base, mode: "advanced" as ScheduleMode, cron: initial.sync_cron };
  }
  if (initial.sync_interval_minutes) {
    const m = initial.sync_interval_minutes;
    return m % 60 === 0
      ? { ...base, mode: "interval" as ScheduleMode, every: m / 60, unit: "hours" as const }
      : { ...base, mode: "interval" as ScheduleMode, every: m, unit: "minutes" as const };
  }
  return base;
}

/** 可视化同步日程选择器：Manual / Interval / Daily / Weekly 构建，Advanced 才暴露 cron。 */
function SchedulePicker({
  onChange,
  initial,
}: {
  onChange: (v: ScheduleValue) => void;
  /** 编辑模式的既有日程；新建缺省从 Manual 起步 */
  initial?: ScheduleValue;
}) {
  type Mode = ScheduleMode;
  const [init] = useState(() => scheduleToPickerState(initial));
  const [mode, setMode] = useState<Mode>(init.mode);
  const [every, setEvery] = useState(init.every);
  const [unit, setUnit] = useState<"minutes" | "hours">(init.unit);
  const [time, setTime] = useState(init.time);
  const [days, setDays] = useState<Set<number>>(init.days);
  const [cron, setCron] = useState(init.cron);

  const emit = (
    m: Mode,
    v: { every?: number; unit?: string; time?: string; days?: Set<number>; cron?: string },
  ) => {
    const t = v.time ?? time;
    const [hh, mm] = t.split(":").map(Number);
    // 时间格式未成形时（手输中途）不更新日程，保留上一个有效值
    if ((m === "daily" || m === "weekly") && (Number.isNaN(hh) || Number.isNaN(mm))) return;
    switch (m) {
      case "manual":
        return onChange({ sync_interval_minutes: null, sync_cron: null });
      case "interval": {
        const n = Math.max(1, v.every ?? every);
        const u = v.unit ?? unit;
        return onChange({
          sync_interval_minutes: u === "hours" ? n * 60 : n,
          sync_cron: null,
        });
      }
      case "daily":
        return onChange({ sync_interval_minutes: null, sync_cron: `${mm} ${hh} * * *` });
      case "weekly": {
        const ds = [...(v.days ?? days)].sort();
        const names = ds.map((i) => S.library.schedule.daysShort[i]).join(",");
        return onChange({
          sync_interval_minutes: null,
          sync_cron: ds.length ? `${mm} ${hh} * * ${names}` : null,
        });
      }
      case "advanced":
        return onChange({
          sync_interval_minutes: null,
          sync_cron: (v.cron ?? cron).trim() || null,
        });
    }
  };

  const modes: { key: Mode; label: string }[] = [
    { key: "manual", label: S.library.schedule.manual },
    { key: "interval", label: S.library.schedule.interval },
    { key: "daily", label: S.library.schedule.daily },
    { key: "weekly", label: S.library.schedule.weekly },
    { key: "advanced", label: S.library.schedule.advanced },
  ];

  return (
    <div>
      <div className="flex rounded-lg overflow-hidden border border-white/10 mb-2">
        {modes.map(({ key, label }) => (
          <button
            key={key}
            onClick={() => {
              setMode(key);
              emit(key, {});
            }}
            className={`flex-1 px-2 py-1.5 text-[11px] transition-colors ${
              mode === key
                ? "bg-white/10 text-neutral-100"
                : "text-neutral-500 hover:bg-white/[0.05] hover:text-neutral-300"
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {mode === "interval" && (
        <div className="flex items-center gap-2 text-sm text-neutral-400">
          {S.library.schedule.every}
          <input
            type="number"
            min={1}
            className="input-dark u-input-plain w-16 px-2 py-1.5 text-sm u-num text-center"
            value={every}
            onChange={(e) => {
              const n = Number(e.target.value) || 1;
              setEvery(n);
              emit("interval", { every: n });
            }}
          />
          {/* 两个选项不配下拉：分段切换，与上方模式条同配方 */}
          <div className="flex rounded-lg overflow-hidden border border-white/10">
            {(["minutes", "hours"] as const).map((u) => (
              <button
                key={u}
                onClick={() => {
                  setUnit(u);
                  emit("interval", { unit: u });
                }}
                className={`px-3 py-1.5 text-xs transition-colors ${
                  unit === u
                    ? "bg-white/10 text-neutral-100"
                    : "text-neutral-500 hover:bg-white/[0.05] hover:text-neutral-300"
                }`}
              >
                {S.library.schedule[u]}
              </button>
            ))}
          </div>
        </div>
      )}

      {(mode === "daily" || mode === "weekly") && (
        <div className="space-y-2">
          {mode === "weekly" && (
            <div className="flex gap-1">
              {S.library.schedule.daysShort.map((d, i) => (
                <button
                  key={d}
                  onClick={() => {
                    const next = new Set(days);
                    if (next.has(i)) next.delete(i);
                    else next.add(i);
                    setDays(next);
                    emit("weekly", { days: next });
                  }}
                  className={`flex-1 rounded-lg px-1 py-1.5 text-[11px] transition-colors ${
                    days.has(i)
                      ? "bg-white text-black font-medium"
                      : "bg-white/[0.05] text-neutral-500 hover:text-neutral-300"
                  }`}
                >
                  {d}
                </button>
              ))}
            </div>
          )}
          <div className="flex items-center gap-2 text-sm text-neutral-400">
            {S.library.schedule.at}
            {/* 24h 纯文本输入：原生 time 控件块头大且跟随系统语言（"上午 09:00"） */}
            <input
              className={`input-dark u-input-plain w-[4.2rem] px-2 py-1.5 text-sm u-num text-center ${
                /^([01]?\d|2[0-3]):[0-5]\d$/.test(time) ? "" : "!border-[var(--u-danger)]"
              }`}
              placeholder="09:00"
              value={time}
              onChange={(e) => {
                const t = e.target.value;
                setTime(t);
                if (/^([01]?\d|2[0-3]):[0-5]\d$/.test(t)) emit(mode, { time: t });
              }}
            />
          </div>
        </div>
      )}

      {mode === "advanced" && (
        <div>
          {/* 表达式用 mono，placeholder 回默认字体（u-placeholder-sans） */}
          <input
            className="input-dark w-full px-3 py-2 text-sm font-mono u-placeholder-sans"
            placeholder={S.library.schedule.cronPlaceholder}
            value={cron}
            onChange={(e) => {
              setCron(e.target.value);
              emit("advanced", { cron: e.target.value });
            }}
          />
          <a
            href={S.library.schedule.cronDocsUrl}
            target="_blank"
            rel="noreferrer"
            className="mt-1.5 inline-block text-[10.5px] text-neutral-500 underline decoration-white/20 underline-offset-2 hover:text-neutral-300 transition-colors"
          >
            {S.library.schedule.whatIsCron}
          </a>
        </div>
      )}
    </div>
  );
}

export function Library() {
  const { kb } = useKb();
  const queryClient = useQueryClient();
  const fileInput = useRef<HTMLInputElement>(null);
  // 从文档查看页的来源栏跳回时带 ?src= 定位到对应文件夹
  const { src } = useSearch({ from: "/app/library" });
  const [dragging, setDragging] = useState(false);
  const [selection, setSelection] = useState<LibrarySelection>(src ?? "all");
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState(false);
  // api 来源密钥弹窗（随时可查看/轮换）
  const [tokenReveal, setTokenReveal] = useState<{ sourceId: string } | null>(null);
  const [cleaning, setCleaning] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [page, setPage] = useState(0);
  const [filter, setFilter] = useState("");

  // 切换文件夹回到第一页、清空过滤、退出历史视图
  useEffect(() => {
    setPage(0);
    setFilter("");
    setShowHistory(false);
  }, [selection]);
  // 过滤词变化回到第一页
  useEffect(() => setPage(0), [filter]);

  // 状态变化经 SSE 事件流推送（useKbEvents 挂在 Shell），无需轮询
  const docs = useQuery({
    queryKey: ["documents", kb?.id],
    queryFn: () => api.documents(kb!.id),
    enabled: !!kb,
  });
  const sources = useQuery({
    queryKey: ["sources", kb?.id],
    queryFn: () => api.sources(kb!.id),
    enabled: !!kb,
  });

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["documents", kb?.id] });
    queryClient.invalidateQueries({ queryKey: ["sources", kb?.id] });
  };

  const upload = useMutation({
    // 选中 folder 来源时，上传直接归入该文件夹
    mutationFn: (files: FileList | File[]) => {
      const folder = sources.data?.sources.find(
        (s) => s.id === selection && s.kind === "folder",
      );
      return api.upload(kb!.id, Array.from(files), folder?.id);
    },
    onSuccess: invalidate,
  });
  const remove = useMutation({ mutationFn: (id: string) => api.deleteDocument(id), onSuccess: invalidate });
  const extract = useMutation({ mutationFn: (id: string) => api.extractDocument(id), onSuccess: invalidate });
  const reprocess = useMutation({
    mutationFn: (id: string) => api.reprocessDocument(id),
    onSuccess: invalidate,
  });
  const syncNow = useMutation({
    mutationFn: (sourceId: string) => api.syncSource(kb!.id, sourceId),
    onSuccess: invalidate,
  });
  const removeSource = useMutation({
    mutationFn: (sourceId: string) => api.deleteSource(kb!.id, sourceId),
    onSuccess: () => {
      setSelection("all");
      invalidate();
    },
  });
  const cleanupMissing = useMutation({
    mutationFn: (sourceId: string) => api.cleanupMissing(kb!.id, sourceId),
    onSuccess: () => {
      setCleaning(false);
      invalidate();
    },
  });

  // 只有手动语义的目的地可上传：All/Uploads/folder。拉取型来源（url/rss/api/custom）
  // 由同步填充，手动塞文件会在下次同步时变成"来源里不存在"的孤儿
  const canUpload =
    selection === "all" ||
    selection === "uploads" ||
    sources.data?.sources.find((s) => s.id === selection)?.kind === "folder";

  const onDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      setDragging(false);
      if (!canUpload) return;
      if (e.dataTransfer.files.length && kb) upload.mutate(e.dataTransfer.files);
    },
    [kb, upload, canUpload],
  );

  if (!kb) return <Loading>{S.nav.loading}</Loading>;

  const allDocs = docs.data ?? [];
  const sourceList = sources.data?.sources ?? [];
  const visibleDocs =
    selection === "all"
      ? allDocs
      : selection === "uploads"
        ? allDocs.filter((d) => !d.source_id)
        : allDocs.filter((d) => d.source_id === selection);
  const selectedSource = sourceList.find((s) => s.id === selection);

  const query = filter.trim().toLowerCase();
  const filteredDocs = query
    ? visibleDocs.filter((d) => d.filename.toLowerCase().includes(query))
    : visibleDocs;

  const { rows: pagedDocs, safe: safePage } = pageSlice(filteredDocs, page, PAGE_SIZE);

  return (
    <div className="h-full flex">
      <SourcesRail
        kbId={kb.id}
        active={selection}
        onSelect={setSelection}
        onAdd={() => setAdding(true)}
      />

      {/* 文档区（scrollbar-gutter 常驻：视图切换时滚动条出没不再引起水平抖动） */}
      <div
        className="flex-1 min-w-0 overflow-y-auto u-scroll px-8 py-6 [scrollbar-gutter:stable]"
        onDragOver={(e) => {
          e.preventDefault();
          if (canUpload) setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={onDrop}
      >
        {/* 工作页居左：与 Review/Ontology 同规——左缘随栏起步，切页不跳 */}
        <div className="max-w-4xl">
          <div className="flex items-center justify-between mb-4">
            <h1 className="u-title text-lg">
              {selectedSource?.name ??
                (selection === "uploads" ? S.library.uploads : S.library.title)}
            </h1>
            <div className="flex items-center gap-2">
              {/* 历史视图下过滤框只藏不撤（invisible 保留占位），标题行高度不塌、不抖 */}
              <div className={`relative ${showHistory ? "invisible" : ""}`}>
                <Search
                  size={13}
                  className="absolute left-2.5 top-1/2 -translate-y-1/2 text-neutral-500 pointer-events-none"
                />
                <input
                  className="input-dark w-52 pl-8 pr-7 py-1.5 text-[13px]"
                  placeholder={S.library.filterPlaceholder}
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                  onKeyDown={(e) => e.key === "Escape" && setFilter("")}
                />
                {filter && (
                  <button
                    onClick={() => setFilter("")}
                    className="absolute right-2 top-1/2 -translate-y-1/2 text-neutral-500 hover:text-neutral-200"
                  >
                    <X size={12} />
                  </button>
                )}
              </div>
              {canUpload && (
                <button
                  onClick={() => fileInput.current?.click()}
                  className="u-btn u-btn-ghost px-3 py-1.5 text-xs flex items-center gap-1.5 shrink-0"
                >
                  <Upload size={12} />
                  {S.library.upload}
                </button>
              )}
            </div>
            <input
              ref={fileInput}
              type="file"
              multiple
              hidden
              accept=".pdf,.docx,.xlsx,.xls,.ods,.pptx,.md,.txt,.html,.htm,.csv,.tsv,.json,.yaml,.yml,.xml,.log"
              onChange={(e) => e.target.files?.length && upload.mutate(e.target.files)}
            />
          </div>

          {selectedSource && (
            <SourceBar
              kbId={kb.id}
              source={selectedSource}
              syncing={syncNow.isPending}
              historyOpen={showHistory}
              onToggleHistory={() => setShowHistory((v) => !v)}
              onSync={() => syncNow.mutate(selectedSource.id)}
              onEdit={() => setEditing(true)}
              onCleanup={() => setCleaning(true)}
              onToken={() => setTokenReveal({ sourceId: selectedSource.id })}
            />
          )}

          {upload.isPending && (
            <div className="mb-3 text-sm text-[var(--u-warn)]">{S.library.uploading}</div>
          )}
          {upload.isError && (
            <div className="mb-3 text-sm text-rose-400">
              {S.library.uploadFailed}: {String((upload.error as Error).message)}
            </div>
          )}

          {showHistory && selectedSource ? (
            <RunsPanel kbId={kb.id} sourceId={selectedSource.id} />
          ) : (
          <>
          <div className={`glass rounded-2xl glass-hover ${dragging ? "u-highlight" : ""}`}>
            {filteredDocs.length ? (
              <table className="w-full text-sm">
                <thead>
                  <tr className="text-left text-xs text-neutral-500 border-b border-white/10">
                    <th className="px-4 py-2.5 font-medium">{S.library.colFile}</th>
                    {selection === "all" && (
                      <th className="px-4 py-2.5 font-medium">{S.library.colSource}</th>
                    )}
                    <th className="px-4 py-2.5 font-medium">{S.library.colStatus}</th>
                    <th className="px-4 py-2.5 font-medium">{S.library.colGraph}</th>
                    <th className="px-4 py-2.5 font-medium">{S.library.colChunks}</th>
                    <th className="px-4 py-2.5 font-medium">{S.library.colSize}</th>
                    <th className="px-4 py-2.5"></th>
                  </tr>
                </thead>
                <tbody>
                  {pagedDocs.map((d) => (
                    <DocRow
                      key={d.id}
                      doc={d}
                      source={
                        selection === "all"
                          ? (sourceList.find((s) => s.id === d.source_id) ?? null)
                          : undefined
                      }
                      onDelete={() => remove.mutate(d.id)}
                      onExtract={() => extract.mutate(d.id)}
                      onReprocess={() => reprocess.mutate(d.id)}
                    />
                  ))}
                </tbody>
              </table>
            ) : query && visibleDocs.length ? (
              <div className="py-20 text-center text-sm text-neutral-500">
                {S.library.filterNoMatch}
              </div>
            ) : (
              <div className="py-20 text-center text-sm text-neutral-500">
                {canUpload ? (
                  <>
                    {S.library.dropHint}
                    <div className="mt-2 text-xs text-neutral-600">{S.library.formats}</div>
                  </>
                ) : (
                  S.library.emptyPull
                )}
              </div>
            )}
          </div>

          <Pager
            total={filteredDocs.length}
            pageSize={PAGE_SIZE}
            page={safePage}
            onPage={setPage}
          />
          </>
          )}
        </div>
      </div>

      {adding && (
        <SourceModal
          kbId={kb.id}
          onDone={(id, isApi) => {
            setAdding(false);
            if (id) setSelection(id);
            // api 来源建好直接打开密钥弹窗（onboarding：端点 + 密钥一步拿全）
            if (id && isApi) setTokenReveal({ sourceId: id });
            invalidate();
          }}
        />
      )}
      {tokenReveal && (
        <TokenModal
          kbId={kb.id}
          sourceId={tokenReveal.sourceId}
          onClose={() => setTokenReveal(null)}
        />
      )}
      {editing && selectedSource && (
        <SourceEditModal
          kbId={kb.id}
          source={selectedSource}
          onDone={() => {
            setEditing(false);
            invalidate();
          }}
          onDelete={() => {
            setEditing(false);
            removeSource.mutate(selectedSource.id);
          }}
        />
      )}
      {cleaning && selectedSource && (
        <DangerConfirm
          title={S.library.cleanupTitle}
          hint={S.library.cleanupHint(selectedSource.missing_count, selectedSource.name)}
          confirmLabel={S.library.cleanupConfirm}
          cancelLabel={S.library.cancel}
          busy={cleanupMissing.isPending}
          onConfirm={() => cleanupMissing.mutate(selectedSource.id)}
          onCancel={() => setCleaning(false)}
        />
      )}
    </div>
  );
}

/** 选中来源的状态条：同步状态 / 上次同步 / History 切换 / Sync now / 设置。 */
function SourceBar({
  kbId,
  source,
  syncing,
  historyOpen,
  onToggleHistory,
  onSync,
  onEdit,
  onCleanup,
  onToken,
}: {
  kbId: string;
  source: SourceView;
  syncing: boolean;
  historyOpen: boolean;
  onToggleHistory: () => void;
  onSync: () => void;
  onEdit: () => void;
  onCleanup: () => void;
  onToken: () => void;
}) {
  const isPull = SYNCING_KINDS.has(source.kind);
  const isApi = source.kind === "api";
  const busy = source.last_sync_status === "running" || source.last_sync_status === "queued";
  // 历史数据的 config 可能是 jsonb null（缺省 Value::Null 落库所致）——防御性兜底
  const cfg = source.config ?? {};
  const configSummary =
    cfg.feed_url ?? cfg.endpoint ?? (cfg.urls ? `${cfg.urls.length} URLs` : "");

  return (
    <div className="glass rounded-xl mb-3">
      <div className="px-4 py-2.5 flex items-center gap-3 text-xs">
        {/* api 与拉取型同一状态语汇：点 + 状态 + 时刻 + 产出/错误；
            端点是一次性集成信息，放 Token 弹窗，不占常驻条 */}
        {(isPull || isApi) && (
          <>
            <span
              className={`h-1.5 w-1.5 rounded-full shrink-0 ${SYNC_DOT[source.last_sync_status]}`}
            />
            <span className="text-neutral-300 whitespace-nowrap shrink-0">
              {isApi
                ? S.library.pushStatus[source.last_sync_status]
                : S.library.syncStatus[source.last_sync_status]}
            </span>
            {source.last_sync_at && (
              <span className="text-neutral-600 u-num whitespace-nowrap shrink-0">
                {source.last_sync_at.slice(0, 16).replace("T", " ")}
              </span>
            )}
            {source.last_sync_status === "ok" && source.last_sync_added > 0 && (
              <span className="text-[var(--u-ok)]">
                {S.library.lastSyncAdded(source.last_sync_added)}
              </span>
            )}
            {source.last_sync_error && (
              <span className="text-rose-400 truncate min-w-0" title={source.last_sync_error}>
                {source.last_sync_error}
              </span>
            )}
            {isPull && (
              <>
                <span className="text-neutral-600 truncate min-w-0">{configSummary}</span>
                <span className="text-neutral-600 shrink-0 u-num">{scheduleLabel(source)}</span>
              </>
            )}
          </>
        )}
        {!isPull && !isApi && (
          <span className="text-neutral-600 truncate min-w-0">
            {S.library.sourceKindHints[source.kind as "folder"] ?? ""}
          </span>
        )}
        <div className="ml-auto flex items-center gap-2 shrink-0">
          {/* 集成型来源（custom 拉取 / api 推送）：接口文档随手可达 */}
          {(source.kind === "custom" || source.kind === "api") && (
            <Link
              to="/docs/$slug"
              params={{ slug: "ingest" }}
              target="_blank"
              title={S.library.ingestGuideTitle}
              className="u-btn u-btn-ghost px-2 py-1"
            >
              <BookOpen size={12} />
            </Link>
          )}
          {source.missing_count > 0 && (
            <button
              onClick={onCleanup}
              className="u-btn u-btn-ghost px-2.5 py-1 text-xs !text-[var(--u-warn)]"
            >
              {S.library.cleanupMissing(source.missing_count)}
            </button>
          )}
          {/* History 对拉取型与 api 推送型都开放：推送失败（格式错等）也记 run */}
          {(isPull || source.kind === "api") && (
            /* 激活态用反色（与弹窗类型 tab、图标选中同一语汇），一眼可辨 */
            <button
              onClick={onToggleHistory}
              className={`u-btn px-2.5 py-1 text-xs flex items-center gap-1.5 ${
                historyOpen ? "u-btn-primary" : "u-btn-ghost"
              }`}
            >
              <HistoryIcon size={11} />
              {S.library.syncHistory}
            </button>
          )}
          {isPull && (
            <button
              onClick={onSync}
              disabled={busy || syncing}
              className="u-btn u-btn-ghost px-2.5 py-1 text-xs flex items-center gap-1.5"
            >
              <RefreshCw size={11} className={busy ? "animate-spin" : ""} />
              {S.library.syncNow}
            </button>
          )}
          {source.kind === "api" && (
            <button
              onClick={onToken}
              className="u-btn u-btn-ghost px-2.5 py-1 text-xs flex items-center gap-1.5"
            >
              <KeyRound size={11} />
              {S.library.viewToken}
            </button>
          )}
          <button
            onClick={onEdit}
            title={S.library.sourceSettings}
            className="u-btn u-btn-ghost px-2 py-1"
          >
            <SettingsIcon size={12} />
          </button>
        </div>
      </div>
    </div>
  );
}

/** api 来源密钥：随时可查（Editor 专用端点），内置轮换。 */
function TokenModal({
  kbId,
  sourceId,
  onClose,
}: {
  kbId: string;
  sourceId: string;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const tokenQuery = useQuery({
    queryKey: ["sourceToken", kbId, sourceId],
    queryFn: () => api.sourceToken(kbId, sourceId),
  });
  const rotate = useMutation({
    mutationFn: () => api.rotateSourceToken(kbId, sourceId),
    onSuccess: (r) => {
      queryClient.setQueryData(["sourceToken", kbId, sourceId], { ingest_token: r.ingest_token });
      toast.success(S.toast.saved);
    },
    onError: (e) => toast.error((e as Error).message),
  });
  const token = tokenQuery.data?.ingest_token ?? null;

  const copy = (text: string, msg: string) =>
    navigator.clipboard.writeText(text).then(() => toast.success(msg)).catch(() => {});
  const endpoint = `${location.origin}/api/v1/sources/${sourceId}/ingest`;
  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/60 backdrop-blur-sm"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="glass-strong w-[32rem] max-w-[calc(100vw-2rem)] rounded-2xl shadow-2xl">
        <div className="flex items-center justify-between px-5 pt-4 pb-3 border-b border-white/10">
          <h2 className="u-title text-[15px] flex items-center gap-2">
            <KeyRound size={14} className="text-neutral-400" />
            {S.library.tokenTitle}
          </h2>
          <button onClick={onClose} className="text-neutral-500 hover:text-neutral-200">
            <X size={15} />
          </button>
        </div>
        <div className="px-5 py-4 space-y-3">
          {tokenQuery.isPending ? (
            <p className="text-sm text-neutral-500">{S.nav.loading}</p>
          ) : token ? (
            <>
              <button
                onClick={() => copy(token, S.library.tokenCopied)}
                title={S.library.copyEndpoint}
                className="w-full text-left font-mono text-[12.5px] text-neutral-200 bg-white/[0.05] border border-white/10 hover:border-white/25 rounded-lg px-3 py-2.5 break-all transition-colors"
              >
                {token}
              </button>
              <p className="text-[11px] leading-relaxed text-neutral-500">
                {S.library.tokenWarning}
              </p>
              <div>
                <p className="mb-1 text-[11px] text-neutral-500">{S.library.tokenUsage}</p>
                <button
                  onClick={() => copy(endpoint, S.library.endpointCopied)}
                  title={S.library.copyEndpoint}
                  className="w-full text-left font-mono text-[11.5px] text-neutral-400 hover:text-neutral-200 bg-white/[0.03] border border-white/10 rounded-lg px-3 py-2 break-all transition-colors"
                >
                  POST {endpoint}
                  {"\n"}Authorization: Bearer &lt;token&gt;
                </button>
              </div>
            </>
          ) : (
            <p className="text-sm text-neutral-500">{S.library.noToken}</p>
          )}
        </div>
        <div className="flex justify-end gap-2 px-5 py-3 border-t border-white/10">
          <button
            className="u-btn u-btn-ghost px-3.5 py-1.5 text-xs flex items-center gap-1.5"
            disabled={rotate.isPending || tokenQuery.isPending}
            onClick={() => rotate.mutate()}
          >
            <RefreshCw size={11} className={rotate.isPending ? "animate-spin" : ""} />
            {token ? S.library.rotateToken : S.library.generateToken}
          </button>
          <button className="u-btn u-btn-primary px-3.5 py-1.5 text-xs" onClick={onClose}>
            {S.library.close}
          </button>
        </div>
      </div>
    </div>
  );
}

/** History 视图：占据文件列表位置的同步运行记录（再点 History 切回）。 */
function RunsPanel({ kbId, sourceId }: { kbId: string; sourceId: string }) {
  const runs = useQuery({
    queryKey: ["sourceRuns", kbId, sourceId],
    queryFn: () => api.sourceRuns(kbId, sourceId),
  });
  const list = runs.data?.runs ?? [];

  return (
    <div className="glass rounded-2xl">
      {runs.isLoading ? (
        <div className="py-20 text-center text-sm text-neutral-500">{S.nav.loading}</div>
      ) : list.length === 0 ? (
        <div className="py-20 text-center text-sm text-neutral-500">{S.library.noRuns}</div>
      ) : (
        <div className="px-4 py-2">
          {list.map((r) => (
            <div
              key={r.id}
              className="flex items-center gap-3 py-2 text-xs border-b border-white/5 last:border-0"
            >
              <span
                className={`h-1.5 w-1.5 rounded-full shrink-0 ${
                  r.status === "ok"
                    ? "bg-[var(--u-ok)]"
                    : r.status === "failed"
                      ? "bg-[var(--u-danger)]"
                      : "bg-[var(--u-warn)] animate-pulse"
                }`}
              />
              <span className="u-num text-neutral-400 whitespace-nowrap shrink-0">
                {r.started_at.slice(0, 16).replace("T", " ")}
              </span>
              <span className="text-neutral-500 whitespace-nowrap shrink-0">
                {r.created_docs > 0 && S.library.runNew(r.created_docs)}
                {r.created_docs > 0 && r.updated_docs > 0 && " · "}
                {r.updated_docs > 0 && S.library.runUpdated(r.updated_docs)}
                {r.status === "ok" && r.created_docs === 0 && r.updated_docs === 0 && (
                  <span className="text-neutral-600">{S.library.runNothing}</span>
                )}
              </span>
              {r.error && (
                <span className="text-rose-400 truncate min-w-0" title={r.error}>
                  {r.error}
                </span>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** 新建来源弹窗：类型 → 名称/图标 → 类型专属配置 → 同步周期。 */
function SourceModal({
  kbId,
  onDone,
}: {
  kbId: string;
  /** isApi=true 时父级紧接着打开密钥弹窗 */
  onDone: (id?: string, isApi?: boolean) => void;
}) {
  const [kind, setKind] = useState<"folder" | "url" | "rss" | "custom" | "api">("folder");
  const [name, setName] = useState("");
  const [icon, setIcon] = useState<string | null>(null);
  const [urls, setUrls] = useState("");
  const [feedUrl, setFeedUrl] = useState("");
  const [endpoint, setEndpoint] = useState("");
  const [authHeader, setAuthHeader] = useState("");
  const [schedule, setSchedule] = useState<ScheduleValue>({
    sync_interval_minutes: null,
    sync_cron: null,
  });
  // folder = 纯容器、api = 推送型：都没有同步日程
  const syncing = kind === "url" || kind === "rss" || kind === "custom";

  const create = useMutation({
    mutationFn: () => {
      const config =
        kind === "url"
          ? { urls: urls.split("\n").map((u) => u.trim()).filter(Boolean) }
          : kind === "rss"
            ? { feed_url: feedUrl.trim() }
            : kind === "custom"
              ? {
                  endpoint: endpoint.trim(),
                  ...(authHeader.trim() ? { auth_header: authHeader.trim() } : {}),
                }
              : {};
      return api.createSource(kbId, {
        kind,
        name: name.trim(),
        config,
        // 内置类型图标固定，只有 custom 允许自选
        icon: kind === "custom" ? icon : null,
        // folder/api 无同步语义：日程强制留空
        ...(syncing ? schedule : { sync_interval_minutes: null, sync_cron: null }),
      });
    },
    onSuccess: (data) => onDone(data.source.id, kind === "api"),
  });

  const valid =
    name.trim() &&
    (kind === "url"
      ? urls.trim()
      : kind === "rss"
        ? feedUrl.trim()
        : kind === "custom"
          ? endpoint.trim()
          : true);

  // 注意用 div 而非 label：label 会把 :hover/click 转发给内部第一个可标记控件
  //（button 也算），导致图标网格/日程选择器悬停时第一个按钮常亮
  const field = (label: string, node: React.ReactNode) => (
    <div className="mb-3">
      <div className="mb-1 text-[11px] font-medium text-neutral-500">{label}</div>
      {node}
    </div>
  );

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/60 backdrop-blur-sm"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onDone();
      }}
    >
      <div className="glass-strong w-[30rem] max-w-[calc(100vw-2rem)] max-h-[calc(100vh-4rem)] overflow-y-auto u-scroll rounded-2xl shadow-2xl">
        <div className="flex items-center justify-between px-5 pt-4 pb-3 border-b border-white/10">
          <h2 className="u-title text-[15px]">{S.library.newSourceTitle}</h2>
          <button onClick={() => onDone()} className="text-neutral-500 hover:text-neutral-200">
            <X size={15} />
          </button>
        </div>

        <div className="px-5 py-4">
          {/* 类型 */}
          <div className="flex gap-2 mb-2">
            {(["folder", "url", "rss", "api", "custom"] as const).map((k) => {
              const Icon = KIND_ICON[k];
              return (
                <button
                  key={k}
                  onClick={() => setKind(k)}
                  className={`u-btn flex-1 px-3 py-2 text-xs flex items-center justify-center gap-1.5 ${
                    kind === k ? "u-btn-primary" : "u-btn-ghost"
                  }`}
                >
                  <Icon size={12} />
                  {S.library.sourceKinds[k]}
                </button>
              );
            })}
          </div>
          {/* 类型自解释：一行说明；接口细节移入内置文档，弹窗只留链接 */}
          <p className="mb-4 text-[11px] leading-relaxed text-neutral-500">
            {S.library.sourceKindHints[kind]}
            {kind === "custom" && (
              <>
                {" "}
                <Link
                  to="/docs/$slug"
                  params={{ slug: "ingest" }}
                  target="_blank"
                  className="u-link"
                >
                  {S.library.ingestGuide}
                </Link>
              </>
            )}
          </p>

          {field(
            S.library.sourceName,
            <input
              className="input-dark w-full px-3 py-2 text-sm"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoFocus
            />,
          )}

          {/* 内置类型图标固定；只有 custom 开放图标选择 */}
          {kind === "custom" &&
            field(
              S.library.iconLabel,
              <div className="grid grid-cols-10 gap-1">
                {Object.entries(SOURCE_ICONS).map(([key, Icon]) => (
                  <button
                    key={key}
                    onClick={() => setIcon(icon === key ? null : key)}
                    title={key}
                    className={`h-8 grid place-items-center rounded-lg transition-colors ${
                      icon === key
                        ? "bg-white text-black"
                        : "text-neutral-400 hover:bg-white/[0.07] hover:text-neutral-200"
                    }`}
                  >
                    <Icon size={14} />
                  </button>
                ))}
              </div>,
            )}

          {kind === "custom" && (
            <>
              {field(
                S.library.endpointField,
                <input
                  className="input-dark w-full px-3 py-2 text-sm font-mono"
                  placeholder="https://your-service/utopia-feed"
                  value={endpoint}
                  onChange={(e) => setEndpoint(e.target.value)}
                />,
              )}
              {field(
                S.library.authHeaderField,
                <input
                  className="input-dark w-full px-3 py-2 text-sm font-mono"
                  type="password"
                  placeholder="Bearer sk-…"
                  value={authHeader}
                  onChange={(e) => setAuthHeader(e.target.value)}
                />,
              )}
            </>
          )}

          {kind === "url" &&
            field(
              S.library.urlsField,
              <textarea
                className="input-dark w-full px-3 py-2 text-sm font-mono h-20 resize-y"
                value={urls}
                onChange={(e) => setUrls(e.target.value)}
              />,
            )}
          {kind === "rss" &&
            field(
              S.library.feedUrl,
              <input
                className="input-dark w-full px-3 py-2 text-sm font-mono"
                value={feedUrl}
                onChange={(e) => setFeedUrl(e.target.value)}
              />,
            )}

          {syncing && field(S.library.interval, <SchedulePicker onChange={setSchedule} />)}

          {create.isError && (
            <p className="text-xs text-rose-400 mb-2">{(create.error as Error).message}</p>
          )}
        </div>

        <div className="flex justify-end gap-2 px-5 py-3 border-t border-white/10">
          <button className="u-btn u-btn-ghost px-3.5 py-1.5 text-xs" onClick={() => onDone()}>
            {S.library.cancel}
          </button>
          <button
            className="u-btn u-btn-primary px-3.5 py-1.5 text-xs"
            disabled={!valid || create.isPending}
            onClick={() => create.mutate()}
          >
            {S.library.createSource}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 来源设置弹窗：改名/图标（仅 custom）/摄取配置/日程；类型不可改；底部 danger zone。 */
function SourceEditModal({
  kbId,
  source,
  onDone,
  onDelete,
}: {
  kbId: string;
  source: SourceView;
  onDone: () => void;
  onDelete: () => void;
}) {
  const kind = source.kind;
  const cfg = source.config ?? {};
  const [name, setName] = useState(source.name);
  const [icon, setIcon] = useState<string | null>(source.icon);
  const [urls, setUrls] = useState((cfg.urls ?? []).join("\n"));
  const [feedUrl, setFeedUrl] = useState(cfg.feed_url ?? "");
  const [endpoint, setEndpoint] = useState(cfg.endpoint ?? "");
  const [authHeader, setAuthHeader] = useState("");
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [schedule, setSchedule] = useState<ScheduleValue>({
    sync_interval_minutes: source.sync_interval_minutes,
    sync_cron: source.sync_cron,
  });
  const syncing = SYNCING_KINDS.has(kind);
  const KindIcon = KIND_ICON[kind as keyof typeof KIND_ICON] ?? Upload;

  // 摄取配置是否有改动——有则展示"旧文档去留"说明
  const ingestChanged =
    (kind === "url" && urls.trim() !== (cfg.urls ?? []).join("\n").trim()) ||
    (kind === "rss" && feedUrl.trim() !== (cfg.feed_url ?? "")) ||
    (kind === "custom" && endpoint.trim() !== (cfg.endpoint ?? ""));

  const save = useMutation({
    mutationFn: () => {
      const config =
        kind === "url"
          ? { urls: urls.split("\n").map((u) => u.trim()).filter(Boolean) }
          : kind === "rss"
            ? { feed_url: feedUrl.trim() }
            : kind === "custom"
              ? {
                  endpoint: endpoint.trim(),
                  // 留空 = 后端保留库里原值（凭据只进不出）
                  ...(authHeader.trim() ? { auth_header: authHeader.trim() } : {}),
                }
              : undefined;
      return api.updateSource(kbId, source.id, {
        name: name.trim(),
        ...(kind === "custom" && icon ? { icon } : {}),
        ...(config ? { config } : {}),
        ...(syncing ? { schedule } : {}),
      });
    },
    onSuccess: () => onDone(),
  });

  const valid =
    name.trim() &&
    (kind === "url"
      ? urls.trim()
      : kind === "rss"
        ? feedUrl.trim()
        : kind === "custom"
          ? endpoint.trim()
          : true);

  // div 而非 label：label 会把 :hover/click 转发给第一个可标记控件
  const field = (label: string, node: React.ReactNode) => (
    <div className="mb-3">
      <div className="mb-1 text-[11px] font-medium text-neutral-500">{label}</div>
      {node}
    </div>
  );

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-black/60 backdrop-blur-sm"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onDone();
      }}
    >
      <div className="glass-strong w-[30rem] max-w-[calc(100vw-2rem)] max-h-[calc(100vh-4rem)] overflow-y-auto u-scroll rounded-2xl shadow-2xl">
        <div className="flex items-center justify-between px-5 pt-4 pb-3 border-b border-white/10">
          <h2 className="u-title text-[15px]">{S.library.editSourceTitle}</h2>
          <button onClick={onDone} className="text-neutral-500 hover:text-neutral-200">
            <X size={15} />
          </button>
        </div>

        <div className="px-5 py-4">
          {/* 类型只读：换类型 = 换身份，应新建来源 */}
          <div className="mb-4 flex items-center gap-2 text-xs text-neutral-400">
            <KindIcon size={13} className="text-neutral-500" />
            {S.library.sourceKinds[kind as keyof typeof S.library.sourceKinds] ?? kind}
          </div>

          {field(
            S.library.sourceName,
            <input
              className="input-dark w-full px-3 py-2 text-sm"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />,
          )}

          {kind === "custom" &&
            field(
              S.library.iconLabel,
              <div className="grid grid-cols-10 gap-1">
                {Object.entries(SOURCE_ICONS).map(([key, Icon]) => (
                  <button
                    key={key}
                    onClick={() => setIcon(icon === key ? null : key)}
                    title={key}
                    className={`h-8 grid place-items-center rounded-lg transition-colors ${
                      icon === key
                        ? "bg-white text-black"
                        : "text-neutral-400 hover:bg-white/[0.07] hover:text-neutral-200"
                    }`}
                  >
                    <Icon size={14} />
                  </button>
                ))}
              </div>,
            )}

          {kind === "url" &&
            field(
              S.library.urlsField,
              <textarea
                className="input-dark w-full px-3 py-2 text-sm font-mono h-20 resize-y"
                value={urls}
                onChange={(e) => setUrls(e.target.value)}
              />,
            )}
          {kind === "rss" &&
            field(
              S.library.feedUrl,
              <input
                className="input-dark w-full px-3 py-2 text-sm font-mono"
                value={feedUrl}
                onChange={(e) => setFeedUrl(e.target.value)}
              />,
            )}
          {kind === "custom" && (
            <>
              {field(
                S.library.endpointField,
                <input
                  className="input-dark w-full px-3 py-2 text-sm font-mono"
                  value={endpoint}
                  onChange={(e) => setEndpoint(e.target.value)}
                />,
              )}
              {field(
                S.library.authHeaderEditField,
                <input
                  className="input-dark w-full px-3 py-2 text-sm font-mono"
                  type="password"
                  placeholder={S.library.authKeepHint}
                  value={authHeader}
                  onChange={(e) => setAuthHeader(e.target.value)}
                />,
              )}
            </>
          )}

          {ingestChanged && (
            <p className="mb-3 text-[11px] leading-relaxed text-neutral-500">
              {S.library.editKeepNote}
            </p>
          )}

          {syncing &&
            field(
              S.library.interval,
              <SchedulePicker
                initial={{
                  sync_interval_minutes: source.sync_interval_minutes,
                  sync_cron: source.sync_cron,
                }}
                onChange={setSchedule}
              />,
            )}

          {save.isError && (
            <p className="text-xs text-rose-400 mb-2">{(save.error as Error).message}</p>
          )}

          {/* Danger zone：删除来源（文档保留，落回 Uploads） */}
          <div className="mt-4 pt-3 border-t border-white/10">
            <div className="mb-1 text-[11px] font-medium text-neutral-500">
              {S.library.dangerZone}
            </div>
            <div className="flex items-center justify-between gap-3">
              <span className="text-[11px] text-neutral-600">{S.library.deleteSourceHint}</span>
              <button
                onClick={() => setConfirmingDelete(true)}
                className="u-btn px-3.5 py-1.5 text-xs font-semibold shrink-0"
                style={{ background: "var(--u-danger-solid)", color: "#ffffff" }}
              >
                {S.library.deleteSource}
              </button>
            </div>
          </div>
        </div>

        <div className="flex justify-end gap-2 px-5 py-3 border-t border-white/10">
          <button className="u-btn u-btn-ghost px-3.5 py-1.5 text-xs" onClick={onDone}>
            {S.library.cancel}
          </button>
          <button
            className="u-btn u-btn-primary px-3.5 py-1.5 text-xs"
            disabled={!valid || save.isPending}
            onClick={() => save.mutate()}
          >
            {S.library.saveChanges}
          </button>
        </div>
      </div>

      {confirmingDelete && (
        <DangerConfirm
          title={S.library.deleteSourceTitle}
          hint={S.library.deleteSourceBody(source.name)}
          requireText={source.name}
          confirmLabel={S.library.deleteSource}
          cancelLabel={S.library.cancel}
          onConfirm={onDelete}
          onCancel={() => setConfirmingDelete(false)}
        />
      )}
    </div>
  );
}

function DocRow({
  doc,
  source,
  onDelete,
  onExtract,
  onReprocess,
}: {
  doc: Doc;
  /** undefined = 不渲染来源列；null = Uploads（source_id 为空） */
  source?: SourceView | null;
  onDelete: () => void;
  onExtract: () => void;
  onReprocess: () => void;
}) {
  const statusText =
    S.library.status[doc.status as keyof typeof S.library.status] ?? doc.status;
  const graphText =
    S.library.graphStatus[doc.graph_status as keyof typeof S.library.graphStatus] ??
    doc.graph_status;
  const SrcIcon = source ? sourceIcon(source) : Upload;
  return (
    <tr className="border-b border-white/5 hover:bg-white/[0.03]">
      <td className="px-4 py-2.5 max-w-xs truncate" title={doc.filename}>
        <Link
          to="/doc/$docId"
          params={{ docId: doc.id }}
          search={{}}
          className="text-neutral-200 hover:text-[var(--u-accent)]"
        >
          {doc.filename}
        </Link>
      </td>
      {source !== undefined && (
        <td className="px-4 py-2.5">
          <span className="flex items-center gap-1.5 text-xs text-neutral-400">
            <SrcIcon size={12} className="shrink-0 text-neutral-500" />
            <span className="truncate max-w-28">{source?.name ?? S.library.uploads}</span>
          </span>
        </td>
      )}
      <td className="px-4 py-2.5">
        <Chip tone={STATUS_TONE[doc.status] ?? "neutral"} title={doc.error ?? ""}>
          {statusText}
        </Chip>
        {/* 解析管道失败：重跑 解析→索引→嵌入（解析器升级/瞬时故障重试） */}
        {doc.status === "failed" && (
          <button onClick={onReprocess} className="u-link ml-1.5 text-xs">
            {S.library.reprocess}
          </button>
        )}
        {doc.missing_since && (
          <span className="ml-1.5 inline-block" title={doc.missing_since.slice(0, 16).replace("T", " ")}>
            <Chip tone="neutral">{S.library.notInSource}</Chip>
          </span>
        )}
      </td>
      <td className="px-4 py-2.5">
        {doc.graph_status === "none" ? (
          <span className="text-xs text-neutral-600">{graphText}</span>
        ) : (
          <Chip tone={GRAPH_TONE[doc.graph_status] ?? "neutral"}>{graphText}</Chip>
        )}
        {/* done 也可重抽：本体（描述/新类）调整后强制全量重抽正是常规操作 */}
        {doc.status === "ready" && ["none", "failed", "done"].includes(doc.graph_status) && (
          <button onClick={onExtract} className="u-link ml-1.5 text-xs">
            {doc.graph_status === "done" ? S.library.reExtract : S.library.extract}
          </button>
        )}
      </td>
      <td className="px-4 py-2.5 text-neutral-400">{doc.chunk_count || "—"}</td>
      <td className="px-4 py-2.5 text-neutral-400">{formatSize(doc.size_bytes)}</td>
      <td className="px-4 py-2.5 text-right">
        <button onClick={onDelete} className="text-xs text-neutral-500 hover:text-rose-400">
          {S.library.delete}
        </button>
      </td>
    </tr>
  );
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
