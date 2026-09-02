# 0009 · 「还没判出来」不该是一个类

- **状态**：已实施 · `entities.type_id` / `entity_retypes.from_type_id` 可空；内置九类与播种函数已连同种子关系一起退场（#110 / #125 / #128 / [0011](0011-a-mapping-is-not-a-fact.md)）；两个开放问题仍开放，第二个已有可见代价（2026-09-02 核）
- **成文**：2026-08-30（约定见 [README](README.md)）
- **相关**：[0008](0008-ontology-packs-as-cold-start.md) 让真词表成为可选起点，本文把内置的那套拆掉；
  [0001](0001-ontology-import-and-governance.md) 的 IRI/key 分工是本文撞名论证的前提

> 本篇没有 benchmark 数字。它是一次归类的更正——`concept` 一直被当成本体的一部分，
> 而它其实是控制流的一部分。更正的依据是代码事实与一个撞名推演，都能重跑。

## 起点：内置九个类，一个都不该留

建库播种九个实体类（`graph.rs` 的 `DEFAULT_ENTITY_TYPES`）：

```
person  organization  project  metric  dimension  product  event  concept  location
```

0008 论证过它们的通病——**零个带类型签名**，方向只能靠散文写。装了 schema.org 之后
它们更是多余：`Person` / `Organization` / `Product` / `Event` / `Project` 会按 key 撞名
被认领，等于内置那份从头到尾只是个占位。

**`location` 是反例中的反例**：schema.org 里它叫 `Place`，key 对不上，于是不被认领，
`City` / `AdministrativeArea` 挂到另建的 `place` 底下，跟 `location` 成了两棵树。
一个我们自己起的名字，把整条地理子树割断了。

`metric` / `dimension` 是语义层概念，schema.org 里确实没有——但代码里除测试外零引用，
它们只是**播种的默认值**，不是机制。

## `concept` 是控制流，不是词表

八个是词表内容，可以换、可以不同意、可以被 schema.org 顶替。`concept` 不是：

```rust
// extraction.rs
let concept_type = *type_ids.get("concept")
    .ok_or_else(|| anyhow!("Ontology missing the 'concept' type"))?;
```

它编码的是**「抽取器抽到了东西，但本体里没有对应的类」**。这个状态在任何本体下都存在，
跟装了 schema.org 还是 FIBO 无关。没有它，抽出来的实体只能被丢掉——而 0001 的 P1
整节就是在修「静默丢弃」。

它承担三件事：白名单外类型的落点（同时 `record_miss` 作为本体扩展信号）、
类型消解的取材范围（`entities_for_type_resolution(kb, DUMPING_GROUND, …)`）、
实体合并时的类型调和（兜底侧让位给具体侧）。

## 决定：不是给它改名，是让它不存在

初稿拟保留哨兵行、改 key 为 `_unclassified`。**否掉**，但这个方案值得记下来，
因为否掉它的理由才是本文的结论。

改名方案能成立，是因为 `key_from_iri` 永远产不出前导下划线——第一个字符若非字母数字，
`out.is_empty()` 为真，那个 `_` 不会被推入。实测：

```
skos:Concept        -> "concept"      ← 会撞
http://x#_Concept   -> "concept"      ← 前导下划线被剥掉
http://x#__unclass  -> "unclassified"
```

所以 `_unclassified` 是导入够不到的命名空间。**这是算法保证，不是约定。**

而且不改名会出真事故：哨兵行**没有 IRI**，而导入逻辑是「占位者没有 IRI 就认领它」——
`skos:Concept` 会**接管哨兵**。于是所有"还没判出来"的实体一夜之间变成正经的
`skos:Concept`。不是撞名被跳过，是语义被静默改写。SKOS 不是冷门词表。

**但改名解决的只是撞名，解决不了泄漏。** 哨兵行必须从每一个消费者那里被过滤掉：
本体页、抽取提示词、候选列表、图谱图例、导出、统计。漏掉任何一处，它就作为一个正经的类
出现在那里，而且是静默出现。

这正是本仓库反复警告的那类缺陷。`ontology_index.rs` 开篇：

> **自愈，不挂钩子。**……漏挂一个钩子会悄悄烂掉，而对比原文不会。

`_unclassified` 是**用命名约定替代类型保证**：它管得住导入撞名，管不住某个 SELECT
忘了写 `WHERE key NOT LIKE '\_%'`。

**所以 `entities.type_id` 改为可空。** 「没有类型」就是没有类型，不用一行假类冒充。

初稿在这里写的是「NULL 忘不掉——SQL 与类型系统会当场让人知道」。**实现下来只对了一半，
记在这儿因为错的那一半更要紧。**

Rust 这边全中：每把一个字段改成 `Option`，编译器就把它的每一个消费点列出来，
一处不漏（`graph.rs` 的取节点语句、审核项两侧、裁决提示词、聊天里的实体清单）。
漏掉一个哨兵不会有任何提示，漏掉一个 `Option` 连编译都过不去。

**SQL 这边不会。** `NULL <> uuid` 求值为 NULL 而不是 true，于是这样的条件

```sql
AND type_id <> $2      -- 认领 / 改类都靠它挑出"要变的那些行"
```

**一行都选不中**，而且不报错。踩到两处，都在最主要的那条路上：`adopt_proposed_types`
（本体长出新类之后认领等着它的实体）与 `retype_entities`（类型消解裁决落库）。
两处的输入几乎全是 `type_id IS NULL` 的实体——功能会静默空转，测试也照样绿。
正解是 `IS DISTINCT FROM`。同一次实测（一个 `proposed_type='vehicle'` 的未分类实体）：
`<>` 选中 0 行，`IS DISTINCT FROM` 选中 1 行。

所以这条决定的真实代价不是「90 处引用要改」，而是**26 处 SQL 里的每一个比较都要重看一遍**：
类型系统数得清 Rust 那 64 处，数不到 SQL 里去。

## 代价，逐条

**90 处引用，26 处在 SQL 字符串里。** 逐个过完了，实际改动 14 个文件、
`+316 / -369`——**删的比加的多**，因为内置那两张表（九个类与它们的中文措辞）
连同播种循环一起没了。

**两个唯一索引带 `type_id` 前缀：**

```sql
CREATE UNIQUE INDEX … ON entities (kb_id, type_id, lower(canonical_name))
  WHERE merged_into IS NULL;
```

Postgres 里 **NULL 不等于 NULL**，所以未分类的同名实体不会被它拦住——两个都没类型的
「张三」可以并存。

**这是要的行为，不是缺陷**：该索引本来的意图就是「同类同名允许重复」（0001 P0 论证过
两个张伟必须能分开存放，宁分勿合）。未分类时我们对它们是不是同一个东西**知道得更少**，
更没有理由合并。写在这里是因为半年后有人看到「未分类的同名实体可以重复」会以为是 bug。

**`CONFUSABLE_TYPE_KEYS` 不受影响。** 它按 key 查（`["organization","project","product"]`），
而这些 key 装了 schema.org 之后仍然存在——只是来源从内置播种变成了词表导入。
没装包的库里它们不存在，于是那一档永不命中，所有跨类型同名判 `Disjoint`（完全分开）。
**那是变严不是变松**，不会造成错误合并。

它的正解仍然是注释里写的那条——等 `disjointWith` 落库后改从本体读，即 0008 的 R0 第三阶段。
本文不动它。

〔**2026-09-02**：前提已经成立，动作仍未做。`owl:disjointWith` 有表（`entity_type_disjoint`）、有导入、有编辑接口，消费者只有 R0 的本体自检；`CONFUSABLE_TYPE_KEYS` 仍是硬编码三个 key，且它上面的注释还停留在旧世界。〕

## 空白库

删光之后，不选任何包的库是**真的空**：抽出来的实体 `type_id` 为 NULL，事实照常落库，
证据链照常。之后装一个包再跑一次类型消解，实体会被重新分配——
`entities_for_type_resolution` 本来就是取「落在兜底上的」，改成取 `type_id IS NULL` 即可。

**所以"先建库后建模"是受支持的路径，不是将就。**

〔落地时多了一列本文没写的 `entities.type_source`（extracted / human / inferred）：0009 让 `type_id IS NULL` 同时承载「还没判」和「人判了就是没有」，这一列把两者分开，也是类型消解取材的过滤条件（0001 P4a）。另有 `specific_type` 与 `proposed_type` 分列——前者是模型对这个实体的自由说法，专供类型消解。〕

配套：schema.org 在建库对话框里默认勾选，可反选。这在 45 秒的时候不成立，
在 0.42 秒的时候成立（导入改批量插入之后的实测）。

## 落地时定下的两件事

**图谱怎么画它。** 取节点的语句从 `JOIN entity_types` 改成 `LEFT JOIN`——内连接会让
未分类实体**整个从图上消失**，事实还在库里却查无此人，是最难发现的一种数据丢失。
`key` 与 `label` 留 NULL，**颜色和形状给缺省值**（灰色圆点）：前者是身份，没有就该说没有；
后者是画布必须拿到的东西，编不出来就没法渲染。同一处理也用在审核项两侧。

**给它定类不算「跨轴」。** 类型消解把「选中的类不在现类子树里」判为重新分类而非精化，
一律进人工。未分类实体身上**没有一个抽取判断要被推翻**，第一次定类是补齐；
若也判跨轴，删掉哨兵的代价就是每个实体都要人看一眼，整条自动化直接废掉。
配套地 `entity_retypes.from_type_id` 改为可空——最常见的一次改类正是「从没有类到有类」，
而那张表是撤销的唯一依据；非空的话第一次定类就写不进账，那批改动不可撤。

## 开放问题

- **未分类实体的消解**：画像里没有类型这一维，`classify_type_drift` 少一个判据。
  两个都没类型的同名实体，今天按 `Recall` 走画像相似度——够不够，没测过。
- **`metric` / `dimension` 的去处**：语义层要用，而没有任何公开词表提供它们。
  是让用户自己建，还是我们出一个「Utopia 语义层」包？后者会把内置本体从代码搬到包里，
  性质完全不同——那是可选、带 IRI、可被替换的。〔**仍未答，且有了可见代价**：映射探查按 `entity_types.key IN ('metric','dimension')` 找类型，找不到就 `continue`——不装包、也没人手建这两个类的库里，探查会**静默产出零条**。〕
