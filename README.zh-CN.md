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

**由 [DeepLethe 深纪元](https://deeplethe.com) 构建的企业知识世界模型。** 它是首个基于本体的被动学习、自我治理的开源知识工程基座——有别于知识图谱或向量知识库，该项目将时间感知与本体论融入了系统底层，根据传入的语料演进知识体系，基于本体论进行冲突检测、知识推理、智能决策。支持离线部署，快速建立企业知识基座、可信决策中枢、合规审计中枢，推进企业的智能化落地。

> 请注意，我们不愿意将项目定义为 Palantir 的开源尝试，而是一种**自底（知识治理）向上（可信智能决策与推演）的企业智能新思路**。

---

<!-- 视频：把 mp4 拖进任意 issue/PR 的评论框，GitHub 会返回一个
     https://github.com/user-attachments/assets/xxx 链接，
     把那个链接单独一行粘在这里即可自动渲染成播放器。 -->

<div align="center">

https://github.com/user-attachments/assets/aa226443-75de-437e-bd80-88e592ed8457

</div>

---

## 项目的世界观

我们给它取了一个稍显浪漫的名字——**Utopia（乌托邦）**。托勒密的地心说曾在很长一段时间中被视为真理，后来被哥白尼、开普勒、伽利略与牛顿一步步证伪。现在回过头来看，我们记住的不止是「日心说是对的」，而是这段历史如何发展。

不同于现有向量知识库、知识图谱工作追求当下知识的正确，Utopia 的设计初衷之一是记录完整的认知变化历程。工程上实现为**双时态知识图谱**。在决策复盘时，该系统可以拿到完整的决策过程与依据。为了提升可用性，我们基于公开的企业信息、教育、金融、法律、科研等领域语料库做了大量迭代。时态能力只是其中一面，如何接纳知识、如何推演未来、如何让逻辑约束行动，见 [utopia.bi/philosophy](https://utopia.bi/philosophy)。

## 功能

系统由 Rust 二进制和 Postgres 服务组成。我们通过 pgvector 和队列表设计减轻了技术栈和服务依赖的负担。

| 能力 | 亮点 |
| --- | --- |
| **完整应用** | 系统控制台 · 图谱浏览器 · 在线本体工作台 · 开箱即用 |
| **文档接入** | 支持多种文档（pdf、md、html、ppt、word、excel）· 支持自定义订阅更新与定时同步（JIRA、飞书正在支持中） · 网页、RSS、GitHub、Jira、Notion、WebDAV、S3 兼容存储定时同步 |
| **混合检索** | Tantivy · pgvector 向量 · RRF 融合 · chunk 溯源 |
| **双时态图谱** | 知识时态+溯源时态 · 支持任意时刻图谱 · 知识变更链 |
| **AgentHarness · AgenticRAG** | 应用本身具备 harness 能力，可通过对话调用系统完整功能 · 内置智能体包含多种工具，支持多轮工具调用与对话 |
| **内置本体包** | 内置 schema.org · W3C Org · PROV-O · FOAF · IOF Core · 不断扩展中 · [我想申请对自己的行业进行额外支持](https://github.com/deeplethe/utopia/issues/new?labels=enhancement&title=Ontology%20pack%20request) |
| **语义抽取** | 实体、关系与时间归一化 · 事实强制带证据引句 · 向量与本体召回加 LLM 裁决自动消歧，可追溯可撤销 · 随文本自动提出本体修订方案 |
| **知识派生与推理** | 时态 Datalog · 前向链 · 本体公理编译 · 派生路径追溯 · 基于 Rust 自建轻量推理引擎 |
| **冲突检测** | 时态冲突 · 自反、反对称、传递环、基数违规 · 本体自身缺陷 · 可撤事实、可改公理、可认可并存 |
| **人工审核与审计台账** | 低置信抽取、待合并候选自动进入审核队列 · 记录每一次操作的用户、时间、变更快照，用于合规审计 |
| **智能映射与问数** | 选定数据库和知识库，智能体自动探索并建立映射关系 · 基于 Ontology2SQL 的问数 · [在 BIRD Mini-Dev 上取得 SOTA（最佳成绩）](https://github.com/bird-bench/bird-bench.github.io/pull/218) |
| **模型接入** | 任何 OpenAI 兼容端点 · 支持本地部署模型 |
| **多用户多知识库** | 以知识库为单位的角色与权限设计，支持系统管理员、用户，知识库管理、编辑、访问权限分级 |
| **[决策智能（开发中）](#路线图)** | 决策记录 · 认知与决策过程回放 · 情景叠加推理 |

## 快速开始

依赖：Docker（本地开发另需 Rust 1.85+、Node 20+、pnpm）。

通过预构建镜像快速启动：

```bash
git clone https://github.com/deeplethe/utopia.git
cd utopia
docker compose --profile app up -d
```

打开 http://localhost:1516 注册 —— 第一个账户自动成为管理员，同时系统会创建所有人可读的公共知识库。抽取业务文档前，请先在「管理 → 模型」里配置模型端点（chat 与 embedding）。

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

- [ ] **决策推理**：计算约束条件，决策复盘
- [ ] **业务规则**：由人写下、作用于实体属性事实的规则（阈值、类别集合），把实体归类为一条带前提的派生事实，规则与前提就是它的解释（[#277](https://github.com/deeplethe/utopia/issues/277)）
- [ ] **执行校验层**：对 Agent 的调用进行本体规则与符号逻辑校验
- [ ] **问数与映射添加数据湖仓支持**：Iceberg / Delta Lake，以及 Databricks、Snowflake、MaxCompute 的映射探索与 Ontology2SQL 支持
- [ ] **更多数据源**：MySQL、ClickHouse、Doris 驱动，飞书连接器
- [ ] **精确到时刻**：在年 / 月 / 日之外加一档 `instant` 精度，给那些本来就带时间戳的来源——现在连接器按 UTC 截到天，跨午夜的事件会差一天
- [ ] **MCP 上的 Agent 记忆**：补齐 episodes 写入、retrieve 端点与 MCP 服务器
- [ ] **企业化**：OIDC SSO、备份恢复命令、10 万文档级别的性能基准

## 当前状态

Utopia 仍处于 **v0.1**。数据库 schema 会随版本演进，迁移只前滚、不提供回退 —— 生产环境请用 `UTOPIA_IMAGE` 锁定具体版本，并在升级前备份数据库与 `data` 目录。

公网部署前请阅读 [SECURITY.md](SECURITY.md)。

## 社区

- 💬 [Discussions](https://github.com/deeplethe/utopia/discussions)：欢迎讨论，分享使用经验，发表评价
- 🐛 [Issues](https://github.com/deeplethe/utopia/issues)：任何 bug 或设计问题、需求
- 🤝 [Contributing](CONTRIBUTING.zh-CN.md)：开发环境、提交前的检查、DCO 签名
- 🔌 [Ontology2SQL](https://github.com/deeplethe/ontology2sql)：本文提到的本体驱动的 Text-to-SQL 方法

## License

[Apache-2.0](LICENSE)
