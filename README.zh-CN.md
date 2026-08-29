<div align="center">

<img src="assets/banner.webp" alt="Utopia" width="820">

</div>

# Utopia

<div align="center">

[世界观](#项目的世界观) · [快速开始](#快速开始) · [功能](#功能) · [路线图](#路线图)

[![Stars](https://img.shields.io/github/stars/deeplethe/utopia?style=flat-square&label=STARS&labelColor=161B22&color=FFC220&logo=github&logoColor=FFFFFF)](https://github.com/deeplethe/utopia/stargazers)
[![License](https://img.shields.io/badge/LICENSE-APACHE%202.0-3FB950?style=flat-square&labelColor=161B22)](LICENSE)
[![Rust](https://img.shields.io/badge/BUILT%20WITH-RUST-F74C00?style=flat-square&labelColor=161B22&logo=rust&logoColor=FFFFFF)](https://www.rust-lang.org)

[![Official site](https://img.shields.io/badge/OFFICIAL-UTOPIA.BI-FFFFFF?style=flat-square&labelColor=161B22&logo=safari&logoColor=FFFFFF)](https://utopia.bi)
[![Container](https://img.shields.io/badge/GHCR-DEEPLETHE%2FUTOPIA-2496ED?style=flat-square&labelColor=161B22&logo=docker&logoColor=FFFFFF)](https://github.com/deeplethe/utopia/pkgs/container/utopia)
[![Discussions](https://img.shields.io/badge/DISCUSSIONS-8957E5?style=flat-square&labelColor=161B22&logo=data:image/svg%2Bxml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxNiIgaGVpZ2h0PSIxNiIgZmlsbD0iI0ZGRkZGRiIgY2xhc3M9ImJpIGJpLWNoYXQtZG90cy1maWxsIiB2aWV3Qm94PSIwIDAgMTYgMTYiPgogIDxwYXRoIGQ9Ik0xNiA4YzAgMy44NjYtMy41ODIgNy04IDdhOSA5IDAgMCAxLTIuMzQ3LS4zMDZjLS41ODQuMjk2LTEuOTI1Ljg2NC00LjE4MSAxLjIzNC0uMi4wMzItLjM1Mi0uMTc2LS4yNzMtLjM2Mi4zNTQtLjgzNi42NzQtMS45NS43Ny0yLjk2NkMuNzQ0IDExLjM3IDAgOS43NiAwIDhjMC0zLjg2NiAzLjU4Mi03IDgtN3M4IDMuMTM0IDggN001IDhhMSAxIDAgMSAwLTIgMCAxIDEgMCAwIDAgMiAwbTQgMGExIDEgMCAxIDAtMiAwIDEgMSAwIDAgMCAyIDBtMyAxYTEgMSAwIDEgMCAwLTIgMSAxIDAgMCAwIDAgMiIvPgo8L3N2Zz4%3D)](https://github.com/deeplethe/utopia/discussions)
[![Built by DeepLethe](https://img.shields.io/badge/BUILT%20BY-DEEPLETHE-2D333B?style=flat-square&labelColor=161B22)](https://github.com/deeplethe)
[![English](https://img.shields.io/badge/LANG-ENGLISH-DA3633?style=flat-square&labelColor=161B22)](README.md)

</div>

**由 [DeepLethe 深纪元](https://deeplethe.com) 构建的企业知识世界模型。** 它是人类首个基于本体的被动学习、自我治理的开源知识工程基座——把时态与本体一起做进底层，语料涌入时迭代本体定义，记忆认知变化的历程，依据公理解决冲突，回溯任意时刻的世界观。可以跑在企业内网里，可以跑在云服务器上，也可以就跑在你的笔记本上——拿它建企业的知识基座、智能体的可信决策中枢，或者属于你个人的史诗。

---

<!-- 视频：把 mp4 拖进任意 issue/PR 的评论框，GitHub 会返回一个
     https://github.com/user-attachments/assets/xxx 链接，
     把那个链接单独一行粘在这里即可自动渲染成播放器。 -->

<div align="center">

https://github.com/user-attachments/assets/PLACEHOLDER

</div>

---

## 项目的世界观

我们给它取了一个稍显浪漫的名字——**乌托邦（Utopia）**。要维持这样一个理想世界当然需要一些伟大的设计，限于篇幅，这里仅展开讲系统独特的时间感知功能：托勒密的地心体系曾被视作对宇宙秩序的合理解释，后来被哥白尼、开普勒、伽利略与牛顿一步步修正和取代。——你看，我们记住的不止是「哦！日心说是对的」，而是整个认知的变化过程，是一个完整的历史。

在本系统中，一条知识不会因为新事实出现就被覆盖。系统会记录它何时被摄入、何时变更、有效期，一个时序的修订链路与认知变更过程，从而实现真正的决策追溯——一年后复盘某笔审批，系统还原的是当时那条决策链完整的过程与依据。工程上，我们称之为双时态知识图谱。为了提升可用性，我们基于公开的企业信息、教育、金融、法律、科研等领域语料库做了大量迭代。

时间感知只是其中的一点，仍有很多设计聚焦于如何接纳知识、如何推演未来、如何让逻辑约束行动：[utopia.bi/philosophy](https://utopia.bi/philosophy)

## 功能

整个系统由 Rust 二进制和 Postgres 服务组成。我们通过引入 pgvector 和队列表设计减轻了技术栈和服务依赖的负担 —— 咻的一下就部署好。

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
