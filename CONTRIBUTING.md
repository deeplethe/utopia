# Contributing to OntoPilot

Thank you for helping build an open, reliable ontology-governance system.

Participation in this project is governed by the [Code of Conduct](CODE_OF_CONDUCT.md).

[中文贡献指南](#中文贡献指南)

## Required Branch Flow

Every change must use a type-prefixed branch and follow this integration flow:

```text
feat/**     ┐
fix/**      │
docs/**     │
refactor/** ├──→ dev ──→ main
test/**     │
chore/**    ┘
```

1. Update local `dev` and create a branch with the prefix that describes the change. Do not develop directly on `dev` or `main`.
2. Open a pull request from the type-prefixed topic branch into `dev`. Pull requests from other source branches into `dev` fail the branch-flow CI check.
3. After review and all required checks pass, merge the pull request into `dev`.
4. Only the project owner, GitHub user `@WaylandYang`, promotes `dev` to `main` through a `dev` → `main` pull request. Contributors must not open change branches directly against `main` or merge `dev` into `main` themselves.

Use these branch types:

| Prefix | Use for |
| --- | --- |
| `feat/**` | New user-facing capabilities or behavior |
| `fix/**` | Bug fixes and regressions |
| `docs/**` | Documentation-only changes |
| `refactor/**` | Behavior-preserving code restructuring |
| `test/**` | Test-only changes |
| `chore/**` | Dependencies, build, CI, tooling, and repository maintenance |

Start work with:

```bash
git fetch origin
git switch dev
git pull --ff-only origin dev
git switch -c <type>/<short-description>
```

Push and open the feature pull request with:

```bash
git push -u origin <type>/<short-description>
gh pr create --base dev --head <type>/<short-description>
```

Use lowercase, hyphen-separated branch descriptions, for example `feat/review-date-filters` or `fix/release-version-reuse`. Keep each branch scoped to one coherent change.

Direct pushes to `dev` and `main` are prohibited. The repository CI validates pull-request topology. GitHub branch rules should additionally require pull requests, passing checks, and code-owner review whenever the repository plan supports protected private branches.

## Before You Start

- Use GitHub Issues for reproducible bugs, focused feature proposals, and design discussion.
- Do not include confidential documents, credentials, model responses, or production graph data.
- Keep changes scoped. Refactors unrelated to the reported problem should be separate pull requests.

## Development Setup

```bash
git clone https://github.com/deeplethe/ontopilot.git
cd ontopilot/backend
python -m venv .venv
source .venv/bin/activate  # PowerShell: .venv\Scripts\Activate.ps1
pip install -r requirements-dev.txt
cp .env.example .env
```

```bash
cd ../frontend
corepack enable
pnpm install --frozen-lockfile
```

PostgreSQL is used by Docker Compose. Local backend development can omit `DATABASE_URL` and use SQLite.

## Required Checks

```bash
cd backend
pytest -q
python scripts/check_tbox_guard.py

cd ../frontend
pnpm lint
pnpm build

cd ..
docker compose config --quiet
```

## Ontology Boundary Changes

Changes to extraction, role classification, terminology mapping, hierarchy repair, or validation must:

1. remain domain-neutral;
2. avoid benchmark-specific entity lists or fixed corpus filters;
3. preserve exact source grounding;
4. add or update cases under `backend/tests/gold/`;
5. pass `scripts/check_tbox_guard.py`;
6. include an OntoLearner result when the taxonomy protocol is affected.

Gold-set updates should explain why the expected role is TBox, ABox, terminology, or literal data.

## Release and Provenance Compatibility

The release manifest, N-Quads shard naming, and provenance JSONL are public interchange formats. Pull requests that change them must:

- document the schema change;
- preserve old release readability or provide a migration path;
- add regression tests;
- increment the manifest schema identifier when compatibility breaks.

## Pull Requests

- Target `dev` from an allowed type-prefixed topic branch; only the project owner may target `main` from `dev`.
- Describe the problem and root cause.
- List the validation commands you ran.
- Include screenshots for user-interface changes.
- Update English and Chinese interface copy together.
- Update documentation when behavior or configuration changes.
- Do not commit `.env`, runtime data, benchmark caches, generated exports, or credentials.

By submitting a contribution, you agree that it is licensed under Apache License 2.0.

## 中文贡献指南

感谢你参与建设开放、可靠的本体治理系统。参与本项目即表示你同意遵守[行为准则](CODE_OF_CONDUCT.md)，提交的贡献采用 Apache License 2.0。

### 强制分支流程

所有改动必须使用能说明变更类型的分支，并遵循以下集成流程：

```text
feat/**     ┐
fix/**      │
docs/**     │
refactor/** ├──→ dev ──→ main
test/**     │
chore/**    ┘
```

1. 从最新的 `dev` 创建使用正确类型前缀的分支，禁止直接在 `dev` 或 `main` 上开发。
2. 从带类型前缀的主题分支向 `dev` 发起 Pull Request；其他来源分支提交到 `dev` 会被 CI 的分支流检查拒绝。
3. 代码审核和全部检查通过后，将 PR 合并到 `dev`。
4. 只有项目所有者 GitHub 用户 `@WaylandYang` 可以通过 `dev` → `main` Pull Request 发布到 `main`。贡献者不得把变更分支直接提交到 `main`，也不得自行将 `dev` 合并到 `main`。

分支类型约定：

| 前缀 | 适用范围 |
| --- | --- |
| `feat/**` | 新增面向用户的能力或行为 |
| `fix/**` | Bug 修复与回归问题 |
| `docs/**` | 仅修改文档 |
| `refactor/**` | 不改变行为的代码重构 |
| `test/**` | 仅修改测试 |
| `chore/**` | 依赖、构建、CI、工具与仓库维护 |

开始开发：

```bash
git fetch origin
git switch dev
git pull --ff-only origin dev
git switch -c <类型>/<简短描述>
```

推送并创建 PR：

```bash
git push -u origin <类型>/<简短描述>
gh pr create --base dev --head <类型>/<简短描述>
```

分支描述使用小写英文和连字符，例如 `feat/review-date-filters` 或 `fix/release-version-reuse`。每个分支只处理一组内聚的改动。

禁止直接推送到 `dev` 和 `main`。仓库 CI 会校验 PR 的源分支和目标分支；当 GitHub 套餐支持私有仓库保护规则时，还应在服务端强制 PR、通过状态检查和 Code Owner 审核。

### 提交前检查

请按上文的开发环境步骤安装依赖，并运行“Required Checks”列出的后端测试、TBox 守卫、前端检查、构建和 Compose 校验。涉及抽取边界、发布清单或溯源格式的改动，还必须满足对应章节中的回归与兼容要求。

### Pull Request 要求

- 清楚描述问题、根因和改动范围。
- 列出已经运行的验证命令和结果。
- 界面改动附真实截图。
- 中英文界面文案同步更新。
- 行为或配置变化同步更新文档。
- 不得提交 `.env`、运行数据、Benchmark 缓存、生成的导出文件或任何凭据。
