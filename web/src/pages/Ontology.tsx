// 本体编辑器：master-detail 双栏（与 Library 的 SourcesRail 同构）。
// 左栏 = filter + Classes/Properties 两小节 + 底部 Unmatched 入口；
// 右侧 = 选中项的表单 / 未匹配信号面板 / 概览。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  ArrowLeft,
  ArrowRight,
  ChevronRight,
  Inbox,
  Scale,
  Network,
  Plus,
  Search,
  Upload,
  Wand2,
  X,
} from "lucide-react";
import {
  api,
  type EntityTypeView,
  type ImportPlan,
  type OntologyMiss,
  type PlannedItem,
  type OntologyProposals,
  type ResolutionOutcome,
  type TypeSuggestion,
  type RelationTypeView,
} from "../api";
import { S } from "../i18n";
import { useKb } from "../kb";
import { toast } from "../toast";
import { OntologySchemaGraph, type SchemaSelection } from "./OntologySchemaGraph";
import { RulesPanel } from "./RulesPanel";
import {
  Button,
  Chip,
  ColorPicker,
  colorForKey,
  DangerConfirm,
  Dropdown,
  Input,
  Loading,
  MultiSearchSelect,
  Pager,
  RAIL_CLS,
  SearchSelect,
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

/** 右侧详情区当前展示什么。
 *
 *  class / relation / new-class / new-relation / schema / null 这一组共享
 *  同一块工作区：模式图常驻做背景，表单（如果有）停靠在右侧——左栏点一个
 *  类名、画布上点一个节点、模式图自己的搜索框选中一个关系，三条路径落到
 *  的是同一个 `sel`，因此也落到同一份表单，不必再各画一遍。
 *  import / refine / misses 仍是独立的整页视图：那三个不是「关于某个类
 *  或关系」的事，跟模式图没有共同的背景可言。 */
type Sel =
  | { kind: "class"; id: string }
  | { kind: "relation"; id: string }
  | { kind: "new-class"; parentId: string | null }
  // 从模式图上一个类发起「新建关系」时带上它的 id，表单里 domain 预填成它
  | { kind: "new-relation"; initialDomain?: string | null }
  | { kind: "misses" }
  // 类型消解：把「大致对」的类换成更具体的那个
  | { kind: "rules" }
  | { kind: "refine" }
  | { kind: "import" }
  // 模式图无选中：看整张图,不停靠表单
  | { kind: "schema" }
  | null;

/** 这次选中会不会在模式图右侧停靠一张表单 */
const onPanel = (s: Sel) =>
  s?.kind === "class" ||
  s?.kind === "relation" ||
  s?.kind === "new-class" ||
  s?.kind === "new-relation";

export function Ontology() {
  const { kb } = useKb();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [sel, setSel] = useState<Sel>(null);
  const [railTab, setRailTab] = useState<"classes" | "properties">("classes");
  // 模式图详情面板停在哪一段。**跨选中保留**：在实例上挨个类看下去，
  // 是一种真实的读法，每换一个类就被弹回定义页会打断它
  const [panelTab, setPanelTab] = useState<
    "definition" | "properties" | "instances"
  >("definition");
  /** 正在退场的那次选择：面板演完 `u-dock-out` 再卸载，而不是一下子消失 */
  const [exitingSel, setExitingSel] = useState<Sel>(null);
  // **当前选中从 ref 里取，不从闭包里取。** 模式图注册 sigma 事件的 effect
  // 只依赖 [schema]，`clickStage` 抓住的是那一次渲染的 onSelect；读闭包里的
  // sel 会把早就过期的那次选中当成"正在退场的东西"，于是点空白处时面板先
  // 冒出来再消失。图谱页的 `selectedRef` 是同一个理由
  const selRef = useRef<Sel>(null);
  useEffect(() => {
    selRef.current = sel;
  }, [sel]);
  const closePanel = useCallback(() => {
    const cur = selRef.current;
    if (!onPanel(cur)) return;
    setExitingSel(cur);
    setSel({ kind: "schema" });
    window.setTimeout(() => setExitingSel(null), 170);
  }, []);
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

  // 本体自洽检查是同步的纯计算（Review 页的「Run check」用的是同一个接口），
  // 不排队、不需要确认——每次本体改动后顺手跑一遍，把新增的矛盾在它们
  // 发生的当下就说出来，而不是等人某天想起去 Review 才看见。
  // 干净就不出声：**只在有新东西时才说话**，否则每次保存都弹一条「没事」
  // 比不弹还烦
  const runCheck = useMutation({
    mutationFn: () => api.runConsistencyCheck(kb!.id),
    onSuccess: (r) => {
      if (r.defects_new > 0) {
        toast.info(S.ontology.schemaCheckDefects(r.defects_new), {
          label: S.ontology.schemaCheckReview,
          onClick: () =>
            navigate({ to: "/kb/$kbId/review", params: { kbId: kb!.id } }),
        });
      }
    },
    // 检查本身失败不该盖过「保存成功」——本体的写已经落了盘，静默重试留给下次改动
  });
  const afterOntologyChange = () => {
    refresh();
    runCheck.mutate();
  };

  if (!kb) return <Loading>{S.nav.loading}</Loading>;
  if (data.isPending) return <Loading>{S.nav.loading}</Loading>;
  if (data.isError) return <Loading>{(data.error as Error).message}</Loading>;

  const { entity_types, relation_types, misses, dismissed_misses } = data.data;
  // 属性不进 Properties 列表：它们挂在类下，在类详情区编辑
  const relations = relation_types.filter((r) => r.kind !== "attribute");
  // 面板要演完退场再卸载：一取消选中就 unmount 是瞬间消失（图谱页同一个做法）。
  // 在两个类之间切换不走这条路——面板留在原地换内容，每点一个节点抖一下反而吵
  const panelSel = onPanel(sel) ? sel : exitingSel;

  const selectedClass =
    panelSel?.kind === "class"
      ? (entity_types.find((t) => t.id === panelSel.id) ?? null)
      : null;
  const selectedProp =
    panelSel?.kind === "relation"
      ? (relation_types.find((r) => r.id === panelSel.id) ?? null)
      : null;
  // 选中类身上挂着的东西，面板分段的计数和内容都用它
  const classRelations = selectedClass
    ? relations.filter(
        (r) =>
          r.domains.includes(selectedClass.id) ||
          r.ranges.includes(selectedClass.id),
      )
    : [];
  const classAttributes = selectedClass
    ? relation_types.filter(
        (r) => r.kind === "attribute" && r.domains.includes(selectedClass.id),
      )
    : [];
  // 新建的类还没有关系也没有实例，只剩定义这一段
  const classTab = selectedClass ? panelTab : "definition";

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
              className="input-dark w-full pl-7 pr-2 py-[7px] text-xs"
              placeholder={S.ontology.filter}
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          </div>
        </div>
        {/* 模式图：本体结构的主视图,不是 Import/Refine/Unmatched 那种管理性操作——
            放在筛选框正下方、列表上方,与那三个钉在底部的按钮拉开位置,
            视觉上就说明了「这是浏览本体的另一种方式」而不是「这是一项维护动作」 */}
        <div className="px-3 pb-2">
          <button
            onClick={() => setSel({ kind: "schema" })}
            className={cn(
              "w-full flex items-center gap-2 rounded-lg px-2.5 py-2 text-[13px] font-medium transition-colors",
              sel?.kind === "schema"
                ? "u-nav-active"
                : "text-neutral-400 hover:bg-white/[0.05] hover:text-neutral-200",
            )}
          >
            <Network size={14} className="text-neutral-500" />
            {S.ontology.schemaDiagram}
          </button>
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
        {/* 业务规则（0021）：本体说「有这么个类」，规则说「什么样的实体算它」。
            所以它归在本体这一页，而不是另开一处——判据与词汇是同一份契约 */}
        <button
          onClick={() => setSel({ kind: "rules" })}
          className={cn(
            "shrink-0 border-t border-white/10 px-4 py-2.5 flex items-center gap-2 text-[13px] transition-colors",
            sel?.kind === "rules"
              ? "u-nav-active"
              : "text-neutral-400 hover:bg-white/[0.05] hover:text-neutral-200",
          )}
        >
          <Scale size={14} className="text-neutral-500" />
          <span>{S.ontology.rulesShort}</span>
        </button>
        {/* 类型消解：把「大致对」的类换成更具体的那个。**紧挨着未匹配**——
            两者都是「本体与数据对不齐」的处置，只是方向相反：那个是本体缺东西，
            这个是本体有更好的选项没被用上 */}
        <button
          onClick={() => setSel({ kind: "refine" })}
          className={cn(
            "shrink-0 border-t border-white/10 px-4 py-2.5 flex items-center gap-2 text-[13px] transition-colors",
            sel?.kind === "refine"
              ? "u-nav-active"
              : "text-neutral-400 hover:bg-white/[0.05] hover:text-neutral-200",
          )}
        >
          <Wand2 size={14} className="text-neutral-500" />
          <span>{S.ontology.refineShort}</span>
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

      {/* 右侧：详情。class/relation/new-class/new-relation/schema/概览共享
          同一块工作区——模式图常驻做背景，表单（有的话）停靠右侧，近实底
          （u-modal-panel），不是浮层玻璃：画布是动的，透出来的内容会跟
          正在读的表单字段打架。左栏点类名、画布点节点、模式图自带的搜索框
          选中关系，三条路径落到同一个 sel，也就落到同一份表单——
          不再各画一遍。import/refine/misses 仍是独立整页视图,那三个
          不是「关于某个类或关系」的事，没有跟模式图共享背景的道理 */}
      {sel?.kind === "import" || sel?.kind === "refine" || sel?.kind === "misses" || sel?.kind === "rules" ? (
        <div className="flex-1 min-w-0 overflow-y-auto u-scroll px-8 py-6">
          <div className="max-w-6xl">
            {sel.kind === "import" ? (
              <div className="max-w-xl">
                <ImportPanel kbId={kb.id} onChanged={refresh} onError={onError} />
              </div>
            ) : sel.kind === "refine" ? (
              <div className="max-w-2xl">
                <RefinePanel kbId={kb.id} onChanged={refresh} onError={onError} />
              </div>
            ) : sel.kind === "rules" ? (
              <div className="max-w-2xl">
                <RulesPanel
                  kbId={kb.id}
                  classes={entity_types}
                  attributes={relation_types.filter((r) => r.kind === "attribute")}
                  onError={onError}
                />
              </div>
            ) : (
              <div className="max-w-xl">
                <MissesPanel
                  kbId={kb.id}
                  misses={misses}
                  dismissedMisses={dismissed_misses ?? []}
                  onChanged={refresh}
                  onError={onError}
                />
              </div>
            )}
          </div>
        </div>
      ) : (
        <div className="flex-1 min-w-0 relative">
          <OntologySchemaGraph
            entityTypes={entity_types}
            relationTypes={relation_types}
            selected={
              sel?.kind === "class" || sel?.kind === "relation"
                ? ({ kind: sel.kind, id: sel.id } as SchemaSelection)
                : null
            }
            // 点空白处取消选中走的也是 closePanel：不然点画布关掉的面板没有
            // 退场动画，而点画布正是最常用的那种关法
            onSelect={(next) => (next ? setSel(next) : closePanel())}
          />
          {(panelSel?.kind === "new-class" || selectedClass) && (
            <DockedPanel
              onClose={closePanel}
              exiting={!onPanel(sel)}
              header={
                <PanelHeader
                  color={selectedClass?.color}
                  square={selectedClass?.shape === "square"}
                  title={selectedClass?.label ?? S.ontology.newClass}
                  builtin={selectedClass?.builtin}
                />
              }
              tabs={
                selectedClass && (
                  <>
                    <PanelTab
                      active={classTab === "definition"}
                      onClick={() => setPanelTab("definition")}
                    >
                      {S.ontology.schemaTabDefinition}
                    </PanelTab>
                    <PanelTab
                      active={classTab === "properties"}
                      count={classRelations.length + classAttributes.length}
                      onClick={() => setPanelTab("properties")}
                    >
                      {S.ontology.schemaTabProperties}
                    </PanelTab>
                    <PanelTab
                      active={classTab === "instances"}
                      count={selectedClass.usage}
                      onClick={() => setPanelTab("instances")}
                    >
                      {S.ontology.schemaTabInstances}
                    </PanelTab>
                  </>
                )
              }
            >
              {/* 三段用 hidden 藏，不卸载：表单里改了一半的字段、实例列表翻到的
                  第几页，都该在切回来的时候还在 */}
              <div className={classTab === "definition" ? "" : "hidden"}>
                <ClassForm
                  key={
                    selectedClass?.id ??
                    `new-${panelSel?.kind === "new-class" ? panelSel.parentId : "root"}`
                  }
                  kbId={kb.id}
                  existing={selectedClass}
                  parentId={
                    panelSel?.kind === "new-class"
                      ? panelSel.parentId
                      : (selectedClass?.primary_parent ?? null)
                  }
                  allTypes={entity_types}
                  headless
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
                        createdId
                          ? { kind: "class", id: createdId }
                          : { kind: "schema" },
                      );
                    afterOntologyChange();
                  }}
                  onError={onError}
                />
              </div>
              {selectedClass && (
                <>
                  <div
                    className={
                      classTab === "properties" ? "flex flex-col gap-3" : "hidden"
                    }
                  >
                    <RelationshipsCard
                      kbId={kb.id}
                      cls={selectedClass}
                      relations={relations}
                      allTypes={entity_types}
                      onChanged={afterOntologyChange}
                      onError={onError}
                      onSelect={(id) => setSel({ kind: "relation", id })}
                      onAddNew={() =>
                        setSel({
                          kind: "new-relation",
                          initialDomain: selectedClass.id,
                        })
                      }
                    />
                    <AttributesCard
                      kbId={kb.id}
                      type={selectedClass}
                      attributes={classAttributes}
                      onChanged={afterOntologyChange}
                      onError={onError}
                    />
                  </div>
                  <div className={classTab === "instances" ? "" : "hidden"}>
                    <InstancesCard kbId={kb.id} type={selectedClass} />
                  </div>
                </>
              )}
            </DockedPanel>
          )}
          {(panelSel?.kind === "new-relation" || selectedProp) && (
            <DockedPanel
              onClose={closePanel}
              exiting={!onPanel(sel)}
              header={
                <PanelHeader
                  title={selectedProp?.label ?? S.ontology.newProperty}
                  sub={
                    selectedProp
                      ? S.ontology.usage(selectedProp.usage)
                      : undefined
                  }
                  builtin={selectedProp?.builtin}
                />
              }
            >
              <div>
                <PropertyForm
                  key={selectedProp?.id ?? "new"}
                  kbId={kb.id}
                  existing={selectedProp}
                  allTypes={entity_types}
                  allRelations={relations}
                  initialDomain={
                    panelSel?.kind === "new-relation" ? panelSel.initialDomain : null
                  }
                  headless
                  onDone={(createdId) => {
                    if (sel?.kind === "new-relation")
                      setSel(
                        createdId
                          ? { kind: "relation", id: createdId }
                          : { kind: "schema" },
                      );
                    afterOntologyChange();
                  }}
                  onError={onError}
                />
              </div>
            </DockedPanel>
          )}
        </div>
      )}
    </div>
  );
}

/* ---------- 停靠面板：模式图右侧的详情外壳 ----------
   与图谱页的实体面板同一个结构：**标题行固定，只有下面的内容滚**。整列一起滚
   的话，正在编辑的那个类叫什么、用了多少次、以及关闭键，都会滚出视野。
   关闭键跟着标题走，不再单开一行——那一行既占掉顶上的高度，又让按钮看着不
   属于任何一张卡片。

   面也跟图谱页一样是 `glass-strong`：**指针一放上去就压实**（120ms 进、260ms
   出，见 styles.css）。作者原本用近实底，理由是画布在动、透上来的动静会跟正在
   读的字段打架——那个顾虑成立，而玻璃的 hover 压实正是对它的回答：人在用这块
   面的时候它就是实的，手移开才透回去，画布的上下文也就没被一块死板挡住。

   退场也照图谱页：`u-dock-out` 演完再卸载，而不是一下子消失 */

function DockedPanel({
  header,
  tabs,
  exiting,
  onClose,
  children,
}: {
  header: React.ReactNode;
  /** 分段控件，跟着标题一起固定在顶上——它要能一直点得到 */
  tabs?: React.ReactNode;
  /** 正在退场：演动画，期间不再接受点击（u-dock-out 里带了 pointer-events） */
  exiting?: boolean;
  onClose: () => void;
  children: React.ReactNode;
}) {
  return (
    <div
      className={`${exiting ? "u-dock-out" : "u-dock-in"} glass-strong absolute top-3 right-3 bottom-3 w-[26rem] z-10 rounded-xl shadow-2xl flex flex-col`}
    >
      <div className="shrink-0 flex items-start justify-between gap-2 px-4 py-3.5 border-b border-white/10">
        <div className="min-w-0">{header}</div>
        <button
          onClick={onClose}
          title={S.ontology.schemaClosePanel}
          className="mt-0.5 shrink-0 text-neutral-500 hover:text-neutral-200 transition-colors"
        >
          <X size={15} />
        </button>
      </div>
      {tabs && (
        <div className="shrink-0 px-4 pt-3 pb-1">
          <div className="flex rounded-lg overflow-hidden border border-white/10">
            {tabs}
          </div>
        </div>
      )}
      <div className="u-scroll flex-1 min-h-0 overflow-y-auto flex flex-col gap-3 px-4 py-3">
        {children}
      </div>
    </div>
  );
}

/** 分段控件里的一格。与 Library / 图谱页那个是同一个写法 */
function PanelTab({
  active,
  count,
  onClick,
  children,
}: {
  active: boolean;
  count?: number;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={`flex-1 flex items-center justify-center gap-1.5 px-2 py-1.5 text-[11px] transition-colors ${
        active
          ? "bg-white/10 text-neutral-100"
          : "text-neutral-500 hover:bg-white/[0.05] hover:text-neutral-300"
      }`}
    >
      {children}
      {count !== undefined && count > 0 && (
        <span className="u-num text-neutral-500">{count}</span>
      )}
    </button>
  );
}

/* 面板标题：色点 + 名字 + 第二行的小字。与图谱页实体面板的头一个写法 */
function PanelHeader({
  color,
  square,
  title,
  sub,
  builtin,
}: {
  color?: string;
  square?: boolean;
  title: string;
  sub?: string;
  builtin?: boolean;
}) {
  return (
    <>
      <div className="flex items-center gap-2">
        {color && (
          <span
            className={`h-2.5 w-2.5 shrink-0 ${square ? "" : "rounded-full"}`}
            style={{ background: color, boxShadow: `0 0 8px ${color}55` }}
          />
        )}
        <span
          className="truncate text-[15px] font-semibold tracking-tight text-white"
          style={{ fontFamily: "var(--font-display)" }}
        >
          {title}
        </span>
        {builtin && <Chip tone="neutral">{S.ontology.builtin}</Chip>}
      </div>
      {sub && <div className="mt-1 text-xs text-neutral-500">{sub}</div>}
    </>
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
            to="/kb/$kbId/graph"
            params={{ kbId }}
            search={{ entity: e.id }}
            className="group flex items-center gap-2 -mx-2 px-2 py-1.5 rounded-lg text-sm text-neutral-300 transition-colors hover:bg-white/[0.05] hover:text-white"
          >
            <span
              className={`h-2 w-2 shrink-0 ${type.shape === "square" ? "" : "rounded-full"}`}
              style={{ background: type.color }}
            />
            <span className="truncate">{e.name}</span>
            <span className="ml-auto shrink-0 u-num text-[10.5px] text-neutral-600 transition-colors group-hover:text-neutral-400">
              {S.ontology.instanceFacts(e.fact_count)}
            </span>
          </Link>
        ))}
      </div>
      <Pager total={total} pageSize={PER} page={page} onPage={setPage} />
    </div>
  );
}

/* ---------- 关系卡片：这个类作为主语/宾语连着哪些关系 + 用已有关系接一条新的 ---------- */

/** 「新建关系」（PropertyForm）解决的是本体里还没有这条关系的情况；这张卡片
 *  解决另一半——**这个类身上已经挂着哪些关系**（从它出发的、指向它的），
 *  以及更常见的那种需求：不是每次都要一条新关系，而是把这个类接到一条
 *  已经存在的关系上（`works_at` 已经连着 Person→Organization，
 *  再建一个 Contractor 时多半是把 Contractor 也加进 works_at 的 domain，
 *  不是另建一条 works_at_2）。后一半直接改写已有关系的 domains/ranges，
 *  走的是 PropertyForm 保存时同一个 updateRelationType，只是把「打开表单、
 *  找到多选框、加一个类」压缩成一步。 */
function RelationshipsCard({
  kbId,
  cls,
  relations,
  allTypes,
  onChanged,
  onError,
  onSelect,
  onAddNew,
}: {
  kbId: string;
  cls: EntityTypeView;
  /** kind === "relation" 的那些——attribute 的宾语是字面值，谈不上「连着」 */
  relations: RelationTypeView[];
  allTypes: EntityTypeView[];
  onChanged: () => void;
  onError: (e: unknown) => void;
  onSelect: (relationId: string) => void;
  onAddNew: () => void;
}) {
  const labelOf = (id: string) =>
    allTypes.find((t) => t.id === id)?.label ?? id;
  // 自环关系（domain 和 range 都是这个类）两边都算——它确实两个方向都成立
  const outgoing = relations.filter((r) => r.domains.includes(cls.id));
  const incoming = relations.filter((r) => r.ranges.includes(cls.id));

  const [connectId, setConnectId] = useState("");
  const [connectSide, setConnectSide] = useState<"domain" | "range">("domain");
  useEffect(() => {
    setConnectId("");
  }, [cls.id]);

  const connect = useMutation({
    mutationFn: () => {
      const rel = relations.find((r) => r.id === connectId);
      if (!rel) return Promise.reject(new Error("relation not found"));
      // 只加不减：已经连着的那一侧原样保留，另一侧才可能被追加。
      // Set 去重——挑到已经连过的关系时这是个无害的空操作，不必先禁用按钮
      const domains =
        connectSide === "domain"
          ? [...new Set([...rel.domains, cls.id])]
          : rel.domains;
      const ranges =
        connectSide === "range"
          ? [...new Set([...rel.ranges, cls.id])]
          : rel.ranges;
      return api.updateRelationType(kbId, rel.id, {
        label: rel.label,
        temporal: rel.temporal,
        functional: rel.functional,
        inverse_functional: rel.inverse_functional,
        is_transitive: rel.is_transitive,
        is_symmetric: rel.is_symmetric,
        is_asymmetric: rel.is_asymmetric,
        is_irreflexive: rel.is_irreflexive,
        inverse_of: rel.inverse_of,
        sub_property_of: rel.sub_property_of,
        description: rel.description,
        domains,
        ranges,
      });
    },
    onSuccess: () => {
      const rel = relations.find((r) => r.id === connectId);
      toast.success(S.ontology.schemaConnected(rel?.label ?? ""));
      const id = connectId;
      setConnectId("");
      onChanged();
      // 连完直接跳到那条关系：域/值域改没改、改对了没有，一眼可见
      onSelect(id);
    },
    onError,
  });

  const RelationRow = ({ r, dir }: { r: RelationTypeView; dir: "out" | "in" }) => (
    // 悬停要有底色：这一行整条可点，只把文字提亮半级在深底上几乎看不出来。
    // 底色用左栏那一档（white/[0.05]），右端的类型小字跟着一起提亮
    <button
      onClick={() => onSelect(r.id)}
      className="group w-full flex items-center gap-1.5 -mx-2 px-2 py-1.5 rounded-lg text-sm text-left text-neutral-300 transition-colors hover:bg-white/[0.05] hover:text-white"
    >
      {dir === "out" ? (
        <ArrowRight size={12} className="shrink-0 text-[#c4a5ff]" />
      ) : (
        <ArrowLeft size={12} className="shrink-0 text-[#c4a5ff]" />
      )}
      <span className="truncate">{r.label}</span>
      <span className="ml-auto shrink-0 text-xs text-neutral-500 truncate max-w-[8rem] transition-colors group-hover:text-neutral-300">
        {(dir === "out" ? r.ranges : r.domains).map(labelOf).join(", ") ||
          S.ontology.anyType}
      </span>
    </button>
  );

  return (
    // 卡片在面板里边，底交给面板；这里与属性卡、实例卡同一个写法
    <div className="glass rounded-xl p-4">
      <div className="mb-1 flex items-baseline gap-2">
        <h3 className="text-sm font-bold text-neutral-200">
          {S.ontology.schemaRelationships}
        </h3>
        {outgoing.length + incoming.length > 0 && (
          <span className="u-num text-xs text-neutral-500">
            {outgoing.length + incoming.length}
          </span>
        )}
      </div>
      {outgoing.length === 0 && incoming.length === 0 ? (
        <p className="text-xs text-neutral-500 mb-2">
          {S.ontology.schemaNoRelationships}
        </p>
      ) : (
        <div className="mb-2">
          {outgoing.length > 0 && (
            <>
              <div className="text-[10px] uppercase tracking-[0.08em] text-neutral-600 mb-0.5">
                {S.ontology.schemaOutgoing}
              </div>
              <div className="divide-y divide-white/[0.06] mb-1.5">
                {outgoing.map((r) => (
                  <RelationRow key={`out:${r.id}`} r={r} dir="out" />
                ))}
              </div>
            </>
          )}
          {incoming.length > 0 && (
            <>
              <div className="text-[10px] uppercase tracking-[0.08em] text-neutral-600 mb-0.5">
                {S.ontology.schemaIncoming}
              </div>
              <div className="divide-y divide-white/[0.06]">
                {incoming.map((r) => (
                  <RelationRow key={`in:${r.id}`} r={r} dir="in" />
                ))}
              </div>
            </>
          )}
        </div>
      )}
      <div className="pt-2 border-t border-white/5">
        <p className="text-[11px] text-neutral-500 mb-1.5">
          {S.ontology.schemaConnectHint}
        </p>
        <div className="flex gap-1.5 mb-1.5">
          <SearchSelect
            value={connectId}
            onChange={setConnectId}
            options={relations.map((r) => ({
              value: r.id,
              label: r.label,
              hint: r.key,
            }))}
            size="sm"
            className="flex-1 min-w-0"
            placeholder={S.ontology.schemaConnectPlaceholder}
          />
        </div>
        <div className="flex items-center gap-1.5 mb-2">
          <span className="text-[11px] text-neutral-500">
            {S.ontology.schemaConnectAs}
          </span>
          <div className="flex rounded-lg overflow-hidden border border-white/10">
            {(["domain", "range"] as const).map((side) => (
              <button
                key={side}
                onClick={() => setConnectSide(side)}
                className={cn(
                  "px-2 py-1 text-[11px] transition-colors",
                  connectSide === side
                    ? "bg-white/[0.12] text-white"
                    : "text-neutral-500 hover:bg-white/[0.05] hover:text-neutral-300",
                )}
              >
                {side === "domain" ? S.ontology.domainLabel : S.ontology.rangeLabel}
              </button>
            ))}
          </div>
        </div>
        <Button
          size="sm"
          variant="ghost"
          disabled={!connectId || connect.isPending}
          onClick={() => connect.mutate()}
        >
          {S.ontology.schemaConnect}
        </Button>
      </div>
      <div className="mt-2 pt-2 border-t border-white/5">
        <button
          onClick={onAddNew}
          className="flex items-center gap-1.5 text-[13px] text-neutral-500 hover:text-neutral-200 transition-colors"
        >
          <Plus size={13} />
          {S.ontology.schemaAddRelationship}
        </button>
      </div>
    </div>
  );
}

/* ---------- 属性卡片：选中类的字面值字段（行内增改删） ---------- */

/** 导出给模式图复用：选中一个类时，检查器里嵌的就是这一张卡片本身，
 *  不是另一份只读摘要——编辑发生在同一处，不必跳回本体主视图 */
export function AttributesCard({
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
              className="w-full flex items-center gap-2 -mx-2 px-2 py-1.5 rounded-lg text-sm text-left text-neutral-300 transition-colors hover:bg-white/[0.05] hover:text-white"
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
          className="input-dark w-full px-3 py-2 text-sm min-h-[6rem] resize-y"
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

/** 导出给模式图复用（见 AttributesCard 上的注释） */
export function ClassForm({
  kbId,
  existing,
  parentId,
  allTypes,
  onNewSub,
  headless,
  onDone,
  onError,
}: {
  kbId: string;
  existing: EntityTypeView | null;
  parentId: string | null;
  allTypes: EntityTypeView[];
  /** 编辑已有类时提供：以当前类为父级新建子类 */
  onNewSub?: () => void;
  /** 停靠面板把标题画在自己的固定头里，表单就不再画第二遍 */
  headless?: boolean;
  /** 创建成功时携带新 id，编辑成功时为 undefined */
  onDone: (createdId?: string) => void;
  onError: (e: unknown) => void;
}) {
  const [key, setKey] = useState(existing?.key ?? "");
  const [label, setLabel] = useState(existing?.label ?? "");
  // 新建时颜色跟着 key 走（与后端 color_for_key 同一个规则），不是一个固定默认值。
  // 用户当然可以改；但**不改的话，手动建的类和导入建的类配色体系一致**
  const [color, setColor] = useState(
    existing?.color ?? colorForKey(existing?.key ?? ""),
  );
  const [colorTouched, setColorTouched] = useState(Boolean(existing?.color));
  const [shape, setShape] = useState<"circle" | "square">(
    existing?.shape ?? "circle",
  );
  const [parents, setParents] = useState<string[]>(
    existing?.parents ?? (parentId ? [parentId] : []),
  );
  const [description, setDescription] = useState(existing?.description ?? "");
  // 互斥：声明「不可能同时是」。一致性检查据此报不可满足的类（0002）——
  // 一个类继承了两个互斥的祖先，就永远不可能有实例，而它不报错，只是永远空着
  const [disjoint, setDisjoint] = useState<string[]>(existing?.disjoint ?? []);

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
            disjoint,
            description,
          })
        : api.createEntityType(kbId, {
            key,
            label,
            color,
            shape,
            parents,
            disjoint,
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
      {!headless && (
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
      )}
      {!existing && (
        <div>
          <label className={lbl}>
            {S.ontology.key}{" "}
            <span className="text-neutral-600">({S.ontology.keyHint})</span>
          </label>
          <Input
            value={key}
            onChange={(e) => {
              setKey(e.target.value);
              // 用户没自己挑过色，就让颜色跟着 key 走——与后端同一个规则
              if (!colorTouched) setColor(colorForKey(e.target.value));
            }}
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
          <ColorPicker value={color} onChange={(c: string) => { setColor(c); setColorTouched(true); }} shape={shape} />
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
      {/* 互斥：**声明「不可能同时是」**。紧挨着父类，因为两者是同一件事的
          两面——父类说「也是」，互斥说「不可能同时是」，而一致性检查正是
          在这两者打架时报出「这个类永远不可能有实例」 */}
      <div>
        <label className={lbl}>{S.ontology.disjoint}</label>
        <p className="text-[11px] leading-relaxed text-neutral-600 mb-1.5">
          {S.ontology.disjointHint}
        </p>
        <MultiSearchSelect
          values={disjoint}
          options={parentOptions(allTypes, existing?.id)}
          onToggle={(id) =>
            setDisjoint((v) =>
              v.includes(id) ? v.filter((x) => x !== id) : [...v, id],
            )
          }
          placeholder={S.ontology.searchTypes}
          emptyHint={S.ontology.noDisjoint}
        />
        {/* 跟自己的父类互斥 = 这个类永远不可能有实例。当场说，
            比让人跑一遍一致性检查再发现要快 */}
        {disjoint.some((d) => parents.includes(d)) && (
          <p className="mt-1.5 text-[11px] text-[var(--u-danger)]">
            {S.ontology.disjointWithParent}
          </p>
        )}
      </div>
      <div>
        <label className={lbl}>{S.ontology.description}</label>
        {/* 语义指引：整段注入抽取 prompt，直接影响抽取归类质量 */}
        <textarea
          className="input-dark w-full px-3 py-2 text-sm min-h-[9rem] resize-y"
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

/** 导出给模式图复用（见 AttributesCard 上的注释） */
export function PropertyForm({
  kbId,
  existing,
  allTypes,
  allRelations,
  initialDomain,
  headless,
  onDone,
  onError,
}: {
  kbId: string;
  existing: RelationTypeView | null;
  allTypes: EntityTypeView[];
  /** 本库的关系（不含属性）。逆与子属性的下拉从这里取——**属性不在其中**，
   *  它的宾语是字面值，反过来无从谈起 */
  allRelations: RelationTypeView[];
  /** 从模式图上一个类出发新建关系时，domain 预填成那个类——**只影响初始值**，
   *  不是约束：下面照旧是个可编辑的 MultiSearchSelect，填错了随手改 */
  initialDomain?: string | null;
  /** 见 `ClassForm.headless` */
  headless?: boolean;
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
  // 其余四条 OWL 公理。**推理机的判据全在这里**——从前只能靠导入 OWL 带进来，
  // 在界面上手工建本体的人永远开不了那台机器（0002）
  const [transitive, setTransitive] = useState(existing?.is_transitive ?? false);
  const [symmetric, setSymmetric] = useState(existing?.is_symmetric ?? false);
  const [asymmetric, setAsymmetric] = useState(existing?.is_asymmetric ?? false);
  const [irreflexive, setIrreflexive] = useState(
    existing?.is_irreflexive ?? false,
  );
  // 同一族的后两条，形状不同：指向另一个关系。空串 = 没声明
  const [inverseOf, setInverseOf] = useState(existing?.inverse_of ?? "");
  const [subPropertyOf, setSubPropertyOf] = useState(
    existing?.sub_property_of ?? "",
  );
  const [description, setDescription] = useState(existing?.description ?? "");
  const [domains, setDomains] = useState<string[]>(
    existing?.domains ?? (initialDomain ? [initialDomain] : []),
  );
  const [ranges, setRanges] = useState<string[]>(existing?.ranges ?? []);
  // 显示标签，不显示 key。**进提示词的 key 由服务端从库里取**，与界面显示什么无关；
  // 而类树、属性列表也都显示标签，这里没有理由例外——中文库里用户该看到
  // "发票记录" 而不是 invoice_record
  const typeOpts = useMemo(
    () => parentOptions(allTypes, undefined),
    [allTypes],
  );
  // 两个下拉的选项：本库的其它关系。
  //
  // **自己不进列表**，两条都是。子属性指向自己数据库直接拒（那是个环）；
  // 逆指向自己在语义上合法——但它等于 `symmetric`，而那个复选框就在上面，
  // 在这里提供第二条路只会让人写出 R0 要报的东西。
  //
  // 唯一的例外是**当前值就是自己**：OWL 导入进来的可以长这样，从列表里漏掉
  // 它就会让下拉显示空白，而空白一保存就把已声明的抹了
  const linkOptions = (current: string) => [
    { value: "", label: S.ontology.noLink },
    // 当前值就是自己时把自己补回列表，否则下拉显示空白，
    // 而空白一保存就把已声明的抹了
    ...(existing && current === existing.id
      ? [{ value: existing.id, label: existing.label, hint: existing.key }]
      : []),
    ...allRelations
      .filter((r) => r.id !== existing?.id)
      .map((r) => ({ value: r.id, label: r.label, hint: r.key })),
  ];
  /** 下拉里选中那条的显示名。找不到就回落到 id——宁可难看，不要空着 */
  const nameOf = (id: string) =>
    allRelations.find((r) => r.id === id)?.label ?? id;
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
            is_transitive: transitive,
            is_symmetric: symmetric,
            is_asymmetric: asymmetric,
            is_irreflexive: irreflexive,
            // 空串要变成 null 再送——服务端收 `Option<Uuid>`，
            // `""` 解不成 UUID，会是一个 422 而不是「清空」
            inverse_of: inverseOf || null,
            sub_property_of: subPropertyOf || null,
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
            is_transitive: transitive,
            is_symmetric: symmetric,
            is_asymmetric: asymmetric,
            is_irreflexive: irreflexive,
            // 空串要变成 null 再送——服务端收 `Option<Uuid>`，
            // `""` 解不成 UUID，会是一个 422 而不是「清空」
            inverse_of: inverseOf || null,
            sub_property_of: subPropertyOf || null,
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
      {!headless && (
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
      )}
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
      {/* 六条公理并成一组。**它们本来就是同一族**——推理机（0002）拿它们当
          判据，散在表单各处会让人以为前两条和后四条是两回事。
          每一条底下写清「勾了会发生什么」：这些开关不是描述，是会改变系统行为的
          声明——`functional` 让时态引擎自动闭合旧值，`transitive` 让推理机往图里
          加边。看不出后果的开关，人只会照着直觉乱勾。 */}
      <div>
        <label className={lbl}>{S.ontology.axioms}</label>
        <p className="text-[11px] leading-relaxed text-neutral-600 mb-1.5">
          {S.ontology.axiomsHint}
        </p>
        <div className="space-y-1.5">
          {(
            [
              [functional, setFunctional, S.ontology.functional, S.ontology.functionalHint],
              [
                inverseFunctional,
                setInverseFunctional,
                S.ontology.inverseFunctional,
                S.ontology.inverseFunctionalHint,
              ],
              [transitive, setTransitive, S.ontology.transitive, S.ontology.transitiveHint],
              [symmetric, setSymmetric, S.ontology.symmetric, S.ontology.symmetricHint],
              [asymmetric, setAsymmetric, S.ontology.asymmetric, S.ontology.asymmetricHint],
              [
                irreflexive,
                setIrreflexive,
                S.ontology.irreflexive,
                S.ontology.irreflexiveHint,
              ],
            ] as const
          ).map(([on, set, title, hint], i) => (
            <label key={i} className="flex items-start gap-2 cursor-pointer">
              <input
                type="checkbox"
                className="mt-0.5 accent-[var(--u-accent)]"
                checked={on}
                onChange={(e) => set(e.target.checked)}
              />
              <span className="min-w-0">
                <span className="block text-[13px] text-neutral-200">{title}</span>
                <span className="block text-[11px] leading-relaxed text-neutral-500">
                  {hint}
                </span>
              </span>
            </label>
          ))}
        </div>
        {/* 对称与反对称同时勾是自相矛盾的（只对空关系成立）。本体自洽性检查
            会报出来，但在这里当场说一句比让人跑一遍检查再发现要快 */}
        {symmetric && asymmetric && (
          <p className="mt-1.5 text-[11px] text-[var(--u-danger)]">
            {S.ontology.axiomConflict}
          </p>
        )}
        {/* 同一组的后两条，只是形状不同：它们指向**另一个关系**，所以是下拉
            不是复选框。放在这里而不是单开一节——推理机的四种规则源里，两条是
            上面的勾，两条是下面的选，分开会让人以为它们是两回事（0002） */}
        <div className="mt-3 space-y-2.5 border-t border-white/5 pt-3">
          {(
            [
              [
                inverseOf,
                setInverseOf,
                S.ontology.inverseOf,
                S.ontology.inverseOfHint,
                linkOptions(inverseOf),
              ],
              [
                subPropertyOf,
                setSubPropertyOf,
                S.ontology.subPropertyOf,
                S.ontology.subPropertyOfHint,
                linkOptions(subPropertyOf),
              ],
            ] as const
          ).map(([value, set, title, hint, options], i) => (
            <div key={i}>
              <div className="text-[13px] text-neutral-200">{title}</div>
              <p className="text-[11px] leading-relaxed text-neutral-500 mb-1">
                {hint}
              </p>
              <SearchSelect
                value={value}
                onChange={set}
                options={options}
                size="sm"
                className="w-full"
                placeholder={S.ontology.noLink}
              />
            </div>
          ))}
          {/* 选了之后当场把话说全。**这两条推出来的事实主宾未必同向**——
              逆要对调，子属性不对调，只看名字分不出来，写出来就分得出 */}
          {(inverseOf || subPropertyOf) && (
            <div className="text-[11px] leading-relaxed text-neutral-500 space-y-0.5">
              {inverseOf && (
                <div>
                  {S.ontology.linkMeansInverse(
                    label.trim() || key || "?",
                    nameOf(inverseOf),
                  )}
                </div>
              )}
              {subPropertyOf && (
                <div>
                  {S.ontology.linkMeansSuper(
                    label.trim() || key || "?",
                    nameOf(subPropertyOf),
                  )}
                </div>
              )}
            </div>
          )}
        </div>
      </div>
      <div>
        <label className={lbl}>{S.ontology.description}</label>
        <textarea
          className="input-dark w-full px-3 py-2 text-sm min-h-[9rem] resize-y"
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


/** 类型消解：把「大致对」的类换成更具体的那个。
 *
 * **两步走，跟本体导入同一个形状**：先看一遍会发生什么，再决定落不落。这里还多
 * 一层理由——改类不进时间轴，不像事实改写那样在实体历史里自己显形，所以
 * 「先看一眼」是唯一能看见它的时机。
 *
 * 跑完分三档，各有各的处置：自动改掉的（可整批撤销）、跨了分类轴留给人的、
 * 裁决说「都不是」的。**最后那一档带着理由**——这一步押在「选择都不是是个
 * 体面答案」上，不记理由，最大的那一档就是不透明的。
 */
function RefinePanel({
  kbId,
  onChanged,
  onError,
}: {
  kbId: string;
  onChanged: () => void;
  onError: (e: Error) => void;
}) {
  const [preview, setPreview] = useState<TypeSuggestion[] | null>(null);
  const [outcome, setOutcome] = useState<ResolutionOutcome | null>(null);

  const look = useMutation({
    mutationFn: () => api.typeResolutionPreview(kbId),
    onSuccess: (d) => {
      setPreview(d.items);
      setOutcome(null);
    },
    onError,
  });
  const run = useMutation({
    mutationFn: () => api.typeResolutionApply(kbId),
    onSuccess: (d) => {
      setOutcome(d);
      setPreview(null);
      onChanged();
    },
    onError,
  });
  const approve = useMutation({
    mutationFn: (v: {
      from_type_id: string;
      to_type_id: string;
      entity_ids: string[];
    }) => api.approveRefinement(kbId, v),
    onSuccess: () => {
      toast.success(S.toast.saved);
      onChanged();
    },
    onError,
  });
  const undo = useMutation({
    mutationFn: (batch: string) => api.typeResolutionUndo(kbId, batch),
    onSuccess: (d) => {
      toast.success(S.ontology.refineUndone(d.reverted));
      setOutcome(null);
      onChanged();
    },
    onError,
  });

  const busy = look.isPending || run.isPending;

  return (
    <div className="space-y-4">
      <div>
        <h3 className="u-title text-lg mb-1">{S.ontology.refineTitle}</h3>
        <p className="text-xs leading-relaxed text-neutral-500 max-w-xl">
          {S.ontology.refineHint}
        </p>
      </div>

      <div className="flex gap-2">
        <button
          className="u-btn text-xs"
          disabled={busy}
          onClick={() => look.mutate()}
        >
          {look.isPending ? S.ontology.refineLooking : S.ontology.refinePreview}
        </button>
        <button
          className="u-btn u-btn-primary text-xs"
          disabled={busy}
          onClick={() => run.mutate()}
        >
          {run.isPending ? S.ontology.refineRunning : S.ontology.refineRun}
        </button>
      </div>

      {/* ---- 只算不写的那一步 */}
      {preview && (
        <div className="space-y-2">
          <p className="text-xs text-neutral-500">
            {preview.length === 0
              ? S.ontology.refineNothing
              : S.ontology.refineCandidates(preview.length)}
          </p>
          {preview.map((s) => (
            <div key={s.entity_id} className="glass rounded-xl p-3">
              <div className="flex items-baseline gap-2 flex-wrap">
                <span className="text-sm text-neutral-100">{s.name}</span>
                <span className="text-[11px] text-neutral-500">
                  {s.coarse ?? S.graph.untyped}
                </span>
                {s.specific_type && (
                  <span className="text-[11px] text-[var(--u-warn)]">
                    {S.ontology.refineModelSays(s.specific_type)}
                  </span>
                )}
                <span className="ml-auto u-num text-[10.5px] text-neutral-600">
                  {S.review.factsCount(s.fact_count)}
                </span>
              </div>
              {/* **把送去检索的那段字显示出来**：找不着的时候，第一个要看的
                  就是我们拿什么去找的，而不是猜画像还是类描述的问题 */}
              <p className="mt-1 text-[11px] text-neutral-500 line-clamp-2">
                {s.profile}
              </p>
              <div className="mt-1.5 flex flex-wrap gap-1">
                {s.candidates.slice(0, 6).map((c) => (
                  <span
                    key={c.id}
                    title={c.description}
                    className="u-chip u-chip-neutral u-num text-[10.5px]"
                  >
                    {c.label} {c.distance.toFixed(2)}
                  </span>
                ))}
                {s.candidates.length === 0 && (
                  <span className="text-[11px] text-neutral-600">
                    {S.ontology.refineNoCandidates}
                  </span>
                )}
              </div>
            </div>
          ))}
        </div>
      )}

      {/* ---- 落库之后的三档 */}
      {outcome && (
        <div className="space-y-3">
          <div className="flex items-center gap-3 flex-wrap">
            <span className="text-xs text-neutral-300">
              {S.ontology.refineRetyped(outcome.retyped)}
            </span>
            {outcome.batch && outcome.retyped > 0 && (
              <button
                className="u-btn u-btn-ghost text-xs"
                disabled={undo.isPending}
                onClick={() => undo.mutate(outcome.batch!)}
              >
                {S.ontology.refineUndo}
              </button>
            )}
          </div>

          {outcome.for_review.length > 0 && (
            <div className="space-y-2">
              <p className="text-xs text-neutral-500">
                {S.ontology.refineForReview(outcome.for_review.length)}
              </p>
              {outcome.for_review.map((r) => (
                <div key={r.entity_id} className="glass rounded-xl p-3">
                  <div className="flex items-baseline gap-2 flex-wrap">
                    <span className="text-sm text-neutral-100">{r.name}</span>
                    <span className="text-[11px] text-neutral-500">
                      {r.coarse ?? S.graph.untyped} → {r.choice}
                    </span>
                    {r.crosses_axis && (
                      <span className="u-chip u-chip-warn text-[10.5px]">
                        {S.ontology.refineCrossesAxis}
                      </span>
                    )}
                    <span className="ml-auto u-num text-[10.5px] text-neutral-600">
                      {Math.round(r.confidence * 100)}%
                    </span>
                  </div>
                  {r.reason && (
                    <p className="mt-1 text-[11px] text-neutral-500">
                      {r.reason}
                    </p>
                  )}
                  {/* **认可的是这一对类，不是这一个实体。** 认可一次，
                      之后同一对不再进人工——那正是这一档大部分条目的成因 */}
                  {r.from_type_id && (
                    <button
                      className="u-btn u-btn-primary mt-2 text-xs"
                      disabled={approve.isPending}
                      onClick={() =>
                        approve.mutate({
                          from_type_id: r.from_type_id!,
                          to_type_id: r.to_type_id,
                          entity_ids: [r.entity_id],
                        })
                      }
                    >
                      {S.ontology.refineApprovePair}
                    </button>
                  )}
                </div>
              ))}
            </div>
          )}

          {outcome.left_alone.length > 0 && (
            <div className="space-y-1.5">
              <p className="text-xs text-neutral-500">
                {S.ontology.refineLeftAlone(outcome.left_alone.length)}
              </p>
              {outcome.left_alone.map((d, i) => (
                <div key={i} className="glass rounded-xl px-3 py-2">
                  <div className="flex items-baseline gap-2 flex-wrap">
                    <span className="text-[13px] text-neutral-200">
                      {d.name}
                    </span>
                    <span className="text-[11px] text-neutral-500">
                      {d.coarse ?? S.graph.untyped}
                    </span>
                  </div>
                  {/* 理由与头一个候选一起给：理由说不通时，看候选就知道是
                      检索没找着还是裁决没看上 */}
                  {d.reason && (
                    <p className="mt-0.5 text-[11px] text-neutral-500">
                      {d.reason}
                    </p>
                  )}
                  {d.top_candidate && (
                    <p className="mt-0.5 text-[11px] text-neutral-600">
                      {S.ontology.refineTopCandidate(d.top_candidate)}
                    </p>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
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
