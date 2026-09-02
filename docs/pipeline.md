# 一份文档如何变成图谱

**这一篇不讲为什么，讲东西怎么流的。** 「为什么这样而不是那样」在 [decisions/](decisions/README.md)；
这里回答另一个问题：**我改的这一行，处在整条链的哪个位置，它上游给我什么、我不给下游什么会断在哪。**

> 图上最值钱的不是箭头，是**箭头断掉的地方**。每一段末尾都有一节「这里会丢东西吗」，
> 列的是代码里真实存在的丢弃点与它们在库里的落点——不是"理论上可能失败"，
> 是**已经在 `extraction_drops` 里数得出来的那几种**。

## 全景

```mermaid
flowchart TB
    U[上传 / 来源同步] --> P[解析<br/>parsers.rs]
    P --> C[分块<br/>1200 字符 · 重叠 150]
    C --> E1[嵌入<br/>chunks.embedding]
    E1 --> RDY[(文档 ready<br/>可搜可问)]
    E1 --> X[抽取<br/>每块一次 LLM]
    X --> ENT[实体消解<br/>这一条是谁]
    X --> FCT[事实落库<br/>双时态账本]
    ENT --> ADJ[裁决<br/>攒批一次 LLM]
    ADJ --> MRG[合并 / 保持分开]
    FCT --> TR[类型消解<br/>这一条是什么]
    FCT --> GROW[本体增长<br/>词表外的说法回流成提案]
    MRG --> G[(图谱)]
    TR --> G
    GROW --> ONT[(本体)]
    ONT -.喂回.-> X
    G --> R0[一致性检查<br/>公理 vs 事实 · 不写库]
    ONT --> R0
    G --> R1[物化推导<br/>开关 · 派生另存]
    ONT --> R1
    R1 --> G

    style RDY fill:#2d4a5a,color:#fff
    style G fill:#2d4a5a,color:#fff
    style ONT fill:#2d4a5a,color:#fff
```

**两段式是有意的**：嵌入完成即 `ready`，搜索与问答立刻可用，抽取在后台排队。
一篇长文档的图谱要几分钟才长出来，但它在几十秒内就能被搜到。

**本体那条回流虚线是这套东西的循环**：抽取用本体，抽取遇到本体没有的说法就把原词记下来，
提案回流补进本体，下一批文档的抽取就用上了。见 [0003](decisions/0003-ontology-growth-loop.md)。

**本体从哪来**：建库时什么都不种。起点是可选的预制包（schema.org 默认勾选，另有 W3C Org、PROV-O、FOAF、IOF Core），
或者用户导入自己的 OWL，或者空着——空库照样能抽，实体没有类型就是没有类型。见 [0008](decisions/0008-ontology-packs-as-cold-start.md)、[0009](decisions/0009-no-type-is-a-type.md)。

**最下面那两个框是本体的公理在干活**：一致性检查不写 `facts`，只把矛盾摆出来（Review 的 violations / defects 两档）；
物化推导默认关，打开后派生事实另存一张表、图上金色，永远不闭合任何断言事实。见第四节。

---

## 一、抽取一个分块

```mermaid
flowchart TB
    subgraph 提示词
        B{本体装得下预算吗?}
        B -->|装得下| FULL[全量铺<br/>小本体的老路]
        B -->|装不下| RET[按这一块的向量检索<br/>约 40 类 / 30 关系 / 30 属性<br/>+ 命中类的祖先一起铺]
    end
    FULL --> LLM[LLM]
    RET --> LLM
    CHK[分块正文 + 本文档已认下的实体] --> LLM
    LLM --> J{输出的每一条}
    J -->|entities| EN[实体<br/>type 从清单挑<br/>specific_type 自由文本]
    J -->|predicate 命中属性| AT[属性事实<br/>值按 datatype 归一]
    J -->|predicate 命中关系| RL[关系事实]
    RL --> DIR{主宾类型<br/>对得上签名?}
    DIR -->|对| OK[落库]
    DIR -->|主语违反且宾语符合| SWAP[按签名对调主宾<br/>留 direction_corrected]
    DIR -->|对调也不合法| NOP[谓词留空<br/>主宾时间证据都留]
    J -->|词表外 + 字面值| LIT[值落 object_value<br/>原词落 proposed_predicate]
    J -->|词表外 + 实体宾语| FB[谓词留空<br/>原词落 proposed_predicate]

    style LLM fill:#3a3a5a,color:#fff
```

**`specific_type` 是这一步最容易被忽略的输出**：自由文本、不校验、不入本体，就是模型自己
对这个实体的说法（"vector database software"）。类型消解靠它把任务从「读懂这是什么」
换回「本体里哪个类叫这个名字」。没有它，实测 17 个实体的 `proposed_type` 全是空的——
因为清单里总有个"差不多"的，模型选了它，心里那个更准的说法就此丢失。

**词表外的两条路都不丢东西**：带字面值的落 `object_value`（而不是凭空造一个叫「2015」的实体），
带实体宾语的**谓词留空**——不是降级成一个叫「有关联」的关系，那是断言不是含糊（[0010](decisions/0010-no-relation-is-no-relation.md)）。
两者的原词都进 `fact_evidence.proposed_predicate`，那是这条事实身上唯一还留着原意的地方，显示时由 `fact_surface_predicate()` 取回。

**命中的关系还要过一道签名**：本体声明了 `employee (organization → person)`，模型照样会写 `Musk employee Microsoft`——
提示词三轮都压不下去，英语的 "X is an employee of Y" 太强。所以写入时掰正：主语违反 domain 而宾语符合就对调，**绝不静默**，留一条 `direction_corrected`；
对调也不合法（`OpenAI affectedBy …`，schema.org 里那是医学检验用的）就丢掉谓词、留下主宾与证据。参数顺序是 key 的编码约定，不是关于世界的断言，所以这一处本体是执法的。
见 [0012](decisions/0012-the-ontology-is-a-contract-not-a-suggestion.md)。

### 这里会丢东西吗

会。十二种原因码，全部记进 `extraction_drops`，界面上可见（其中一种不是丢弃，是留痕）：

| 原因 | 什么时候 |
|---|---|
| `truncated_reply` | 模型的输出被截断，这一块整块作废 |
| `malformed_item` | 一条事实格式不对——**只丢这一条**，不丢整块（#127） |
| `not_an_entity_name` | 「实体名」是一整句话（按词数 + 限定动词判，#143） |
| `low_confidence` | 模型自报置信度低于阈值 |
| `subject_not_declared` | 主语没在 `entities` 里声明——关系与属性两条路径**都**记 |
| `attr_domain_mismatch` | 属性挂到了 domain 之外的类上（沿父类 DAG 上溯仍不匹配） |
| `attr_no_value` / `attr_datatype` | 属性事实没给值，或值换算不出声明的 datatype |
| `object_missing` | 关系事实没有宾语 |
| `direction_corrected` | **不是丢弃**：主宾按签名对调了，记下来是为了不静默 |
| `domain_mismatch` | 对调也不合法，谓词被丢掉（主宾与证据留下） |

**`attr_domain_mismatch` 是最贵的一种**：它在落地当场丢弃，而事后改类救不回来——
那条事实从没写入过，只能重抽。

从前还有一种 `fallback_relation_missing`（兜底关系被删了就整条消失）——随 `related_to` 一起没了。

---

## 二、实体消解：这一条是谁

```mermaid
flowchart TB
    M[一次 mention<br/>类型 + 名字 + 分块向量] --> EQ[等值召回<br/>canonical_name 或 aliases 相等]
    EQ --> S{画像相似度}
    S -->|0.55 及以上| ATT[并进已有实体<br/>更新画像]
    S -->|0.35 到 0.55| NEW1[新建 + 入审阅队列]
    S -->|低于 0.35| NEW2[新建，不打扰队列]
    EQ -->|一个都没有| NEW3[新建]
    NEW1 --> CT
    NEW2 --> CT
    NEW3 --> CT[包含关系召回<br/>只在新建时跑一次]
    CT --> Q[(审阅队列<br/>pending)]
    Q --> AD[裁决<br/>攒批一次 LLM]
    AD -->|同一个| ME[合并<br/>名字进 aliases<br/>事实搬过去]
    AD -->|不是| KP[保持分开]
    ME --> RD[把该 source 上其余 pending<br/>改指到合并目标]
    RD --> Q

    style AD fill:#3a3a5a,color:#fff
    style Q fill:#2d4a5a,color:#fff
```

**三个阈值分三档**（`SIM_ATTACH = 0.55`、`SIM_NEW = 0.35`）：像得没话说就并，
像得可疑就新建但入队，不像就新建且不打扰队列。**宁分勿合**——错合的代价是两个实体的
事实混在一起，比多一个实体贵得多。

**包含关系召回**（`Holmes` ⊂ `Sherlock Holmes`）补等值召回的盲区：前缀枚举不完，
简称会静默变成第二个实体。它有三条约束：较短那个名字至少 4 字符（低于此多是通名）、
单次最多产出 4 对、SQL 侧多扫 16 行（硬互斥类型在 Rust 侧才筛得掉）。

**改指那一步**是整张图里最不直觉的一环，也是最容易被误删的：合并之后，涉及被合实体的
其余待审阅对**不能关掉**，要改指到合并目标。理由见下。

### 这一段修过三个洞，三个都是同一份语料照出来的

用《福尔摩斯冒险史》前六篇（`scripts/bench/corpora/holmes.json`）连跑四次，每次修一层：

| | 原始 | 修同类型 | 加别名召回 | 加改指 |
|---|---|---|---|---|
| 已合并实体 | 14 | 37 | 47 | **57** |
| `Holmes` 并入 | ✗ | ✓ | ✓ | ✓ |
| `Mr. Holmes` 并入 | ✗ | ✗ | ✗ | **✓** |

**第一层**：`classify_type_drift` 没有「两个类型相同」这一档，`person × person` 落进
`Disjoint`——"永不可能是同一个"。那个函数生来服务「类型漂移」（同名被抽成两种类型），
那里两边相同不会发生；后来被包含关系召回借去当相容性判据，**而那里两边相同才是常态**。
全文最明显的同指关系一对都没进过队列，十二个既有单元测试全在测跨类型。

**第二层**：召回只看 `canonical_name`。合并把名字搬进 `aliases`，于是**每成功合并一次
就拆掉一条桥**——`Holmes` 并入之后，后来的 `Mr. Holmes` 跟 `Sherlock Holmes` 谁也不含谁，
本来正是靠 `Holmes` 桥接。修好第一层反而让第二层的漏显形了。

**第三层**：合并会把涉及被合实体的其余 pending 审阅关成 `superseded by merge`，
代码注释里的理由是"疑点若仍在会由后续 mention 重新提起"。**那句是错的**：包含关系召回
只在新建实体时跑，而这些实体早就存在、不会再被新建。关掉即永久关闭。现在改指到合并目标，
只有两类真正过时的才关——重定向后成自环的，和目标对已在队列里的。

**这三层是一层套一层的**：不修第一层看不见第二层，不修第二层看不见第三层。
基准语料的价值不在第一次跑出的数字，在**每修一次就再照出下一层**。

### 这里会丢东西吗

不会丢事实，但会**留下不该分开的实体**。两个已知缺口：

- 两个名字既不互相包含、又没有共同别名做桥（`启明 X7 加速卡` vs `启明 X7 推理加速卡`）。
  要三元组相似度，而 `CREATE EXTENSION pg_trgm` 需要超级权限，本仓库是受限角色连库。
- 单次包含关系召回上限 4 对：一个通名可能被几十个实体包含，全放进去会淹掉队列。

合并本身**可撤销**（`entity_merges` 记着改之前的一切，`revert_merge` 放回去）。

---

## 三、本体消解：这一条是什么

```mermaid
flowchart TB
    subgraph 抽取留下的线索
        PT[proposed_type<br/>词表外的类型名]
        ST[specific_type<br/>模型自己的说法]
        PP[proposed_predicate<br/>词表外的谓词原词]
    end
    PT --> TR
    ST --> TR
    PP --> GP[本体提案<br/>检索候选 + 裁决]
    TR[类型消解] --> C1[候选一<br/>画像 → 类的描述]
    TR --> C2[候选二<br/>语境近邻的类当票投]
    C1 --> AD2{裁决}
    C2 --> AD2
    AD2 -->|在原类子树里| AUTO[自动改类<br/>entity_retypes]
    AD2 -->|跨了分类轴| REV[待人工<br/>类对认可一次即免问]
    AD2 -->|都不是| NONE[不动<br/>并记下理由]
    GP -->|已有的| MAP[映射到已有类型<br/>改写等待的事实]
    GP -->|没有的| NEWT[新建类型 + 改写]

    style AD2 fill:#3a3a5a,color:#fff
```

**两路候选取并集，不合分数。** 距离在三处都不可比：跨实体不可比
（`清华大学计算机系→computer_store` 0.46 比 `星云科技→corporation` 0.59 还近，而前者荒谬）、
两路之间不可比（一个在类空间一个在实体空间）、同一路的两个查询之间也不可比
（短查询"医药集团"产生的距离系统性小于一整段画像）。**一律交替取。**

**分档不看模型自报的 confidence**——实测是双峰的（15 条全 ≥0.85、4 条 null，中间没有），
自报置信度是语气不是概率。改用「选中的类在不在原类的子树里」：在 = 往下走一格，自动；
不在 = 换了分类轴，进人工。

**纠正也走人工，而且天然如此**：抽取按块检索候选之后自己就会挑细类，也会挑错
（`绍兴 → address`）；正确答案是错类的**兄弟**不是后代，所以必然判为跨轴。
推翻抽取的判断比细化它风险大，不该自动发生。

### 这里会丢东西吗

不丢事实，但**改类不进时间轴**——它是 `entities` 上一次 UPDATE 加一行 `entity_retypes`〔实体历史现在会显示 `retyped` / `retype_reverted` 两种事件，这一条已经补上〕。**可撤销不等于会被撤销**，
这是先做 preview 再做 apply 的理由。

**类型消解今天只能手动跑**——本体页 preview → apply，没有任何自动触发。抽取结束只入队本体扩展与实体裁决。
所以大本体下新实体的细化依赖人记得去点一下，这是个真缺口（0001 P3a）。

**人拍过板的不再被引擎重判**：`entities.type_source` 是 `human` 的实体不进取材，包括「人判了，就是没有类型」（0001 P4a）。

**拒绝要给理由。** `left_alone` 曾经只是个数，而这一步的设计押在"选择都不是是个体面答案"上——
最大的一档不透明。记上理由之后第一次跑就回答了此前答不出的问题：失败**全在检索一侧**
（`administrative_area`、`periodical` 从没被端上来过），不在裁决。

---

## 四、公理：检查与推导

```mermaid
flowchart TB
    ONT[(本体的公理<br/>functional · symmetric · asymmetric<br/>transitive · inverseOf · subPropertyOf · disjoint)] --> SELF[本体自检<br/>八类缺陷]
    SELF --> DEF[(ontology_defects)]
    ONT --> R0[事实层检查<br/>自环 · 反对称 · 传递环 · 基数]
    G[(图谱)] --> R0
    R0 --> VIO[(axiom_violations<br/>带完整路径)]
    VIO --> DEC{人裁}
    DEC -->|撤回事实| RET[事实作废]
    DEC -->|放宽公理| RLX[改本体]
    DEC -->|接受| ACC[两条都留]
    ONT --> R1{materialize_inferences<br/>开关，默认关}
    G --> R1
    R1 -->|开| DER[(derived_facts<br/>另一张表 · rule_id · 前提链)]
    DER --> GV[图上金色边<br/>实体面板「推出来的」]

    style DEC fill:#3a3a5a,color:#fff
```

**检查不写库，推导写另一张表。** 一致性检查（R0）只指出问题，风险面为零；物化推导（R1）会往图里加东西，所以它的每条约束都是必要的：
规则**只从本体公理编译**，没有用户 DSL；**断言硬性优先于派生**——已经断言过的三元组不再派生，「这条是谁说的」有唯一答案；
深度上限加环检测，每谓词封顶两万条且被截掉的**要说出来**；有效时间取前提的交集，空交集不推。
派生不进 `facts`：四十多处读 `facts` 的查询只有一处认得标记，分开之后忘了 UNION 的后果是**看不见**派生，而不是**混进去**。

**本体自检排在前面**：自相矛盾的本体（一个关系既 symmetric 又 asymmetric、子类成环、逆没指回来）会让事实层的结论全部可疑。

**没装本体包的库跑出来是零**，那是实情不是故障——没有公理就没有判据，不报矛盾比猜一个公理出来安全。

**什么时候跑**：导入本体后自动跑一次检查（公理刚变，最该重算的时刻）；Review 页可手动跑；推导按 `inference_interval_minutes`（缺省 60）定时全量重推——增量维护还没做。

### 这里会丢东西吗

不丢，但**有两处沉默**：派生 vs 断言矛盾时派生不落地，今天**不记信号**；同一三元组有多条推导路径只留第一条证明。两者都是 [0002](decisions/0002-reasoning-engine.md) 里写了而没做的。

## 想自己跑一遍

`scripts/bench/` 是可重跑的测量台，**每一组一个新库**——复用一个库省几分钟，
换来的是一整段无效结论（那是踩过的坑，不是假设）。

```bash
node scripts/bench/run.mjs --corpus pharma --label seeds-only
node scripts/bench/run.mjs --corpus holmes --label holmes
```

三份语料各测一件事，用途不同、要求也不同：

| 语料 | 测什么 | 有答案键 |
|---|---|---|
| `tech` / `pharma` | 类型准确性 | 有（弱：自己写的） |
| `holmes` | 实体消解 · demo 空镜 | **无，故意的** |

福尔摩斯那份**不该有**准确性答案键：模型早就读过它，量类型准确率量到的是记忆，
不是这条流水线。**编一份假答案比不打分更糟。**

## 相关决策

- [0001](decisions/0001-ontology-import-and-governance.md) 本体导入与治理，含 P3 的实测修订
- [0003](decisions/0003-ontology-growth-loop.md) 本体从语料里长出来，人站在哪一环
- [0006](decisions/0006-ontology-scale-and-the-prompt.md) 本体规模与抽取提示词，含曲线与一次撤回
- [0002](decisions/0002-reasoning-engine.md) 推理机的顺序与安全边界；[0012](decisions/0012-the-ontology-is-a-contract-not-a-suggestion.md) 写入时的方向掰正
- [0009](decisions/0009-no-type-is-a-type.md) / [0010](decisions/0010-no-relation-is-no-relation.md) 为什么「还没判出来」不是类、「说不出」不是关系
