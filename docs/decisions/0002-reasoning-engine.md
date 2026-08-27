# 0002 · 推理机

- **状态**：规划中 · `utopia-reason` 目前是 3 行占位
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

### 2. 派生事实绝不能闭合断言事实

`reconcile_new_fact`（`temporal.rs:49`）对 functional 谓词会自动闭合旧事实。派生事实若走同一条路，**一条错规则就能系统性闭掉人工断言的事实**——`part_of` 那个坑的十倍版，而且这次是自动化的。

硬性优先级 **asserted > derived**：

| 情形 | 处理 |
|---|---|
| 派生 vs 断言 | 派生**不落地**，记一条"规则与事实矛盾"信号 |
| 派生 vs 派生 | 规则集自相矛盾 → 进 Review，**不自动裁** |
| 断言 vs 断言 | 现有逻辑不变 |

这是 0001 判据 2「本体是引导不是执法」在推理层的同一条：**声明可能是错的，所以它不能驱动对既有数据的改写。**

### 3. 前提撤回时——双时态是最优解，不是负担

不能 UPDATE（append-only 是地基）。做法是置 `invalidated_at`，与拒绝一条事实**完全同构**。派生事实的记录轴上于是留下"我们曾据此推出，后来前提没了"。

真值维护在别的系统里是苦活（要么标记删除要么重算全量），在这个数据模型里是既有机制的自然延伸——**而且实体历史页面（PR #37）直接就能展示它**，不需要新 UI。

---

## 分步

### R0 · 一致性检查（不产生任何事实）

- 规则求值引擎：半朴素求值 + 环检测 + 深度上限
- 三类检查：**反对称违反**（`part_of` 双向）、**函数性违反**（漏网的）、**disjointWith 违反**（等 0001 P2 导入）
- 输出进 `fact_conflicts`，复用现有 Review UI，零新界面
- **立刻有货**：现存 11 处矛盾直接冒出来

价值在于：引擎的难点（规则表示、求值、终止）全部建成并验证，而风险面为零——它不写 `facts` 表。

### R1 · 物化推导（打开开关）

- `rules` 表 + `fact_derivations` 表
- 规则**只从本体公理编译**：`TransitiveProperty` / `SymmetricProperty` / `inverseOf` / `subPropertyOf`。不做用户自定义 DSL——那是另一个产品
- 优先级 asserted > derived，硬性
- 深度上限 + 环检测（实测必需）
- 派生事实在图上可视觉区分，且可整体过滤

### R2 · 解释

证明树 API + UI。展开到叶子（chunk）为止，中间节点是派生事实与规则。

### R3 · 增量维护

前提失效 → 派生失效。半朴素增量而非全量重算。放最后是因为前面三步都能靠"重算整个 KB"活着，而增量的正确性最难验。

---

## 开放问题

- **有效时间的交集语义**：前提 A `[2020,2023)`、前提 B `[2022,∞)` → 派生 `[2022,2023)`。属性事实的字面值宾语怎么参与？闭区间与开区间混合时的边界？
- **物化 vs 查询时求值的临界点**：实测 4.5 倍膨胀（185 → 828）。大语料上这个倍数会怎么变，什么规模该切换？
- **`related_to` 39% 是推理的前置障碍**：推理机面对的图有近四成的边语义是空的。这把 0001 P3b（谓词消解）的优先级顶了上来——**推理机的输入质量取决于它**。
- **环该怎么处置**：检测出来只是第一步。自动拒绝哪一条？还是全部进 Review 让人判？倾向后者——两条都是模型抽的，没有先验理由信任其一。
