// 本体编辑器：master-detail 双栏（与 Library 的 SourcesRail 同构）。
// 左栏 = filter + Classes/Properties 两小节 + 底部 Unmatched 入口；
// 右侧 = 选中项的详情面板（只展示；建、改、删、连都在弹窗里，见 ontologyDialogs）
// / 未匹配信号面板 / 概览。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import {
  ArrowLeft,
  ArrowLeftRight,
  ArrowRight,
  ChevronRight,
  Inbox,
  Link2,
  Network,
  Pencil,
  Plus,
  Scale,
  Search,
  Split,
  Table as TableIcon,
  Upload,
  Wand2,
  X,
} from "lucide-react";
import {
  api,
  type EntityTypeView,
  type ImportPlan,
  type OntologyImportView,
  type OntologyMiss,
  type AgentProposalFields,
  type CompetencyQuestion,
  type PlannedItem,
  type OntologyProposals,
  type ResolutionOutcome,
  type TypeSuggestion,
  type RelationTypeView,
  type UniquenessCandidate,
} from "../api";
import { OntologyTables, treeParent } from "./ontologyTables";
import { S } from "../i18n";
import { useKb } from "../kb";
import { toast } from "../toast";
import { OntologySchemaGraph, type SchemaSelection } from "./OntologySchemaGraph";
import { RulesPanel } from "./RulesPanel";
import {
  AttributeDialog,
  ClassDialog,
  ConnectDialog,
  PropertyDialog,
} from "./ontologyDialogs";
import {
  Button,
  Chip,
  DangerConfirm,
  Dialog,
  Disclosure,
  IconButton,
  Input,
  LinkButton,
  Loading,
  PageHeader,
  Pager,
  RAIL_CLS,
  ROW_TRAILING,
  RailItem,
  Row,
  Segmented,
  SkeletonRows,
  Spinner,
  Status,
  TBody,
  THead,
  Table,
  Td,
  Th,
  Tr,
  chipLike,
  cn,
  localDateTime,
  pageSlice,
  rowClass,
} from "../ui";

/** 左栏行高（py-2 + 13px 文字 + space-y 间隙）与底部预留（新建行 + 分页器） */
const RAIL_ROW_H = 34;
const RAIL_RESERVED = 80;
/** 上次看的是表还是图。**per 浏览器不 per 库**：这是读法的习惯，
 *  换个知识库不会换读法 */
const VIEW_KEY = "utopia.ontology.view";
/** 兜底页行数（首帧未量到高度时用） */
const RAIL_PAGE = 14;
/** 过滤模式两节混排时每节的行数 */

/** 右侧详情区当前展示什么。
 *
 *  class / relation / schema / null 这一组共享同一块工作区：模式图常驻做
 *  背景，详情面板停靠在右侧——左栏点一个类名、画布上点一个节点、模式图
 *  自己的搜索框选中一个关系，三条路径落到的是同一个 `sel`，因此也落到同一
 *  块面板，不必再各画一遍。面板只展示；改动在 `Edit` 的弹窗里。
 *  import / refine / misses 仍是独立的整页视图：那三个不是「关于某个类
 *  或关系」的事，跟模式图没有共同的背景可言。 */
type Sel =
  | { kind: "class"; id: string }
  | { kind: "relation"; id: string }
  | { kind: "misses" }
  | { kind: "uniqueness" }
  // 业务规则那一页。**focusId 是从模式图上点一条规则边过来的**——那一行高亮，
  // 不必在一页规则里再找一遍
  | { kind: "rules"; focusId?: string }
  | { kind: "refine" }
  | { kind: "import" }
  // 模式图无选中：看整张图,不停靠表单
  | { kind: "schema" }
  | null;

/** 这次选中会不会在模式图右侧停靠一块面板 */
const onPanel = (s: Sel) => s?.kind === "class" || s?.kind === "relation";

/** 打开着的编辑弹窗。面板只展示；建、改、删、连都在弹窗里发生 */
type Edit =
  | { kind: "class"; existing: EntityTypeView | null; parentId: string | null }
  | {
      kind: "property";
      existing: RelationTypeView | null;
      // 从一个类出发新建关系时带上它的 id，domain 预填成它
      initialDomain: string | null;
    }
  | { kind: "attribute"; typeId: string; existing: RelationTypeView | null }
  | { kind: "connect"; cls: EntityTypeView }
  | null;

export function Ontology() {
  const { kb } = useKb();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [sel, setSel] = useState<Sel>(null);
  const [edit, setEdit] = useState<Edit>(null);
  const [railTab, setRailTab] = useState<"classes" | "properties">("classes");
  /* 主区看图还是看表（#498）。**默认图**，而且**记住上次那一版**——这是
     "我习惯怎么读这一页"，不是"这个链接指向什么"，所以进 localStorage 不进
     URL（与知识库切换器同一条判断，见 KbScope）。隐私模式下读写都会抛，
     catch 掉回落到默认，别让整页挂掉 */
  const [view, setView] = useState<"table" | "diagram">(() => {
    try {
      return localStorage.getItem(VIEW_KEY) === "table" ? "table" : "diagram";
    } catch {
      return "diagram";
    }
  });
  const switchView = (v: "table" | "diagram") => {
    setView(v);
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      // 存不下就只在这一次会话里记着
    }
    if (!onPanel(sel)) setSel({ kind: "schema" });
  };
  /** Import / Rules / Refine / Overlaps / Unmatched 那几页把主区整个接管了 */
  const inWorkflow =
    sel?.kind === "import" ||
    sel?.kind === "refine" ||
    sel?.kind === "uniqueness" ||
    sel?.kind === "rules" ||
    sel?.kind === "misses";
  // 模式图详情面板停在哪一段。**跨选中保留**：在实例上挨个类看下去，
  // 是一种真实的读法，每换一个类就被弹回定义页会打断它
  const [panelTab, setPanelTab] = useState<
    "definition" | "relations" | "attributes" | "instances"
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
  // 并存的取值（#341）：单独取，因为它扫的是事实不是本体，而本体页
  // 每改一个字段都要重取——把这趟扫描搭进去等于每次输入都扫一遍全库
  const uniqueness = useQuery({
    queryKey: ["uniqueness", kb?.id],
    queryFn: () => api.uniquenessCandidates(kb!.id),
    enabled: !!kb,
  });
  const overlaps = uniqueness.data?.candidates ?? [];
  /* 业务规则：模式图要把它们画成边，规则页要列出来——同一个 query key
     取一次，两处共用缓存 */
  const rules = useQuery({
    queryKey: ["rules", kb?.id],
    queryFn: () => api.rules(kb!.id),
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
  if (data.isError) return <Loading>{(data.error as Error).message}</Loading>;

  /* 本体还没到的时候**照样把这一页搭出来**。
     从前这里一句 `data.isPending` 就把整页换成一行 "Loading"：一个九百多个类
     的库要等好几秒，这几秒里人看到的是一片空白，连自己有没有点错页都不知道
     ——而左栏那两组入口（换视图、Import / Rules / Refine…）跟本体取没取回来
     根本无关，它们本可以立刻就在。
     现在等的只是内容：左栏清单出骨架（一列缩进的树，形状是已知的），
     右边那块还不知道会画成图还是表，出转圈。 */
  const loading = data.isPending;
  const ont = data.data;
  const entity_types = ont?.entity_types ?? [];
  const relation_types = ont?.relation_types ?? [];
  const misses = ont?.misses ?? [];
  const dismissed_misses = ont?.dismissed_misses ?? [];
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
  // 选中类身上挂着的属性，Attributes 一段用它
  const classAttributes = selectedClass
    ? relation_types.filter(
        (r) => r.kind === "attribute" && r.domains.includes(selectedClass.id),
      )
    : [];

  return (
    <div className="h-full flex">
      {/* 左栏：filter + 两小节 + Unmatched */}
      <aside className={`${RAIL_CLS} flex flex-col`}>
        {/* 与图谱页的搜索框同一副身材、同一个角落（左上各 12px、中号、232 宽）：
            两个标签页切来切去，框留在原地 */}
        {/* 图 / 表：这一页的两种画法。**排在筛选框上面**——它定的是这一页
            是什么，筛选框是在里面找东西；而且表格那一档筛选框是收起的，
            切换键排在下面的话位置会跟着跳。

            **一个键，写的是要去的那一版**：两档并排的话，得先读出哪一档亮着
            才知道自己在看什么，而主区摆的是表还是图一眼就看得出。

            **Import / Rules / Refine / Overlaps / Unmatched 接管主区时，它是
            回去的路**——那时候写的是你原本在看的那一版，点了就回去。从前这条路
            是「Schema diagram」那一行，换成切换键之后断过一阵：进了 Import
            再没有任何一处能回到图或表。

            **自成一组**：与底下钉住那一组同一个做法，外层一条线加 py-2 隔开，
            行本身不画框——它跟筛选框不是同一类东西，只隔 4px 会读成一串 */}
        {/* 上面一行是**动作**（换到另一版），下面一行是**位置**（现在看的是哪一版）。
            一行里塞两件事，就成了「这个键写的到底是我在哪儿、还是我要去哪儿」；
            分成两行之后，位置那一行还兼着从 Import / Rules 那几页回来的路——
            它们把主区整个接管，从前没有任何一处能回到图或表。
            **自成一组**：与底下钉住那一组同一个做法，外层一条线加 py-2 隔开 */}
        <div className="u-rail-list shrink-0 border-b border-line px-2 py-2">
          <Row
            density="nav"
            icon={<ArrowLeftRight size={14} />}
            onClick={() => switchView(view === "diagram" ? "table" : "diagram")}
          >
            {view === "diagram"
              ? S.ontology.switchToTable
              : S.ontology.switchToGraph}
          </Row>
          <Row
            density="nav"
            active={!inWorkflow}
            icon={
              view === "diagram" ? <Network size={14} /> : <TableIcon size={14} />
            }
            onClick={() => setSel({ kind: "schema" })}
          >
            {view === "diagram" ? S.ontology.viewDiagram : S.ontology.viewTable}
          </Row>
        </div>
        {/* 筛选框、两档、清单**只属于图那一档**——表格自己就是清单，而且比这一列
            强（有列、能排序、带计数）。换档时它们**折起来**而不是瞬间消失：
            左栏是一直在的，一整段凭空没掉会让人以为跳到了别的页 */}
        <div
          className={cn("u-rail-fold", view === "table" && "is-folded")}
        >
          <div className="px-2 pt-3 pb-1">
            <Input
              icon={<Search size={12} />}
              placeholder={S.ontology.filter}
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          </div>
          <div className="px-2 pb-1">
            <Segmented
              fill
              value={railTab}
              onChange={setRailTab}
              options={[
                { value: "classes", label: S.ontology.tabClasses },
                { value: "properties", label: S.ontology.tabProperties },
              ]}
            />
          </div>
        <div
          ref={listRef}
          className="flex-1 min-h-0 overflow-hidden px-2 pb-2 flex flex-col"
        >
          {/* 新建行置顶：随当前段建类/建关系 */}
          {/* 本体还没到就不给建：新类要挑父类，而父类清单此刻是空的 */}
          {!loading && !filter.trim() && (
            <Row
              className="mb-1"
              icon={<Plus size={14} />}
              onClick={() =>
                setEdit(
                  railTab === "classes"
                    ? { kind: "class", existing: null, parentId: null }
                    : { kind: "property", existing: null, initialDomain: null },
                )
              }
            >
              {railTab === "classes"
                ? S.ontology.newClass
                : S.ontology.newProperty}
            </Row>
          )}
          {/* **筛选也认这两档**。从前一有筛选词就两节混排、类与关系一起给，
              理由是"搜的时候未必知道要找的是哪一种"；代价是标签明明停在
              Classes 上却不算数，而它就在上面两厘米处。一个选中却被无视的
              控件，比多点一下糟。想找关系就切过去，那一下是明的 */}
          {loading ? (
            <SkeletonRows />
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
        </div>
        {/* 底部常驻：关于本体的几个入口——从外部拿一份本体、业务规则、类型消解，
            以及数据顶回来的两种信号。一条分隔线说明它们是钉住的，行本身与上面
            列表里的行同一副样子 */}
        <div className="u-rail-list shrink-0 border-t border-line px-2 py-2">
        <RailItem
          active={sel?.kind === "import"}
          icon={<Upload size={14} />}
          onClick={() => setSel({ kind: "import" })}
        >
          {S.ontology.importShort}
        </RailItem>
        {/* 业务规则（0021）：本体说「有这么个类」，规则说「什么样的实体算它」。
            所以它归在本体这一页，而不是另开一处——判据与词汇是同一份契约 */}
        <RailItem
          active={sel?.kind === "rules"}
          icon={<Scale size={14} />}
          onClick={() => setSel({ kind: "rules" })}
        >
          {S.ontology.rulesShort}
        </RailItem>
        {/* 类型消解：把「大致对」的类换成更具体的那个。**紧挨着未匹配**——
            两者都是「本体与数据对不齐」的处置，只是方向相反：那个是本体缺东西，
            这个是本体有更好的选项没被用上 */}
        <RailItem
          active={sel?.kind === "refine"}
          icon={<Wand2 size={14} />}
          onClick={() => setSel({ kind: "refine" })}
        >
          {S.ontology.refineShort}
        </RailItem>
        {/* 并存的取值（#341）：**与未匹配同一类**——都是数据在说本体缺了什么。
            那个说缺一个词，这个说缺一条公理，而缺公理的代价是接任不闭合前任，
            "六月谁在管"两个人都答 */}
        <RailItem
          active={sel?.kind === "uniqueness"}
          icon={<Split size={14} />}
          count={loading ? undefined : overlaps.length}
          onClick={() => setSel({ kind: "uniqueness" })}
        >
          {S.ontology.uniquenessShort}
        </RailItem>
        {/* 底部常驻：抽取未匹配信号（有存量时带数量徽标） */}
        <RailItem
          active={sel?.kind === "misses"}
          icon={<Inbox size={14} />}
          count={loading ? undefined : misses.length}
          onClick={() => setSel({ kind: "misses" })}
        >
          {S.ontology.missesShort}
        </RailItem>
        </div>
      </aside>

      {/* 右侧：详情。class/relation/new-class/new-relation/schema/概览共享
          同一块工作区——模式图常驻做背景，表单（有的话）停靠右侧，近实底
          （u-modal-panel），不是浮层玻璃：画布是动的，透出来的内容会跟
          正在读的表单字段打架。左栏点类名、画布点节点、模式图自带的搜索框
          选中关系，三条路径落到同一个 sel，也就落到同一份表单——
          不再各画一遍。import/refine/misses 仍是独立整页视图,那三个
          不是「关于某个类或关系」的事，没有跟模式图共享背景的道理 */}
      {/* **转圈只留给接管主区的那几页**（Import / Rules / Refine / Overlaps /
          Unmatched）：它们整块是表单和列表，本体没到就什么都画不出来。
          图和表不走这条路——它们各自都有一大半跟本体无关的东西可以先画出来
          （网格、缩放塔、静态图例；分段控件、过滤框），所以 `loading` 传下去，
          由它们自己决定哪一块该等 */}
      {loading && inWorkflow ? (
        <div className="flex-1 min-w-0 grid place-items-center">
          <Spinner size={20} label={S.nav.loading} />
        </div>
      ) : sel?.kind === "import" ||
      sel?.kind === "refine" ||
      sel?.kind === "misses" ||
      sel?.kind === "uniqueness" ||
      sel?.kind === "rules" ? (
        <div className="flex-1 min-w-0 overflow-y-auto u-scroll px-8 py-6">
          <div>
            {sel.kind === "import" ? (
              <div>
                <ImportPanel kbId={kb.id} onChanged={refresh} onError={onError} />
              </div>
            ) : sel.kind === "refine" ? (
              <div>
                <RefinePanel kbId={kb.id} onChanged={refresh} onError={onError} />
              </div>
            ) : sel.kind === "uniqueness" ? (
              <div>
                <UniquenessPanel
                  kbId={kb.id}
                  candidates={overlaps}
                  relations={relation_types}
                  pending={uniqueness.isPending}
                  onChanged={() => {
                    uniqueness.refetch();
                    refresh();
                  }}
                  onError={onError}
                />
              </div>
            ) : sel.kind === "rules" ? (
              <div>
                <RulesPanel
                  kbId={kb.id}
                  focusId={sel.kind === "rules" ? sel.focusId : undefined}
                  classes={entity_types}
                  attributes={relation_types.filter((r) => r.kind === "attribute")}
                  relations={relation_types.filter((r) => r.kind === "relation")}
                  onError={onError}
                />
              </div>
            ) : (
              <div>
                <MissesPanel
                  kbId={kb.id}
                  misses={misses}
                  dismissedMisses={dismissed_misses ?? []}
                  onChanged={refresh}
                  onError={onError}
                />
                <QuestionsPanel kbId={kb.id} onError={onError} />
              </div>
            )}
          </div>
        </div>
      ) : (
        <div className="flex-1 min-w-0 relative">
          {view === "table" ? (
            <OntologyTables
              loading={loading}
              selected={
                sel?.kind === "class" || sel?.kind === "relation" ? sel : null
              }
              entityTypes={entity_types}
              relationTypes={relation_types}
              onOpenClass={(t) => setSel({ kind: "class", id: t.id })}
              onOpenProperty={(r) => setSel({ kind: "relation", id: r.id })}
              onOpenAttribute={(a) =>
                setEdit({
                  kind: "attribute",
                  typeId: a.domains[0] ?? "",
                  existing: a,
                })
              }
              onSeeInstances={(t) => {
                setPanelTab("instances");
                setSel({ kind: "class", id: t.id });
              }}
              onCreateClass={() =>
                setEdit({ kind: "class", existing: null, parentId: null })
              }
              onCreateProperty={() =>
                setEdit({
                  kind: "property",
                  existing: null,
                  initialDomain: null,
                })
              }
            />
          ) : (
            <OntologySchemaGraph
            loading={loading}
            entityTypes={entity_types}
            relationTypes={relation_types}
            rules={rules.data?.rules ?? []}
            selected={
              sel?.kind === "class" || sel?.kind === "relation"
                ? ({ kind: sel.kind, id: sel.id } as SchemaSelection)
                : null
            }
            // 点空白处取消选中走的也是 closePanel：不然点画布关掉的面板没有
            // 退场动画，而点画布正是最常用的那种关法
            // 点一条规则边：切到业务规则那一页，并把那一行点亮
            onSelect={(next) =>
              next
                ? setSel(
                    next.kind === "rule"
                      ? { kind: "rules", focusId: next.id }
                      : next,
                  )
                : closePanel()
            }
            />
          )}
          {selectedClass && (
            <DockedPanel
              onClose={closePanel}
              exiting={!onPanel(sel)}
              actions={
                <IconButton
                  size="sm"
                  label={S.ontology.editClass}
                  onClick={() =>
                    setEdit({
                      kind: "class",
                      existing: selectedClass,
                      parentId: selectedClass.primary_parent ?? null,
                    })
                  }
                >
                  <Pencil size={13} />
                </IconButton>
              }
              header={
                <PanelHeader
                  color={selectedClass.color}
                  square={selectedClass.shape === "square"}
                  title={selectedClass.label}
                  sub={`${selectedClass.key} · ${S.ontology.usage(selectedClass.usage)}`}
                  builtin={selectedClass.builtin}
                />
              }
              tabs={
                <Segmented
                  fill
                  size="sm"
                  value={panelTab}
                  onChange={setPanelTab}
                  options={[
                    {
                      value: "definition",
                      label: S.ontology.schemaTabDefinition,
                    },
                    {
                      value: "relations",
                      label: S.ontology.schemaTabRelations,
                    },
                    {
                      value: "attributes",
                      label: S.ontology.schemaTabAttributes,
                    },
                    {
                      value: "instances",
                      label: S.ontology.schemaTabInstances,
                    },
                  ]}
                />
              }
            >
              {/* 四段用 hidden 藏，不卸载：实例列表翻到的第几页，切回来的时候还在 */}
              <div className={panelTab === "definition" ? "" : "hidden"}>
                <ClassDefinition
                  cls={selectedClass}
                  allTypes={entity_types}
                  onSelectClass={(id) => setSel({ kind: "class", id })}
                  onNewSub={() =>
                    setEdit({ kind: "class", existing: null, parentId: selectedClass.id })
                  }
                />
              </div>
              <div className={panelTab === "relations" ? "" : "hidden"}>
                <RelationshipsCard
                  cls={selectedClass}
                  relations={relations}
                  allTypes={entity_types}
                  onSelect={(id) => setSel({ kind: "relation", id })}
                  onConnect={() => setEdit({ kind: "connect", cls: selectedClass })}
                  onAddNew={() =>
                    setEdit({
                      kind: "property",
                      existing: null,
                      initialDomain: selectedClass.id,
                    })
                  }
                />
              </div>
              <div className={panelTab === "attributes" ? "" : "hidden"}>
                <AttributesCard
                  attributes={classAttributes}
                  onEdit={(a) =>
                    setEdit({ kind: "attribute", typeId: selectedClass.id, existing: a })
                  }
                  onNew={() =>
                    setEdit({ kind: "attribute", typeId: selectedClass.id, existing: null })
                  }
                />
              </div>
              <div className={panelTab === "instances" ? "" : "hidden"}>
                <InstancesCard kbId={kb.id} type={selectedClass} />
              </div>
            </DockedPanel>
          )}
          {selectedProp && (
            <DockedPanel
              onClose={closePanel}
              exiting={!onPanel(sel)}
              actions={
                <IconButton
                  size="sm"
                  label={S.ontology.editProperty}
                  onClick={() =>
                    setEdit({ kind: "property", existing: selectedProp, initialDomain: null })
                  }
                >
                  <Pencil size={13} />
                </IconButton>
              }
              header={
                <PanelHeader
                  title={selectedProp.label}
                  sub={`${selectedProp.key} · ${S.ontology.usage(selectedProp.usage)}`}
                  builtin={selectedProp.builtin}
                />
              }
            >
              <PropertyDefinition
                rel={selectedProp}
                allTypes={entity_types}
                allRelations={relations}
              />
            </DockedPanel>
          )}
        </div>
      )}

      {/* 编辑弹窗：面板只展示，建、改、删、连都在这里发生。建成的立刻选中——
          能看到、能接着改；删掉的把面板收起来 */}
      {edit?.kind === "class" && (
        <ClassDialog
          kbId={kb.id}
          existing={edit.existing}
          parentId={edit.parentId}
          allTypes={entity_types}
          onClose={() => setEdit(null)}
          onSaved={(createdId) => {
            setEdit(null);
            if (createdId) setSel({ kind: "class", id: createdId });
            afterOntologyChange();
          }}
          onDeleted={() => {
            setEdit(null);
            closePanel();
            afterOntologyChange();
          }}
          onError={onError}
        />
      )}
      {edit?.kind === "property" && (
        <PropertyDialog
          kbId={kb.id}
          existing={edit.existing}
          allTypes={entity_types}
          allRelations={relations}
          initialDomain={edit.initialDomain}
          onClose={() => setEdit(null)}
          onSaved={(createdId) => {
            setEdit(null);
            if (createdId) setSel({ kind: "relation", id: createdId });
            afterOntologyChange();
          }}
          onDeleted={() => {
            setEdit(null);
            closePanel();
            afterOntologyChange();
          }}
          onError={onError}
        />
      )}
      {edit?.kind === "attribute" && (
        <AttributeDialog
          kbId={kb.id}
          typeId={edit.typeId}
          existing={edit.existing}
          onClose={() => setEdit(null)}
          onSaved={() => {
            setEdit(null);
            afterOntologyChange();
          }}
          onDeleted={() => {
            setEdit(null);
            afterOntologyChange();
          }}
          onError={onError}
        />
      )}
      {edit?.kind === "connect" && (
        <ConnectDialog
          kbId={kb.id}
          cls={edit.cls}
          relations={relations}
          onClose={() => setEdit(null)}
          onConnected={(id) => {
            setEdit(null);
            afterOntologyChange();
            // 连完直接跳到那条关系：域/值域改没改、改对了没有，一眼可见
            setSel({ kind: "relation", id });
          }}
          onError={onError}
        />
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
  actions,
  tabs,
  exiting,
  onClose,
  children,
}: {
  header: React.ReactNode;
  /** 关闭键左边的动作（编辑）：面板只展示，改动从这里开弹窗 */
  actions?: React.ReactNode;
  /** 分段控件，跟着标题一起固定在顶上——它要能一直点得到 */
  tabs?: React.ReactNode;
  /** 正在退场：演动画，期间不再接受点击（u-dock-out 里带了 pointer-events） */
  exiting?: boolean;
  onClose: () => void;
  children: React.ReactNode;
}) {
  return (
    <div
      // 与图谱页的实体面板同一副壳：同宽（w-96）、同一个顶部起点（给顶上那排
      // 药丸让位），同一个头部解剖。两页并排看是同一件东西
      className={`${exiting ? "u-dock-out" : "u-dock-in"} glass-strong absolute top-3 right-3 bottom-3 w-96 z-10 rounded-overlay u-lift-strong flex flex-col`}
    >
      <div className="shrink-0 flex items-start justify-between gap-2 px-4 py-4 border-b border-line">
        <div className="min-w-0">{header}</div>
        <div className="-mr-1 -mt-1 flex shrink-0 items-center gap-1">
          {actions}
          <IconButton size="sm" label={S.ontology.schemaClosePanel} onClick={onClose}>
            <X size={15} />
          </IconButton>
        </div>
      </div>
      {tabs && <div className="shrink-0 px-4 pt-3 pb-1">{tabs}</div>}
      <div className="u-scroll flex-1 min-h-0 overflow-y-auto flex flex-col gap-3 px-4 py-3">
        {children}
      </div>
    </div>
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
            className={`h-2.5 w-2.5 shrink-0 ${square ? "scale-90" : "rounded-full"}`}
            style={{ background: color, boxShadow: `0 0 8px ${color}55` }}
          />
        )}
        <span
          className="truncate text-title font-semibold tracking-tight text-ink"
          style={{ fontFamily: "var(--font-display)" }}
        >
          {title}
        </span>
        {builtin && <Chip tone="neutral">{S.ontology.builtin}</Chip>}
      </div>
      {sub && <div className="mt-1 text-small text-ink-2">{sub}</div>}
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
  // 这一段自己就是 Instances 那个 tab：没有实例就说一句，不顶同名的标题
  if (!q.isPending && total === 0)
    return <p className="text-small text-ink-2">{S.ontology.schemaNoInstances}</p>;

  return (
    // 平铺在面板里：面板已经是一块面，里面不再套卡片
    <div>
      <div>
        {rows.map((e) => (
          <Link
            key={e.id}
            to="/kb/$kbId/graph"
            params={{ kbId }}
            search={{ entity: e.id }}
            className={cn(rowClass(), "-mx-2")}
          >
            <span
              className={`h-2 w-2 shrink-0 ${type.shape === "square" ? "scale-90" : "rounded-full"}`}
              style={{ background: type.color }}
            />
            <span className="truncate">{e.name}</span>
            <span className={cn("u-num", ROW_TRAILING)}>
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
  cls,
  relations,
  allTypes,
  onSelect,
  onConnect,
  onAddNew,
}: {
  cls: EntityTypeView;
  /** kind === "relation" 的那些——attribute 的宾语是字面值，谈不上「连着」 */
  relations: RelationTypeView[];
  allTypes: EntityTypeView[];
  onSelect: (relationId: string) => void;
  /** 用一个已有的关系连接这个类：开弹窗 */
  onConnect: () => void;
  onAddNew: () => void;
}) {
  const labelOf = (id: string) =>
    allTypes.find((t) => t.id === id)?.label ?? id;
  // 自环关系（domain 和 range 都是这个类）两边都算——它确实两个方向都成立
  const outgoing = relations.filter((r) => r.domains.includes(cls.id));
  const incoming = relations.filter((r) => r.ranges.includes(cls.id));

  /** 一组可折叠（#455 的解剖：折叠柄、头、缩进的正文）。缺省展开——
   *  折起来是为了在长列表里跳过一段，不是为了藏 */
  const Group = ({
    dir,
    rows,
  }: {
    dir: "out" | "in";
    rows: RelationTypeView[];
  }) => {
    const [open, setOpen] = useState(true);
    return (
      <div>
        {/* 头是一行 Row：折叠柄占图标格（16 宽），正文缩进同样的 24，
            于是下面每一行的方向箭头正好落在组标题的箭头底下 */}
        <Row
          className="-mx-2 mt-1"
          icon={
            <span className="flex w-4 justify-center">
              <ChevronRight size={12} className={cn("u-turn", open && "rotate-90")} />
            </span>
          }
          onClick={() => setOpen((v) => !v)}
        >
          <span className="flex items-center gap-2 text-small font-medium">
            {/* 与下面每条关系行的箭头**同一尺寸**（12）：同一个记号排成一列，
                标题这个小 2px 就只会读成没对齐 */}
            {dir === "out" ? <ArrowRight size={12} /> : <ArrowLeft size={12} />}
            <span className="truncate">
              {dir === "out" ? S.ontology.schemaOutgoing : S.ontology.schemaIncoming}
            </span>
            {rows.length > 1 && <span className="u-num">{rows.length}</span>}
          </span>
        </Row>
        {open && (
          <div className="pl-6">
            {rows.map((r) => (
              <RelationRow key={`${dir}:${r.id}`} r={r} dir={dir} />
            ))}
          </div>
        )}
      </div>
    );
  };

  const RelationRow = ({ r, dir }: { r: RelationTypeView; dir: "out" | "in" }) => (
    // 悬停要有底色：这一行整条可点，只把文字提亮半级在深底上几乎看不出来。
    // 底色用左栏那一档（white/[0.05]），右端的类型小字跟着一起提亮
    <Row
      className="-mx-2"
      icon={
        dir === "out" ? (
          <ArrowRight size={12} className="text-violet" />
        ) : (
          <ArrowLeft size={12} className="text-violet" />
        )
      }
      trailing={
        <span className="block max-w-32 truncate">
          {(dir === "out" ? r.ranges : r.domains).map(labelOf).join(", ") ||
            S.ontology.anyType}
        </span>
      }
      onClick={() => onSelect(r.id)}
    >
      {r.label}
    </Row>
  );

  return (
    // 平铺在面板里，与图谱面板的关系分组同一个骨架：带方向箭头的小标题 + 行。
    // 这一段自己就是 Relations 那个 tab，不再顶一个同名的标题
    <div>
      {outgoing.length === 0 && incoming.length === 0 ? (
        <p className="mb-2 text-small text-ink-2">
          {S.ontology.schemaNoRelationships}
        </p>
      ) : (
        <div className="mb-2">
          {outgoing.length > 0 && <Group dir="out" rows={outgoing} />}
          {incoming.length > 0 && <Group dir="in" rows={incoming} />}
        </div>
      )}
      {/* 改动开弹窗：连一个已有的关系、或新建一条。面板这一段只展示。
          两行与上面的关系行同一副身材 */}
      <Row className="-mx-2 mt-2" icon={<Link2 size={14} />} onClick={onConnect}>
        {S.ontology.connectOpen}
      </Row>
      <Row className="-mx-2" icon={<Plus size={14} />} onClick={onAddNew}>
        {S.ontology.schemaAddRelationship}
      </Row>
    </div>
  );
}

/* ---------- 属性卡片：选中类的字面值字段（改动开弹窗） ---------- */

function AttributesCard({
  attributes,
  onEdit,
  onNew,
}: {
  attributes: RelationTypeView[];
  onEdit: (attribute: RelationTypeView) => void;
  onNew: () => void;
}) {
  return (
    <div>
      <p className="mb-2 text-fine text-ink-2">
        {S.ontology.attributesHint}
      </p>
      <div className="divide-y divide-line">
        {attributes.map((a) => (
          <Row
            key={a.id}
            className="-mx-2"
            trailing={<span className="u-num">{S.ontology.usage(a.usage)}</span>}
            onClick={() => onEdit(a)}
          >
            <span className="flex items-center gap-2">
              <span className="truncate">{a.label}</span>
              <Chip tone="neutral">
                {S.ontology.datatypeNames[a.datatype ?? "text"]}
              </Chip>
              {a.unit && (
                <span className="shrink-0 text-small text-ink-2">{a.unit}</span>
              )}
              {a.functional && <Chip tone="info">1:1</Chip>}
            </span>
          </Row>
        ))}
      </div>
      <Row className="-mx-2 mt-2" icon={<Plus size={14} />} onClick={onNew}>
        {S.ontology.newAttribute}
      </Row>
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
      const p = treeParent(t);
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
    <div className="u-rail-list">
      {paged.map(({ t, depth, hasChildren }) => (
        <Row
          key={t.id}
          indent={depth}
          active={selectedId === t.id}
          onClick={() => onSelect(t.id)}
          icon={
            hasChildren ? (
              // 折叠柄：有子类才渲染，点击不选中。是 span 不是按钮——按钮里不能再嵌按钮
              <span
                onClick={(e) => {
                  e.stopPropagation();
                  onToggle(t.id);
                }}
                className="u-toggle"
              >
                <ChevronRight
                  size={12}
                  className={cn("u-turn", !collapsed.has(t.id) && "rotate-90")}
                />
              </span>
            ) : (
              <span className="block w-3" />
            )
          }
        >
          <span className="flex items-center gap-2">
            {/* 方形是直角：与圆形拉开区分度（图谱节点同理） */}
            <span
              className={`h-2.5 w-2.5 shrink-0 ${t.shape === "square" ? "scale-90" : "rounded-full"}`}
              style={{ background: t.color }}
            />
            {/* 不在列表里放逐项用量读数：数量级上来后统计和渲染都是负担，用量看表单 */}
            <span className="truncate">{t.label}</span>
          </span>
        </Row>
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
    <div className="u-rail-list">
      {paged.map((r) => (
        <Row
          key={r.id}
          active={selectedId === r.id}
          onClick={() => onSelect(r.id)}
          // 前导只留折叠柄槽：文字起点对齐类行"标识点"的左端
          icon={<span className="block w-3" />}
        >
          <span className="flex items-center gap-2">
            <span className="truncate">{r.label}</span>
            {r.functional && <Chip tone="info">1:1</Chip>}
          </span>
        </Row>
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

/* ---------- 定义（只读）：面板展示，改动开弹窗 ---------- */

/** 一行「标签 / 值」；值空着就写占位，不留白 */
function Def({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="border-b border-line py-2 last:border-0">
      <div className="text-small text-ink-2">{label}</div>
      <div className="mt-1 break-words text-body text-ink">{children}</div>
    </div>
  );
}

function Description({ text }: { text: string | null | undefined }) {
  return text?.trim() ? (
    <span className="whitespace-pre-wrap">{text}</span>
  ) : (
    <span className="text-ink-2">{S.ontology.noDescription}</span>
  );
}

export function ClassDefinition({
  cls,
  allTypes,
  onSelectClass,
  onNewSub,
}: {
  cls: EntityTypeView;
  allTypes: EntityTypeView[];
  onSelectClass: (id: string) => void;
  /** 以当前类为父级新建子类：开弹窗 */
  onNewSub: () => void;
}) {
  const nameOf = (id: string) => allTypes.find((t) => t.id === id)?.label ?? id;
  const classLinks = (ids: string[]) =>
    ids.map((id, index) => (
      <span key={id}>
        {index > 0 && ", "}
        <LinkButton onClick={() => onSelectClass(id)}>{nameOf(id)}</LinkButton>
      </span>
    ));
  const subclasses = allTypes.filter((type) => type.parents.includes(cls.id));
  return (
    <div>
      <Def label={S.ontology.parent}>
        {cls.parents.length > 0 ? classLinks(cls.parents) : S.ontology.noParent}
      </Def>
      <Def label={S.ontology.subclasses}>
        {subclasses.length > 0
          ? classLinks(subclasses.map((type) => type.id))
          : S.ontology.noSubclasses}
      </Def>
      <Def label={S.ontology.disjoint}>
        {cls.disjoint.length > 0 ? cls.disjoint.map(nameOf).join(", ") : S.ontology.noDisjoint}
      </Def>
      <Def label={S.ontology.description}>
        <Description text={cls.description} />
      </Def>
      {/* 新增入口与列表里的行同一副身材（左栏的「New class」也是这样一行）：
          图标落在文字的左缘上，不再用带内距的按钮去对 */}
      <Row className="-mx-2 mt-2" icon={<Plus size={14} />} onClick={onNewSub}>
        {S.ontology.newSubClass}
      </Row>
    </div>
  );
}

function PropertyDefinition({
  rel,
  allTypes,
  allRelations,
}: {
  rel: RelationTypeView;
  allTypes: EntityTypeView[];
  allRelations: RelationTypeView[];
}) {
  const typeName = (id: string) => allTypes.find((t) => t.id === id)?.label ?? id;
  const relName = (id: string) => allRelations.find((r) => r.id === id)?.label ?? id;
  const temporal =
    (
      {
        state: S.ontology.temporalState,
        event: S.ontology.temporalEvent,
        eternal: S.ontology.temporalEternal,
      } as Record<string, string>
    )[rel.temporal] ?? rel.temporal;
  // 勾了的公理才列：没勾的不是信息
  const axioms = (
    [
      [rel.functional, S.ontology.functional],
      [rel.inverse_functional, S.ontology.inverseFunctional],
      [rel.is_transitive, S.ontology.transitive],
      [rel.is_symmetric, S.ontology.symmetric],
      [rel.is_asymmetric, S.ontology.asymmetric],
      [rel.is_irreflexive, S.ontology.irreflexive],
    ] as const
  )
    .filter(([on]) => on)
    .map(([, name]) => name);
  return (
    <div>
      <Def label={S.ontology.domainLabel}>
        {rel.domains.length > 0 ? rel.domains.map(typeName).join(", ") : S.ontology.anyType}
      </Def>
      <Def label={S.ontology.rangeLabel}>
        {rel.ranges.length > 0 ? rel.ranges.map(typeName).join(", ") : S.ontology.anyType}
      </Def>
      {/* 边上的属性（0037）：这条关系的边能带哪些属性，按属性的 label 列 */}
      <Def label={S.ontology.qualifiers}>
        {/* `?? []`：这一格是后加的（0037），**旧版本的后端不发它**。
            少一格不该让整块面板崩掉——点一条关系就白屏，正是这么来的 */}
        {(rel.qualifiers ?? []).length > 0
          ? (rel.qualifiers ?? []).map(typeName).join(", ")
          : S.ontology.noQualifiers}
      </Def>
      <Def label={S.ontology.temporal}>{temporal}</Def>
      <Def label={S.ontology.axioms}>
        {axioms.length > 0 ? (
          <span className="flex flex-wrap gap-1">
            {axioms.map((a) => (
              <Chip key={a} tone="info">
                {a}
              </Chip>
            ))}
          </span>
        ) : (
          <span className="text-ink-2">{S.ontology.axiomsNone}</span>
        )}
      </Def>
      {rel.inverse_of && (
        <Def label={S.ontology.inverseOf}>{relName(rel.inverse_of)}</Def>
      )}
      {rel.sub_property_of && (
        <Def label={S.ontology.subPropertyOf}>{relName(rel.sub_property_of)}</Def>
      )}
      <Def label={S.ontology.description}>
        <Description text={rel.description} />
      </Def>
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
      <PageHeader className="mb-2" title={S.ontology.refineTitle} sub={S.ontology.refineHint} />

      <div className="flex gap-2">
        <Button variant="secondary" size="sm" disabled={busy} onClick={() => look.mutate()}>
          {look.isPending ? S.ontology.refineLooking : S.ontology.refinePreview}
        </Button>
        <Button variant="primary" size="sm" disabled={busy} onClick={() => run.mutate()}>
          {run.isPending ? S.ontology.refineRunning : S.ontology.refineRun}
        </Button>
      </div>

      {/* ---- 只算不写的那一步 */}
      {preview && (
        <div className="space-y-2">
          <p className="text-small text-ink-2">
            {preview.length === 0
              ? S.ontology.refineNothing
              : S.ontology.refineCandidates(preview.length)}
          </p>
          {preview.map((s) => (
            <div key={s.entity_id} className="glass rounded-panel p-3">
              <div className="flex items-baseline gap-2 flex-wrap">
                <span className="text-body text-ink">{s.name}</span>
                <span className="text-fine text-ink-2">
                  {s.coarse ?? S.graph.untyped}
                </span>
                {s.specific_type && (
                  <span className="text-fine text-warn">
                    {S.ontology.refineModelSays(s.specific_type)}
                  </span>
                )}
                <span className="ml-auto u-num text-fine text-ink-2">
                  {S.review.factsCount(s.fact_count)}
                </span>
              </div>
              {/* **把送去检索的那段字显示出来**：找不着的时候，第一个要看的
                  就是我们拿什么去找的，而不是猜画像还是类描述的问题 */}
              <p className="mt-1 text-fine text-ink-2 line-clamp-2">
                {s.profile}
              </p>
              <div className="mt-2 flex flex-wrap gap-1">
                {s.candidates.slice(0, 6).map((c) => (
                  <span
                    key={c.id}
                    title={c.description}
                    className={chipLike("neutral", "u-num text-fine")}
                  >
                    {c.label} {c.distance.toFixed(2)}
                  </span>
                ))}
                {s.candidates.length === 0 && (
                  <span className="text-fine text-ink-2">
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
            <span className="text-small text-ink-2">
              {S.ontology.refineRetyped(outcome.retyped)}
            </span>
            {outcome.batch && outcome.retyped > 0 && (
              <Button
                variant="secondary"
                size="sm"
                disabled={undo.isPending}
                onClick={() => undo.mutate(outcome.batch!)}
              >
                {S.ontology.refineUndo}
              </Button>
            )}
          </div>

          {outcome.for_review.length > 0 && (
            <div className="space-y-2">
              <p className="text-small text-ink-2">
                {S.ontology.refineForReview(outcome.for_review.length)}
              </p>
              {outcome.for_review.map((r) => (
                <div key={r.entity_id} className="glass rounded-panel p-3">
                  <div className="flex items-baseline gap-2 flex-wrap">
                    <span className="text-body text-ink">{r.name}</span>
                    <span className="text-fine text-ink-2">
                      {r.coarse ?? S.graph.untyped} → {r.choice}
                    </span>
                    {r.crosses_axis && (
                      <Status tone="warn" className="text-fine">
                        {S.ontology.refineCrossesAxis}
                      </Status>
                    )}
                    <span className="ml-auto u-num text-fine text-ink-2">
                      {Math.round(r.confidence * 100)}%
                    </span>
                  </div>
                  {r.reason && (
                    <p className="mt-1 text-fine text-ink-2">
                      {r.reason}
                    </p>
                  )}
                  {/* **认可的是这一对类，不是这一个实体。** 认可一次，
                      之后同一对不再进人工——那正是这一档大部分条目的成因 */}
                  {r.from_type_id && (
                    <Button
                      variant="primary"
                      size="sm"
                      className="mt-2"
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
                    </Button>
                  )}
                </div>
              ))}
            </div>
          )}

          {outcome.left_alone.length > 0 && (
            <div className="space-y-2">
              <p className="text-small text-ink-2">
                {S.ontology.refineLeftAlone(outcome.left_alone.length)}
              </p>
              {outcome.left_alone.map((d, i) => (
                <div key={i} className="glass rounded-panel px-3 py-2">
                  <div className="flex items-baseline gap-2 flex-wrap">
                    <span className="text-body text-ink">
                      {d.name}
                    </span>
                    <span className="text-fine text-ink-2">
                      {d.coarse ?? S.graph.untyped}
                    </span>
                  </div>
                  {/* 理由与头一个候选一起给：理由说不通时，看候选就知道是
                      检索没找着还是裁决没看上 */}
                  {d.reason && (
                    <p className="mt-1 text-fine text-ink-2">
                      {d.reason}
                    </p>
                  )}
                  {d.top_candidate && (
                    <p className="mt-1 text-fine text-ink-2">
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
/** 一端挂着两个以上开放值的谓词（#341）。
 *
 *  自动闭合靠 `functional` / `inverse_functional`，而本体自己长出来的库里
 *  没人声明过它们——引擎也不替人推断（那会让账本被一条猜出来的公理改写）。
 *  于是接任不闭合前任：两条 `leads` 都开着，问六月谁在管，两个都答。
 *
 *  **这一档不替人做决定，只把证据摆出来。** 谁挂着哪些值、对账会闭合几条、
 *  几条拿不准要进人审——按钮按下去之前都写在脸上，因为闭合一条从 2023 年
 *  开着的区间是对账本的真实改动。 */
function UniquenessPanel({
  kbId,
  candidates,
  relations,
  pending,
  onChanged,
  onError,
}: {
  kbId: string;
  candidates: UniquenessCandidate[];
  relations: RelationTypeView[];
  pending: boolean;
  onChanged: () => void;
  onError: (e: unknown) => void;
}) {
  // 刚做完的那一条留一行结果：面板会重取，卡片随之消失，
  // 不留话的话人只看到东西没了，不知道闭合了几条
  const [done, setDone] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);

  const apply = async (c: UniquenessCandidate) => {
    setBusy(c.predicate_id);
    try {
      // 两步：先补声明（已声明的跳过），再对账。声明是本体的事，
      // 对账是账本的事——后端的 reconcile 在没有声明时会拒绝执行。
      //
      // **整条一起发。** 这个端点是整体替换：缺省字段等于清空（后端在
      // `RelationTypeReq` 上写了为什么——一半覆盖一半保留会让「我把逆去掉了」
      // 和「我没碰逆」长得一样）。只发一个公理会把另外五条连同 inverse_of
      // 一起抹掉，而那是本体里最难重建的部分
      if (!c.declared) {
        const rel = relations.find((r) => r.id === c.predicate_id);
        if (!rel) throw new Error(c.label);
        await api.updateRelationType(kbId, c.predicate_id, {
          label: rel.label,
          temporal: rel.temporal,
          functional: c.axiom === "functional" ? true : rel.functional,
          inverse_functional:
            c.axiom === "inverse_functional" ? true : rel.inverse_functional,
          is_transitive: rel.is_transitive,
          is_symmetric: rel.is_symmetric,
          is_asymmetric: rel.is_asymmetric,
          is_irreflexive: rel.is_irreflexive,
          inverse_of: rel.inverse_of,
          sub_property_of: rel.sub_property_of,
          description: rel.description,
          domains: rel.domains,
          ranges: rel.ranges,
          qualifiers: rel.qualifiers,
        });
      }
      const r = await api.reconcileRelationType(kbId, c.predicate_id);
      setDone((d) => ({
        ...d,
        [c.predicate_id]: S.ontology.uniquenessDone(r.corrected, r.conflicts),
      }));
      onChanged();
    } catch (e) {
      onError(e);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-4">
      <PageHeader className="mb-2" title={S.ontology.uniqueness} sub={S.ontology.uniquenessHint} />

      {pending ? (
        <p className="text-small text-ink-2">{S.nav.loading}</p>
      ) : candidates.length === 0 ? (
        <p className="text-small text-ink-2">{S.ontology.uniquenessEmpty}</p>
      ) : (
        <div className="space-y-2">
          {candidates.map((c) => (
            <div
              key={`${c.predicate_id}-${c.side}`}
              className="rounded-panel border border-line bg-surface px-3 py-3"
            >
              <div className="flex items-center gap-2">
                <span className="text-body text-ink">{c.label}</span>
                <span className="font-mono text-fine text-ink-2">
                  {c.key}
                </span>
                {c.declared && (
                  <Chip tone="neutral">{S.ontology.uniquenessDeclared}</Chip>
                )}
                <span className="ml-auto">
                  {done[c.predicate_id] ? (
                    <span className="text-small text-ink-2">
                      {done[c.predicate_id]}
                    </span>
                  ) : (
                    <Button variant="primary"
                      size="sm"
                      onClick={() => apply(c)}
                      disabled={busy === c.predicate_id}
                    >
                      {busy === c.predicate_id
                        ? S.ontology.uniquenessBusy
                        : c.declared
                          ? S.ontology.uniquenessReconcile
                          : S.ontology.uniquenessDeclare}
                    </Button>
                  )}
                </span>
              </div>

              <p className="mt-1 text-small text-ink-2">
                {c.side === "subject"
                  ? S.ontology.uniquenessSubject(c.holders)
                  : S.ontology.uniquenessObject(c.holders)}
                {" · "}
                {S.ontology.uniquenessEffect(c.would_close, c.would_review)}
              </p>

              {/* 证据：谁挂着哪些值。**这是人做判断的依据**，不是装饰——
                  看见「张三 leads 凤凰 / 李四 leads 凤凰」才知道该不该声明 */}
              {c.examples.length > 0 && (
                <div className="mt-2 space-y-1 border-t border-line pt-2">
                  {c.examples.slice(0, 3).map((ex) => (
                    <div
                      key={ex.holder}
                      className="flex items-baseline gap-2 text-small"
                    >
                      <span className="shrink-0 text-ink-2">
                        {ex.holder}
                      </span>
                      <span className="flex flex-wrap gap-x-3 gap-y-1 text-ink-2">
                        {ex.values.map((v) => (
                          <span key={v.fact_id}>
                            {v.name ?? "—"}
                            {v.valid_from && (
                              <span className="u-num ml-1 text-ink-2">
                                {S.ontology.uniquenessSince(
                                  v.valid_from.slice(0, 10),
                                )}
                              </span>
                            )}
                          </span>
                        ))}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** Suggest 提的那部分：批量加入只吃它们。代理提的每条都是人签的契约（0012），不进「全部加入」 */
function manualOnly(d: OntologyProposals): OntologyProposals {
  const own = <T extends { proposed_by?: string }>(xs: T[] | undefined) =>
    (xs ?? []).filter((x) => x.proposed_by !== "agent");
  return {
    entity_types: own(d.entity_types),
    relation_types: own(d.relation_types),
    attribute_types: own(d.attribute_types),
    map_to: own(d.map_to),
  };
}
function manualCount(d: OntologyProposals): number {
  const m = manualOnly(d);
  return (
    m.entity_types.length +
    m.relation_types.length +
    (m.attribute_types?.length ?? 0) +
    (m.map_to?.length ?? 0)
  );
}

/** 库里现在有几条代理提的提案：等代理答完的判据 */
function agentCount(d: OntologyProposals | null | undefined): number {
  if (!d) return 0;
  return [
    ...(d.map_to ?? []),
    ...d.entity_types,
    ...d.relation_types,
    ...(d.attribute_types ?? []),
  ].filter((p) => p.proposed_by === "agent").length;
}

/** 代理提案多带的几格（0061）：定义、会绑的形状、类别词、服务的问题。
    Suggest 的一份没有 proposed_by，什么都不画 */
function AgentMeta({
  p,
}: {
  p: AgentProposalFields & { description?: string; forms?: string[] };
}) {
  if (p.proposed_by !== "agent") return null;
  const shapes = p.signatures?.phrases ?? [];
  const shapeText = shapes
    .map(
      (sh) =>
        `${sh.subject ?? "?"} — ${sh.phrase} → ${sh.value ? "value" : (sh.object ?? "?")}` +
        (sh.direction === "reverse" ? " (reverse)" : ""),
    )
    .join("\n");
  const examples = (p.examples ?? []).join("\n");
  return (
    <>
      <Chip tone="success" title={S.ontology.askAgentHint}>
        {S.ontology.agentChip}
      </Chip>
      {p.description && (
        <span className="text-small text-ink-2 truncate" title={p.description}>
          {p.description}
        </span>
      )}
      {shapes.length > 0 && (
        <span
          className="text-small text-accent whitespace-nowrap"
          title={examples ? `${shapeText}\n\n${examples}` : shapeText}
        >
          {S.ontology.bindsShapes(shapes.length)}
        </span>
      )}
      {!!p.kind_words?.length && (
        <span
          className="text-small text-accent whitespace-nowrap"
          title={p.kind_words.join(" · ")}
        >
          {S.ontology.bindsKindWords(p.kind_words.length)}
        </span>
      )}
      {!!p.serves?.length && (
        <Chip tone="info">{S.ontology.servesQuestions(p.serves.length)}</Chip>
      )}
    </>
  );
}

/** 能力问题（0061 决定 1）：本体该答得上来的问题。人写下即 accepted；退役的留着看 */
function QuestionsPanel({
  kbId,
  onError,
}: {
  kbId: string;
  onError: (e: unknown) => void;
}) {
  const qc = useQueryClient();
  // 叫了代理之后等它提问题：隔几秒看一眼，多出 proposed 的就停
  const [awaiting, setAwaiting] = useState<{ since: number; had: number } | null>(null);
  const questions = useQuery({
    queryKey: ["questions", kbId],
    queryFn: () => api.listQuestions(kbId),
    refetchInterval: awaiting ? 4000 : false,
  });
  const report = useQuery({
    queryKey: ["questionReport", kbId],
    queryFn: () => api.questionReport(kbId),
  });
  useEffect(() => {
    if (!awaiting || !questions.data) return;
    const now = questions.data.filter((q) => q.status === "proposed").length;
    if (now > awaiting.had || Date.now() - awaiting.since > 120_000) setAwaiting(null);
  }, [awaiting, questions.data]);
  const [draft, setDraft] = useState("");
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ["questions", kbId] });
    qc.invalidateQueries({ queryKey: ["questionReport", kbId] });
  };
  const askForQuestions = useMutation({
    mutationFn: () => api.proposeQuestions(kbId),
    onSuccess: () => {
      toast.info(S.ontology.questionsQueued);
      setAwaiting({
        since: Date.now(),
        had: (questions.data ?? []).filter((q) => q.status === "proposed").length,
      });
    },
    onError,
  });
  const decide = useMutation({
    mutationFn: ({ q, status }: { q: CompetencyQuestion; status: "accepted" | "rejected" }) =>
      api.updateQuestion(kbId, q.id, { status }),
    onSuccess: refresh,
    onError,
  });
  const add = useMutation({
    mutationFn: (question: string) => api.createQuestion(kbId, { question }),
    onSuccess: () => {
      toast.success(S.ontology.questionAdded);
      setDraft("");
      refresh();
    },
    onError,
  });
  const retire = useMutation({
    mutationFn: (q: CompetencyQuestion) =>
      api.updateQuestion(kbId, q.id, { status: "retired" }),
    onSuccess: () => {
      toast.success(S.ontology.questionRetired);
      refresh();
    },
    onError,
  });
  const remove = useMutation({
    mutationFn: (q: CompetencyQuestion) => api.deleteQuestion(kbId, q.id),
    onSuccess: refresh,
    onError,
  });
  const rows = (questions.data ?? []).filter((q) => q.status !== "rejected");
  const submit = () => {
    const text = draft.trim();
    if (text) add.mutate(text);
  };
  return (
    <div className="mt-6 border-t border-line pt-3">
      <div className="flex items-center gap-2">
        <h4 className="text-small font-semibold text-ink-2">{S.ontology.questions}</h4>
        {report.data && report.data.questions.checked > 0 && (
          <Chip tone="info" title={S.ontology.questionsScoreHint}>
            {S.ontology.questionsScore(
              report.data.questions.answered,
              report.data.questions.checked,
            )}
          </Chip>
        )}
        {report.data && report.data.proposals.decided > 0 && (
          <Chip tone="neutral" title={S.ontology.proposalsChangedHint}>
            {S.ontology.proposalsChanged(
              report.data.proposals.changed,
              report.data.proposals.decided,
            )}
          </Chip>
        )}
        <Button
          size="sm"
          variant="secondary"
          className="ml-auto"
          title={S.ontology.askQuestionsHint}
          onClick={() => askForQuestions.mutate()}
          disabled={askForQuestions.isPending || awaiting !== null}
        >
          {awaiting ? S.ontology.agentWaiting : S.ontology.askQuestions}
        </Button>
      </div>
      <p className="mb-2 text-small text-ink-2">{S.ontology.questionsHint}</p>
      <div className="mb-2 flex items-center gap-2">
        <Input
          size="sm"
          className="flex-1"
          value={draft}
          placeholder={S.ontology.questionPlaceholder}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
        />
        <Button
          size="sm"
          variant="secondary"
          onClick={submit}
          disabled={add.isPending || !draft.trim()}
        >
          {S.ontology.addQuestion}
        </Button>
      </div>
      {rows.length === 0 ? (
        <p className="text-body text-ink-2">{S.ontology.noQuestions}</p>
      ) : (
        <div className="space-y-1">
          {rows.map((q) => (
            <div key={q.id} className="flex items-center gap-2 text-body">
              <span
                className={cn(
                  "flex-1 min-w-0 truncate",
                  q.status === "retired" ? "text-ink-2 line-through" : "text-ink",
                )}
                title={q.question}
              >
                {q.question}
              </span>
              {q.origin === "agent" && (
                <Chip tone="success">{S.ontology.agentChip}</Chip>
              )}
              {q.last_result != null &&
                typeof q.last_result === "object" &&
                "answered" in q.last_result && (
                  <Chip
                    tone={(q.last_result as { answered?: boolean }).answered ? "success" : "warn"}
                  >
                    {(q.last_result as { answered?: boolean }).answered
                      ? S.ontology.questionAnswered
                      : S.ontology.questionUnanswered}
                  </Chip>
                )}
              {q.status === "proposed" && (
                <>
                  <Button
                    size="sm"
                    variant="primary"
                    onClick={() => decide.mutate({ q, status: "accepted" })}
                    disabled={decide.isPending}
                  >
                    {S.ontology.acceptQuestion}
                  </Button>
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => decide.mutate({ q, status: "rejected" })}
                    disabled={decide.isPending}
                  >
                    {S.ontology.rejectProposal}
                  </Button>
                </>
              )}
              {q.status === "accepted" && (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => retire.mutate(q)}
                  disabled={retire.isPending}
                >
                  {S.ontology.retireQuestion}
                </Button>
              )}
              <IconButton
                size="sm"
                label={S.ontology.deleteQuestion}
                onClick={() => remove.mutate(q)}
                disabled={remove.isPending}
              >
                <X size={13} />
              </IconButton>
            </div>
          ))}
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
  // 叫了代理之后等它：任务在后台跑，隔几秒看一眼库里有没有新提案（0061）。
  // 记的是叫它那一刻代理提案的条数——多出来了就是它答完了；两分钟没动静就不等了
  const [awaitingAgent, setAwaitingAgent] = useState<{
    since: number;
    had: number;
  } | null>(null);
  const storedProposals = useQuery({
    queryKey: ["storedProposals", kbId],
    queryFn: () => api.storedProposals(kbId),
    refetchInterval: awaitingAgent ? 4000 : false,
  });
  useEffect(() => {
    if (!awaitingAgent || !storedProposals.data) return;
    const d = storedProposals.data;
    const now = agentCount(d);
    if (now > awaitingAgent.had || Date.now() - awaitingAgent.since > 120_000) {
      setProposals(d);
      setAwaitingAgent(null);
    }
  }, [awaitingAgent, storedProposals.data]);
  useEffect(() => {
    // 只在还没有本地结果时回填。刚点完 Suggest 的那一批更新，不该被覆盖
    if (proposals === null && storedProposals.data) {
      const d = storedProposals.data;
      const empty =
        !d.entity_types?.length &&
        !d.relation_types?.length &&
        !d.attribute_types?.length &&
        !d.map_to?.length;
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

  const askAgent = useMutation({
    mutationFn: () => api.proposeOntology(kbId),
    onSuccess: () => {
      toast.info(S.ontology.agentQueued);
      setAwaitingAgent({
        since: Date.now(),
        had: agentCount(storedProposals.data ?? proposals),
      });
    },
    onError,
  });
  // 拒绝连理由：改状态不删行，理由下一轮代理读得到（0061）
  const rejectProposal = useMutation({
    mutationFn: ({
      section,
      key,
      reason,
    }: {
      section: "map_to" | "entity_types" | "relation_types" | "attribute_types";
      key: string;
      reason?: string;
    }) => api.decideProposal(kbId, section, key, "rejected", reason),
    onSuccess: (_d, v) => {
      toast.success(S.ontology.rejectedProposal);
      setProposals(
        (prev) =>
          prev && {
            ...prev,
            [v.section]: (prev[v.section] ?? []).filter((x) => x.key !== v.key),
          },
      );
    },
    onError,
  });
  const reject = (
    section: "map_to" | "entity_types" | "relation_types" | "attribute_types",
    key: string,
  ) => {
    const reason = window.prompt(S.ontology.rejectReasonPrompt) ?? "";
    rejectProposal.mutate({ section, key, reason: reason.trim() || undefined });
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
    // 代理提的走采纳端点：服务端建类、标记提案、排对齐（0061 决定 3）
    mutationFn: (p: {
      key: string;
      label: string;
      description?: string;
      proposed_by?: string;
    }) =>
      p.proposed_by === "agent"
        ? api.adoptProposal(kbId, "entity_types", p.key)
        : api.createEntityType(kbId, {
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
      proposed_by?: string;
    }) =>
      p.proposed_by === "agent"
        ? api.adoptProposal(kbId, "relation_types", p.key)
        : p.forms?.length
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
      proposed_by?: string;
    }) =>
      p.proposed_by === "agent"
        ? api
            .adoptProposal(kbId, "attribute_types", p.key)
            .then(() => ({}) as Awaited<ReturnType<typeof api.adoptPredicate>>)
        : api.adoptPredicate(kbId, {
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
    // 代理提的「已有」：形状各写一条人的绑定判定（0061 cut 1.1），走采纳端点
    mutationFn: (p: {
      key: string;
      kind?: string;
      forms?: string[];
      proposed_by?: string;
    }) =>
      p.proposed_by === "agent"
        ? api
            .adoptProposal(kbId, "map_to", p.key)
            .then(() => ({}) as Awaited<ReturnType<typeof api.adoptPredicate>>)
        : api.adoptPredicate(kbId, {
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
    // 整页视图不再套一层卡片：标题是页标题，正文平铺（与 Refine / Rules 同一副样子）
    <div>
      <PageHeader
        title={S.ontology.misses}
        sub={S.ontology.missesHint}
        actions={
          <>
            {misses.length > 0 && (
              <Button
                size="sm"
                variant="secondary"
                onClick={() => suggest.mutate()}
                disabled={suggest.isPending}
              >
                {suggest.isPending ? S.ontology.suggesting : S.ontology.suggest}
              </Button>
            )}
            <Button
              size="sm"
              variant="secondary"
              title={S.ontology.askAgentHint}
              onClick={() => askAgent.mutate()}
              disabled={askAgent.isPending || awaitingAgent !== null}
            >
              {awaitingAgent ? S.ontology.agentWaiting : S.ontology.askAgent}
            </Button>
          </>
        }
      />

      {misses.length === 0 ? (
        <p className="text-body text-ink-2">{S.ontology.noMisses}</p>
      ) : (
        <div className="flex flex-wrap gap-2">
          {misses.map((m) => (
            <span
              key={`${m.kind}:${m.key}`}
              className="glass rounded-cell px-3 py-1 text-small flex items-center gap-2"
              title={m.example ?? ""}
            >
              <Chip tone={m.kind === "entity_type" ? "info" : "violet"}>
                {m.kind === "entity_type" ? "C" : "P"}
              </Chip>
              <span className="font-mono text-ink-2">{m.key}</span>
              <span className="text-ink-2">×{m.count}</span>
              <IconButton
                size="sm"
                label={S.ontology.dismiss}
                className="-mr-1"
                onClick={() => dismiss.mutate({ kind: m.kind, key: m.key })}
              >
                ✕
              </IconButton>
            </span>
          ))}
        </div>
      )}
      {dismissedMisses.length > 0 && (
        <div className="mt-3 border-t border-line pt-3">
          <Button
            variant="ghost"
            size="sm"
            className="-ml-2"
            onClick={() => setShowDismissed((v) => !v)}
          >
            {showDismissed ? "▾" : "▸"} {S.ontology.dismissed(dismissedMisses.length)}
          </Button>
          {showDismissed && (
            <>
              <p className="text-small text-ink-2 mt-2 mb-2">
                {S.ontology.dismissedHint}
              </p>
              <div className="flex flex-wrap gap-2">
                {dismissedMisses.map((m) => (
                  <span
                    key={`d:${m.kind}:${m.key}`}
                    className="glass rounded-cell px-3 py-1 text-small flex items-center gap-2 opacity-60"
                    title={m.example ?? ""}
                  >
                    <Chip tone={m.kind === "entity_type" ? "info" : "violet"}>
                      {m.kind === "entity_type" ? "C" : "P"}
                    </Chip>
                    <span className="font-mono text-ink-2 line-through">
                      {m.key}
                    </span>
                    <span className="text-ink-2">×{m.count}</span>
                    <IconButton
                      size="sm"
                      label={S.ontology.restore}
                      className="-mr-1"
                      onClick={() => restore.mutate({ kind: m.kind, key: m.key })}
                    >
                      ↺
                    </IconButton>
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
        <div className="mt-3 rounded-panel border border-line-strong bg-surface px-3 py-3">
          <div className="flex items-start gap-2">
            <div className="min-w-0 flex-1">
              <p className="text-small text-ink">
                {S.ontology.autoRanTitle}
              </p>
              <p className="mt-1 text-fine text-ink-2">
                {S.ontology.autoRanBody(
                  autoRun.data.run.relations ?? [],
                  autoRun.data.run.facts_remapped ?? 0,
                )}
              </p>
              <p className="mt-1 text-fine text-ink-2">
                {S.ontology.autoRanOff}
              </p>
            </div>
            <Button
              size="sm"
              variant="secondary"
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
        <div className="mt-3 flex items-center gap-2 rounded-panel border border-line bg-surface px-3 py-2">
          <span className="text-small text-ink-2">
            {S.ontology.undoAdopt(lastAdopt.key, lastAdopt.moved)}
          </span>
          <span className="text-fine text-ink-2">
            {S.ontology.undoKeepsRelation}
          </span>
          <Button
            size="sm"
            variant="secondary"
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
        <div className="mt-4 border-t border-line pt-3">
          <div className="mb-2 flex items-center gap-2">
            <h4 className="text-small font-semibold text-ink-2">
              {S.ontology.proposals}
            </h4>
            {/* 常见情形是"这些都对"——一条条点是把一个决定拆成八个 */}
            {manualCount(proposals) > 1 && (
              <Button
                size="sm"
                variant="secondary"
                className="ml-auto"
                disabled={addAll.isPending}
                onClick={() => addAll.mutate(manualOnly(proposals))}
              >
                {addAll.isPending
                  ? S.ontology.addingAll
                  : S.ontology.addAll(manualCount(proposals))}
              </Button>
            )}
          </div>
          <div className="space-y-2">
            {/* 排在最前：它说的是"本体已经有了"，而这正是最该先看见的一句。
                排在新建后面的话，人一路点下来就把重复建出来了 */}
            {(proposals.map_to ?? []).map((p) => (
              <div key={`map-${p.key}`} className="flex items-center gap-2 text-body">
                <Chip tone="success">=</Chip>
                <span className="font-mono text-ink-2">{p.key}</span>
                {p.label && <span className="text-ink">{p.label}</span>}
                {!!p.forms?.length && (
                  <span
                    className="text-small text-ink-2 truncate"
                    title={p.forms.join(" · ")}
                  >
                    {p.forms.join(" · ")}
                  </span>
                )}
                {/* 改写多少条是 0003 那条路的账；代理提的走绑定，形状数在 AgentMeta 里 */}
                {!!p.forms?.length && p.proposed_by !== "agent" && (
                  <span className="text-small text-accent">
                    {S.ontology.willRemap(factsWaiting(p.forms))}
                  </span>
                )}
                {p.reason && (
                  <span className="text-small text-ink-2 truncate">
                    {p.reason}
                  </span>
                )}
                <AgentMeta p={p} />
                <Button variant="primary"
                  size="sm"
                  className="ml-auto"
                  onClick={() => approveMapping.mutate(p)}
                  disabled={approveMapping.isPending}
                >
                  {S.ontology.mapOver}
                </Button>
                {p.proposed_by === "agent" && (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => reject("map_to", p.key)}
                    disabled={rejectProposal.isPending}
                  >
                    {S.ontology.rejectProposal}
                  </Button>
                )}
              </div>
            ))}
            {proposals.entity_types.map((p) => (
              <div key={p.key} className="flex items-center gap-2 text-body">
                <Chip tone="info">C</Chip>
                <span className="font-mono text-ink-2">{p.key}</span>
                <span className="text-ink">{p.label}</span>
                {p.reason && (
                  <span className="text-small text-ink-2 truncate">
                    {p.reason}
                  </span>
                )}
                <AgentMeta p={p} />
                <Button variant="primary"
                  size="sm"
                  className="ml-auto"
                  onClick={() => approveEntity.mutate(p)}
                  disabled={approveEntity.isPending}
                >
                  {S.ontology.approve}
                </Button>
                {p.proposed_by === "agent" && (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => reject("entity_types", p.key)}
                    disabled={rejectProposal.isPending}
                  >
                    {S.ontology.rejectProposal}
                  </Button>
                )}
              </div>
            ))}
            {proposals.relation_types.map((p) => (
              <div key={p.key} className="flex items-center gap-2 text-body">
                <Chip tone="violet">P</Chip>
                <span className="font-mono text-ink-2">{p.key}</span>
                <span className="text-ink">{p.label}</span>
                {p.temporal && <Chip tone="neutral">{p.temporal}</Chip>}
                {/* 影响面：采纳后会改写多少条、归并了哪些写法。没有这个，
                    "approve" 就只是凭空多一个空关系 */}
                {!!p.forms?.length && p.proposed_by !== "agent" && (
                  <span
                    className="text-small text-accent"
                    title={p.forms.join(" · ")}
                  >
                    {S.ontology.willRemap(factsWaiting(p.forms))}
                  </span>
                )}
                {p.reason && (
                  <span className="text-small text-ink-2 truncate">
                    {p.reason}
                  </span>
                )}
                <AgentMeta p={p} />
                <Button variant="primary"
                  size="sm"
                  className="ml-auto"
                  onClick={() => approveRelation.mutate(p)}
                  disabled={approveRelation.isPending}
                >
                  {S.ontology.approve}
                </Button>
                {p.proposed_by === "agent" && (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => reject("relation_types", p.key)}
                    disabled={rejectProposal.isPending}
                  >
                    {S.ontology.rejectProposal}
                  </Button>
                )}
              </div>
            ))}
            {(proposals.attribute_types ?? []).map((p) => (
              <div key={`attr-${p.key}`} className="flex items-center gap-2 text-body">
                {/* A 而不是 P：字面值那一档跟关系是两回事，界面上分得清 */}
                <Chip tone="warn">A</Chip>
                <span className="font-mono text-ink-2">{p.key}</span>
                <span className="text-ink">{p.label}</span>
                <Chip tone="neutral">{p.datatype ?? "text"}</Chip>
                {p.unit && <Chip tone="neutral">{p.unit}</Chip>}
                {!!p.forms?.length && p.proposed_by !== "agent" && (
                  <span
                    className="text-small text-accent"
                    title={p.forms.join(" · ")}
                  >
                    {S.ontology.willRemap(factsWaiting(p.forms))}
                  </span>
                )}
                {p.reason && (
                  <span className="text-small text-ink-2 truncate">
                    {p.reason}
                  </span>
                )}
                <AgentMeta p={p} />
                <Button variant="primary"
                  size="sm"
                  className="ml-auto"
                  onClick={() => approveAttribute.mutate(p)}
                  disabled={approveAttribute.isPending}
                >
                  {S.ontology.approve}
                </Button>
                {p.proposed_by === "agent" && (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => reject("attribute_types", p.key)}
                    disabled={rejectProposal.isPending}
                  >
                    {S.ontology.rejectProposal}
                  </Button>
                )}
              </div>
            ))}
            {proposals.entity_types.length === 0 &&
              proposals.relation_types.length === 0 &&
              !proposals.attribute_types?.length &&
              !proposals.map_to?.length && (
                <p className="text-body text-ink-2">—</p>
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
    <div>
      <PageHeader title={S.ontology.importTitle} sub={S.ontology.importHint} />

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
          variant="secondary"
          disabled={busy}
          onClick={() => pick.current?.click()}
        >
          {file ? S.ontology.importChange : S.ontology.importPick}
        </Button>
        {file && (
          <span className="text-small text-ink-2 truncate">
            <span className="font-mono">{file.name}</span>
            <span className="text-ink-2">
              {" "}
              · {S.ontology.importSize(file.size)}
            </span>
          </span>
        )}
        {preview.isPending && (
          <span className="text-small text-ink-2">
            {S.ontology.importReading}
          </span>
        )}
      </div>

      {plan && (
        <div className="mt-4">
          <p className="u-num text-fine text-ink-2">
            {S.ontology.importParsed(plan.format, plan.triples)}
          </p>

          {empty ? (
            <p className="mt-2 text-body text-ink-2">
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
                <Disclosure
                  className="mt-3"
                  summary={
                    <>
                      {S.ontology.importUnprojected} ({plan.unprojected.length})
                    </>
                  }
                >
                  <p className="mt-2 text-fine text-ink-2">
                    {S.ontology.importUnprojectedBody}
                  </p>
                  <ul className="mt-2 space-y-1">
                    {plan.unprojected.map(([iri, n]) => (
                      <li key={iri} className="flex gap-2 text-fine">
                        <span
                          className="font-mono text-ink-2 truncate"
                          title={iri}
                        >
                          {shortIri(iri)}
                        </span>
                        <span className="u-num text-ink-2 shrink-0">
                          ×{n}
                        </span>
                      </li>
                    ))}
                  </ul>
                </Disclosure>
              )}

              <div className="mt-4 flex items-center gap-2">
                <Button variant="primary"
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
                  variant="secondary"
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

      {/* 谁在什么时候拿哪个文件动过本体，以及那一次到底进来了什么 */}
      <ImportHistory kbId={kbId} />
    </div>
  );
}

/** 导入历史：**服务端每次导入记下的是一整本账**——建了几个类、更新了几个、
 *  几个键被占、逆属性连上了几条、属性跳过了几个、多少三元组、哪些 IRI 没投影
 *  下来。从前这一段只印「文件名 · 大小 · 谁在哪天」，其余全落在地上：
 *  导完之后想知道「到底进来了什么」，界面上没有一个地方说得出。
 *
 *  所以这里是一张表（每行一次导入的账），细账在行末的详情里——那些数只有
 *  出问题时才有人读，不该占着表宽。 */
function ImportHistory({ kbId }: { kbId: string }) {
  const [detail, setDetail] = useState<OntologyImportView | null>(null);
  const history = useQuery({
    queryKey: ["ontology-imports", kbId],
    queryFn: () => api.ontologyImports(kbId),
  });
  const rows = history.data?.imports ?? [];

  return (
    <div className="mt-6">
      <h4 className="mb-2 text-small font-medium text-ink-2">
        {S.ontology.importHistory}
      </h4>
      {/* 还在取的时候不能说「还没有导入过」——那句话是假的，而且它跟真的
          没导入过长得一模一样，读的人分不出自己看到的是哪一种 */}
      {history.isLoading ? (
        <p className="text-small text-ink-2">{S.nav.loading}</p>
      ) : !rows.length ? (
        <p className="text-small text-ink-2">{S.ontology.importNoHistory}</p>
      ) : (
        <div className="glass overflow-hidden rounded-panel">
          <Table>
            <THead>
              <Tr>
                <Th>{S.ontology.importColFile}</Th>
                <Th>{S.ontology.importColFormat}</Th>
                <Th>{S.ontology.importColSize}</Th>
                <Th>{S.ontology.importClasses}</Th>
                <Th>{S.ontology.importRelations}</Th>
                <Th>{S.ontology.importAttributes}</Th>
                <Th>{S.ontology.importColTriples}</Th>
                <Th>{S.ontology.importColWhen}</Th>
                <Th />
              </Tr>
            </THead>
            <TBody>
              {rows.map((im) => {
                const s = im.summary ?? {};
                return (
                  <Tr key={im.id}>
                    {/* 一格一件事，一行一条导入。**格式与大小各占一列**，不是
                        叠在文件名底下——叠着把每一行都撑成两行高，而右边还空着
                        半张表。文件名不用等宽：等宽是给键、id、代码和 URL 的
                        （DESIGN.md 1），一个文件名在一列 Geist 里只显得突兀 */}
                    <Td>
                      <div className="max-w-64 truncate text-small text-ink" title={im.filename}>
                        {im.filename}
                      </div>
                    </Td>
                    <Td className="text-small text-ink-2">{im.format}</Td>
                    <Td className="u-num whitespace-nowrap text-small text-ink-2">
                      {S.ontology.importSize(im.byte_size)}
                    </Td>
                    <Td className="text-small text-ink-2">
                      <Counts
                        created={s.classes_created}
                        updated={s.classes_updated}
                        taken={s.classes_key_taken}
                      />
                    </Td>
                    <Td className="text-small text-ink-2">
                      <Counts created={s.relations_created} updated={s.relations_updated} />
                    </Td>
                    <Td className="text-small text-ink-2">
                      <Counts
                        created={s.attributes_created}
                        skipped={sumCounts(s.attributes_skipped)}
                      />
                    </Td>
                    <Td className="u-num text-small text-ink-2">{s.triples ?? "—"}</Td>
                    <Td className="text-small text-ink-2 whitespace-nowrap">
                      {S.ontology.importBy(
                        im.imported_by_name ?? "—",
                        localDateTime(im.imported_at).slice(0, 16),
                      )}
                    </Td>
                    <Td className="text-right">
                      <LinkButton onClick={() => setDetail(im)}>
                        {S.ontology.importDetail}
                      </LinkButton>
                    </Td>
                  </Tr>
                );
              })}
            </TBody>
          </Table>
        </div>
      )}

      {/* 细账：只有出了问题才有人读，所以收在这里，而不是摊在表上 */}
      <Dialog
        open={!!detail}
        onOpenChange={(o) => !o && setDetail(null)}
        width="lg"
        closeLabel={S.ui.close}
        title={detail?.filename ?? ""}
        description={
          detail
            ? S.ontology.importBy(
                detail.imported_by_name ?? "—",
                localDateTime(detail.imported_at),
              )
            : undefined
        }
      >
        {detail && <ImportDetail im={detail} />}
      </Dialog>
    </div>
  );
}

/** `{no_domain: 3, unknown_domain: 1}` → 4。缺就是 0 */
function sumCounts(m: Record<string, number> | undefined): number {
  return Object.values(m ?? {}).reduce((a, b) => a + b, 0);
}

/** 表里一格：建了几个 / 更新了几个 / 占了几个键 / 跳过几个。全零就是一横 */
function Counts({
  created,
  updated,
  taken,
  skipped,
}: {
  created?: number;
  updated?: number;
  taken?: number;
  skipped?: number;
}) {
  const parts = [
    created ? S.ontology.importCreatedN(created) : null,
    updated ? S.ontology.importUpdatedN(updated) : null,
    skipped ? S.ontology.importSkippedN(skipped) : null,
    taken ? S.ontology.importTakenN(taken) : null,
  ].filter(Boolean);
  return <>{parts.length ? parts.join(" · ") : "—"}</>;
}

function ImportDetail({ im }: { im: OntologyImportView }) {
  const s = im.summary ?? {};
  const stat = (label: string, n: number | undefined) =>
    n === undefined ? null : (
      <div key={label} className="flex items-baseline justify-between gap-3 py-1">
        <span className="text-small text-ink-2">{label}</span>
        <span className="u-num text-small text-ink">{n}</span>
      </div>
    );
  const L = S.ontology;
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-2 gap-x-6">
        <div className="divide-y divide-line">
          {stat(L.statClassesCreated, s.classes_created)}
          {stat(L.statClassesUpdated, s.classes_updated)}
          {stat(L.statClassesTaken, s.classes_key_taken)}
          {stat(L.statClassesNoDesc, s.classes_without_description)}
          {stat(L.statTriples, s.triples)}
        </div>
        <div className="divide-y divide-line">
          {stat(L.statRelationsSeen, s.relations_seen)}
          {stat(L.statRelationsCreated, s.relations_created)}
          {stat(L.statRelationsUpdated, s.relations_updated)}
          {stat(L.statFunctional, s.functional_relations)}
          {stat(L.statInverseLinked, s.inverse_linked)}
          {stat(L.statSubPropertyLinked, s.sub_property_linked)}
          {stat(L.statAttributesSeen, s.attributes_seen)}
          {stat(L.statAttributesCreated, s.attributes_created)}
          {/* 跳过的属性**按理由分**：数字只说"少了几个"，理由才说得出下一步
              该改本体的哪里（域没写、域不在这个库里、值域用不了…） */}
          {Object.entries(s.attributes_skipped ?? {}).map(([reason, n]) =>
            stat(
              `${L.statAttributesSkipped} · ${S.ontology.skipReason[reason] ?? reason}`,
              n,
            ),
          )}
        </div>
      </div>

      {/* 没投影下来的 IRI：引用外部词汇表是常态，可「少连了多少」得说得出来 */}
      {!!s.unprojected?.length && (
        <div>
          <p className="mb-1 text-small font-medium text-ink-2">
            {S.ontology.importUnprojected} ({s.unprojected.length})
          </p>
          <p className="mb-2 text-fine leading-relaxed text-ink-2">
            {S.ontology.importUnprojectedBody}
          </p>
          <ul className="u-scroll max-h-48 space-y-1 overflow-y-auto">
            {s.unprojected.map(([iri, n]) => (
              <li key={iri} className="flex gap-2 text-fine">
                <span className="min-w-0 truncate font-mono text-ink-2" title={iri}>
                  {shortIri(iri)}
                </span>
                <span className="u-num shrink-0 text-ink-2">×{n}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
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
        "mt-3 rounded-panel border px-3 py-3",
        tone === "danger"
          ? "border-danger/25 bg-danger/[0.06]"
          : "border-warn/25 bg-warn/[0.06]",
      )}
    >
      <p className="text-small text-ink">{title}</p>
      <p className="mt-1 text-fine text-ink-2">{body}</p>
      {items.length > 0 && (
        <Disclosure
          className="mt-2"
          summary={items.length > 1 ? `${items.length} items` : "1 item"}
        >
          <ul className="mt-1 space-y-1">
            {items.map((it) => (
              <li key={it} className="font-mono text-fine text-ink-2">
                {it}
              </li>
            ))}
          </ul>
        </Disclosure>
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
    <div className="rounded-panel bg-surface px-3 py-2">
      <div className="flex items-center gap-2">
        <span className="text-small text-ink-2">{label}</span>
        <span className="ml-auto flex items-center gap-2">
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
      {note && <p className="mt-1 text-fine text-ink-2">{note}</p>}
    </div>
  );
}
