# 类型消解的测量台

**每一组一个新库。** 这条规则是整个目录存在的理由。

此前连着三轮在同一个库上调检索，而那个库带着前几轮的改类结果——容易的实体早已
精化，拒绝理由里直接写着 `already correctly typed as pharmacy`。后两轮的数字跟
第一轮根本不可比，却被当成依据改了两次代码。复用一个库省下的几分钟，换来的是
一整段无效的结论。

## 跑一组

```
node scripts/bench/run.mjs --corpus pharma --label seeds-only
node scripts/bench/run.mjs --corpus pharma --ontology /tmp/schemaorg.ttl --label schemaorg
```

前置：`utopia-server` 已启动、连着一个能写的库、工作区配好了对话与嵌入模型。
环境变量见 `run.mjs` 头部。

## 目录

- `corpora/*.json` —— 固定语料。**实体跨文档反复出现**是刻意的：单篇语料里每个
  实体只有一两条事实，画像基本只剩名字，量不出消解的真实水平。
- `truth/*.json` —— 每个实体期望落到哪个类。key 是名字的一个足以认出它的片段
  （抽取给的名字每次略有出入，全等匹配会把这种变化算成失败）；值是可接受的类，
  任一命中算对。**空数组 = 本体里没有对得上的类，此时正确行为是不动它**。
- `truth/*.wikidata.json` —— **不经人手的答案卷**（0016 的 C1）：维基百科语料里每个
  实体在 Wikidata 上的 P31（instance of），经 `p31-to-classes.json` 映射到 schema.org
  包的类。`*.wikidata.raw.json` 是抓下来的原始材料（QID、P31、类落在哪些锚下），
  可重抓、可 diff；答案卷由它生成，不要手改——改映射或重抓。
- `p31-to-classes.json` —— 人的判断只进这一张表：Wikidata 的锚类（人、企业、软件、
  城市……）→ 本体里可接受的类。空数组沿用上面的语义（正确行为是不动它）；
  一个锚都落不到的实体不写进答案卷，不知道就不猜。
- `run.mjs` —— 一组：新建库 → 灌语料 → 可选导入本体 → 跑消解 → 打分。
  `--truth <file>` 换一份答案卷：同一组结果对手填答案与 Wikidata 答案各打一次分。
- `fetch-wikidata-truth.mjs` —— 抓 Wikidata 答案的原始材料：条目主题与**正文里出现过的**
  链接条目（导航框里的几百个不算）的 QID 与 P31，P31 类顺着 P279 归到锚类。
- `make-truth.mjs` —— 原始材料 + 映射表 → `truth/<corpus>.wikidata.json`。
- `fetch-ai-timeline.mjs` —— 抓条目的**当前版**（`prop=extracts`）。
- `fetch-wiki-history.mjs` —— 抓**历史快照**（`action=parse&oldid`）。演认知时间靠它：
  同一条目的多张快照按 `doc_time` 灌进去，图会真的改主意。
- `subset-corpus.mjs` —— 从一份语料里挑几个条目做成新语料。**整条目取**，
  因为 `supersedes` 只在同一条目的相邻快照之间发生，随机抽块会把时态那根轴废掉。
- `subset.mjs` —— 把 schema.org 的 TTL 切成前 N 个类，给退化曲线用。

## 读数怎么算

- `prompt_tokens_est` 是**本体段**的估算，不是整个提示词。实测 4.0 字符 ≈ 1 token
  （377,735↔81,855、396,716↔99,041）。真实 token 数在 LLM 客户端里，穿出来要改
  一路签名；这里要量的是"本体规模"，比例稳定就够用。
- `for_review` 按**没改**算进 miss。它确实还没改——算成命中就是把人的活记在机器账上。
- `absent` = 标准答案里有、但抽取压根没抽出这个实体。它不是消解的错，单独一栏。
- **命中 = 可接受类或它的子类**（按库里的类层级算）。答案卷给的是锚（organization、
  place），引擎答的常常更具体（research_organization、city）——精化正是要它做的事。
- 手填的答案卷按子串匹配名字，生成的（`match: "exact"`）按全名精确匹配。
- `score.tiers`：分档打分。`auto` 是自动落地那一档对答案卷的命中，`review` 是待人工那一档
  若盲目照收会怎样——放不放开自动跑看的是前者。

## 标准答案会写错

第一次跑就写窄了一个：`心血管健康论坛` 只写了 `business_event|event_series`，
而系统给的 `conference_event` 是对的。**答案错了要改答案**——但要在结果出来之后
才改、且写清楚为什么，否则这份答案就变成了"系统这次答了什么"的记录，量不出任何东西。

## 加一个语料

两个文件：`corpora/x.json` 与 `truth/x.json`。语料换行业是有意的——同一套判断在
两个领域上都成立，才谈得上不是过拟合到某一批词上。
