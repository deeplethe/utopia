# 0002 · 推理机

- **状态**：R0 已建成（事实层四类违规 + 本体自检八类缺陷，两张表两个 Review 页签）；R1 已建成，受 KB 开关 `materialize_inferences` 约束、**默认关**，四种公理规则都编得出（#132、#177、#179）；R2 只做了一层直接前提，证明树 API / UI 未建；R3 未做，每次全量重推，靠 `inference_interval_minutes`（缺省 60）定时兜着。2026-09-02 对照代码复核，修订见各节
- **成文**：2026-08-28
- **相关**：[0001](0001-ontology-import-and-governance.md) 的 P5（该节保留原判断，本文取代其排期）

placeholder 里写着意图：*"Temporal Datalog interpreter for derived facts with explanations, plus the ontology axiom compiler."* 方向对。本文定的是**顺序**和**安全边界**。

---

## 核心判断：第一交付是一致性检查，不是推导

**推理机是缺陷放大器。** 在 `Industry Corpus`（28 篇公开新闻稿）上实测的底子：

| 指标 | 数值 | 含义 |
|---|---|---|
| `related_to` 占比 | **359 / 922 = 39%** | 近四成的边语义是空的（词表外谓词降级，见 0001 P3b） |
| `part_of` 事实 | 185 | 传递规则的第一个目标 |
| `part_of` 传递闭包 | 828（深度上限 10） | 4.5 倍膨胀，且**不收敛** |
| 闭包深度分布 | 1:185 2:181 3:141 4:45 5:52 6:40 7:52 8:40 9:52 10:40 | 深度 5 起振荡而非衰减 = **有环** |

环是真的，抽取错误造成：

```
Microsoft → FarmBeats for Students → Microsoft
FarmBeats for Students → National FFA Organization → FarmBeats for Students
```

「FarmBeats for Students 属于 Microsoft」是对的，反向那条是模型从"Microsoft 的 FarmBeats 项目"这类句式里抽反了。**语料第一天就带环**——环检测不是防御性编程，是开工前提。

在这个底子上开传递闭包，第一天就产出「Microsoft 属于 Microsoft」，而且**带证据链**——比没有更难收拾。

但同一套规则求值机器反过来用就是**一致性检查**：找环、找反对称违反、找 disjoint 违反。没有真值维护负担、没有爆炸风险、输出直接进现有的 `fact_conflicts` + Review UI。**先用检查把引擎建起来并把语料洗干净，再打开物化开关。**

现存可检出的矛盾（三个 KB 合计 11 处，全是真缺陷）：`part_of` 双向环 2 处、任意谓词双向重复 9 对、自环 0（`extraction.rs:363` 已拦）。

---

## 三个架构前提

### 1. 派生事实没有证据位

`fact_evidence.chunk_id` 是 `NOT NULL`。派生事实没有分块——**它的证据是别的事实**。而 README 的承诺是 "Nothing enters the graph without evidence"，所以这不是可选项。

```sql
CREATE TABLE fact_derivations (
  fact_id         UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
  premise_fact_id UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
  rule_id         UUID NOT NULL,
  seq             SMALLINT NOT NULL,
  PRIMARY KEY (fact_id, premise_fact_id, seq)
);
```

这同时是 "with explanations" 的落点：**解释 = 前提集 + 规则**，递归展开成证明树，叶子才是真正的 chunk。`facts.derived_by_rule` 列在 `0004_graph.sql:71` 就留好了，至今零使用——它等的就是这个。

> **修订记录（2026-09-02）：形状不是这样，而且那一列没等到。** 落地时**派生事实不进 `facts`**，另建 `derived_facts`，迁移 `0013_reasoning.sql` 给了三条理由：
> 四十多处读 `facts` 的查询只有一处认识那个标记，新写一条查询默认就把派生当断言看，**失败方向反了**——分开之后忘了 UNION 的后果是看不见派生，而不是混进去；列本来就不一样；数量级差一档。
> `fact_derivations` 的实际列是 `(derived_fact_id, premise_fact_id, seq)`，规则挂在 `derived_facts.rule_id` 上。`facts.derived_by_rule` 今天仍然零写入，唯一引用是 Review 的一处过滤。
> 这条「失败方向」的判据后来被 [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) 原样引用去否掉了 `facts.nod` 列。

### 2. 派生事实绝不能闭合断言事实

`reconcile_new_fact`（`temporal.rs:49`）对 functional 谓词会自动闭合旧事实。派生事实若走同一条路，**一条错规则就能系统性闭掉人工断言的事实**——`part_of` 那个坑的十倍版，而且这次是自动化的。

硬性优先级 **asserted > derived**：

| 情形 | 处理 |
|---|---|
| 派生 vs 断言 | 派生**不落地**，记一条"规则与事实矛盾"信号 |
| 派生 vs 派生 | 规则集自相矛盾 → 进 Review，**不自动裁** |
| 断言 vs 断言 | 现有逻辑不变 |

这是 0001 判据 2「本体是引导不是执法」在推理层的同一条：**声明可能是错的，所以它不能驱动对既有数据的改写。**

> **修订记录（2026-09-02）**：表格三行里只有第一行的前半兑现了——`asserted` 硬性优先，已断言的三元组不再派生（`derive.rs`）。
> 「记一条规则与事实矛盾的信号」**没有**，「派生 vs 派生进 Review」**没有**（同一三元组的多条推导只留第一条证明）。两者都还是待做。
>
> **修订记录（2026-09-03）**：两行都兑现了，方案见 [0017](0017-a-contradiction-points-upstream.md)。派生撞断言：`derive::contradictions` 逐条算出，`run` 记成 `axiom_violations` 一种 `derived_contradiction`（单谓词封顶 50），`materialize` 用同一个函数拦下不落地；卡片给线索（旧断言没结束日期、同名实体、置信度低）与修法（闭合、撤回、认可）。派生撞派生：按规则对聚合进 `ontology_defects` 一种 `rules_disagree`，两边都不落——根子是那两条声明，不是哪条事实。

### 3. 前提撤回时——双时态是最优解，不是负担

不能 UPDATE（append-only 是地基）。做法是置 `invalidated_at`，与拒绝一条事实**完全同构**。派生事实的记录轴上于是留下"我们曾据此推出，后来前提没了"。

真值维护在别的系统里是苦活（要么标记删除要么重算全量），在这个数据模型里是既有机制的自然延伸——**而且实体历史页面（PR #37）直接就能展示它**，不需要新 UI。

> **修订记录（2026-09-02）**：「实体历史直接能展示」**没兑现**——`entity_history` 的 UNION 只有 facts / merges / retypes 三段，不含 `derived_facts`。
> 派生的可见性落在别处：实体面板单独一档「推出来的」、图上金色专属边（派生为零时开关整个不出现、逆关系推出来的边并到同一条弧上）。
> 而且今天没有「前提撤回」这个增量动作，R3 未做，每次全量重推并整体对账。

---

## 分步

### R0 · 一致性检查（不产生任何事实）

- 规则求值引擎：半朴素求值 + 环检测 + 深度上限
- 三类检查：**反对称违反**（`part_of` 双向）、**函数性违反**（漏网的）、**disjointWith 违反**（等 0001 P2 导入）
- 输出进 `fact_conflicts`，复用现有 Review UI，零新界面
- **立刻有货**：现存 11 处矛盾直接冒出来

价值在于：引擎的难点（规则表示、求值、终止）全部建成并验证，而风险面为零——它不写 `facts` 表。

> **修订记录（2026-09-02）：R0 建成，四处与上面写的不同。**
> 一、**四类不是三类，且不是那三类**：`self_loop` / `asymmetry` / `cycle` / `functional`（兼含 inverse_functional）。**事实层的 disjointWith 违反没有做**——disjoint 只进了本体自检。
> 〔后来多了第五类 `signature`（#190 / #196）：主语不在谓词的 domain、或宾语不在 range。它不由纯逻辑引擎算——要看实体类型与闭包，那是库里的东西——`store::reasoning::signature_breaks` 用 SQL 量，算出来后与其它四类走同一条落库、清陈、裁决的路。合并之后对搬动过的事实立刻查，手动跑检查时全量查。未分类实体不算。〕
> 二、**不进 `fact_conflicts`，另建 `axiom_violations`**：那张表问的是「哪条对」，公理违规问的是「错在数据还是错在定义」，出路是 `fact_retracted` / `axiom_relaxed` / `accepted`。「零新界面」也没守住，Review 页多了 `violations` 与 `defects` 两个页签。
> 三、**多出另一半：本体自身的自洽性**（`ontology_defects`，八类：symmetric 且 asymmetric、transitive 且 functional、子类成环、与祖先 disjoint、继承来的 disjoint、自逆、逆没指回来、子属性成环）。理由是自相矛盾的本体会让事实层的结论全部可疑，所以缺陷优先于违规展示。
> 四、环检测是深度优先不是半朴素——闭包只告诉你 A 推出了 A，人要的是**路径**；半朴素在 R1。
> 另：没装本体包的库跑出来是零，那是实情不是故障——没有公理就没有判据。本文「现存 11 处矛盾」的前提（种子本体带 `functional` 声明）已不存在。导入本体后会自动跑一次检查：公理刚变，正是最该重算的时刻。

### R1 · 物化推导（打开开关）

- `rules` 表 + `fact_derivations` 表
- 规则**只从本体公理编译**：`TransitiveProperty` / `SymmetricProperty` / `inverseOf` / `subPropertyOf`。不做用户自定义 DSL——那是另一个产品
- 优先级 asserted > derived，硬性
- 深度上限 + 环检测（实测必需）
- 派生事实在图上可视觉区分，且可整体过滤

> **修订记录（2026-09-02）：R1 建成。** `rules` / `derived_facts` / `fact_derivations` 三张表；开关 `materialize_inferences` 默认 FALSE，`inference_interval_minutes` 默认 60，调度器每分钟扫到期库入队，作业里**先记时间再推**防失败循环；开关关着时端点回明确错误而非静默跳过。
> 规则从四种公理编译：Transitive / Symmetric（#132）、inverseOf / subPropertyOf（#177 / #179，投影侧在迁移 `0016`）。逆的归一化放在载入公理那一处，只补空缺不覆盖人写的，两边指得不一样留给 R0 报 `inverse_not_mutual`。
> **单谓词封顶 20,000 条，被截掉必须说出来**（`capped`）；另有一个正常恒为零的 `unruled` 计数器，记录的是一个真踩过的 bug（落库曾按谓词而非 `via` 查规则，跨谓词规则一加就静默丢弃）。
> `rules` 的身份是 `(kb, 谓词, 种类)`，公理撤了也不删行——否则每跑一次 `rule_id` 就换新，历史全断。
> 冷启动自动扩本体现在**一位公理都不替人声明**（`Axioms::default()`）：推理机的判据必须是人写下来的。
### R2 · 解释

证明树 API + UI。展开到叶子（chunk）为止，中间节点是派生事实与规则。〔**已做**（0016 B1，`feat/proof-tree`）：`reasoning::proof` + `GET /kbs/{id}/derived/{id}/proof`，实体面板里派生行展开即证明链。**形状是链不是树**——`fact_derivations` 只记断言前提（推导的输入排除派生），所以「递归展开」退化成按 `seq` 的一条链：派生 → 断言 → 各自的证据 → chunk，界面一路点到文档。撤了的前提照样列出并打标记，派生失效后证明仍可回看——记录轴的用法。先前写的「中间节点是派生事实」在这个数据模型里不会出现。〕

### R3 · 增量维护

前提失效 → 派生失效。半朴素增量而非全量重算。放最后是因为前面三步都能靠"重算整个 KB"活着，而增量的正确性最难验。〔**未做**，每次全量重算并整体对账，迁移注释明写「在增量维护做出来之前每次都是全库重算」。〕

---

## 开放问题

- **有效时间的交集语义**：前提 A `[2020,2023)`、前提 B `[2022,∞)` → 派生 `[2022,2023)`。属性事实的字面值宾语怎么参与？闭区间与开区间混合时的边界？〔**已答**：半开区间取交集、空交集不推、端点相接不算重叠；精度取最粗、置信度取前提最小；**字面值宾语不参与**——取边与推导都要求 `object_id IS NOT NULL`。〕
- **物化 vs 查询时求值的临界点**：实测 4.5 倍膨胀（185 → 828）。大语料上这个倍数会怎么变，什么规模该切换？〔未定。实际做法是每谓词封顶 + 定时全量重推。〕
- **`related_to` 39% 是推理的前置障碍**：推理机面对的图有近四成的边语义是空的。这把 0001 P3b 的优先级顶了上来——**推理机的输入质量取决于它**。补充实测：359 条里只有约 38 条是词表外降级，**321 条是模型自己从提示词清单里挑的 `related_to`**——它就在本体里，被当合法选项列了出去。所以修法是两条腿：**先撤掉提示词里这个逃生舱**，再做映射。细节见 0001 P3b 的修订记录。
- **环该怎么处置**：检测出来只是第一步。自动拒绝哪一条？还是全部进 Review 让人判？倾向后者——两条都是模型抽的，没有先验理由信任其一。〔**已按倾向落地**：`cycle` 违规带完整 `path` 进 Review；R1 一律不推自环。`related_to` 那条前置障碍也随 [0010](0010-no-relation-is-no-relation.md) 消失——空谓词边天然不进推理。〕
