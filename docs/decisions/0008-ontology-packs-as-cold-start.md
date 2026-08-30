# 0008 · 预制本体包作为冷启动

- **状态**：规划中 · 建库仍只发 10 个种子关系
- **成文**：2026-08-30（约定见 [README](README.md)）
- **相关**：[0001](0001-ontology-import-and-governance.md) 定了 IRI/key 分工与贯穿性判据；
  [0006](0006-ontology-scale-and-the-prompt.md) 拆掉了大本体的提示词障碍；
  [0007](0007-who-decides-what-becomes-a-relation.md) 定了从种子长大的采纳规则

> 与 0006、0007 同规格：先摆数字，并标明每个数字**不能**说明什么。
> 本篇的数字**只来自官方发布文件**（2026-08-30 抓取），任何人都能重跑。
> **刻意不引用开发库的统计**——那是 mock 语料，能说明错误的形状，
> 不能说明真实部署的比率，写进决策记录只会被后人当成依据。

## 问题

新建的库只发 10 个种子关系（`graph.rs:290` 的 `RELATION_TEXT_ZH`）。这 10 个里
**零个带类型签名**——没有 domain，没有 range。

抽取提示词是支持签名的（`extract/lib.rs:498` 的测试：`- buys_from (employee|team → *)`），
有签名的谓语会带上「主语类型 → 宾语类型」。种子没得可喂，于是退化成一句白话：
`- works_at: 受雇于某个组织。`

**一句白话约束不了方向。** `produces` 的定义是「一个组织或项目制造、发布或推出了宾语」——
「组织或项目」是对主语的**类型提示**，不是**方向约束**。模型读到「Anthropic 的 Claude」，
知道两者有 `produces` 关系，但没有任何机器可检的东西告诉它谁该在主语位。
而产品在本体里往往也归到 Project 或 Product，于是两个方向读起来都合法。

十个种子里只有 `part_of` 写了反例（"**不是「可以在……上用」**"），显然是踩过坑之后补的。
**这正是问题所在**：用散文补方向，每个谓语都要单独踩一次坑，而且补丁只在那一条下面生效——
`part_of` 警告过的"能在某服务上玩"，换个谓语照样踩。

`graph.rs:4` 的自陈说明这不是个别谓语的事：

> 文档，里面的关系大半不在这 10 个里，于是大量事实降级成 related_to

**十个谓语覆盖不了真实文档，而扩到几十个也不解决方向问题**——只要方向靠散文写，
就是 O(谓语数) 次踩坑。

## schema.org 把方向变成结构

| | 类 | 属性 | domain+range 齐全 |
|---|---|---|---|
| 我们的 10 个种子 | — | 10 | **0** |
| schema.org（2026-08-30） | 1010 | 1521 | **1488 = 97.8%** |

```
schema:manufacturer   Product      → Organization
schema:worksFor       Person       → Organization
```

`Claude manufacturer Anthropic` 合法；反向因为 `Anthropic` 不是 `Product`，
**在 domain 校验时就被拦掉**，根本进不了图。方向不再靠散文描述，而是靠声明。

注意 `manufacturer` 的方向与我们的 `produces`（组织 → 产品）**正好相反**。
这不影响结论——重点不是哪个方向对，是方向被显式声明了。

## 候选包（实测）

抓取并按各自语法计数。判据三条：能被现有导入器吃下（Turtle / RDF-XML）、开放许可、带 domain/range。

| 包 | 体积 | 类 | 属性 | domain | range | 补什么 |
|---|---|---|---|---|---|---|
| **schema.org** | 1078 KB | 1010 | 1521 | 1521 | 1521 | 通用：人、组织、产品、事件、创作 |
| **W3C Org** | 82 KB | 13 | 34 | 36 | 32 | **组织架构**：Unit、Post、Membership、Role、任期 |
| **PROV-O** | 110 KB | 62 | 69 | 63 | 62 | 溯源：Activity、Agent、wasDerivedFrom |
| **FOAF** | 43 KB | 12 | 62 | 60 | 57 | 社交关系 |
| **IOF Core** | 394 KB | 294 | 75 | 94 | 94 | 工业制造 |

**W3C Org 只有 13 类 34 属性，却补上 schema.org 最弱的一块。** schema.org 的
`Organization` 是给网页用的，没有部门、职位、任期、汇报关系的概念。

**PROV-O 顺带补上一个对外的缺口**：竞品对比里"Provenance: W3C PROV-O"与
"Compliance export"两格，我们今天是自有 schema，导进来就有了标准词汇。

### 下不到或不适合的

- **FIBO**（金融）模块化发布，完整版要按 catalog 抓几百个文件。单模块（People）
  只有 31 类。想做金融是单独一件工程，不是随手加一个包。
- **Brick**（1438 类）与 **QUDT**：属性对类的比例极低（Brick 是 25 : 1438）——
  它们是分类树不是关系本体，对抽取帮助有限。
- **SAREF**：解析出的类与属性都是 0，声明形式我们的投影暂不覆盖，需先验证。
- **SNOMED CT**：需授权。

## 重叠有多少

按 `key_from_iri()` 的实际算法比对——撞的是 key 不是 IRI。

| 组合 | 撞名 | 具体 |
|---|---|---|
| org ∩ schema | **6** / 43 | `identifier` `location` `member` `member_of` `organization` `role` |
| foaf ∩ schema | **15** / 72 | `agent` `name` `person` `knows` `member` `organization` `title` `image` `logo` `thumbnail` `given_name` `family_name` `gender` `project` `status` |
| prov ∩ schema | **4** / 95 | `agent` `contributor` `creator` `publisher` |
| org ∩ foaf | 2 | `member` `organization` |

去重后**约 20 个词**。而且分两类，不能一概映射：

**真同义 —— 映射掉**：`foaf:Person ≡ schema:Person`、`org:Organization ≡ schema:Organization`、
`foaf:knows ≡ schema:knows`

**同名不同义 —— 必须两份**：
- `org:role` 是组织中的职位，`schema:role` 是创作作品里的角色（演员饰演）
- `foaf:status` 是即时通讯的在线状态（2000 年代遗留），`schema:status` 是订单/动作状态

**FOAF 重叠率最高（21%）而独有价值最低**——核心概念 schema.org 全有，剩下的
`mbox_sha1sum`、`icqChatID` 是遗物。它进候选列表，但不进推荐默认。

## 决定

### 1. 建库时多选，不做预制捆绑

初稿曾拟三个固定组合（通用 / 溯源合规 / 工业）。**否掉**：对齐表是两两声明的，
`foaf:Person ≡ schema:Person` 这条不管用户选了几个包都成立，所以多选**不增加对齐成本**。
而固定捆绑是拍脑袋的分类，现实里一定不匹配——做金融科技的要 IOF 加未来的 FIBO，
咨询公司要 schema 加 Org 加 PROV，没有哪个捆绑覆盖得了。

界面上每个包标出规模与**跟已选项的重叠率**，让用户自己掂量，而不是我们替他决定。

### 2. 不做导入撤销

`ON DELETE RESTRICT`（`0004_graph.sql:41`）加应用层计数（`ontology.rs:232`）加
`NOT builtin` 三道防线，已经保证了安全边界：**有实体挂着的类型删不掉**。

所以"选错了怎么办"的实际路径是：没被用到的类型直接删，被用到的删不掉——
**而删不掉恰恰是对的**，那些类型上挂着真实知识。

需要的不是 `undo_import`，是一个**批量视图**：「这次导入建了 294 个类，
12 个有实体在用，282 个空着」，一键删空的。比真正的撤销简单得多，
且语义清楚——不碰任何有数据的东西。

> 这三道防线值得单独记一笔，它是判据 2「本体是引导不是执法」的**反面**：
> 本体不执法，但**知识可以否决本体的删除**。方向是知识保护本体，不是本体裁剪知识。

### 3. 对齐表静态维护，不做运行时推断

约 20 行，我们自己写。**上游已经写好了一部分**——W3C Org 官方文档声明了与 FOAF 的
对齐，PROV-O 声明了与 FOAF、Dublin Core 的对齐，抄过来即可。

不做自动对齐（标签相似度、嵌入匹配）：预制包是我们选的，不是用户随便传的，
只有五六套，两两重叠有限且可穷举。**把无限的对齐问题缩成一张静态表**，
这与判据 6「治理经验固化进 schema，不要固化成隐形规则」一致。

## 开放问题

- **混装的抽取准确率没人测过。** 0006 的数据是单一本体下的。全选之后本体约 2400 类，
  按块检索会同时召回 `org:role` 与 `schema:role`。0006 的预算保证**塞得下**，
  但塞得下不等于**选得准**。这是本篇最大的未知，应在实施前用 `scripts/bench/` 补一组对照。
- **schema.org 面向网页内容**（Recipe、JobPosting、Event），企业语料的"合同""审批"
  "供应商资质"它没有。它是好的冷启动底座，不是终点——0007 的增长回路仍然必要，
  只是起点从 10 变成 1500。
- **中文标签**：schema.org 与所有候选包都只有英文。我们 10 个种子是带中文名的
  （`produces` → 出品）。1500 个属性的中文化不可能手工做，也不该机器翻——
  `rdfs:label` 是模型读的令牌，译错比不译更糟。[0004](0004-language-and-localization.md)
  定的分工在这里够不够用，待查。
- **起点规模本身是个变量。** 0007 的死路里那条 Snowball 实测顺带证明了这点：
  词表从 10 涨到 629，匹配行为会质变（捞回 49 次 → 18 次）。没人正面研究过
  "从 1500 个词起步"对采纳回路意味着什么——高频提议还会提议什么？
