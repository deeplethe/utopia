<div align="center">

<img src="assets/banner.webp" alt="Utopia" width="100%">

**人类首个开源的企业世界模型。**

[世界观](#项目的世界观) · [快速开始](#快速开始) · [功能](#功能) · [路线图](#路线图)

[![Official site](https://img.shields.io/badge/OFFICIAL-UTOPIA.BI-8A6D1F?style=flat-square&labelColor=161B22)](https://utopia.bi)
[![License](https://img.shields.io/badge/LICENSE-APACHE%202.0-1F4B3F?style=flat-square&labelColor=161B22)](LICENSE)
[![Rust](https://img.shields.io/badge/BUILT%20WITH-RUST-6B3524?style=flat-square&labelColor=161B22&logo=rust&logoColor=C9D1D9)](https://www.rust-lang.org)
[![Container](https://img.shields.io/badge/GHCR-DEEPLETHE%2FUTOPIA-1E3A5F?style=flat-square&labelColor=161B22&logo=docker&logoColor=C9D1D9)](https://github.com/deeplethe/utopia/pkgs/container/utopia)

[![Discussions](https://img.shields.io/badge/DISCUSSIONS-3B2A52?style=flat-square&labelColor=161B22&logo=github&logoColor=C9D1D9)](https://github.com/deeplethe/utopia/discussions)
[![Built by DeepLethe](https://img.shields.io/badge/BUILT%20BY-DEEPLETHE-2D333B?style=flat-square&labelColor=161B22)](https://github.com/deeplethe)
[![English](https://img.shields.io/badge/LANG-ENGLISH-5C2A2A?style=flat-square&labelColor=161B22)](README.md)

</div>

---

<!-- 视频：把 mp4 拖进任意 issue/PR 的评论框，GitHub 会返回一个
     https://github.com/user-attachments/assets/xxx 链接，
     把那个链接单独一行粘在这里即可自动渲染成播放器。 -->

<div align="center">

https://github.com/user-attachments/assets/PLACEHOLDER

</div>

---

## 项目的世界观

> **我们强烈建议阅读本章节**，约 5 分钟。

**一个 Agent Memory？又一个 Graph RAG？或者，又一套 DAG？** 都可以是，但又不止于此。

我们想搭建的，是一个属于企业自己的世界模型：它理解企业中的人、事、规则与关系，也理解它们如何随时间变化、为何成立、何时失效。记忆、知识、规则、推理与行动，都是这个世界模型的一部分。

所以我们给它取了一个稍显浪漫的名字：**乌托邦（Utopia）**。工程上的说法则是：一个被动演化的企业世界模型——**知识的枢纽，决策的基座，受控执行的闸门，推演未来的试验场。**

但真正维护起来才会发现：建立一个世界并不难，难的是让它长期、稳定而可信地运转下去。要维持一个理想国度，需要一些伟大的制度。于是有了下面这些属于「乌托邦」的法则。

| 法则 | |
|---|---|
| **宙斯之律**<br/>Zeus's Law | 善待每一条来到这个世界的知识 |
| **历史法则**<br/>Law of History | 知识不是一张静止的事实表，而是一段不断发生的历史 |
| **理想演绎**<br/>Law of Deduction | 从「理解现在」，进一步走向「推演未来」 |
| **世界铁律**<br/>The Iron Gate | 让规则托住智能，让逻辑约束行动 |

### 宙斯之律（Zeus's Law）

> **善待每一条来到这个世界的知识。**

在古希腊，宙斯守护陌生人与旅人。无论来者从哪里出发，在被了解之前，都应先得到接纳。「乌托邦」也希望如此对待每一条进入世界的知识。

它可以来自文档，也可以来自数据库、数据仓库、网页与持续订阅；可以结构完整，也可以只是一次新的表达。我们不会因为它暂时无法归入既有结构，就将它拒之门外。

系统会尝试从知识中识别实体、事实与关系，在冷启动阶段逐渐形成自己的本体；新的谓语可以出现，旧的谓语不会轻易消失，事实发生变化时，也会保留它曾经存在过的痕迹与变更链。

而善待知识，并不意味着不计代价地理解一切。我们致力于在准确度、抽取效率、成本控制与人类介入之间取得四位一体的平衡。

这是「乌托邦」的宙斯之律：善待每一条来到这个世界的知识。

### 历史法则（Law of History）

> **知识不是一张静止的事实表，而是一段不断发生的历史。**

传统的知识图谱，往往试图记录一个个确定的事实，并不断追求什么才是「正确」的知识。可是，知识是否正确，有时只有时间才能回答。

托勒密的地心体系曾在漫长的历史中被视作对宇宙秩序的合理解释；后来，哥白尼提出日心体系，开普勒修正行星运动模型，伽利略带来新的观测证据，牛顿又用统一的力学体系重新解释天体的运行。今天，我们当然知道地心说不再是描述太阳系结构的正确模型。

可是，我们依然记得它。

我们不仅记得地心说曾经存在，也记得它为什么会被相信、在什么时代被接受，又是如何被新的观测、理论与证据一步步修正和取代。因为一个真实的世界，需要记录的从来不只是「什么是正确的」，它还需要记录：我们曾经相信什么，又是如何走到今天的。

因此，在「乌托邦」中，一条知识不会因为新的事实出现便被简单覆盖或删除。系统会记录它何时被摄入、何时发生变更、在现实世界中的有效时间，以及我们在什么时候获知了这一变化。于是，我们不仅可以问「现在什么是真的？」，还可以追问：

```
「当时我们认为什么是真的？」
「这件事从什么时候起成立？」
「我们又是什么时候知道它变了？」
```

只有保存这些变化，知识才能真正参与历史进程的演绎。在工程上，我们称之为**双时态知识图谱（Bitemporal Knowledge Graph）**。

### 理想演绎（Law of Deduction）

> **从「理解现在」，进一步走向「推演未来」。**

拉普拉斯曾设想：如果存在一个足够强大的智能，能够在某一瞬间知晓宇宙中所有粒子的状态，以及支配它们运动的全部规律，那么理论上，它便可以推演整个宇宙的过去与未来。这意味着，它不仅能够预测星辰的运行，也能够推演此刻正在编写 README 的我在想什么，以及正在阅读 README 的你，下一秒又会产生怎样的念头。

在我们的「乌托邦」中，我们试图以一种工程化的方式逼近这一理想：通过前向链（Forward Chaining）驱动事实不断演绎，以符号系统表达规则、状态与因果关系，再辅以大语言模型处理难以被完全形式化的推理过程。如果能够掌握足够多的事实、规则与因果关系，我们或许可以让系统从「理解现在」，进一步走向「推演未来」。

### 世界铁律（The Iron Gate）

> **让规则托住智能，让逻辑约束行动。**

理想乡并不意味着随心所欲。恰恰相反，在这里，一切行为都应当运行于规律、规则与边界之内。我们最不希望看到的，是大模型的幻觉、人为疏忽或推理偏差，最终演变成逻辑错误、越权操作，甚至不可逆的错误执行。

因此，在「乌托邦」中，Agent 的判断并不天然等于行动。任何对下游服务的调用，在真正执行之前，都必须经过本体规则、约束条件与符号逻辑的共同校验：

```
它是否符合事实？        是否满足前置条件？
是否拥有相应权限？      是否触碰既定边界？
        其结果是否与当前世界状态相容？
```

只有通过这些规则，推理才能成为行动。大模型可以大胆思考，但执行必须克制；Agent 可以探索未知，但不能越过世界的铁律。让规则托住智能，让逻辑约束行动。

> 加油，Agent。
> 稳稳地接住每一个任务，也稳稳地守住每一道边界。

## 功能

系统的主体为单个二进制文件加 Postgres 服务。我们通过引入 pgvector 和队列表设计减轻了技术栈和服务依赖的负担 —— 咻的一下就部署好。

| | |
|---|---|
| **知识摄入** | 支持 PDF、DOCX、PPTX、XLSX/XLS/ODS、CSV/TSV、Markdown、HTML 和纯文本，自动识别中文编码。网页和 RSS 可按 cron 定时同步，也可以通过来源级 ingest token 从任何地方推送文档。解析失败的文档可以原地重处理，无需重新上传；整个来源或整个知识库也可以批量重抽。 |
| **搜索与对话** | 混合检索采用 Tantivy 全文搜索和 pgvector 向量搜索，并通过 RRF 融合结果；中文全文检索使用 jieba 分词。回答支持流式输出和引用角标，点击引用可直接跳到原文段落。LLM 走 OpenAI 兼容协议，DeepSeek、Qwen、GLM、Ollama、vLLM 都可以接入，整套系统也可以运行在完全内网环境。 |
| **本体与冷启动** | 建库即自带一套内置本体（人、组织、项目、产品、事件、概念、地点，及其间的关系），无需先设计模型就能开始摄入。抽取过程中遇到本体之外的类型与谓语，会被记录并计数；高频项可由模型给出扩充建议，确认后并入本体。本体因此随语料生长，而不是要求你在第一天就把世界定义完整。 |
| **双时态图谱** | 基于可编辑的本体进行 LLM 抽取，生成实体与事实。每条事实都带有效区间与对应的证据行。修正事实时不会覆盖旧版本，而是闭合旧事实并链上新版本。图谱与邻域查询都可以指定任意历史时点回读。实体面板同时展示两条时间线：一件事在现实中何时成立，以及系统何时形成这一判断、又何时改变判断。 |
| **实体消解与审核** | 采用三段式实体消解，每一次合并都会写入日志，并且可以撤销。低置信抽取、待合并候选和基数冲突会进入审核队列，不会打断使用者。手工确认、驳回或闭合事实时，所有决策都会留痕。 |
| **推理与派生** | 规则以时态 Datalog 表达，通过前向链驱动事实不断演绎。派生出的事实与抽取事实一样带有效时间与来源，并可展开完整的推导路径——每一条结论都能追问「为什么」，一路回溯到最初的原文。本体公理（类型继承、关系层级、传递、对称、互逆、互斥、基数）同样编译为规则参与推理，约束违例进入冲突队列。 |
| **基于本体的智能问数** | 在系统层注册一次 Postgres 连接，再挂载到知识库，对话就可以同时查询文档和数据库。这条路线的方法（[Ontology2SQL](https://github.com/deeplethe/ontology2sql)）在 BIRD Mini-Dev 上取得 SQLite 70.20 / PostgreSQL 65.80，两项均为当前 SOTA，分别领先第二名 12.2 与 9.0 分（[榜单提交](https://github.com/bird-bench/bird-bench.github.io/pull/218)）。 |
| **多用户与权限** | 权限以知识库为单位，每个库有自己的成员与角色，公共库对部署内所有人可读，私有库仅受邀用户可访问。部署后会自动建立全员可读的公共空间（General Knowledge Base）。 |
| **决策台账** | 事实的确认与驳回、实体合并与撤销、图谱重建等操作均留有记录，含操作者、时间与当时的对象快照；对象被作废或重建后，记录依然可查。 |


## 快速开始

依赖：Docker（本地开发另需 Rust 1.85+、Node 20+、pnpm）。

通过预构建镜像快速启动：

```bash
git clone https://github.com/deeplethe/utopia.git
cd utopia
docker compose --profile app up -d
```

打开 http://localhost:1516 注册 —— 第一个账户自动成为管理员，同时系统会创建所有人可读的公共知识库。摄入文档前，请先在系统设置里配置模型端点（chat 与 embedding）。

或者从源码构建：

```bash
docker compose -f docker-compose.yml -f docker-compose.build.yml --profile app up -d --build
```

### 本地开发

```bash
# 1. 启动数据库（pgvector 版 Postgres）
docker compose up -d db

# 2. 启动后端（自动跑迁移，默认 :1516）
cargo run -p utopia-server

# 3. 启动前端（:5173，/api 代理到后端）
cd web && pnpm install && pnpm dev
```

## 路线图

- [ ] **推演引擎**：情景叠加不写进账本，算出差异与违反的约束
- [ ] **执行校验层**：Agent 的每一次调用先过本体规则与符号逻辑，通不过不落地
- [ ] **数据湖仓**：Iceberg / Delta Lake，以及 Databricks、Snowflake、MaxCompute
- [ ] **更多数据源**：MySQL、ClickHouse、Doris 驱动，S3、WebDAV、Notion、飞书连接器
- [ ] **MCP 上的 Agent 记忆**：补齐 episodes 写入、retrieve 端点与 MCP 服务器
- [ ] **企业化**：OIDC SSO、备份恢复命令、10 万文档级别的性能基准

## 当前状态

Utopia 仍处于 **v0.1**。数据库 schema 会随版本演进，迁移只前滚、不提供回退 —— 生产环境请用 `UTOPIA_IMAGE` 锁定具体版本，并在升级前备份数据库与 `data` 目录。

部署到公网之前请读一下 [SECURITY.md](SECURITY.md)。

## 社区

- 💬 [Discussions](https://github.com/deeplethe/utopia/discussions) —— 提问、聊设计、说说你拿它做了什么
- 🐛 [Issues](https://github.com/deeplethe/utopia/issues) —— 报 bug、提需求
- 🔌 [Ontology2SQL](https://github.com/deeplethe/ontology2sql) —— 本体驱动的 Text-to-SQL，「基于本体的智能问数」背后的方法

## License

[Apache-2.0](LICENSE)
