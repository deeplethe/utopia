import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
  Waypoints,
  X,
} from "lucide-react";
import { api, type Doc, type ExtractionDrop, type SourceView } from "../api";
import { S } from "../i18n";
import {
  CREATABLE_SOURCE_KINDS,
  type CreatableSourceKind,
} from "../sourceKinds";
import { useKb, useKbId } from "../kb";
import { toast } from "../toast";
import {
  Button,
  Checkbox,
  Chip,
  type ChipTone,
  cn,
  DangerConfirm,
  Dialog,
  IconButton,
  Input,
  LinkButton,
  Loading,
  NativeSelect,
  Pager,
  Segmented,
  Textarea,
} from "../ui";
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
      <Segmented
        fill
        size="sm"
        className="mb-2"
        value={mode}
        onChange={(k) => {
          setMode(k);
          emit(k, {});
        }}
        options={modes.map(({ key, label }) => ({ value: key, label }))}
      />

      {mode === "interval" && (
        <div className="flex items-center gap-2 text-body text-ink-2">
          {S.library.schedule.every}
          <Input size="sm" className="u-input-plain w-16 u-num text-center"
            type="number"
            min={1}
            value={every}
            onChange={(e) => {
              const n = Number(e.target.value) || 1;
              setEvery(n);
              emit("interval", { every: n });
            }}
          />
          {/* 两个选项不配下拉：分段切换，与上方模式条同配方 */}
          <Segmented
            size="sm"
            value={unit}
            onChange={(u) => {
              setUnit(u);
              emit("interval", { unit: u });
            }}
            options={(["minutes", "hours"] as const).map((u) => ({
              value: u,
              label: S.library.schedule[u],
            }))}
          />
        </div>
      )}

      {(mode === "daily" || mode === "weekly") && (
        <div className="space-y-2">
          {mode === "weekly" && (
            <div className="flex gap-1">
              {S.library.schedule.daysShort.map((d, i) => (
                <Button
                  variant={days.has(i) ? "primary" : "secondary"}
                  size="sm"
                  key={d}
                  className="flex-1"
                  onClick={() => {
                    const next = new Set(days);
                    if (next.has(i)) next.delete(i);
                    else next.add(i);
                    setDays(next);
                    emit("weekly", { days: next });
                  }}
                >
                  {d}
                </Button>
              ))}
            </div>
          )}
          <div className="flex items-center gap-2 text-body text-ink-2">
            {S.library.schedule.at}
            {/* 24h 纯文本输入：原生 time 控件块头大且跟随系统语言（"上午 09:00"） */}
            <Input
              size="sm"
              className={cn(
                "u-input-plain u-num w-20 text-center",
                !/^([01]?\d|2[0-3]):[0-5]\d$/.test(time) && "!border-danger",
              )}
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
          <Input className="w-full font-mono u-placeholder-sans"
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
            className="u-link mt-2 inline-block text-fine"
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
  const { src } = useSearch({ from: "/app/kb/$kbId/library" });
  const [dragging, setDragging] = useState(false);
  const [selection, setSelection] = useState<LibrarySelection>(src ?? "all");
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState(false);
  // api 来源密钥弹窗（随时可查看/轮换）
  const [tokenReveal, setTokenReveal] = useState<{ sourceId: string } | null>(null);
  const [cleaning, setCleaning] = useState(false);
  const [reExtracting, setReExtracting] = useState(false);
  const [rebuilding, setRebuilding] = useState(false);
  // 失败详情弹窗：{文件名, 哪条管道, 原文}
  const [errorView, setErrorView] = useState<{
    file: string;
    kind: string;
    text: string;
  } | null>(null);
  // 丢弃详情弹窗：这篇文档抽出来却没落地的事实
  const [dropsView, setDropsView] = useState<{ file: string; rows: ExtractionDrop[] } | null>(
    null,
  );
  const [showHistory, setShowHistory] = useState(false);
  const [page, setPage] = useState(0);
  const [filter, setFilter] = useState("");
  // 按抽取状态筛。**空 = 不筛**——五篇失败混在二十七篇里，从前只能一页页看
  const [graphFilter, setGraphFilter] = useState("");


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
    // **服务端筛选与分页**：作用域、名字、抽取状态、页码都进 queryKey，
    // 换任何一个都重新取一页。从前是一次取回整库、客户端切片
    queryKey: ["documents", kb?.id, selection, filter, graphFilter, page],
    queryFn: () =>
      api.documents(kb!.id, {
        source:
          selection === "all" || selection === "deleted"
            ? undefined
            : selection === "uploads"
              ? "none"
              : selection,
        state: selection === "deleted" ? "deleted" : undefined,
        q: filter.trim() || undefined,
        graph: graphFilter || undefined,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      }),
    enabled: !!kb,
    placeholderData: (prev) => prev,
  });
  const sources = useQuery({
    queryKey: ["sources", kb?.id],
    queryFn: () => api.sources(kb!.id),
    enabled: !!kb,
  });
  // 整库一次取回后按文档分组：抽取丢弃的行数很小，好过每行发一个请求
  const drops = useQuery({
    queryKey: ["extraction-drops", kb?.id],
    queryFn: () => api.extractionDrops(kb!.id),
    enabled: !!kb,
  });
  const dropsByDoc = useMemo(() => {
    const m = new Map<string, ExtractionDrop[]>();
    for (const d of drops.data?.drops ?? []) {
      const list = m.get(d.document_id);
      if (list) list.push(d);
      else m.set(d.document_id, [d]);
    }
    return m;
  }, [drops.data]);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["documents", kb?.id] });
    queryClient.invalidateQueries({ queryKey: ["docCount", kb?.id] });
    queryClient.invalidateQueries({ queryKey: ["sources", kb?.id] });
    // 重抽会重写这篇文档的丢弃信号，跟着文档一起失效
    queryClient.invalidateQueries({ queryKey: ["extraction-drops", kb?.id] });
  };

  // 一键重试这个作用域里全部失败的。**逐篇入队在服务端做**——排队还带着
  // 别的动作（解雇在跑的任务、清增量标记），绕过去会留下半截状态
  const retryFailed = useMutation({
    mutationFn: () =>
      api.retryFailedDocs(
        kb!.id,
        selection === "all"
          ? undefined
          : selection === "uploads"
            ? "none"
            : selection,
      ),
    onSuccess: (r) => {
      toast.success(S.library.retryQueued(r.queued));
      invalidate();
    },
    onError: (e: Error) => toast.error(e.message),
  });

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
  // 删除是墓碑（#268）：提示里给一个「撤销」，点了原路复活
  const restore = useMutation({
    mutationFn: (id: string) => api.restoreDocument(id),
    onSuccess: () => {
      toast.success(S.library.restored);
      invalidate();
    },
    onError: (e) => toast.error(String(e)),
  });
  const remove = useMutation({
    mutationFn: (id: string) => api.deleteDocument(id),
    onSuccess: (r, id) => {
      toast.info(S.library.deletedWithFacts(r.invalidated_facts), {
        label: S.library.undo,
        onClick: () => restore.mutate(id),
      });
      invalidate();
    },
  });
  // 真删（#268 下半）：打字级确认，库管理员，回不来
  const [purging, setPurging] = useState<Doc | null>(null);
  const purge = useMutation({
    mutationFn: (id: string) => api.purgeDocument(id),
    onSuccess: () => {
      setPurging(null);
      toast.success(S.library.purged);
      invalidate();
    },
    onError: (e) => toast.error(String(e)),
  });
  const extract = useMutation({ mutationFn: (id: string) => api.extractDocument(id), onSuccess: invalidate });
  const reprocess = useMutation({
    mutationFn: (id: string) => api.reprocessDocument(id),
    onSuccess: invalidate,
  });
  // 本库角色：门控"重建图谱"入口（KB admin 起步，与 API 端一致）
  const kbDetail = useQuery({
    queryKey: ["kbOne", kb?.id],
    queryFn: () => api.kbDetail(kb!.id),
    enabled: !!kb,
  });
  const myRole = kbDetail.data?.my_role ?? "";
  const canEdit = ["editor", "admin", "owner"].includes(myRole);
  const canRebuild = ["admin", "owner"].includes(myRole);

  const reExtractSource = useMutation({
    mutationFn: (sourceId: string) => api.reExtractSource(kb!.id, sourceId),
    onSuccess: (r) => {
      toast.success(S.library.queuedDocs(r.queued));
      setReExtracting(false);
      invalidate();
    },
    onError: (e) => toast.error((e as Error).message),
  });
  const rebuildGraph = useMutation({
    mutationFn: () => api.rebuildGraph(kb!.id),
    onSuccess: (r) => {
      toast.success(S.library.rebuildDone(r.entities_removed, r.facts_removed, r.queued));
      setRebuilding(false);
      invalidate();
    },
    onError: (e) => toast.error((e as Error).message),
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

  // 整库可抽的篇数。**重建是库级动作**，不该显示当前来源的数字
  const kbStats = useQuery({
    queryKey: ["docCount", kb?.id],
    queryFn: () => api.documents(kb!.id, { limit: 1, offset: 0 }),
    enabled: !!kb,
  });

  // **最后一个 hook 之后才能提前返回。** 这一行从前排在 kbStats 前面：整页
  // 刷新或直接打开链接时第一次渲染 kb 还没到，提前返回跳过了后面的 hook，
  // 下一次渲染多出一个 hook，React 直接抛 "Rendered more hooks"。站内导航
  // 时 kb 已在缓存里，所以只有深链和刷新会撞上
  if (!kb) return <Loading>{S.nav.loading}</Loading>;

  // 服务端已经切好了这一页：筛选、作用域、页码都在请求里
  const pagedDocs = docs.data?.docs ?? [];
  const totalDocs = docs.data?.total ?? 0;
  const kbReady = kbStats.data?.ready ?? 0;
  const sourceList = sources.data?.sources ?? [];
  const selectedSource = sourceList.find((s) => s.id === selection);
  const safePage = page;
  const query = filter.trim().toLowerCase();

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
            <h1 className="u-title text-title">
              {selectedSource?.name ??
                (selection === "uploads"
                  ? S.library.uploads
                  : selection === "deleted"
                    ? S.library.deleted
                    : S.library.title)}
            </h1>
            <div className="flex items-center gap-2">
              {/* 历史视图下过滤框只藏不撤（invisible 保留占位），标题行高度不塌、不抖 */}
              <div className={`relative ${showHistory ? "invisible" : ""}`}>
                <Search
                  size={13}
                  className="absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-3 pointer-events-none"
                />
                <Input size="sm" className="w-52 pl-8 pr-8"
                  placeholder={S.library.filterPlaceholder}
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                  onKeyDown={(e) => e.key === "Escape" && setFilter("")}
                />
                {filter && (
                  <IconButton size="sm" label={S.library.close} className="absolute right-2 top-1/2 -translate-y-1/2"
                    onClick={() => setFilter("")}
                  >
                    <X size={12} />
                  </IconButton>
                )}
              </div>
              {/* 按抽取状态筛。**选项写死成那五个**，不按库里实际有的填——
                  它是一组固定的管道状态，而「这个库现在没有失败的」正是用户
                  想通过筛一下确认的事 */}
              {selection !== "deleted" && (
              <NativeSelect size="sm" className="shrink-0"
                value={graphFilter}
                onChange={(e) => {
                  setGraphFilter(e.target.value);
                  setPage(0);
                }}
              >
                <option value="">{S.library.anyStatus}</option>
                <option value="failed">{S.library.statusFailed}</option>
                <option value="done">{S.library.statusDone}</option>
                <option value="queued">{S.library.statusQueued}</option>
                <option value="extracting">{S.library.statusExtracting}</option>
                <option value="none">{S.library.statusNone}</option>
              </NativeSelect>
              )}
              {/* 一键重试。**只在真有失败时出现**——没有失败的库不该看到一个
                  点了什么都不会发生的按钮。数字写在按钮上，点之前就知道会动几篇 */}
              {(docs.data?.failed ?? 0) > 0 && canUpload && (
                <Button variant="secondary" size="sm" className="flex items-center gap-2 shrink-0"
                  onClick={() => retryFailed.mutate()}
                  disabled={retryFailed.isPending}
                >
                  <RefreshCw size={12} />
                  {S.library.retryFailed(docs.data!.failed)}
                </Button>
              )}
              {/* 全库重建：清算语义，仅 KB admin；放在 All documents 视图。
                  **按钮本身是普通色**：它只是打开确认框，危险色留给确认框里那个
                  真正动手的按钮——工具栏上常驻一块红，看久了就不红了 */}
              {selection === "all" && canRebuild && (
                <Button
                  variant="secondary"
                  size="sm"
                  className="shrink-0"
                  icon={<RefreshCw size={12} />}
                  onClick={() => setRebuilding(true)}
                >
                  {S.library.rebuild}
                </Button>
              )}
              {canUpload && (
                <Button variant="secondary" size="sm" className="flex items-center gap-2 shrink-0"
                  onClick={() => fileInput.current?.click()}
                >
                  <Upload size={12} />
                  {S.library.upload}
                </Button>
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
              onReExtract={canEdit ? () => setReExtracting(true) : undefined}
              onToken={() => setTokenReveal({ sourceId: selectedSource.id })}
            />
          )}

          {/* 抽取进度。**数来自服务端**，按来源作用域算——从前是数当前页里的，
              翻一页进度条就跳 */}
          {(() => {
            const pending = docs.data?.extracting ?? 0;
            if (pending === 0) return null;
            const total = (docs.data?.ready ?? 0) + pending;
            const done = total - pending;
            return (
              <div className="mb-3 glass rounded-xl px-4 py-3">
                <div className="flex items-center justify-between text-small text-ink-2 mb-2">
                  <span>{S.library.extractProgress(done, total)}</span>
                  <span className="u-num text-ink-3">
                    {Math.round((done / Math.max(total, 1)) * 100)}%
                  </span>
                </div>
                <div className="h-1 rounded-full bg-surface-2 overflow-hidden">
                  <div
                    className="u-progress-fill h-full bg-warn"
                    style={{ width: `${(done / Math.max(total, 1)) * 100}%` }}
                  />
                </div>
              </div>
            );
          })()}

          {upload.isPending && (
            <div className="mb-3 text-body text-warn">{S.library.uploading}</div>
          )}
          {upload.isError && (
            <div className="mb-3 text-body text-danger">
              {S.library.uploadFailed}: {String((upload.error as Error).message)}
            </div>
          )}

          {showHistory && selectedSource ? (
            <RunsPanel kbId={kb.id} sourceId={selectedSource.id} />
          ) : (
          <>
          <div className={`glass rounded-xl glass-hover ${dragging ? "u-highlight" : ""}`}>
            {selection === "deleted" ? (
              <DeletedTable
                docs={pagedDocs}
                sources={sourceList}
                canPurge={canRebuild}
                onRestore={(id) => restore.mutate(id)}
                onPurge={setPurging}
              />
            ) : pagedDocs.length ? (
              <table className="w-full text-body">
                <thead>
                  <tr className="text-left text-small text-ink-3 border-b border-line">
                    <th className="px-4 py-3 font-medium">{S.library.colFile}</th>
                    {selection === "all" && (
                      <th className="px-4 py-3 font-medium">{S.library.colSource}</th>
                    )}
                    <th className="px-4 py-3 font-medium">{S.library.colStatus}</th>
                    <th className="px-4 py-3 font-medium">{S.library.colGraph}</th>
                    <th className="px-4 py-3 font-medium">{S.library.colChunks}</th>
                    <th className="px-4 py-3 font-medium">{S.library.colSize}</th>
                    <th className="px-4 py-3"></th>
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
                      onShowError={(kind, text) =>
                        setErrorView({ file: d.filename, kind, text })
                      }
                      drops={dropsByDoc.get(d.id)}
                      onShowDrops={(rows) => setDropsView({ file: d.filename, rows })}
                    />
                  ))}
                </tbody>
              </table>
            ) : query || graphFilter ? (
              <div className="py-20 text-center text-body text-ink-3">
                {S.library.filterNoMatch}
              </div>
            ) : (
              <div className="py-20 text-center text-body text-ink-3">
                {canUpload ? (
                  <>
                    {S.library.dropHint}
                    <div className="mt-2 text-small text-ink-3">{S.library.formats}</div>
                  </>
                ) : (
                  S.library.emptyPull
                )}
              </div>
            )}
          </div>

          <Pager
            total={totalDocs}
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
      {/* 来源重抽：轻确认（不毁数据，只是费时费钱），无需打字解锁 */}
      {reExtracting && selectedSource && (
        <DangerConfirm
          title={S.library.reExtractTitle}
          hint={S.library.reExtractHint(
            docs.data?.ready ?? 0,
            selectedSource.name,
          )}
          confirmLabel={S.library.reExtractConfirm}
          cancelLabel={S.library.cancel}
          busy={reExtractSource.isPending}
          onConfirm={() => reExtractSource.mutate(selectedSource.id)}
          onCancel={() => setReExtracting(false)}
        />
      )}
      {errorView && (
        <ErrorModal
          file={errorView.file}
          kind={errorView.kind}
          text={errorView.text}
          onClose={() => setErrorView(null)}
        />
      )}
      {dropsView && (
        <DropsModal
          file={dropsView.file}
          rows={dropsView.rows}
          onClose={() => setDropsView(null)}
        />
      )}
      {/* 真删：打字级确认（内容没了，回不来） */}
      {purging && (
        <DangerConfirm
          title={S.library.purgeTitle}
          hint={S.library.purgeHint(purging.filename)}
          requireText={purging.filename}
          confirmLabel={S.library.purgeConfirm}
          cancelLabel={S.library.cancel}
          busy={purge.isPending}
          onConfirm={() => purge.mutate(purging.id)}
          onCancel={() => setPurging(null)}
        />
      )}
      {/* 全库重建：打字级确认（清空图层不可逆） */}
      {rebuilding && (
        <DangerConfirm
          title={S.library.rebuildTitle}
          hint={S.library.rebuildHint(
            kbReady,
            kb.name,
          )}
          requireText={kb.name}
          confirmLabel={S.library.rebuildConfirm}
          cancelLabel={S.library.cancel}
          busy={rebuildGraph.isPending}
          onConfirm={() => rebuildGraph.mutate()}
          onCancel={() => setRebuilding(false)}
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
  onReExtract,
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
  /** 缺省 = 无编辑权限，不渲染重抽入口 */
  onReExtract?: () => void;
  onToken: () => void;
}) {
  const isPull = SYNCING_KINDS.has(source.kind);
  const isApi = source.kind === "api";
  const busy = source.last_sync_status === "running" || source.last_sync_status === "queued";
  // 历史数据的 config 可能是 jsonb null（缺省 Value::Null 落库所致）——防御性兜底
  const cfg = source.config ?? {};
  const configSummary =
    cfg.feed_url ??
    cfg.endpoint ??
    cfg.repo ??
    (cfg.urls ? `${cfg.urls.length} URLs` : "");

  return (
    <div className="glass rounded-xl mb-3">
      <div className="px-4 py-3 flex items-center gap-3 text-small">
        {/* api 与拉取型同一状态语汇：点 + 状态 + 时刻 + 产出/错误；
            端点是一次性集成信息，放 Token 弹窗，不占常驻条 */}
        {(isPull || isApi) && (
          <>
            <span
              className={`h-1.5 w-1.5 rounded-full shrink-0 ${SYNC_DOT[source.last_sync_status]}`}
            />
            <span className="text-ink-2 whitespace-nowrap shrink-0">
              {isApi
                ? S.library.pushStatus[source.last_sync_status]
                : S.library.syncStatus[source.last_sync_status]}
            </span>
            {source.last_sync_at && (
              <span className="text-ink-3 u-num whitespace-nowrap shrink-0">
                {source.last_sync_at.slice(0, 16).replace("T", " ")}
              </span>
            )}
            {source.last_sync_status === "ok" && source.last_sync_added > 0 && (
              <span className="text-ok">
                {S.library.lastSyncAdded(source.last_sync_added)}
              </span>
            )}
            {source.last_sync_error && (
              <span className="text-danger truncate min-w-0" title={source.last_sync_error}>
                {source.last_sync_error}
              </span>
            )}
            {isPull && (
              <>
                <span className="text-ink-3 truncate min-w-0">{configSummary}</span>
                <span className="text-ink-3 shrink-0 u-num">{scheduleLabel(source)}</span>
              </>
            )}
          </>
        )}
        {!isPull && !isApi && (
          <span className="text-ink-3 truncate min-w-0">
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
            <Button variant="secondary" size="sm"
              onClick={onCleanup}
            >
              {S.library.cleanupMissing(source.missing_count)}
            </Button>
          )}
          {/* History 对拉取型与 api 推送型都开放：推送失败（格式错等）也记 run */}
          {(isPull || source.kind === "api") && (
            /* 激活态用反色（与弹窗类型 tab、图标选中同一语汇），一眼可辨 */
            <Button variant="secondary" size="sm"
              onClick={onToggleHistory}
              className={`u-btn px-3 py-1 text-small flex items-center gap-2 ${
                historyOpen ? "u-btn-primary" : "u-btn-ghost"
              }`}
            >
              <HistoryIcon size={11} />
              {S.library.syncHistory}
            </Button>
          )}
          {isPull && (
            <Button variant="secondary" size="sm" className="flex items-center gap-2"
              onClick={onSync}
              disabled={busy || syncing}
            >
              <RefreshCw size={11} className={busy ? "animate-spin" : ""} />
              {S.library.syncNow}
            </Button>
          )}
          {source.kind === "api" && (
            <Button variant="secondary" size="sm" className="flex items-center gap-2"
              onClick={onToken}
            >
              <KeyRound size={11} />
              {S.library.viewToken}
            </Button>
          )}
          {/* 全量重抽本来源：所有类型都给（有文档就能重抽） */}
          {onReExtract && source.doc_count > 0 && (
            <Button variant="secondary" size="sm" className="flex items-center gap-2"
              onClick={onReExtract}
            >
              <Waypoints size={11} />
              {S.library.reExtractSource}
            </Button>
          )}
          <Button variant="secondary" size="sm"
            onClick={onEdit}
            title={S.library.sourceSettings}
          >
            <SettingsIcon size={12} />
          </Button>
        </div>
      </div>
    </div>
  );
}

/** 失败详情：完整原文，可复制。tooltip 装不下也拷不走，所以要弹窗。 */
/** 抽取丢弃详情：抽出来了、被挡掉了，这里说清楚挡在哪、丢了多少。 */
function DropsModal({
  file,
  rows,
  onClose,
}: {
  file: string;
  rows: ExtractionDrop[];
  onClose: () => void;
}) {
  return (
    <Dialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={S.library.dropsTitle}
      description={
        <>
          <span className="block truncate" title={file}>
            {file}
          </span>
          <span className="mt-1 block">{S.library.dropsNote}</span>
        </>
      }
      closeLabel={S.library.close}
    >
      <div className="u-scroll max-h-80 overflow-y-auto">
          {rows.map((r) => (
            <div
              key={`${r.reason}:${r.detail}`}
              className="border-b border-line py-3 last:border-0"
            >
              <div className="flex items-baseline gap-2">
                <span className="text-body text-ink">
                  {S.library.dropReason[r.reason] ?? r.reason}
                </span>
                <span className="u-num ml-auto text-small text-ink-3">×{r.count}</span>
              </div>
              <div className="mt-1 font-mono text-fine text-ink-2 break-all">
                {r.detail}
              </div>
              {r.example && (
                <div className="mt-1 text-fine text-ink-3 break-all">
                  {S.library.dropsExample} {r.example}
                </div>
              )}
            </div>
          ))}
      </div>
    </Dialog>
  );
}

function ErrorModal({
  file,
  kind,
  text,
  onClose,
}: {
  file: string;
  kind: string;
  text: string;
  onClose: () => void;
}) {
  return (
    <Dialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={S.library.errorTitle}
      description={
        <span className="block truncate" title={file}>
          {file} · {kind}
        </span>
      }
      closeLabel={S.library.close}
      footer={
        <>
          <Button
            variant="secondary"
            size="sm"
            onClick={() =>
              navigator.clipboard
                .writeText(text)
                .then(() => toast.success(S.library.errorCopied))
                .catch(() => {})
            }
          >
            {S.library.copyError}
          </Button>
          <Button variant="primary" size="sm" onClick={onClose}>
            {S.library.close}
          </Button>
        </>
      }
    >
      <pre className="u-scroll max-h-72 overflow-auto rounded-lg border border-line bg-surface p-3 text-small leading-relaxed text-ink-2 whitespace-pre-wrap break-words">
        {text}
      </pre>
    </Dialog>
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
    <Dialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={
        <span className="flex items-center gap-2">
          <KeyRound size={14} className="text-ink-2" />
          {S.library.tokenTitle}
        </span>
      }
      closeLabel={S.library.close}
      footer={
        <>
          <Button
            variant="secondary"
            size="sm"
            className="flex items-center gap-2"
            disabled={rotate.isPending || tokenQuery.isPending}
            onClick={() => rotate.mutate()}
          >
            <RefreshCw size={11} className={rotate.isPending ? "animate-spin" : ""} />
            {token ? S.library.rotateToken : S.library.generateToken}
          </Button>
          <Button variant="primary" size="sm" onClick={onClose}>
            {S.library.close}
          </Button>
        </>
      }
    >
      <div className="space-y-3">
          {tokenQuery.isPending ? (
            <p className="text-body text-ink-3">{S.nav.loading}</p>
          ) : token ? (
            <>
              <Button
                variant="secondary"
                className="h-auto w-full justify-start whitespace-normal break-all py-3 text-left font-mono"
                title={S.library.copyEndpoint}
                onClick={() => copy(token, S.library.tokenCopied)}
              >
                {token}
              </Button>
              <p className="text-fine leading-relaxed text-ink-3">
                {S.library.tokenWarning}
              </p>
              <div>
                <p className="mb-1 text-fine text-ink-3">{S.library.tokenUsage}</p>
                <Button
                  variant="secondary"
                  className="h-auto w-full justify-start whitespace-pre-wrap break-all py-2 text-left font-mono text-ink-2"
                  title={S.library.copyEndpoint}
                  onClick={() => copy(endpoint, S.library.endpointCopied)}
                >
                  POST {endpoint}
                  {"\n"}Authorization: Bearer &lt;token&gt;
                </Button>
              </div>
            </>
          ) : (
            <p className="text-body text-ink-3">{S.library.noToken}</p>
          )}
      </div>
    </Dialog>
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
    <div className="glass rounded-xl">
      {runs.isLoading ? (
        <div className="py-20 text-center text-body text-ink-3">{S.nav.loading}</div>
      ) : list.length === 0 ? (
        <div className="py-20 text-center text-body text-ink-3">{S.library.noRuns}</div>
      ) : (
        <div className="px-4 py-2">
          {list.map((r) => (
            <div
              key={r.id}
              className="flex items-center gap-3 py-2 text-small border-b border-line last:border-0"
            >
              <span
                className={`h-1.5 w-1.5 rounded-full shrink-0 ${
                  r.status === "ok"
                    ? "bg-ok"
                    : r.status === "failed"
                      ? "bg-danger"
                      : "bg-warn animate-pulse"
                }`}
              />
              <span className="u-num text-ink-2 whitespace-nowrap shrink-0">
                {r.started_at.slice(0, 16).replace("T", " ")}
              </span>
              <span className="text-ink-3 whitespace-nowrap shrink-0">
                {r.created_docs > 0 && S.library.runNew(r.created_docs)}
                {r.created_docs > 0 && r.updated_docs > 0 && " · "}
                {r.updated_docs > 0 && S.library.runUpdated(r.updated_docs)}
                {r.status === "ok" && r.created_docs === 0 && r.updated_docs === 0 && (
                  <span className="text-ink-3">{S.library.runNothing}</span>
                )}
              </span>
              {r.error && (
                <span className="text-danger truncate min-w-0" title={r.error}>
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
  const [kind, setKind] = useState<CreatableSourceKind>("folder");
  const [name, setName] = useState("");
  const [icon, setIcon] = useState<string | null>(null);
  const [urls, setUrls] = useState("");
  const [feedUrl, setFeedUrl] = useState("");
  const [endpoint, setEndpoint] = useState("");
  const [authHeader, setAuthHeader] = useState("");
  const [repo, setRepo] = useState("");
  const [jiraUrl, setJiraUrl] = useState("");
  const [jiraProject, setJiraProject] = useState("");
  const [s3Bucket, setS3Bucket] = useState("");
  const [s3Prefix, setS3Prefix] = useState("");
  const [s3Endpoint, setS3Endpoint] = useState("");
  const [s3Region, setS3Region] = useState("");
  const [s3Key, setS3Key] = useState("");
  const [s3Secret, setS3Secret] = useState("");
  const [azAccount, setAzAccount] = useState("");
  const [azKey, setAzKey] = useState("");
  const [gcsKey, setGcsKey] = useState("");
  const [davUrl, setDavUrl] = useState("");
  const [davPath, setDavPath] = useState("");
  const [davUser, setDavUser] = useState("");
  const [davPass, setDavPass] = useState("");
  const [notionToken, setNotionToken] = useState("");
  const [notionQuery, setNotionQuery] = useState("");
  // PR 在 GitHub 的模型里也是工单。默认不收——问「工单」要的是工单；
  // 但有些仓库的决策记录实际写在 PR 描述里，所以给个开关
  const [includePrs, setIncludePrs] = useState(false);
  const [schedule, setSchedule] = useState<ScheduleValue>({
    sync_interval_minutes: null,
    sync_cron: null,
  });
  // folder = 纯容器、api = 推送型：都没有同步日程
  const syncing =
    kind === "url" ||
    kind === "rss" ||
    kind === "custom" ||
    kind === "github_issues" ||
    kind === "jira_issues" ||
    kind === "s3" ||
    kind === "azure_blob" ||
    kind === "gcs" ||
    kind === "webdav" ||
    kind === "notion";

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
              : kind === "github_issues"
                ? {
                    repo: repo.trim(),
                    ...(authHeader.trim() ? { auth_header: authHeader.trim() } : {}),
                    ...(includePrs ? { include_pull_requests: true } : {}),
                  }
                : kind === "jira_issues"
                  ? {
                      base_url: jiraUrl.trim(),
                      project: jiraProject.trim(),
                      ...(authHeader.trim() ? { auth_header: authHeader.trim() } : {}),
                    }
                  : kind === "s3"
                    ? {
                        bucket: s3Bucket.trim(),
                        ...(s3Prefix.trim() ? { prefix: s3Prefix.trim() } : {}),
                        // 端点留空 = 公有云 S3；填了就是自建，后端据此走 path-style
                        ...(s3Endpoint.trim() ? { endpoint: s3Endpoint.trim() } : {}),
                        ...(s3Region.trim() ? { region: s3Region.trim() } : {}),
                        ...(s3Key.trim() ? { access_key_id: s3Key.trim() } : {}),
                        ...(s3Secret.trim() ? { secret_access_key: s3Secret.trim() } : {}),
                      }
                    : kind === "azure_blob"
                      ? {
                          bucket: s3Bucket.trim(),
                          ...(s3Prefix.trim() ? { prefix: s3Prefix.trim() } : {}),
                          ...(s3Endpoint.trim() ? { endpoint: s3Endpoint.trim() } : {}),
                          ...(azAccount.trim() ? { account_name: azAccount.trim() } : {}),
                          ...(azKey.trim() ? { account_key: azKey.trim() } : {}),
                        }
                      : kind === "gcs"
                        ? {
                            bucket: s3Bucket.trim(),
                            ...(s3Prefix.trim() ? { prefix: s3Prefix.trim() } : {}),
                            ...(s3Endpoint.trim() ? { endpoint: s3Endpoint.trim() } : {}),
                            ...(gcsKey.trim()
                              ? { service_account_key: gcsKey.trim() }
                              : {}),
                          }
                        : kind === "webdav"
                          ? {
                              base_url: davUrl.trim(),
                              ...(davPath.trim() ? { path: davPath.trim() } : {}),
                              ...(davUser.trim() ? { username: davUser.trim() } : {}),
                              ...(davPass ? { password: davPass } : {}),
                            }
                          : kind === "notion"
                            ? {
                                token: notionToken.trim(),
                                ...(notionQuery.trim()
                                  ? { query: notionQuery.trim() }
                                  : {}),
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
          : kind === "notion"
            ? notionToken.trim()
            : kind === "webdav"
              ? davUrl.trim()
            : kind === "s3" || kind === "azure_blob" || kind === "gcs"
            ? s3Bucket.trim()
            : true);

  // 注意用 div 而非 label：label 会把 :hover/click 转发给内部第一个可标记控件
  //（button 也算），导致图标网格/日程选择器悬停时第一个按钮常亮
  const field = (label: string, node: React.ReactNode) => (
    <div className="mb-3">
      <div className="mb-1 text-fine font-medium text-ink-3">{label}</div>
      {node}
    </div>
  );

  return (
    <Dialog
      open
      onOpenChange={(o) => !o && onDone()}
      title={S.library.newSourceTitle}
      closeLabel={S.library.close}
      footer={
        <>
          <Button variant="secondary" size="sm" onClick={() => onDone()}>
            {S.library.cancel}
          </Button>
          <Button
            variant="primary"
            size="sm"
            disabled={!valid || create.isPending}
            onClick={() => create.mutate()}
          >
            {S.library.createSource}
          </Button>
        </>
      }
    >
      <div>
          {/* 类型：十二种排不下一行，换行，按钮按内容宽。选中的那个是 primary，
              与日程选择器里的多选同一个说法 */}
          <div className="mb-2 flex flex-wrap gap-2">
            {CREATABLE_SOURCE_KINDS.map((k) => {
              const Icon = KIND_ICON[k];
              return (
                <Button
                  key={k}
                  variant={kind === k ? "primary" : "secondary"}
                  size="sm"
                  icon={<Icon size={12} />}
                  onClick={() => setKind(k)}
                >
                  {S.library.sourceKinds[k]}
                </Button>
              );
            })}
          </div>
          {/* 类型自解释：一行说明；接口细节移入内置文档，弹窗只留链接 */}
          <p className="mb-4 text-fine leading-relaxed text-ink-3">
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
            <Input className="w-full"
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
                  <IconButton
                    key={key}
                    variant={icon === key ? "primary" : "ghost"}
                    label={key}
                    onClick={() => setIcon(icon === key ? null : key)}
                  >
                    <Icon size={14} />
                  </IconButton>
                ))}
              </div>,
            )}

          {kind === "custom" && (
            <>
              {field(
                S.library.endpointField,
                <Input className="w-full font-mono"
                  placeholder="https://your-service/utopia-feed"
                  value={endpoint}
                  onChange={(e) => setEndpoint(e.target.value)}
                />,
              )}
              {field(
                S.library.authHeaderField,
                <Input className="w-full font-mono"
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
              <Textarea className="w-full font-mono resize-y"
                value={urls}
                onChange={(e) => setUrls(e.target.value)}
              />,
            )}
          {kind === "rss" &&
            field(
              S.library.feedUrl,
              <Input className="w-full font-mono"
                value={feedUrl}
                onChange={(e) => setFeedUrl(e.target.value)}
              />,
            )}
          {kind === "jira_issues" && (
            <>
              {field(
                S.library.jiraUrlField,
                <Input className="w-full font-mono"
                  placeholder="https://jira.example.com"
                  value={jiraUrl}
                  onChange={(e) => setJiraUrl(e.target.value)}
                />,
              )}
              {field(
                S.library.jiraProjectField,
                <Input className="w-full font-mono"
                  placeholder="KAFKA"
                  value={jiraProject}
                  onChange={(e) => setJiraProject(e.target.value)}
                />,
              )}
              {field(
                S.library.tokenField,
                <Input className="w-full font-mono"
                  type="password"
                  placeholder="Basic dXNlcjp0b2tlbg=="
                  value={authHeader}
                  onChange={(e) => setAuthHeader(e.target.value)}
                />,
              )}
            </>
          )}
          {(kind === "s3" || kind === "azure_blob" || kind === "gcs") && (
            <>
              {field(
                S.library.s3BucketField,
                <Input className="w-full font-mono"
                  placeholder="documents"
                  value={s3Bucket}
                  onChange={(e) => setS3Bucket(e.target.value)}
                />,
              )}
              {field(
                S.library.s3PrefixField,
                <Input className="w-full font-mono"
                  placeholder="reports/2026/"
                  value={s3Prefix}
                  onChange={(e) => setS3Prefix(e.target.value)}
                />,
              )}
              {field(
                S.library.s3EndpointField,
                <Input className="w-full font-mono"
                  placeholder="http://minio.internal:9000"
                  value={s3Endpoint}
                  onChange={(e) => setS3Endpoint(e.target.value)}
                />,
              )}
              {field(
                S.library.s3RegionField,
                <Input className="w-full font-mono"
                  placeholder="us-east-1"
                  value={s3Region}
                  onChange={(e) => setS3Region(e.target.value)}
                />,
              )}
              {kind === "s3" && (
                <>
                  {field(
                    S.library.s3KeyField,
                    <Input className="w-full font-mono"
                      value={s3Key}
                      onChange={(e) => setS3Key(e.target.value)}
                    />,
                  )}
                  {field(
                    S.library.s3SecretField,
                    <Input className="w-full font-mono"
                      type="password"
                      value={s3Secret}
                      onChange={(e) => setS3Secret(e.target.value)}
                    />,
                  )}
                </>
              )}
              {kind === "azure_blob" && (
                <>
                  {field(
                    S.library.azAccountField,
                    <Input className="w-full font-mono"
                      value={azAccount}
                      onChange={(e) => setAzAccount(e.target.value)}
                    />,
                  )}
                  {field(
                    S.library.azKeyField,
                    <Input className="w-full font-mono"
                      type="password"
                      value={azKey}
                      onChange={(e) => setAzKey(e.target.value)}
                    />,
                  )}
                </>
              )}
              {kind === "gcs" &&
                field(
                  S.library.gcsKeyField,
                  <Textarea className="w-full font-mono"
                    placeholder='{"type":"service_account",...}'
                    value={gcsKey}
                    onChange={(e) => setGcsKey(e.target.value)}
                  />,
                )}
            </>
          )}
          {kind === "webdav" && (
            <>
              {field(
                S.library.davUrlField,
                <Input className="w-full font-mono"
                  placeholder="https://cloud.example.com/remote.php/dav/files/alice"
                  value={davUrl}
                  onChange={(e) => setDavUrl(e.target.value)}
                />,
              )}
              {field(
                S.library.davPathField,
                <Input className="w-full font-mono"
                  placeholder="/Documents"
                  value={davPath}
                  onChange={(e) => setDavPath(e.target.value)}
                />,
              )}
              {field(
                S.library.davUserField,
                <Input className="w-full font-mono"
                  value={davUser}
                  onChange={(e) => setDavUser(e.target.value)}
                />,
              )}
              {field(
                S.library.davPassField,
                <Input className="w-full font-mono"
                  type="password"
                  value={davPass}
                  onChange={(e) => setDavPass(e.target.value)}
                />,
              )}
            </>
          )}
          {kind === "notion" && (
            <>
              {field(
                S.library.notionTokenField,
                <Input className="w-full font-mono"
                  type="password"
                  placeholder="ntn_..."
                  value={notionToken}
                  onChange={(e) => setNotionToken(e.target.value)}
                />,
              )}
              {field(
                S.library.notionQueryField,
                <Input className="w-full font-mono"
                  value={notionQuery}
                  onChange={(e) => setNotionQuery(e.target.value)}
                />,
              )}
            </>
          )}
          {kind === "github_issues" && (
            <>
              {field(
                S.library.repoField,
                <Input className="w-full font-mono"
                  placeholder="owner/name"
                  value={repo}
                  onChange={(e) => setRepo(e.target.value)}
                />,
              )}
              {field(
                S.library.tokenField,
                <Input className="w-full font-mono"
                  type="password"
                  placeholder="Bearer ghp_…"
                  value={authHeader}
                  onChange={(e) => setAuthHeader(e.target.value)}
                />,
              )}
              <Checkbox
                className="mb-4"
                checked={includePrs}
                onChange={(e) => setIncludePrs(e.target.checked)}
                label={S.library.includePullRequests}
              />
            </>
          )}

          {syncing && field(S.library.interval, <SchedulePicker onChange={setSchedule} />)}

          {create.isError && (
            <p className="text-small text-danger mb-2">{(create.error as Error).message}</p>
          )}
      </div>
    </Dialog>
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
  const [repo, setRepo] = useState(cfg.repo ?? "");
  const [includePrs, setIncludePrs] = useState(Boolean(cfg.include_pull_requests));
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
    (kind === "custom" && endpoint.trim() !== (cfg.endpoint ?? "")) ||
    // 换仓库或改收不收 PR，两者都会换掉这个来源里文档的集合
    (kind === "github_issues" &&
      (repo.trim() !== (cfg.repo ?? "") ||
        includePrs !== Boolean(cfg.include_pull_requests)));

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
              : kind === "github_issues"
                ? {
                    repo: repo.trim(),
                    // 留空 = 后端保留库里原值（凭据只进不出）
                    ...(authHeader.trim() ? { auth_header: authHeader.trim() } : {}),
                    include_pull_requests: includePrs,
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
      <div className="mb-1 text-fine font-medium text-ink-3">{label}</div>
      {node}
    </div>
  );

  return (
    <Dialog
      open
      onOpenChange={(o) => !o && onDone()}
      title={S.library.editSourceTitle}
      closeLabel={S.library.close}
      footer={
        <>
          <Button variant="secondary" size="sm" onClick={onDone}>
            {S.library.cancel}
          </Button>
          <Button
            variant="primary"
            size="sm"
            disabled={!valid || save.isPending}
            onClick={() => save.mutate()}
          >
            {S.library.saveChanges}
          </Button>
        </>
      }
    >
      <div>
          {/* 类型只读：换类型 = 换身份，应新建来源 */}
          <div className="mb-4 flex items-center gap-2 text-small text-ink-2">
            <KindIcon size={13} className="text-ink-3" />
            {S.library.sourceKinds[kind as keyof typeof S.library.sourceKinds] ?? kind}
          </div>

          {field(
            S.library.sourceName,
            <Input className="w-full"
              value={name}
              onChange={(e) => setName(e.target.value)}
            />,
          )}

          {kind === "custom" &&
            field(
              S.library.iconLabel,
              <div className="grid grid-cols-10 gap-1">
                {Object.entries(SOURCE_ICONS).map(([key, Icon]) => (
                  <IconButton
                    key={key}
                    variant={icon === key ? "primary" : "ghost"}
                    label={key}
                    onClick={() => setIcon(icon === key ? null : key)}
                  >
                    <Icon size={14} />
                  </IconButton>
                ))}
              </div>,
            )}

          {kind === "url" &&
            field(
              S.library.urlsField,
              <Textarea className="w-full font-mono resize-y"
                value={urls}
                onChange={(e) => setUrls(e.target.value)}
              />,
            )}
          {kind === "rss" &&
            field(
              S.library.feedUrl,
              <Input className="w-full font-mono"
                value={feedUrl}
                onChange={(e) => setFeedUrl(e.target.value)}
              />,
            )}
          {kind === "github_issues" && (
            <>
              {field(
                S.library.repoField,
                <Input className="w-full font-mono"
                  placeholder="owner/name"
                  value={repo}
                  onChange={(e) => setRepo(e.target.value)}
                />,
              )}
              {field(
                S.library.tokenField,
                <Input className="w-full font-mono"
                  type="password"
                  placeholder="Bearer ghp_…"
                  value={authHeader}
                  onChange={(e) => setAuthHeader(e.target.value)}
                />,
              )}
              <Checkbox
                className="mb-4"
                checked={includePrs}
                onChange={(e) => setIncludePrs(e.target.checked)}
                label={S.library.includePullRequests}
              />
            </>
          )}
          {kind === "custom" && (
            <>
              {field(
                S.library.endpointField,
                <Input className="w-full font-mono"
                  value={endpoint}
                  onChange={(e) => setEndpoint(e.target.value)}
                />,
              )}
              {field(
                S.library.authHeaderEditField,
                <Input className="w-full font-mono"
                  type="password"
                  placeholder={S.library.authKeepHint}
                  value={authHeader}
                  onChange={(e) => setAuthHeader(e.target.value)}
                />,
              )}
            </>
          )}

          {ingestChanged && (
            <p className="mb-3 text-fine leading-relaxed text-ink-3">
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
            <p className="text-small text-danger mb-2">{(save.error as Error).message}</p>
          )}

          {/* Danger zone：删除来源（文档保留，落回 Uploads） */}
          <div className="mt-4 pt-3 border-t border-line">
            <div className="mb-1 text-fine font-medium text-ink-3">
              {S.library.dangerZone}
            </div>
            <div className="flex items-center justify-between gap-3">
              <span className="text-fine text-ink-3">{S.library.deleteSourceHint}</span>
              {/* 同一个道理：这里只打开确认框，红色在确认框里 */}
              <Button variant="secondary" size="sm" className="shrink-0"
                onClick={() => setConfirmingDelete(true)}
              >
                {S.library.deleteSource}
              </Button>
            </div>
          </div>
      </div>

      {/* 危险确认叠在设置弹窗之上：Radix 的层叠只让最上面那层响应 Esc 与遮罩 */}
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
    </Dialog>
  );
}

/** 「已删除」视图（#268）：墓碑在这里等着被恢复或清除。清除只有库管理员能按 */
function DeletedTable({
  docs,
  sources,
  canPurge,
  onRestore,
  onPurge,
}: {
  docs: Doc[];
  sources: SourceView[];
  canPurge: boolean;
  onRestore: (id: string) => void;
  onPurge: (doc: Doc) => void;
}) {
  if (!docs.length) {
    return (
      <div className="py-20 text-center text-body text-ink-3">{S.library.deletedEmpty}</div>
    );
  }
  return (
    <table className="w-full text-body">
      <thead>
        <tr className="text-left text-small text-ink-3 border-b border-line">
          <th className="px-4 py-3 font-medium">{S.library.colFile}</th>
          <th className="px-4 py-3 font-medium">{S.library.colSource}</th>
          <th className="px-4 py-3 font-medium">{S.library.colDeleted}</th>
          <th className="px-4 py-3"></th>
        </tr>
      </thead>
      <tbody>
        {docs.map((d) => {
          const src = sources.find((s) => s.id === d.source_id);
          return (
            <tr key={d.id} className="border-b border-line last:border-0">
              <td className="px-4 py-3 text-ink-2">{d.filename}</td>
              <td className="px-4 py-3 text-ink-3">{src?.name ?? S.library.uploads}</td>
              <td className="px-4 py-3 u-num text-ink-3">
                {d.deleted_at ? new Date(d.deleted_at).toLocaleString() : ""}
              </td>
              <td className="px-4 py-3 text-right whitespace-nowrap">
                <LinkButton onClick={() => onRestore(d.id)}>
                  {S.library.restore}
                </LinkButton>
                {canPurge && (
                  <LinkButton tone="danger" className="ml-4" onClick={() => onPurge(d)}>
                    {S.library.purge}
                  </LinkButton>
                )}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function DocRow({
  doc,
  source,
  onDelete,
  onExtract,
  onReprocess,
  onShowError,
  drops,
  onShowDrops,
}: {
  doc: Doc;
  /** undefined = 不渲染来源列；null = Uploads（source_id 为空） */
  source?: SourceView | null;
  onDelete: () => void;
  onExtract: () => void;
  onReprocess: () => void;
  onShowError: (kind: string, text: string) => void;
  /** 这篇文档抽出来却没落地的事实；undefined = 一条都没有 */
  drops?: ExtractionDrop[];
  onShowDrops: (rows: ExtractionDrop[]) => void;
}) {
  const kbId = useKbId();
  const dropTotal = drops?.reduce((n, d) => n + d.count, 0) ?? 0;
  const statusText =
    S.library.status[doc.status as keyof typeof S.library.status] ?? doc.status;
  const graphText =
    S.library.graphStatus[doc.graph_status as keyof typeof S.library.graphStatus] ??
    doc.graph_status;
  const SrcIcon = source ? sourceIcon(source) : Upload;
  return (
    <tr className="border-b border-line">
      <td className="px-4 py-3 max-w-xs truncate" title={doc.filename}>
        <Link
          to="/kb/$kbId/doc/$docId"
          params={{ kbId, docId: doc.id }}
          search={{}}
          className="u-inline-link text-ink"
        >
          {doc.filename}
        </Link>
      </td>
      {source !== undefined && (
        <td className="px-4 py-3">
          <span className="flex items-center gap-2 text-small text-ink-2">
            <SrcIcon size={12} className="shrink-0 text-ink-3" />
            <span className="truncate max-w-28">{source?.name ?? S.library.uploads}</span>
          </span>
        </td>
      )}
      <td className="px-4 py-3">
        {/* 失败可点开看原文：tooltip 会截断、也没法复制 */}
        {doc.status === "failed" && doc.error ? (
          <Chip
            tone="danger"
            onClick={() => onShowError(S.library.errorParse, doc.error!)}
          >
            {statusText}
          </Chip>
        ) : (
          <Chip tone={STATUS_TONE[doc.status] ?? "neutral"}>{statusText}</Chip>
        )}
        {/* 解析管道失败：重跑 解析→索引→嵌入（解析器升级/瞬时故障重试） */}
        {doc.status === "failed" && (
          <LinkButton underline className="ml-2" onClick={onReprocess}>
            {S.library.reprocess}
          </LinkButton>
        )}
        {doc.missing_since && (
          <span className="ml-2 inline-block" title={doc.missing_since.slice(0, 16).replace("T", " ")}>
            <Chip tone="neutral">{S.library.notInSource}</Chip>
          </span>
        )}
      </td>
      <td className="px-4 py-3">
        {doc.graph_status === "none" ? (
          <span className="text-small text-ink-3">{graphText}</span>
        ) : doc.graph_status === "failed" && doc.graph_error ? (
          <Chip
            tone="danger"
            onClick={() => onShowError(S.library.errorGraph, doc.graph_error!)}
          >
            {graphText}
          </Chip>
        ) : (
          <Chip tone={GRAPH_TONE[doc.graph_status] ?? "neutral"}>{graphText}</Chip>
        )}
        {/* 抽出来却没落地的事实。抽取成功不代表全须全尾，所以这个 chip 与
            graph_status 并列而不是替代它——"done" 和 "3 dropped" 同时为真 */}
        {dropTotal > 0 && drops && (
          <Chip tone="warn" className="ml-2" onClick={() => onShowDrops(drops)}>
            {S.library.dropsChip(dropTotal)}
          </Chip>
        )}
        {/* done 也可重抽：本体（描述/新类）调整后强制全量重抽正是常规操作 */}
        {doc.status === "ready" && ["none", "failed", "done"].includes(doc.graph_status) && (
          <LinkButton underline className="ml-2" onClick={onExtract}>
            {doc.graph_status === "done" ? S.library.reExtract : S.library.extract}
          </LinkButton>
        )}
      </td>
      <td className="px-4 py-3 text-ink-2">{doc.chunk_count || "—"}</td>
      <td className="px-4 py-3 text-ink-2">{formatSize(doc.size_bytes)}</td>
      <td className="px-4 py-3 text-right">
        <LinkButton tone="danger" onClick={onDelete}>
          {S.library.delete}
        </LinkButton>
      </td>
    </tr>
  );
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
