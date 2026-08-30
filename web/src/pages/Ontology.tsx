// 本体编辑器：master-detail 双栏（与 Library 的 SourcesRail 同构）。
// 左栏 = filter + Classes/Properties 两小节 + 底部 Unmatched 入口；
// 右侧 = 选中项的表单 / 未匹配信号面板 / 概览。
import { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ChevronRight, Inbox, Plus, Search, Upload } from "lucide-react";
import {
  api,
  type EntityTypeView,
  type ImportPlan,
  type OntologyMiss,
  type PlannedItem,
  type OntologyProposals,
  type RelationTypeView,
} from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { toast } from "../toast";
import {
  Button,
  Chip,
  ColorPicker,
  DangerConfirm,
  Dropdown,
  Input,
  Loading,
  MultiSearchSelect,
  Pager,
  PageTitle,
  RAIL_CLS,
  cn,
  pageSlice,
} from "../ui";

/** 左栏行高（py-1.5 + 13px 文字 + space-y 间隙）与底部预留（新建行 + 分页器） */
const RAIL_ROW_H = 34;
const RAIL_RESERVED = 80;
/** 兜底页行数（首帧未量到高度时用） */
const RAIL_PAGE = 14;
/** 过滤模式两节混排时每节的行数 */
const RAIL_PAGE_MIXED = 6;

/** 右侧详情区当前展示什么 */
type Sel =
  | { kind: "class"; id: string }
  | { kind: "relation"; id: string }
  | { kind: "new-class"; parentId: string | null }
  | { kind: "new-relation" }
  | { kind: "misses" }
  | { kind: "import" }
  | null;

export function Ontology() {
  const { kb } = useKb();
  const queryClient = useQueryClient();
  const [sel, setSel] = useState<Sel>(null);
  const [railTab, setRailTab] = useState<"classes" | "properties">("classes");
  const [filter, setFilter] = useState("");
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  // 每页行数按列表区实际高度动态算：窗口多高铺多满，不滚动也不留大空
  const listRef = useRef<HTMLDivElement>(null);
  const [railRows, setRailRows] = useState(RAIL_PAGE);
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      setRailRows(
        Math.max(5, Math.floor((el.clientHeight - RAIL_RESERVED) / RAIL_ROW_H)),
      );
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const data = useQuery({
    queryKey: ["ontology", kb?.id],
    queryFn: () => api.ontology(kb!.id),
    enabled: !!kb,
  });

  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: ["ontology", kb?.id] });
  // 错误统一走全局 toast，不再用页面内嵌错误行
  const onError = (e: unknown) => toast.error((e as Error).message);

  if (!kb) return <Loading>{S.nav.loading}</Loading>;
  if (data.isPending) return <Loading>{S.nav.loading}</Loading>;
  if (data.isError) return <Loading>{(data.error as Error).message}</Loading>;

  const { entity_types, relation_types, misses, dismissed_misses } = data.data;
  // 属性不进 Properties 列表：它们挂在类下，在类详情区编辑
  const relations = relation_types.filter((r) => r.kind !== "attribute");
  const selectedClass =
    sel?.kind === "class"
      ? (entity_types.find((t) => t.id === sel.id) ?? null)
      : null;
  const selectedProp =
    sel?.kind === "relation"
      ? (relation_types.find((r) => r.id === sel.id) ?? null)
      : null;

  return (
    <div className="h-full flex">
      {/* 左栏：filter + 两小节 + Unmatched */}
      <aside className={`${RAIL_CLS} flex flex-col`}>
        <div className="px-3 pt-3 pb-2.5">
          <div className="relative">
            <Search
              size={12}
              className="absolute left-2.5 top-1/2 -translate-y-1/2 text-neutral-600"
            />
            <input
              className="input-dark w-full pl-7 pr-2 py-1.5 text-xs"
              placeholder={S.ontology.filter}
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          </div>
        </div>
        {/* 分段切换：与登录页模式切换/日程选择器同一语汇（bg-white/5 容器 + 激活反白）；
            过滤时列表例外：两节混排同时给出命中 */}
        <div className="mx-3 mb-1 flex gap-1 rounded-lg bg-white/5 p-1">
          {(
            [
              ["classes", S.ontology.tabClasses],
              ["properties", S.ontology.tabProperties],
            ] as const
          ).map(([k, label]) => (
            <button
              key={k}
              onClick={() => setRailTab(k)}
              className={cn(
                "flex-1 rounded-md py-1 text-[12px] font-medium text-center transition-colors",
                railTab === k
                  ? "bg-white/10 text-neutral-100"
                  : "text-neutral-500 hover:text-neutral-300",
              )}
            >
              {label}
            </button>
          ))}
        </div>
        <div
          ref={listRef}
          className="flex-1 min-h-0 overflow-hidden px-2 pt-1.5 pb-2 flex flex-col"
        >
          {/* 新建行置顶：随当前段建类/建关系 */}
          {!filter.trim() && (
            <button
              onClick={() =>
                railTab === "classes"
                  ? setSel({ kind: "new-class", parentId: null })
                  : setSel({ kind: "new-relation" })
              }
              className="w-full flex items-center gap-1.5 rounded-lg px-2 py-2 mb-0.5 text-[13px] text-neutral-500 hover:bg-white/[0.05] hover:text-neutral-200 transition-colors"
            >
              <Plus size={13} />
              {railTab === "classes"
                ? S.ontology.newClass
                : S.ontology.newProperty}
            </button>
          )}
          {filter.trim() ? (
            <>
              <div className="px-2 pt-2 pb-1 text-[10px] font-medium uppercase tracking-[0.08em] text-neutral-600">
                {S.ontology.tabClasses}
              </div>
              <ClassTree
                types={entity_types}
                filter={filter}
                collapsed={collapsed}
                onToggle={() => {}}
                selectedId={selectedClass?.id ?? null}
                onSelect={(id) => setSel({ kind: "class", id })}
                pageSize={RAIL_PAGE_MIXED}
              />
              <div className="px-2 pt-3 pb-1 text-[10px] font-medium uppercase tracking-[0.08em] text-neutral-600">
                {S.ontology.tabProperties}
              </div>
              <PropertyList
                relations={relations}
                filter={filter}
                selectedId={selectedProp?.id ?? null}
                onSelect={(id) => setSel({ kind: "relation", id })}
                pageSize={RAIL_PAGE_MIXED}
              />
            </>
          ) : railTab === "classes" ? (
            <ClassTree
              types={entity_types}
              filter={filter}
              collapsed={collapsed}
              onToggle={(id) => {
                const next = new Set(collapsed);
                if (next.has(id)) next.delete(id);
                else next.add(id);
                setCollapsed(next);
              }}
              selectedId={selectedClass?.id ?? null}
              onSelect={(id) => setSel({ kind: "class", id })}
              pageSize={railRows}
            />
          ) : (
            <PropertyList
              relations={relations}
              filter={filter}
              selectedId={selectedProp?.id ?? null}
              onSelect={(id) => setSel({ kind: "relation", id })}
              pageSize={railRows}
            />
          )}
        </div>
        {/* 底部常驻：两个"关于本体"的入口——从外部拿一份本体，或看抽取顶回来的信号 */}
        <button
          onClick={() => setSel({ kind: "import" })}
          className={cn(
            "shrink-0 border-t border-white/10 px-4 py-2.5 flex items-center gap-2 text-[13px] transition-colors",
            sel?.kind === "import"
              ? "u-nav-active"
              : "text-neutral-400 hover:bg-white/[0.05] hover:text-neutral-200",
          )}
        >
          <Upload size={14} className="text-neutral-500" />
          <span>{S.ontology.importShort}</span>
        </button>
        {/* 底部常驻：抽取未匹配信号（有存量时带数量徽标） */}
        <button
          onClick={() => setSel({ kind: "misses" })}
          className={cn(
            "shrink-0 border-t border-white/10 px-4 py-2.5 flex items-center gap-2 text-[13px] transition-colors",
            sel?.kind === "misses"
              ? "u-nav-active"
              : "text-neutral-400 hover:bg-white/[0.05] hover:text-neutral-200",
          )}
        >
          <Inbox size={14} className="text-neutral-500" />
          <span>{S.ontology.missesShort}</span>
          {misses.length > 0 && (
            <span className="ml-auto u-num text-[10.5px] text-neutral-500 bg-white/[0.08] rounded-full px-1.5 py-px">
              {misses.length}
            </span>
          )}
        </button>
      </aside>

      {/* 右侧：详情。选中类时表单 + 实例列表双栏铺开，提高宽屏利用率 */}
      <div className="flex-1 min-w-0 overflow-y-auto u-scroll px-8 py-6">
        {/* 放宽到 6xl 供三列铺开；misses/关系/概览各自带 max-w-xl 内衬不受影响 */}
        <div className="max-w-6xl">
          {sel?.kind === "import" ? (
            <div className="max-w-xl">
              <ImportPanel kbId={kb.id} onChanged={refresh} onError={onError} />
            </div>
          ) : sel?.kind === "misses" ? (
            <div className="max-w-xl">
              <MissesPanel
                kbId={kb.id}
                misses={misses}
                dismissedMisses={dismissed_misses ?? []}
                onChanged={refresh}
                onError={onError}
              />
            </div>
          ) : sel?.kind === "new-class" || selectedClass ? (
            /* lg 两列（表单 | 属性+实例堆叠）；xl 三列并排（包装器 xl:contents 解散入栅格） */
            <div className="grid gap-4 items-start lg:grid-cols-[minmax(0,24rem)_minmax(0,1fr)] xl:grid-cols-[minmax(0,22rem)_minmax(0,1fr)_minmax(0,1fr)]">
              <div className="glass rounded-xl p-4">
                <ClassForm
                  key={
                    selectedClass?.id ??
                    `new-${sel?.kind === "new-class" ? sel.parentId : "root"}`
                  }
                  kbId={kb.id}
                  existing={selectedClass}
                  parentId={
                    sel?.kind === "new-class"
                      ? sel.parentId
                      : (selectedClass?.primary_parent ?? null)
                  }
                  allTypes={entity_types}
                  onNewSub={
                    selectedClass
                      ? () =>
                          setSel({
                            kind: "new-class",
                            parentId: selectedClass.id,
                          })
                      : undefined
                  }
                  onDone={(createdId) => {
                    // 新建成功即选中它：立刻能看到、能继续编辑
                    if (sel?.kind === "new-class")
                      setSel(
                        createdId ? { kind: "class", id: createdId } : null,
                      );
                    refresh();
                  }}
                  onError={onError}
                />
              </div>
              {/* lg 右列堆叠属性+实例；xl 解散为两个独立栅格列 */}
              {selectedClass && (
                <div className="grid gap-4 items-start xl:contents">
                  <AttributesCard
                    kbId={kb.id}
                    type={selectedClass}
                    attributes={relation_types.filter(
                      (r) =>
                        r.kind === "attribute" &&
                        r.domains.includes(selectedClass.id),
                    )}
                    onChanged={refresh}
                    onError={onError}
                  />
                  <InstancesCard kbId={kb.id} type={selectedClass} />
                </div>
              )}
            </div>
          ) : sel?.kind === "new-relation" || selectedProp ? (
            <div className="glass rounded-xl p-4 max-w-xl">
              <PropertyForm
                key={selectedProp?.id ?? "new"}
                kbId={kb.id}
                existing={selectedProp}
                allTypes={entity_types}
                onDone={(createdId) => {
                  if (sel?.kind === "new-relation")
                    setSel(
                      createdId ? { kind: "relation", id: createdId } : null,
                    );
                  refresh();
                }}
                onError={onError}
              />
            </div>
          ) : (
            /* 概览：未选中任何条目 */
            <div className="glass rounded-xl p-6 max-w-xl">
              <PageTitle className="mb-1">{S.ontology.title}</PageTitle>
              <p className="text-xs text-neutral-500 u-num">
                {S.ontology.overviewStats(
                  entity_types.length,
                  relations.length,
                )}
              </p>
              <p className="mt-3 text-sm text-neutral-400">
                {S.ontology.overviewHint}
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/* ---------- 实例列表：选中类的实体（服务端分页，点击进图谱） ---------- */

function InstancesCard({ kbId, type }: { kbId: string; type: EntityTypeView }) {
  const PER = 12;
  const [page, setPage] = useState(0);
  useEffect(() => setPage(0), [type.id]);
  const q = useQuery({
    queryKey: ["type-entities", kbId, type.id, page],
    queryFn: () => api.typeEntities(kbId, type.id, page, PER),
  });
  const total = q.data?.total ?? 0;
  const rows = q.data?.entities ?? [];
  if (!q.isPending && total === 0) return null; // 没有实例时不占版面

  return (
    <div className="glass rounded-xl p-4">
      <div className="mb-1.5 flex items-baseline gap-2">
        <h3 className="text-sm font-bold text-neutral-200">
          {S.ontology.instances}
        </h3>
        <span className="u-num text-xs text-neutral-500">{total}</span>
      </div>
      <div className="divide-y divide-white/[0.06]">
        {rows.map((e) => (
          <Link
            key={e.id}
            to="/graph"
            search={{ entity: e.id }}
            className="flex items-center gap-2 py-1.5 text-sm text-neutral-300 hover:text-white"
          >
            <span
              className={`h-2 w-2 shrink-0 ${type.shape === "square" ? "" : "rounded-full"}`}
              style={{ background: type.color }}
            />
            <span className="truncate">{e.name}</span>
            <span className="ml-auto shrink-0 u-num text-[10.5px] text-neutral-600">
              {S.ontology.instanceFacts(e.fact_count)}
            </span>
          </Link>
        ))}
      </div>
      <Pager total={total} pageSize={PER} page={page} onPage={setPage} />
    </div>
  );
}

/* ---------- 属性卡片：选中类的字面值字段（行内增改删） ---------- */

function AttributesCard({
  kbId,
  type,
  attributes,
  onChanged,
  onError,
}: {
  kbId: string;
  type: EntityTypeView;
  attributes: RelationTypeView[];
  onChanged: () => void;
  onError: (e: unknown) => void;
}) {
  // 行内编辑：一次只展开一行（属性 id 或 "new"）
  const [editing, setEditing] = useState<string | null>(null);
  useEffect(() => setEditing(null), [type.id]);

  return (
    <div className="glass rounded-xl p-4">
      <div className="mb-1 flex items-baseline gap-2">
        <h3 className="text-sm font-bold text-neutral-200">
          {S.ontology.attributes}
        </h3>
        {attributes.length > 0 && (
          <span className="u-num text-xs text-neutral-500">
            {attributes.length}
          </span>
        )}
      </div>
      <p className="text-xs text-neutral-500 mb-2">
        {S.ontology.attributesHint}
      </p>
      <div className="divide-y divide-white/[0.06]">
        {attributes.map((a) =>
          editing === a.id ? (
            <AttributeForm
              key={a.id}
              kbId={kbId}
              typeId={type.id}
              existing={a}
              onDone={() => {
                setEditing(null);
                onChanged();
              }}
              onCancel={() => setEditing(null)}
              onError={onError}
            />
          ) : (
            <button
              key={a.id}
              onClick={() => setEditing(a.id)}
              className="w-full flex items-center gap-2 py-1.5 text-sm text-left text-neutral-300 hover:text-white"
            >
              <span className="truncate">{a.label}</span>
              <Chip tone="neutral">
                {S.ontology.datatypeNames[a.datatype ?? "text"]}
              </Chip>
              {a.unit && (
                <span className="text-xs text-neutral-500 shrink-0">
                  {a.unit}
                </span>
              )}
              {a.functional && <Chip tone="info">1:1</Chip>}
              <span className="ml-auto shrink-0 u-num text-[10.5px] text-neutral-600">
                {S.ontology.usage(a.usage)}
              </span>
            </button>
          ),
        )}
      </div>
      {editing === "new" ? (
        <div className="pt-2">
          <AttributeForm
            kbId={kbId}
            typeId={type.id}
            existing={null}
            onDone={() => {
              setEditing(null);
              onChanged();
            }}
            onCancel={() => setEditing(null)}
            onError={onError}
          />
        </div>
      ) : (
        <button
          onClick={() => setEditing("new")}
          className="mt-1.5 flex items-center gap-1.5 text-[13px] text-neutral-500 hover:text-neutral-200 transition-colors"
        >
          <Plus size={13} />
          {S.ontology.newAttribute}
        </button>
      )}
    </div>
  );
}

function AttributeForm({
  kbId,
  typeId,
  existing,
  onDone,
  onCancel,
  onError,
}: {
  kbId: string;
  typeId: string;
  existing: RelationTypeView | null;
  onDone: () => void;
  onCancel: () => void;
  onError: (e: unknown) => void;
}) {
  const [key, setKey] = useState(existing?.key ?? "");
  const [label, setLabel] = useState(existing?.label ?? "");
  const [datatype, setDatatype] = useState(existing?.datatype ?? "text");
  const [unit, setUnit] = useState(existing?.unit ?? "");
  // 单值 = functional：新值经时态引擎闭合旧值（属性历史的来源）。多数属性如此，默认开
  const [single, setSingle] = useState(existing?.functional ?? true);
  const [description, setDescription] = useState(existing?.description ?? "");

  const save = useMutation({
    mutationFn: async (): Promise<unknown> =>
      existing
        ? api.updateRelationType(kbId, existing.id, {
            label,
            temporal: existing.temporal,
            functional: single,
            inverse_functional: false,
            description,
            datatype,
            unit,
          })
        : api.createRelationType(kbId, {
            key,
            label,
            kind: "attribute",
            domains: [typeId],
            temporal: "state",
            functional: single,
            inverse_functional: false,
            description,
            datatype,
            unit,
          }),
    onSuccess: () => {
      toast.success(existing ? S.toast.saved : S.toast.created);
      onDone();
    },
    onError,
  });
  const remove = useMutation({
    mutationFn: () => api.deleteRelationType(kbId, existing!.id),
    onSuccess: () => {
      toast.success(S.toast.deleted);
      onDone();
    },
    onError,
  });

  const lbl = "block text-xs font-medium text-neutral-500 mb-1";
  return (
    <div className="py-2.5 space-y-2.5">
      {!existing && (
        <div className="flex gap-2">
          <div className="flex-1">
            <label className={lbl}>{S.ontology.key}</label>
            <Input
              value={key}
              onChange={(e) => setKey(e.target.value)}
              className="w-full"
              placeholder="salary"
            />
          </div>
          <div className="flex-1">
            <label className={lbl}>{S.ontology.label}</label>
            <Input
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              className="w-full"
            />
          </div>
        </div>
      )}
      {existing && (
        <div>
          <label className={lbl}>{S.ontology.label}</label>
          <Input
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            className="w-full"
          />
        </div>
      )}
      <div className="flex gap-2">
        <div className="flex-1">
          <label className={lbl}>{S.ontology.attrDatatype}</label>
          <Dropdown
            value={datatype}
            onChange={(v) => setDatatype(v as typeof datatype)}
            className="w-full"
            options={(["text", "number", "date", "bool"] as const).map((d) => ({
              value: d,
              label: S.ontology.datatypeNames[d],
            }))}
          />
        </div>
        <div className="flex-1">
          <label className={lbl}>
            {S.ontology.attrUnit}{" "}
            <span className="text-neutral-600">
              ({S.ontology.attrUnitHint})
            </span>
          </label>
          <Input
            value={unit}
            onChange={(e) => setUnit(e.target.value)}
            className="w-full"
          />
        </div>
      </div>
      <label className="flex items-center gap-2 text-[13px] text-neutral-300">
        <input
          type="checkbox"
          checked={single}
          onChange={(e) => setSingle(e.target.checked)}
        />
        {S.ontology.attrSingle}
      </label>
      <div>
        <label className={lbl}>{S.ontology.description}</label>
        <textarea
          className="input-dark w-full px-3 py-2 text-sm min-h-[3.5rem] resize-y"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
      </div>
      <div className="flex gap-2">
        <Button
          size="sm"
          onClick={() => save.mutate()}
          disabled={
            save.isPending || !label.trim() || (!existing && !key.trim())
          }
        >
          {S.ontology.save}
        </Button>
        <Button size="sm" variant="ghost" onClick={onCancel}>
          {S.ontology.cancel}
        </Button>
        {existing && (
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto"
            disabled={existing.usage > 0}
            title={existing.usage > 0 ? S.ontology.deleteBlocked : undefined}
            onClick={() => remove.mutate()}
          >
            {S.ontology.delete}
          </Button>
        )}
      </div>
    </div>
  );
}

/* ---------- 左栏小节头 ---------- */

/* ---------- 类层级树（可折叠；filter 时拍平） ---------- */

function ClassTree({
  types,
  filter,
  collapsed,
  onToggle,
  selectedId,
  onSelect,
  pageSize,
}: {
  types: EntityTypeView[];
  filter: string;
  collapsed: Set<string>;
  onToggle: (id: string) => void;
  selectedId: string | null;
  onSelect: (id: string) => void;
  pageSize: number;
}) {
  const rows = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (q) {
      // filter 模式：拍平命中项（label/key 都参与匹配）
      return types
        .filter(
          (t) =>
            t.label.toLowerCase().includes(q) ||
            t.key.toLowerCase().includes(q),
        )
        .map((t) => ({ t, depth: 0, hasChildren: false }));
    }
    const children = new Map<string | null, EntityTypeView[]>();
    for (const t of types) {
      const p = t.primary_parent ?? null;
      if (!children.has(p)) children.set(p, []);
      children.get(p)!.push(t);
    }
    const out: { t: EntityTypeView; depth: number; hasChildren: boolean }[] =
      [];
    const walk = (parent: string | null, depth: number) => {
      for (const t of children.get(parent) ?? []) {
        const kids = children.get(t.id) ?? [];
        out.push({ t, depth, hasChildren: kids.length > 0 });
        if (!collapsed.has(t.id)) walk(t.id, depth + 1);
      }
    };
    walk(null, 0);
    return out;
  }, [types, filter, collapsed]);

  // 半屏分页：过滤词变化回第一页
  const [page, setPage] = useState(0);
  useEffect(() => setPage(0), [filter]);
  const { rows: paged, safe } = pageSlice(rows, page, pageSize);

  return (
    <div className="space-y-0.5">
      {paged.map(({ t, depth, hasChildren }) => (
        <button
          key={t.id}
          onClick={() => onSelect(t.id)}
          style={{ paddingLeft: `${6 + depth * 14}px` }}
          className={cn(
            "w-full text-left rounded-lg py-1.5 pr-2 text-[13px] flex items-center gap-1.5",
            selectedId === t.id
              ? "u-nav-active"
              : "hover:bg-white/[0.05] text-neutral-400 hover:text-neutral-200",
          )}
        >
          {/* 折叠柄：有子类才渲染，点击不选中 */}
          {hasChildren ? (
            <span
              onClick={(e) => {
                e.stopPropagation();
                onToggle(t.id);
              }}
              className="shrink-0 text-neutral-600 hover:text-neutral-300"
            >
              <ChevronRight
                size={12}
                className={cn(
                  "transition-transform",
                  !collapsed.has(t.id) && "rotate-90",
                )}
              />
            </span>
          ) : (
            <span className="w-3 shrink-0" />
          )}
          {/* 方形是直角：与圆形拉开区分度（图谱节点同理） */}
          <span
            className={`h-2.5 w-2.5 shrink-0 ${t.shape === "square" ? "" : "rounded-full"}`}
            style={{ background: t.color }}
          />
          {/* 不在列表里放逐项用量读数：数量级上来后统计和渲染都是负担，用量看表单 */}
          <span className="truncate">{t.label}</span>
        </button>
      ))}
      <Pager
        total={rows.length}
        pageSize={pageSize}
        page={safe}
        onPage={setPage}
      />
    </div>
  );
}

/* ---------- 关系列表 ---------- */

function PropertyList({
  relations,
  filter,
  selectedId,
  onSelect,
  pageSize,
}: {
  relations: RelationTypeView[];
  filter: string;
  selectedId: string | null;
  onSelect: (id: string) => void;
  pageSize: number;
}) {
  const q = filter.trim().toLowerCase();
  const rows = q
    ? relations.filter(
        (r) =>
          r.label.toLowerCase().includes(q) || r.key.toLowerCase().includes(q),
      )
    : relations;
  // 半屏分页：过滤词变化回第一页
  const [page, setPage] = useState(0);
  useEffect(() => setPage(0), [filter]);
  const { rows: paged, safe } = pageSlice(rows, page, pageSize);
  return (
    <div className="space-y-0.5">
      {paged.map((r) => (
        <button
          key={r.id}
          onClick={() => onSelect(r.id)}
          style={{ paddingLeft: "6px" }}
          className={cn(
            "w-full text-left rounded-lg py-1.5 pr-2 text-[13px] flex items-center gap-1.5",
            selectedId === r.id
              ? "u-nav-active"
              : "hover:bg-white/[0.05] text-neutral-400 hover:text-neutral-200",
          )}
        >
          {/* 前导只留折叠柄槽：文字起点对齐类行"标识点"的左端 */}
          <span className="w-3 shrink-0" />
          <span className="truncate">{r.label}</span>
          {r.functional && <Chip tone="info">1:1</Chip>}
        </button>
      ))}
      <Pager
        total={rows.length}
        pageSize={pageSize}
        page={safe}
        onPage={setPage}
      />
    </div>
  );
}

/* ---------- 类表单 ---------- */

/** 父类下拉的候选：树序 + 缩进（层级可见），排除自己与全部后代（防成环）。 */
function parentOptions(
  allTypes: EntityTypeView[],
  selfId: string | undefined,
): { value: string; label: string; indent: number }[] {
  const excluded = new Set<string>();
  if (selfId) {
    excluded.add(selfId);
    // 收后代：反复扫描直到收敛（类的数量级很小，O(n²) 无所谓）
    let grew = true;
    while (grew) {
      grew = false;
      for (const t of allTypes) {
        if (t.parents.some((p) => excluded.has(p)) && !excluded.has(t.id)) {
          excluded.add(t.id);
          grew = true;
        }
      }
    }
  }
  const children = new Map<string | null, EntityTypeView[]>();
  for (const t of allTypes) {
    const p = t.primary_parent ?? null;
    if (!children.has(p)) children.set(p, []);
    children.get(p)!.push(t);
  }
  const out: { value: string; label: string; indent: number }[] = [];
  const walk = (parent: string | null, depth: number) => {
    for (const t of children.get(parent) ?? []) {
      if (excluded.has(t.id)) continue;
      out.push({ value: t.id, label: t.label, indent: depth });
      walk(t.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

function ClassForm({
  kbId,
  existing,
  parentId,
  allTypes,
  onNewSub,
  onDone,
  onError,
}: {
  kbId: string;
  existing: EntityTypeView | null;
  parentId: string | null;
  allTypes: EntityTypeView[];
  /** 编辑已有类时提供：以当前类为父级新建子类 */
  onNewSub?: () => void;
  /** 创建成功时携带新 id，编辑成功时为 undefined */
  onDone: (createdId?: string) => void;
  onError: (e: unknown) => void;
}) {
  const [key, setKey] = useState(existing?.key ?? "");
  const [label, setLabel] = useState(existing?.label ?? "");
  const [color, setColor] = useState(existing?.color ?? "#8ea5bd");
  const [shape, setShape] = useState<"circle" | "square">(
    existing?.shape ?? "circle",
  );
  const [parents, setParents] = useState<string[]>(
    existing?.parents ?? (parentId ? [parentId] : []),
  );
  const [description, setDescription] = useState(existing?.description ?? "");

  // 从左栏点"+ 子类"进来时预填那个父。多父下它是第一个，也就是主父
  useEffect(() => setParents(parentId ? [parentId] : []), [parentId]);

  const save = useMutation({
    mutationFn: async (): Promise<unknown> =>
      existing
        ? api.updateEntityType(kbId, existing.id, {
            label,
            color,
            shape,
            parents,
            description,
          })
        : api.createEntityType(kbId, {
            key,
            label,
            color,
            shape,
            parents,
            description,
          }),
    onSuccess: (res) => {
      toast.success(existing ? S.toast.saved : S.toast.created);
      onDone(existing ? undefined : (res as { id?: string })?.id);
    },
    onError,
  });
  const remove = useMutation({
    mutationFn: () => api.deleteEntityType(kbId, existing!.id),
    onSuccess: () => {
      toast.success(S.toast.deleted);
      onDone();
    },
    onError,
  });

  const lbl = "block text-xs font-medium text-neutral-500 mb-1";
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <span
          className={`h-3 w-3 ${shape === "square" ? "" : "rounded-full"}`}
          style={{ background: color }}
        />
        <span className="font-bold text-neutral-100">
          {existing?.label ?? S.ontology.newClass}
        </span>
        {/* key 是纯技术标识：已存在时干脆不展示，只在创建时输入 */}
        {existing?.builtin && <Chip tone="neutral">{S.ontology.builtin}</Chip>}
        {existing && (
          <span className="ml-auto text-xs text-neutral-500">
            {S.ontology.usage(existing.usage)}
          </span>
        )}
      </div>
      {!existing && (
        <div>
          <label className={lbl}>
            {S.ontology.key}{" "}
            <span className="text-neutral-600">({S.ontology.keyHint})</span>
          </label>
          <Input
            value={key}
            onChange={(e) => setKey(e.target.value)}
            className="w-full"
            placeholder="contract"
          />
        </div>
      )}
      <div>
        <label className={lbl}>{S.ontology.label}</label>
        <Input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          className="w-full"
        />
      </div>
      <div>
        <label className={lbl}>{S.ontology.shapeColor}</label>
        <div className="flex items-center gap-2">
          <ColorPicker value={color} onChange={setColor} shape={shape} />
          {/* 形状：与图谱节点渲染一一对应（circle=四层圆 / square=四层方） */}
          <div className="flex rounded-lg overflow-hidden border border-white/10">
            {(["circle", "square"] as const).map((sh) => (
              <button
                key={sh}
                onClick={() => setShape(sh)}
                title={sh}
                className={`h-8 w-10 grid place-items-center transition-colors ${
                  shape === sh
                    ? "bg-white/[0.12] text-white"
                    : "text-neutral-500 hover:bg-white/[0.05] hover:text-neutral-300"
                }`}
              >
                <span
                  className={`h-3 w-3 border-[1.5px] border-current ${
                    sh === "circle" ? "rounded-full" : ""
                  }`}
                />
              </button>
            ))}
          </div>
        </div>
      </div>
      {/* 多父：subClassOf 可以有多条。左栏按树画，一个类只能出现一次——
          所以第一个当主父。界面说明这条，不另加一个"选主父"的控件 */}
      <div>
        <label className={lbl}>{S.ontology.parent}</label>
        <MultiSearchSelect
          values={parents}
          options={parentOptions(allTypes, existing?.id)}
          onToggle={(id) =>
            setParents((v) =>
              v.includes(id) ? v.filter((x) => x !== id) : [...v, id],
            )
          }
          placeholder={S.ontology.searchTypes}
          emptyHint={S.ontology.noParent}
        />
        {parents.length > 1 && (
          <p className="mt-1 text-[11px] text-neutral-600">
            {S.ontology.primaryParentHint}
          </p>
        )}
      </div>
      <div>
        <label className={lbl}>{S.ontology.description}</label>
        {/* 语义指引：整段注入抽取 prompt，直接影响抽取归类质量 */}
        <textarea
          className="input-dark w-full px-3 py-2 text-sm min-h-[4.5rem] resize-y"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
        <p className="mt-1 text-[10.5px] text-neutral-600">
          {S.ontology.descriptionHint}
        </p>
      </div>
      <div className="flex gap-2 pt-1">
        <Button
          size="sm"
          onClick={() => save.mutate()}
          disabled={save.isPending || !label.trim()}
        >
          {S.ontology.save}
        </Button>
        {onNewSub && (
          <Button size="sm" variant="ghost" onClick={onNewSub}>
            {S.ontology.newSubClass}
          </Button>
        )}
        {existing && !existing.builtin && (
          <Button
            size="sm"
            variant="ghost"
            disabled={existing.usage > 0}
            title={existing.usage > 0 ? S.ontology.deleteBlocked : undefined}
            onClick={() => remove.mutate()}
          >
            {S.ontology.delete}
          </Button>
        )}
      </div>
    </div>
  );
}

/* ---------- 关系表单 ---------- */

function PropertyForm({
  kbId,
  existing,
  allTypes,
  onDone,
  onError,
}: {
  kbId: string;
  existing: RelationTypeView | null;
  allTypes: EntityTypeView[];
  onDone: (createdId?: string) => void;
  onError: (e: unknown) => void;
}) {
  const [key, setKey] = useState(existing?.key ?? "");
  const [label, setLabel] = useState(existing?.label ?? "");
  const [temporal, setTemporal] = useState(existing?.temporal ?? "state");
  const [functional, setFunctional] = useState(existing?.functional ?? false);
  const [inverseFunctional, setInverseFunctional] = useState(
    existing?.inverse_functional ?? false,
  );
  const [description, setDescription] = useState(existing?.description ?? "");
  const [domains, setDomains] = useState<string[]>(existing?.domains ?? []);
  const [ranges, setRanges] = useState<string[]>(existing?.ranges ?? []);
  // 显示标签，不显示 key。**进提示词的 key 由服务端从库里取**，与界面显示什么无关；
  // 而类树、属性列表也都显示标签，这里没有理由例外——中文库里用户该看到
  // "发票记录" 而不是 invoice_record
  const typeOpts = useMemo(
    () => parentOptions(allTypes, undefined),
    [allTypes],
  );
  const toggle = (
    set: React.Dispatch<React.SetStateAction<string[]>>,
    id: string,
  ) => set((v) => (v.includes(id) ? v.filter((x) => x !== id) : [...v, id]));

  const save = useMutation({
    mutationFn: async (): Promise<unknown> =>
      existing
        ? api.updateRelationType(kbId, existing.id, {
            label,
            temporal,
            functional,
            inverse_functional: inverseFunctional,
            description,
            domains,
            ranges,
          })
        : api.createRelationType(kbId, {
            key,
            label,
            temporal,
            functional,
            inverse_functional: inverseFunctional,
            description,
            domains,
            ranges,
          }),
    onSuccess: (res) => {
      toast.success(existing ? S.toast.saved : S.toast.created);
      onDone(existing ? undefined : (res as { id?: string })?.id);
    },
    onError,
  });
  const remove = useMutation({
    mutationFn: () => api.deleteRelationType(kbId, existing!.id),
    onSuccess: () => {
      toast.success(S.toast.deleted);
      onDone();
    },
    onError,
  });

  const lbl = "block text-xs font-medium text-neutral-500 mb-1";
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <span className="font-bold text-neutral-100">
          {existing?.label ?? S.ontology.newProperty}
        </span>
        {existing?.builtin && <Chip tone="neutral">{S.ontology.builtin}</Chip>}
        {existing && (
          <span className="ml-auto text-xs text-neutral-500">
            {S.ontology.usage(existing.usage)}
          </span>
        )}
      </div>
      {!existing && (
        <div>
          <label className={lbl}>
            {S.ontology.key}{" "}
            <span className="text-neutral-600">({S.ontology.keyHint})</span>
          </label>
          <Input
            value={key}
            onChange={(e) => setKey(e.target.value)}
            className="w-full"
            placeholder="signed_with"
          />
        </div>
      )}
      <div>
        <label className={lbl}>{S.ontology.label}</label>
        <Input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          className="w-full"
        />
      </div>
      {/* 类型签名。界面显示标签，而进提示词的是 key —— 那一步在服务端，
          与这里显示什么无关（docs/decisions/0004 定的是提示词里必须用 key） */}
      <div>
        <label className={lbl}>{S.ontology.signature}</label>
        <p className="text-[11px] leading-relaxed text-neutral-600 mb-1.5">
          {S.ontology.signatureHint}
        </p>
        <div className="grid gap-2 sm:grid-cols-2">
          <div className="min-w-0">
            <div className="text-[10px] uppercase tracking-[0.08em] text-neutral-600 mb-1">
              {S.ontology.domainLabel}
            </div>
            <MultiSearchSelect
              values={domains}
              options={typeOpts}
              onToggle={(id) => toggle(setDomains, id)}
              placeholder={S.ontology.searchTypes}
              emptyHint={S.ontology.anyType}
            />
          </div>
          <div className="min-w-0">
            <div className="text-[10px] uppercase tracking-[0.08em] text-neutral-600 mb-1">
              {S.ontology.rangeLabel}
            </div>
            <MultiSearchSelect
              values={ranges}
              options={typeOpts}
              onToggle={(id) => toggle(setRanges, id)}
              placeholder={S.ontology.searchTypes}
              emptyHint={S.ontology.anyType}
            />
          </div>
        </div>
      </div>
      <div>
        <label className={lbl}>{S.ontology.temporal}</label>
        <Dropdown
          value={temporal}
          onChange={setTemporal}
          className="w-full"
          options={[
            { value: "state", label: S.ontology.temporalState },
            { value: "event", label: S.ontology.temporalEvent },
            { value: "eternal", label: S.ontology.temporalEternal },
          ]}
        />
      </div>
      <label className="flex items-center gap-2 text-sm text-neutral-300">
        <input
          type="checkbox"
          checked={functional}
          onChange={(e) => setFunctional(e.target.checked)}
        />
        {S.ontology.functional}
      </label>
      <label className="flex items-center gap-2 text-sm text-neutral-300">
        <input
          type="checkbox"
          checked={inverseFunctional}
          onChange={(e) => setInverseFunctional(e.target.checked)}
        />
        {S.ontology.inverseFunctional}
      </label>
      <div>
        <label className={lbl}>{S.ontology.description}</label>
        <textarea
          className="input-dark w-full px-3 py-2 text-sm min-h-[4.5rem] resize-y"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
        <p className="mt-1 text-[10.5px] text-neutral-600">
          {S.ontology.descriptionHint}
        </p>
      </div>
      <div className="flex gap-2 pt-1">
        <Button
          size="sm"
          onClick={() => save.mutate()}
          disabled={save.isPending || !label.trim()}
        >
          {S.ontology.save}
        </Button>
        {existing && !existing.builtin && (
          <Button
            size="sm"
            variant="ghost"
            disabled={existing.usage > 0}
            title={existing.usage > 0 ? S.ontology.deleteBlocked : undefined}
            onClick={() => remove.mutate()}
          >
            {S.ontology.delete}
          </Button>
        )}
      </div>
    </div>
  );
}

/* ---------- 未匹配信号 + AI 建议 ---------- */

function MissesPanel({
  kbId,
  misses,
  dismissedMisses,
  onChanged,
  onError,
}: {
  kbId: string;
  misses: OntologyMiss[];
  dismissedMisses: OntologyMiss[];
  onChanged: () => void;
  onError: (e: unknown) => void;
}) {
  // 默认收起：已忽略的是**背景信息**，不该跟待处理的挤在一起抢注意力
  const [showDismissed, setShowDismissed] = useState(false);
  const [proposals, setProposals] = useState<OntologyProposals | null>(null);
  // 上次算出来、还没人表态的那些：刷新页面之后从库里捞回来（0049）。
  //
  // 从前这里只有上面那个 useState——刷新一次、切走一次，整批提议就没了，
  // 想再看只能重跑一次模型，而重跑未必给出同一批归并。归并了哪些说法正是
  // 唯一能查证过并的东西（0003 的 optimized_for → runs_on 就是这么抓出来的）
  const storedProposals = useQuery({
    queryKey: ["storedProposals", kbId],
    queryFn: () => api.storedProposals(kbId),
  });
  useEffect(() => {
    // 只在还没有本地结果时回填。刚点完 Suggest 的那一批更新，不该被覆盖
    if (proposals === null && storedProposals.data) {
      const d = storedProposals.data;
      const empty =
        !d.entity_types?.length &&
        !d.relation_types?.length &&
        !d.attribute_types?.length;
      if (!empty) setProposals(d);
    }
  }, [storedProposals.data, proposals]);
  // 最近一次采纳，供撤销。只留最近一次——旧批次要撤销走审计台账，
  // 那里本来就记着每次采纳动了哪个关系、多少条
  const [lastAdopt, setLastAdopt] = useState<{
    batches: string[];
    key: string;
    moved: number;
  } | null>(null);
  // 撤销要二次确认：一次改回成批事实
  const [confirmUndo, setConfirmUndo] = useState<{
    batches: string[];
    moved: number;
  } | null>(null);
  // 系统自动扩过本体没有——横幅要据此显示，撤干净了后端返回 null
  const autoRun = useQuery({
    queryKey: ["auto-extension", kbId],
    queryFn: () => api.lastAutoExtension(kbId),
  });
  // 待认领的表层谓词：提案的影响面（"将改写 57 条"）从这里算
  const surface = useQuery({
    queryKey: ["proposed-predicates", kbId],
    queryFn: () => api.proposedPredicates(kbId),
  });
  const factsWaiting = (forms: string[]) => {
    const byForm = new Map(
      (surface.data?.forms ?? []).map((f) => [f.form, f.fact_count]),
    );
    return forms.reduce((n, f) => n + (byForm.get(f) ?? 0), 0);
  };

  const suggest = useMutation({
    mutationFn: () => api.suggestOntology(kbId),
    onSuccess: setProposals,
    onError,
  });
  const dismiss = useMutation({
    mutationFn: ({ kind, key }: { kind: string; key: string }) =>
      api.dismissMiss(kbId, kind, key),
    onSuccess: onChanged,
    onError,
  });
  const restore = useMutation({
    mutationFn: ({ kind, key }: { kind: string; key: string }) =>
      api.restoreMiss(kbId, kind, key),
    onSuccess: onChanged,
    onError,
  });
  const approveEntity = useMutation({
    mutationFn: (p: { key: string; label: string; description?: string }) =>
      api.createEntityType(kbId, {
        key: p.key,
        label: p.label,
        description: p.description,
      }),
    onSuccess: (_data, p) => {
      // 已采纳：从提案列表移除，并顺带清掉对应的未匹配统计 chip（本体已覆盖）
      toast.success(S.toast.added);
      setProposals(
        (prev) =>
          prev && {
            ...prev,
            entity_types: prev.entity_types.filter((x) => x.key !== p.key),
          },
      );
      api.dismissMiss(kbId, "entity_type", p.key).catch(() => {});
      // 提案表态落库（0049）：下一轮 Suggest 不会把它刷回待看
      api.decideProposal(kbId, "entity_types", p.key, "adopted").catch(() => {});
      onChanged();
    },
    onError,
  });
  const approveRelation = useMutation({
    // 带 forms 的提案走 adopt：建关系顺带把等着它的无谓词事实认过去。
    // 只建关系的话本体长大了、图没变好——那些事实会继续是"有关联"
    mutationFn: (p: {
      key: string;
      label: string;
      temporal?: string;
      functional?: boolean;
      description?: string;
      forms?: string[];
    }) =>
      p.forms?.length
        ? api.adoptPredicate(kbId, {
            key: p.key,
            label: p.label,
            temporal: p.temporal ?? "state",
            functional: p.functional ?? false,
            description: p.description,
            forms: p.forms,
          })
        : api.createRelationType(kbId, {
            key: p.key,
            label: p.label,
            temporal: p.temporal ?? "state",
            functional: p.functional ?? false,
            description: p.description,
          }),
    onSuccess: (data, p) => {
      const d = data as { remapped?: number; batch?: string };
      const moved = d.remapped ?? 0;
      toast.success(moved > 0 ? S.ontology.adopted(moved) : S.toast.added);
      // 撤销的把手：采纳改写了成批事实，没有回头路的话没人敢点第一下
      if (moved > 0 && d.batch)
        setLastAdopt({ batches: [d.batch], key: p.key, moved });
      setProposals(
        (prev) =>
          prev && {
            ...prev,
            relation_types: prev.relation_types.filter((x) => x.key !== p.key),
          },
      );
      api.dismissMiss(kbId, "relation_type", p.key).catch(() => {});
      api.decideProposal(kbId, "relation_types", p.key, "adopted").catch(() => {});
      onChanged();
    },
    onError,
  });

  // 逐条串行而不是加个批量端点：每个谓词各有自己的批次和撤销粒度，
  // 而且部分失败能如实报告（"5 个成功，1 个 key 已存在"）而不是整批回滚
  // 属性提案：宾语是字面值的那些。走同一个采纳入口，但值要按 datatype
  // 换算，换不动的不改写——所以回执里的 unconvertible 必须说出来
  const approveAttribute = useMutation({
    mutationFn: (p: {
      key: string;
      label: string;
      datatype?: string;
      unit?: string;
      description?: string;
      forms?: string[];
    }) =>
      api.adoptPredicate(kbId, {
        key: p.key,
        kind: "attribute",
        label: p.label,
        datatype: p.datatype ?? "text",
        unit: p.unit,
        description: p.description,
        forms: p.forms ?? [],
      }),
    onSuccess: (data, p) => {
      const moved = data.remapped ?? 0;
      const left = data.unconvertible ?? 0;
      toast.success(
        left > 0
          ? S.ontology.adoptedPartly(moved, left)
          : moved > 0
            ? S.ontology.adopted(moved)
            : S.toast.added,
      );
      if (moved > 0 && data.batch)
        setLastAdopt({ batches: [data.batch], key: p.key, moved });
      setProposals(
        (prev) =>
          prev && {
            ...prev,
            attribute_types: (prev.attribute_types ?? []).filter(
              (x) => x.key !== p.key,
            ),
          },
      );
      for (const form of p.forms ?? [])
        api.dismissMiss(kbId, "attribute_type", form).catch(() => {});
      api.decideProposal(kbId, "attribute_types", p.key, "adopted").catch(() => {});
      onChanged();
    },
    onError,
  });
  // 映射到已有类型：不建东西，只把这些说法的事实挂过去。
  // 跟新建走同一个采纳入口，因为它对图做的事一模一样——也因此同样可撤销
  const approveMapping = useMutation({
    mutationFn: (p: { key: string; kind?: string; forms?: string[] }) =>
      api.adoptPredicate(kbId, {
        key: p.key,
        existing: true,
        // 目标是属性时值要按它的 datatype 换算，服务端据此分道
        kind: p.kind === "attribute" ? "attribute" : "relation",
        forms: p.forms ?? [],
      }),
    onSuccess: (data, p) => {
      const moved = data.remapped ?? 0;
      const left = data.unconvertible ?? 0;
      toast.success(
        left > 0
          ? S.ontology.adoptedPartly(moved, left)
          : moved > 0
            ? S.ontology.adopted(moved)
            : S.toast.saved,
      );
      if (moved > 0 && data.batch)
        setLastAdopt({ batches: [data.batch], key: p.key, moved });
      setProposals(
        (prev) =>
          prev && {
            ...prev,
            map_to: (prev.map_to ?? []).filter((x) => x.key !== p.key),
          },
      );
      for (const form of p.forms ?? [])
        api.dismissMiss(kbId, "relation_type", form).catch(() => {});
      onChanged();
    },
    onError,
  });
  const addAll = useMutation({
    mutationFn: async (all: OntologyProposals) => {
      const batches: string[] = [];
      let moved = 0;
      const failed: string[] = [];
      for (const p of all.entity_types) {
        try {
          await api.createEntityType(kbId, { key: p.key, label: p.label });
        } catch {
          failed.push(p.key);
        }
      }
      for (const p of all.relation_types) {
        try {
          if (p.forms?.length) {
            const r = await api.adoptPredicate(kbId, {
              key: p.key,
              label: p.label,
              temporal: p.temporal ?? "state",
              functional: p.functional ?? false,
              description: p.description,
              forms: p.forms,
            });
            moved += r.remapped;
            if (r.remapped > 0) batches.push(r.batch);
          } else {
            await api.createRelationType(kbId, {
              key: p.key,
              label: p.label,
              temporal: p.temporal ?? "state",
              functional: p.functional ?? false,
              description: p.description,
            });
          }
        } catch {
          failed.push(p.key);
        }
      }
      for (const p of all.attribute_types ?? []) {
        if (!p.forms?.length) continue;
        try {
          const r = await api.adoptPredicate(kbId, {
            key: p.key,
            kind: "attribute",
            label: p.label,
            datatype: p.datatype ?? "text",
            unit: p.unit,
            description: p.description,
            forms: p.forms,
          });
          moved += r.remapped;
          if (r.remapped > 0) batches.push(r.batch);
        } catch {
          failed.push(p.key);
        }
      }
      for (const p of all.map_to ?? []) {
        if (!p.forms?.length) continue;
        try {
          const r = await api.adoptPredicate(kbId, {
            key: p.key,
            existing: true,
            kind: p.kind === "attribute" ? "attribute" : "relation",
            forms: p.forms,
          });
          moved += r.remapped;
          if (r.remapped > 0) batches.push(r.batch);
        } catch {
          failed.push(p.key);
        }
      }
      return { batches, moved, failed };
    },
    onSuccess: (r) => {
      if (r.failed.length) toast.error(S.ontology.addAllPartial(r.failed));
      else toast.success(S.ontology.adopted(r.moved));
      if (r.batches.length)
        setLastAdopt({
          batches: r.batches,
          key: S.ontology.addAllLabel,
          moved: r.moved,
        });
      setProposals(null);
      onChanged();
    },
    onError,
  });

  const unadopt = useMutation({
    mutationFn: async (batches: string[]) => {
      let reverted = 0;
      for (const b of batches)
        reverted += (await api.unadoptPredicate(kbId, b)).reverted;
      return { reverted };
    },
    onSuccess: (r) => {
      toast.success(S.ontology.reverted(r.reverted));
      setLastAdopt(null);
      setConfirmUndo(null);
      autoRun.refetch();
      onChanged();
    },
    onError,
  });

  return (
    <div className="glass rounded-xl p-4">
      <div className="flex items-center gap-3 mb-1">
        <h3 className="text-sm font-bold text-neutral-200">
          {S.ontology.misses}
        </h3>
        {misses.length > 0 && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => suggest.mutate()}
            disabled={suggest.isPending}
          >
            {suggest.isPending ? S.ontology.suggesting : S.ontology.suggest}
          </Button>
        )}
      </div>
      <p className="text-xs text-neutral-500 mb-3">{S.ontology.missesHint}</p>

      {misses.length === 0 ? (
        <p className="text-sm text-neutral-500">{S.ontology.noMisses}</p>
      ) : (
        <div className="flex flex-wrap gap-1.5">
          {misses.map((m) => (
            <span
              key={`${m.kind}:${m.key}`}
              className="glass rounded-full px-2.5 py-1 text-xs flex items-center gap-1.5"
              title={m.example ?? ""}
            >
              <Chip tone={m.kind === "entity_type" ? "info" : "violet"}>
                {m.kind === "entity_type" ? "C" : "P"}
              </Chip>
              <span className="font-mono text-neutral-300">{m.key}</span>
              <span className="text-neutral-500">×{m.count}</span>
              <button
                onClick={() => dismiss.mutate({ kind: m.kind, key: m.key })}
                className="text-neutral-600 hover:text-neutral-300"
              >
                ✕
              </button>
            </span>
          ))}
        </div>
      )}
      {dismissedMisses.length > 0 && (
        <div className="mt-3 border-t border-white/5 pt-3">
          <button
            onClick={() => setShowDismissed((v) => !v)}
            className="text-xs text-neutral-500 hover:text-neutral-300"
          >
            {showDismissed ? "▾" : "▸"} {S.ontology.dismissed(dismissedMisses.length)}
          </button>
          {showDismissed && (
            <>
              <p className="text-xs text-neutral-600 mt-1.5 mb-2">
                {S.ontology.dismissedHint}
              </p>
              <div className="flex flex-wrap gap-1.5">
                {dismissedMisses.map((m) => (
                  <span
                    key={`d:${m.kind}:${m.key}`}
                    className="glass rounded-full px-2.5 py-1 text-xs flex items-center gap-1.5 opacity-60"
                    title={m.example ?? ""}
                  >
                    <Chip tone={m.kind === "entity_type" ? "info" : "violet"}>
                      {m.kind === "entity_type" ? "C" : "P"}
                    </Chip>
                    <span className="font-mono text-neutral-400 line-through">
                      {m.key}
                    </span>
                    <span className="text-neutral-500">×{m.count}</span>
                    <button
                      onClick={() => restore.mutate({ kind: m.kind, key: m.key })}
                      className="text-neutral-600 hover:text-neutral-200"
                      title={S.ontology.restore}
                    >
                      ↺
                    </button>
                  </span>
                ))}
              </div>
            </>
          )}
        </div>
      )}
      {/* 系统自己动了本体，必须让人看见——只记在审计台账里不算可见。
          默认开启的前提是它的动作可见且可退，这条横幅是"可见"那一半 */}
      {autoRun.data?.run && !lastAdopt && (
        <div className="mt-3 rounded-lg border border-[var(--u-accent)]/25 bg-[var(--u-accent)]/[0.06] px-3 py-2.5">
          <div className="flex items-start gap-2">
            <div className="min-w-0 flex-1">
              <p className="text-xs text-neutral-200">
                {S.ontology.autoRanTitle}
              </p>
              <p className="mt-0.5 text-[11px] text-neutral-400">
                {S.ontology.autoRanBody(
                  autoRun.data.run.relations ?? [],
                  autoRun.data.run.facts_remapped ?? 0,
                )}
              </p>
              <p className="mt-0.5 text-[11px] text-neutral-600">
                {S.ontology.autoRanOff}
              </p>
            </div>
            <Button
              size="sm"
              variant="ghost"
              disabled={unadopt.isPending}
              onClick={() =>
                setConfirmUndo({
                  batches: autoRun.data!.run!.batches,
                  moved: autoRun.data!.run!.facts_remapped ?? 0,
                })
              }
            >
              {S.ontology.undoAdoptBtn}
            </Button>
          </div>
        </div>
      )}

      {/* 采纳改写了成批事实——没有回头路的话没人敢点第一下 */}
      {lastAdopt && (
        <div className="mt-3 flex items-center gap-2 rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2">
          <span className="text-xs text-neutral-300">
            {S.ontology.undoAdopt(lastAdopt.key, lastAdopt.moved)}
          </span>
          <span className="text-[11px] text-neutral-600">
            {S.ontology.undoKeepsRelation}
          </span>
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto"
            disabled={unadopt.isPending}
            onClick={() =>
              setConfirmUndo({
                batches: lastAdopt.batches,
                moved: lastAdopt.moved,
              })
            }
          >
            {S.ontology.undoAdoptBtn}
          </Button>
        </div>
      )}

      {/* 撤销一次改回成批事实：轻确认——它本身也是可逆的，不必打字解锁 */}
      {confirmUndo && (
        <DangerConfirm
          title={S.ontology.undoTitle}
          hint={S.ontology.undoHint(confirmUndo.moved)}
          confirmLabel={S.ontology.undoConfirm}
          cancelLabel={S.ontology.undoCancel}
          busy={unadopt.isPending}
          onConfirm={() => unadopt.mutate(confirmUndo.batches)}
          onCancel={() => setConfirmUndo(null)}
        />
      )}
      {proposals && (
        <div className="mt-4 border-t border-white/10 pt-3">
          <div className="mb-2 flex items-center gap-2">
            <h4 className="text-xs font-bold text-neutral-400">
              {S.ontology.proposals}
            </h4>
            {/* 常见情形是"这些都对"——一条条点是把一个决定拆成八个 */}
            {proposals.relation_types.length +
              proposals.entity_types.length +
              (proposals.attribute_types?.length ?? 0) +
              (proposals.map_to?.length ?? 0) >
              1 && (
              <Button
                size="sm"
                variant="ghost"
                className="ml-auto"
                disabled={addAll.isPending}
                onClick={() => addAll.mutate(proposals)}
              >
                {addAll.isPending
                  ? S.ontology.addingAll
                  : S.ontology.addAll(
                      proposals.relation_types.length +
                        proposals.entity_types.length +
                        (proposals.attribute_types?.length ?? 0) +
                        (proposals.map_to?.length ?? 0),
                    )}
              </Button>
            )}
          </div>
          <div className="space-y-1.5">
            {/* 排在最前：它说的是"本体已经有了"，而这正是最该先看见的一句。
                排在新建后面的话，人一路点下来就把重复建出来了 */}
            {(proposals.map_to ?? []).map((p) => (
              <div key={`map-${p.key}`} className="flex items-center gap-2 text-sm">
                <Chip tone="success">=</Chip>
                <span className="font-mono text-neutral-300">{p.key}</span>
                {!!p.forms?.length && (
                  <span
                    className="text-xs text-neutral-400 truncate"
                    title={p.forms.join(" · ")}
                  >
                    {p.forms.join(" · ")}
                  </span>
                )}
                {!!p.forms?.length && (
                  <span className="text-xs text-[var(--u-accent)]">
                    {S.ontology.willRemap(factsWaiting(p.forms))}
                  </span>
                )}
                {p.reason && (
                  <span className="text-xs text-neutral-500 truncate">
                    {p.reason}
                  </span>
                )}
                <Button
                  size="sm"
                  className="ml-auto"
                  onClick={() => approveMapping.mutate(p)}
                  disabled={approveMapping.isPending}
                >
                  {S.ontology.mapOver}
                </Button>
              </div>
            ))}
            {proposals.entity_types.map((p) => (
              <div key={p.key} className="flex items-center gap-2 text-sm">
                <Chip tone="info">C</Chip>
                <span className="font-mono text-neutral-300">{p.key}</span>
                <span className="text-neutral-200">{p.label}</span>
                {p.reason && (
                  <span className="text-xs text-neutral-500 truncate">
                    {p.reason}
                  </span>
                )}
                <Button
                  size="sm"
                  className="ml-auto"
                  onClick={() => approveEntity.mutate(p)}
                  disabled={approveEntity.isPending}
                >
                  {S.ontology.approve}
                </Button>
              </div>
            ))}
            {proposals.relation_types.map((p) => (
              <div key={p.key} className="flex items-center gap-2 text-sm">
                <Chip tone="violet">P</Chip>
                <span className="font-mono text-neutral-300">{p.key}</span>
                <span className="text-neutral-200">{p.label}</span>
                {p.temporal && <Chip tone="neutral">{p.temporal}</Chip>}
                {/* 影响面：采纳后会改写多少条、归并了哪些写法。没有这个，
                    "approve" 就只是凭空多一个空关系 */}
                {!!p.forms?.length && (
                  <span
                    className="text-xs text-[var(--u-accent)]"
                    title={p.forms.join(" · ")}
                  >
                    {S.ontology.willRemap(factsWaiting(p.forms))}
                  </span>
                )}
                {p.reason && (
                  <span className="text-xs text-neutral-500 truncate">
                    {p.reason}
                  </span>
                )}
                <Button
                  size="sm"
                  className="ml-auto"
                  onClick={() => approveRelation.mutate(p)}
                  disabled={approveRelation.isPending}
                >
                  {S.ontology.approve}
                </Button>
              </div>
            ))}
            {(proposals.attribute_types ?? []).map((p) => (
              <div key={`attr-${p.key}`} className="flex items-center gap-2 text-sm">
                {/* A 而不是 P：字面值那一档跟关系是两回事，界面上分得清 */}
                <Chip tone="warn">A</Chip>
                <span className="font-mono text-neutral-300">{p.key}</span>
                <span className="text-neutral-200">{p.label}</span>
                <Chip tone="neutral">{p.datatype ?? "text"}</Chip>
                {p.unit && <Chip tone="neutral">{p.unit}</Chip>}
                {!!p.forms?.length && (
                  <span
                    className="text-xs text-[var(--u-accent)]"
                    title={p.forms.join(" · ")}
                  >
                    {S.ontology.willRemap(factsWaiting(p.forms))}
                  </span>
                )}
                {p.reason && (
                  <span className="text-xs text-neutral-500 truncate">
                    {p.reason}
                  </span>
                )}
                <Button
                  size="sm"
                  className="ml-auto"
                  onClick={() => approveAttribute.mutate(p)}
                  disabled={approveAttribute.isPending}
                >
                  {S.ontology.approve}
                </Button>
              </div>
            ))}
            {proposals.entity_types.length === 0 &&
              proposals.relation_types.length === 0 &&
              !proposals.attribute_types?.length &&
              !proposals.map_to?.length && (
                <p className="text-sm text-neutral-500">—</p>
              )}
          </div>
        </div>
      )}
    </div>
  );
}

/* ---------- 本体导入：上传 → 预览计划 → 确认落库 ---------- */
/* 预览与落库共用服务端同一个 plan。这个面板的全部工作是把计划里
   **会咬人的三件事**放到人点确认之前：函数性关系（错误的唯一性声明会造出
   成队假冲突）、没有描述的类（description 逐字进抽取提示词，缺了就静默抽差）、
   key 撞车（报告不解决——自动改名会让下次重导入认不出自己上次建的是哪个）。 */

function ImportPanel({
  kbId,
  onChanged,
  onError,
}: {
  kbId: string;
  onChanged: () => void;
  onError: (e: unknown) => void;
}) {
  const [file, setFile] = useState<File | null>(null);
  const pick = useRef<HTMLInputElement>(null);
  const queryClient = useQueryClient();

  const history = useQuery({
    queryKey: ["ontology-imports", kbId],
    queryFn: () => api.ontologyImports(kbId),
  });

  const preview = useMutation({
    mutationFn: (f: File) => api.previewOntologyImport(kbId, f),
    onError: (e) => {
      setFile(null);
      onError(e);
    },
  });

  const apply = useMutation({
    mutationFn: (f: File) => api.applyOntologyImport(kbId, f),
    onSuccess: (res) => {
      const p = res.plan;
      toast.success(
        S.ontology.importDone(
          p.classes.filter((c) => c.disposition === "create").length,
          p.classes.filter((c) => c.disposition === "update").length,
        ),
      );
      setFile(null);
      preview.reset();
      queryClient.invalidateQueries({ queryKey: ["ontology-imports", kbId] });
      onChanged();
    },
    onError,
  });

  const choose = (f: File | undefined) => {
    if (!f) return;
    setFile(f);
    preview.mutate(f);
  };

  const plan = preview.data?.plan ?? null;
  const busy = preview.isPending || apply.isPending;
  const empty =
    plan &&
    plan.classes.length === 0 &&
    plan.relations.length === 0 &&
    plan.attributes.length === 0;

  return (
    <div className="glass rounded-xl p-4">
      <h3 className="text-sm font-bold text-neutral-200 mb-1">
        {S.ontology.importTitle}
      </h3>
      <p className="text-xs text-neutral-500 mb-3">{S.ontology.importHint}</p>

      <input
        ref={pick}
        type="file"
        accept=".owl,.rdf,.ttl,.xml,.n3"
        className="hidden"
        onChange={(e) => {
          choose(e.target.files?.[0]);
          e.target.value = "";
        }}
      />
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          variant="ghost"
          disabled={busy}
          onClick={() => pick.current?.click()}
        >
          {file ? S.ontology.importChange : S.ontology.importPick}
        </Button>
        {file && (
          <span className="text-xs text-neutral-400 truncate">
            <span className="font-mono">{file.name}</span>
            <span className="text-neutral-600">
              {" "}
              · {S.ontology.importSize(file.size)}
            </span>
          </span>
        )}
        {preview.isPending && (
          <span className="text-xs text-neutral-500">
            {S.ontology.importReading}
          </span>
        )}
      </div>

      {plan && (
        <div className="mt-4">
          <p className="text-[11px] uppercase tracking-[0.08em] text-neutral-600 u-num">
            {S.ontology.importParsed(plan.format, plan.triples)}
          </p>

          {empty ? (
            <p className="mt-2 text-sm text-neutral-500">
              {S.ontology.importNothing}
            </p>
          ) : (
            <>
              {/* 三条警告在计数之前：人只会读第一屏 */}
              <Warning
                show={plan.functional_relations > 0}
                tone="warn"
                title={S.ontology.warnFunctional(plan.functional_relations)}
                body={S.ontology.warnFunctionalBody}
                items={plan.relations
                  .filter((r) => r.functional)
                  .map((r) => r.key)}
              />
              <Warning
                show={plan.classes_without_description > 0}
                tone="warn"
                title={S.ontology.warnNoDescription(
                  plan.classes_without_description,
                )}
                body={S.ontology.warnNoDescriptionBody}
                items={plan.classes
                  .filter((c) => !c.has_description)
                  .map((c) => c.key)}
              />
              <Warning
                show={takenCount(plan) > 0}
                tone="danger"
                title={S.ontology.warnKeyTaken(takenCount(plan))}
                body={S.ontology.warnKeyTakenBody}
                items={[...plan.classes, ...plan.relations, ...plan.attributes]
                  .filter((i) => i.disposition === "key_taken")
                  .map(
                    (i) =>
                      `${i.key} — ${S.ontology.importTakenBy(i.conflict_with ?? null)}`,
                  )}
              />

              <div className="mt-3 grid gap-2">
                <PlanRow
                  label={S.ontology.importClasses}
                  items={plan.classes}
                />
                <PlanRow
                  label={S.ontology.importRelations}
                  items={plan.relations}
                />
                <PlanRow
                  label={S.ontology.importAttributes}
                  items={plan.attributes}
                  note={
                    plan.attributes.length > 0
                      ? S.ontology.importAttributesLater
                      : undefined
                  }
                />
              </div>

              {plan.unprojected.length > 0 && (
                <details className="mt-3">
                  <summary className="cursor-pointer text-xs text-neutral-500 hover:text-neutral-300">
                    {S.ontology.importUnprojected} ({plan.unprojected.length})
                  </summary>
                  <p className="mt-1.5 text-[11px] text-neutral-600">
                    {S.ontology.importUnprojectedBody}
                  </p>
                  <ul className="mt-1.5 space-y-0.5">
                    {plan.unprojected.map(([iri, n]) => (
                      <li key={iri} className="flex gap-2 text-[11px]">
                        <span
                          className="font-mono text-neutral-500 truncate"
                          title={iri}
                        >
                          {shortIri(iri)}
                        </span>
                        <span className="u-num text-neutral-600 shrink-0">
                          ×{n}
                        </span>
                      </li>
                    ))}
                  </ul>
                </details>
              )}

              <div className="mt-4 flex items-center gap-2">
                <Button
                  size="sm"
                  disabled={busy}
                  onClick={() => file && apply.mutate(file)}
                >
                  {apply.isPending
                    ? S.ontology.importApplying
                    : S.ontology.importApply}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => {
                    setFile(null);
                    preview.reset();
                  }}
                >
                  {S.ontology.importCancel}
                </Button>
              </div>
            </>
          )}
        </div>
      )}

      {/* 导入历史：谁在什么时候拿哪个文件动过本体。原文按 sha256 存着 */}
      <div className="mt-5 border-t border-white/10 pt-3">
        <h4 className="text-xs font-medium text-neutral-400 mb-2">
          {S.ontology.importHistory}
        </h4>
        {!history.data?.imports.length ? (
          <p className="text-xs text-neutral-600">
            {S.ontology.importNoHistory}
          </p>
        ) : (
          <ul className="space-y-1.5">
            {history.data.imports.map((im) => (
              <li key={im.id} className="flex items-baseline gap-2 text-xs">
                <span className="font-mono text-neutral-300 truncate">
                  {im.filename}
                </span>
                <span className="u-num text-neutral-600 shrink-0">
                  {S.ontology.importSize(im.byte_size)}
                </span>
                <span className="ml-auto text-[11px] text-neutral-600 shrink-0">
                  {S.ontology.importBy(
                    im.imported_by_name ?? "—",
                    new Date(im.imported_at).toLocaleDateString(),
                  )}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function takenCount(p: ImportPlan) {
  return [...p.classes, ...p.relations, ...p.attributes].filter(
    (i) => i.disposition === "key_taken",
  ).length;
}

/** IRI 尾巴才是人认得出的部分，前缀在列表里只占宽度 */
function shortIri(iri: string) {
  const i = Math.max(iri.lastIndexOf("#"), iri.lastIndexOf("/"));
  return i < 0 ? iri : iri.slice(i + 1);
}

/** 一条警告：标题给数，正文一句给后果，条目折在 details 里 */
function Warning({
  show,
  tone,
  title,
  body,
  items,
}: {
  show: boolean;
  tone: "warn" | "danger";
  title: string;
  body: string;
  items: string[];
}) {
  if (!show) return null;
  return (
    <div
      className={cn(
        "mt-3 rounded-lg border px-3 py-2.5",
        tone === "danger"
          ? "border-rose-500/25 bg-rose-500/[0.06]"
          : "border-amber-500/25 bg-amber-500/[0.06]",
      )}
    >
      <p className="text-xs text-neutral-200">{title}</p>
      <p className="mt-0.5 text-[11px] text-neutral-400">{body}</p>
      {items.length > 0 && (
        <details className="mt-1.5">
          <summary className="cursor-pointer text-[11px] text-neutral-500 hover:text-neutral-300">
            {items.length > 1 ? `${items.length} items` : "1 item"}
          </summary>
          <ul className="mt-1 space-y-0.5">
            {items.map((it) => (
              <li key={it} className="font-mono text-[11px] text-neutral-400">
                {it}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

/** 一段的去向计数：新建 / 更新 / 跳过，零的不显示 */
function PlanRow({
  label,
  items,
  note,
}: {
  label: string;
  items: PlannedItem[];
  note?: string;
}) {
  if (items.length === 0) return null;
  const n = (d: PlannedItem["disposition"]) =>
    items.filter((i) => i.disposition === d).length;
  return (
    <div className="rounded-lg bg-white/[0.03] px-3 py-2">
      <div className="flex items-center gap-2">
        <span className="text-xs text-neutral-300">{label}</span>
        <span className="ml-auto flex items-center gap-1.5">
          {n("create") > 0 && (
            <Chip tone="success">
              {S.ontology.importWillCreate(n("create"))}
            </Chip>
          )}
          {n("update") > 0 && (
            <Chip tone="info">{S.ontology.importWillUpdate(n("update"))}</Chip>
          )}
          {n("key_taken") > 0 && (
            <Chip tone="neutral">
              {S.ontology.importKeyTaken(n("key_taken"))}
            </Chip>
          )}
        </span>
      </div>
      {note && <p className="mt-1 text-[11px] text-neutral-600">{note}</p>}
    </div>
  );
}
