# OntoPilot Roadmap

[English](#principles) · [简体中文](#中文摘要)

This roadmap communicates direction, not promised dates. Priorities may change after user feedback, benchmark evidence, security findings, or compatibility work. Shipped behavior is documented in the README and in-app documentation; an unchecked roadmap item is not a supported feature.

## Principles

1. **Governance before automation.** Models propose; permissions, evidence, deterministic guards, review, and audit decide what becomes durable state.
2. **Stable delivery before feature breadth.** Releases, public APIs, MCP tools, and provenance formats need compatibility discipline.
3. **Domain-neutral quality.** Improvements should generalize across corpora instead of encoding benchmark-specific or industry-specific shortcuts.
4. **Self-hosting by default.** Operators control documents, graph stores, credentials, and artifacts.
5. **Measure before claiming.** Quality and performance claims require reproducible protocols, preserved results, and clearly stated limitations.

## Now — Stabilize the pre-1.0 foundation

- [ ] Add an explicit schema-migration framework and upgrade/downgrade integration tests.
- [ ] Provide documented backup, restore, and disaster-recovery commands for Compose deployments.
- [ ] Add structured application logs, metrics, health/readiness separation, and deployment diagnostics.
- [ ] Expand browser-level regression coverage for ingestion, review filters, ontology interaction, releases, docs, and settings.
- [ ] Establish accessibility checks and keyboard-navigation acceptance criteria.
- [ ] Define API, MCP tool-schema, release-manifest, and provenance compatibility policies.
- [ ] Add resource ceilings and performance baselines for large uploads, graph views, review queues, and exports.

Exit criteria: a clean installation and an upgrade from the previous supported snapshot pass automated and manual acceptance; backup/restore is rehearsed; public-contract changes are detected in CI.

## Next — Team governance and integrations

- [ ] Review assignments, comments, mentions, notification preferences, and saved queue filters.
- [ ] Organization/workspace administration beyond a single installation-wide user list.
- [ ] OIDC/OAuth identity-provider integration and deployer-oriented role mapping.
- [ ] S3-compatible artifact storage and pluggable document/blob backends.
- [ ] Pluggable parsing adapters for MinerU and other document-understanding frameworks, with normalized chunks, layout metadata, and provenance across backends.
- [ ] Signed webhooks or an event-delivery API for extraction, review, release, and deployment state changes.
- [ ] Deployment recipes for reverse proxies, object storage, managed PostgreSQL, and common container platforms.
- [ ] Retention controls for source chunks, provider payloads, audit evidence, and exported artifacts.

Exit criteria: multi-reviewer teams can coordinate work without external spreadsheets, and operators can integrate release events and storage without modifying core code.

## Later — Agent-assisted ontology engineering

- [ ] First-party chat UI backed by short-lived, user-scoped MCP tokens.
- [ ] Evidence-aware proposals that cite source chunks and current graph context.
- [ ] Mandatory preview and impact analysis before any agent mutation.
- [ ] Conversation-level budgets, tool allowlists, cancellation, and complete audit playback.
- [ ] Proposal comparison, partial approval, and reusable review policies.
- [ ] Evaluation suites for agent safety, permission enforcement, hallucinated evidence, and rollback correctness.
- [ ] Spatiotemporal modeling and governed sandbox simulation with versioned scenarios, explicit assumptions, what-if analysis, and reproducible results.

Exit criteria: an agent can suggest and execute approved changes without receiving browser credentials, bypassing live roles, mutating published releases, or hiding the exact diff from the user; simulations remain isolated from published state and traceable to versioned graphs, inputs, and assumptions.

## Toward 1.0

- [ ] Stable versioning and deprecation policy for REST, MCP, manifests, provenance, and configuration.
- [ ] Tested migrations across every supported release line.
- [ ] Documented high-availability boundaries and recovery-point/recovery-time expectations.
- [ ] Independent security review and remediation of high-severity findings.
- [ ] Reproducible large-corpus quality/performance report with declared hardware and provider/model versions.
- [ ] Long-term support and responsible-disclosure commitments published in `SECURITY.md`.

## Explicit Non-goals

- Fully autonomous publishing without a permissioned human-controlled release step.
- Hiding model/provider uncertainty behind a single opaque quality score.
- Benchmark-specific entity allowlists or hard-coded domain ontologies in the generic extraction path.
- Allowing external SPARQL updates or arbitrary agent HTTP/file access through OntoPilot tools.
- Treating the mutable workspace as a substitute for immutable, version-pinned production consumption.

## How to Propose a Roadmap Change

Open a focused GitHub issue describing the user problem, affected workflow, security/compatibility impact, and measurable acceptance criteria. Large public-contract changes should be discussed before implementation and include a migration plan.

## 中文摘要

本路线图用于表达方向，不承诺发布日期；未勾选事项不属于已支持功能。

- **近期：稳定 1.0 前基础。** 正式数据库迁移、备份恢复、可观测性、浏览器回归、无障碍、公共契约兼容检查和性能基线。
- **下一阶段：团队治理与集成。** 审核分配/评论/通知、组织管理、OIDC、对象存储、Webhook 和生产部署模板。
- **解析框架扩展。** 增加 MinerU 等文档理解框架的可插拔适配，统一不同后端输出的 Chunk、版面元数据和溯源信息。
- **后续：Agent 辅助本体工程与高级建模。** 第一方对话、短期用户 MCP Token、证据引用、强制 Diff 预览、预算/Tool 白名单和安全评测；支持时空建模与受治理的沙盘推演，场景、假设和结果可版本化、可复现、可追溯。
- **1.0 标准：** 稳定版本策略、完整迁移链路、灾难恢复目标、独立安全审查以及可复现的大规模质量/性能报告。

明确不做：无人审核自动发布、用不透明单分数掩盖不确定性、为 Benchmark 写特例、开放 SPARQL 更新或任意 Agent 网络/文件访问、让可变工作区替代固定版本的生产服务。
