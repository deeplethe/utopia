<div align="center">

# Utopia

**企业的活记忆。**

开源、自部署的知识平台：在自己的文档上做 RAG，底下是一张记得每条事实"什么时候成立"的知识图谱。

[快速开始](#快速开始) · [功能](#功能) · [配置](#配置) · [路线图](#路线图) · [English](README.md)

[![CI](https://github.com/deeplethe/utopia/actions/workflows/ci.yml/badge.svg)](https://github.com/deeplethe/utopia/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

</div>

---

普通知识库只回答"是什么"，Utopia 的差异化是**时间**。

- **"2024 年 Q3 谁负责 X 项目？"** —— 每条事实都带有效区间，图谱可以按任意时点读取。
- **"这条规定什么时候改的？改之前是什么？"** —— 事实只追加不覆盖。修正是闭合旧版本、写入新版本，历史永远可以回放。
- **拖动图谱页的时间轴**，看邻域图在那个时刻长什么样。

每条边都能跳回它来自的那句原文。没有证据的事实不进图谱。

一个二进制加一个 Postgres。不需要 Elasticsearch，不需要向量服务，不需要消息队列。

## 功能

**摄入** —— PDF、DOCX、PPTX、XLSX/XLS/ODS、CSV/TSV、Markdown、HTML、纯文本，中文编码自动识别。网页和 RSS 按 cron 定时同步。用来源级别的 ingest token 可以从任何地方推送文档。解析失败的文档可以原地重处理，不必重新上传。

**搜索与对话** —— 混合检索：Tantivy 全文（中文走 jieba 分词）+ pgvector 向量，用 RRF 融合。流式回答带引用角标，点击直接跳到原文段落。LLM 走 OpenAI 兼容协议 —— DeepSeek、Qwen、GLM、Ollama、vLLM 都行，整套可以跑在完全内网环境。

**知识图谱** —— 基于可编辑的本体做 LLM 抽取，产出实体与事实。三段式实体消解，合并日志里每一次合并都可撤销。每条事实存 `valid_from` / `valid_to`，并带证据行指回原始分块。

**审核队列** —— 低置信抽取、待合并候选、基数冲突都进队列，而不是打断使用者。可以手工确认、驳回或闭合一条事实，决策都会留痕。

**问你的数据库** —— 在系统层注册一次 Postgres 连接，挂载到知识库上，对话就能在文档之外一并查询它。

**多租户** —— 组织 → 工作区 → 知识库，知识库级成员管理、角色权限和审计流水。

## 快速开始

依赖：Docker（本地开发另需 Rust 1.85+、Node 20+、pnpm）。

```bash
git clone https://github.com/deeplethe/utopia.git
cd utopia
docker compose --profile app up -d
```

打开 http://localhost:8080 注册 —— 第一个账号即管理员。然后在设置里填一个 OpenAI 兼容端点，建一个知识库，把文件拖进去。

这会从 `ghcr.io/deeplethe/utopia` 拉预构建镜像。上传的原始文件与全文索引在 `./data`，备份时和数据库一起带上。改了代码、或者想跑未发布的版本，则从源码构建：

```bash
docker compose -f docker-compose.yml -f docker-compose.build.yml --profile app up -d --build
```

### 本地开发

```bash
# 1. 启动数据库（pgvector 版 Postgres）
docker compose up -d db

# 2. 启动后端（自动跑迁移，默认 :8080）
cargo run -p utopia-server

# 3. 启动前端（:5173，/api 代理到后端）
cd web && pnpm install && pnpm dev
```

## 配置

所有配置都是 `UTOPIA_` 前缀的环境变量，复制 [.env.example](.env.example) 为 `.env` 即可开始。

| 变量 | 默认值 | 用途 |
|---|---|---|
| `UTOPIA_DATABASE_URL` | `postgres://utopia:utopia@localhost:5432/utopia` | Postgres 连接串 |
| `UTOPIA_BIND_ADDR` | `0.0.0.0:8080` | 监听地址 |
| `UTOPIA_JWT_SECRET` | `dev-secret-change-me` | **生产环境必须修改** |
| `UTOPIA_WEB_DIST` | `web/dist` | 前端构建产物；存在时由服务端托管 SPA |
| `UTOPIA_DATA_DIR` | `data` | 原始文件与全文索引 |
| `UTOPIA_OPEN_REGISTRATION` | `true` | 为 false 时仅首个账号可自助注册 |

LLM 的端点、模型和 API Key 在界面里配置，不走环境变量。

## 架构

```
React + Vite + Tailwind + TanStack
              │  /api/v1
        ┌─────┴─────┐
        │  axum     │  utopia-server   HTTP · 认证 · 任务
        └─────┬─────┘
   ┌──────────┼──────────┬────────────┐
utopia-ingest  utopia-search  utopia-extract  utopia-llm
  解析/分块      tantivy+RRF     实体/事实      OpenAI 兼容
   └──────────┴──────────┴────────────┘
                   utopia-store
                        │
              PostgreSQL + pgvector
```

Rust（axum · sqlx · tokio · tantivy）+ PostgreSQL/pgvector 作为唯一外部依赖。后台任务走 `SKIP LOCKED` 任务表，启动时自动接管孤儿任务。

## 路线图

已完成：文档摄入、混合搜索、带引用的 RAG 对话、本体编辑、实体与事实抽取、实体消解、审核队列、带时间轴的图谱浏览、多租户权限。

接下来：

- **时态查询 API** —— 数据模型已是双时态，时间轴目前在客户端过滤，服务端 as-of 查询尚未开放。
- **推理引擎** —— 时态 Datalog 派生事实并给出解释。`utopia-reason` 目前是空占位。
- **MCP 上的 Agent 记忆** —— 记忆空间已在 schema 中（`kb.kind = 'memory'`），episodes 写入、retrieve 端点与 MCP 服务器尚未实现。
- **更多连接器** —— S3/WebDAV、Notion、飞书。
- **OIDC SSO**、备份恢复命令、10 万文档级别的性能基准。

## 当前状态

Utopia 目前是 **v0.1**，由一名维护者在持续开发。可用，但 schema 在版本之间仍会变化。

部署前有两件事值得知道：

- 界面里录入的凭据 —— LLM API Key 和数据库连接串 —— 在 Postgres 里是**明文存储**的。静态加密是 1.0 前的硬化项。请部署在可信网络内。
- 暂不保证升级路径。迁移会自动前滚，但如果在意数据，请锁定版本。

欢迎 issue 和 PR。

## License

[Apache-2.0](LICENSE)
