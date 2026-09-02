# 参与 Utopia

[English](CONTRIBUTING.md)

欢迎。这份文件只写这个仓库特有的规矩 —— 通用的开源礼仪就不重复了。

## 分支与合并流程

| 分支 | 是什么 |
|---|---|
| `main` | 稳定分支，对应已发布的版本。只由仓库管理者从 `dev` 合入 |
| `dev` | 集成分支，所有贡献先到这里 |

贡献者的路径：

```bash
git switch dev && git pull
git switch -c fix/some-thing        # 从 dev 开新分支，不要从 main
# 改代码，提交时带 -s（见下面 DCO）
git push -u origin fix/some-thing
```

然后开 PR，**base 选 `dev`，不是 `main`**。CI 通过、review 通过之后由维护者合并。

`dev → main` 的合并由仓库管理者择期发起，贡献者不需要管。有两条规矩防止两条分支越走越远，都是给管理者的：

- **发版之后紧接着把 `main` 回合进 `dev`。** `dev → main` 那个 merge 节点只存在于 `main`，不回合的话 `main` 会显示成领先，尽管两边文件一模一样，而且每发一版就多领先一个。
- **紧急修复也走 `dev`。** 直接对 `main` 开 PR 是唯一会让两条分支真正分叉的口子，分叉之后就得有人手工对账。

两个分支都开了保护：必须走 PR，CI（`backend` 与 `web`）必须通过，禁止强推与删除，管理员同样受约束。

## 先开 issue 还是直接提 PR

| 改动 | 怎么做 |
|---|---|
| Bug 修复、文档、i18n 文案、测试 | 直接提 PR |
| 新功能、依赖变更 | 先开 [issue](https://github.com/deeplethe/utopia/issues) 说清场景 |
| 动数据模型、本体契约、公开 API | 先开 issue 讨论，落一篇 [ADR](docs/decisions/) 再动手 |

`docs/decisions/` 是这个项目主要的决策载体。里面记的不是「改了什么」，是**「为什么这样而不是那样」**，以及当时试过、失败了的做法。改动够大时，那篇文档比代码本身更值钱。

## 起本地环境

依赖：Docker、Rust 1.85+、Node 20+、pnpm。

```bash
docker compose up -d db                 # pgvector 版 Postgres
cargo run -p utopia-server              # 自动跑迁移，:1516
cd web && pnpm install && pnpm dev      # :5173，/api 代理到后端
```

## 提交前跑什么

CI 就是下面这几条，本地过了 CI 基本不会红：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd web && pnpm install --frozen-lockfile && pnpm build   # build 含类型检查
```

### 带库的测试没设环境变量会**跳过**，不是失败

这是这个仓库最容易误判的一点。`cargo test --workspace` 全绿不等于全跑了 —— 一批测试开头长这样：

```rust
let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
    eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
    return Ok(());
};
```

它们守的是**编译器看不见的东西**：SQL 里的表别名、`NULL` 参与比较时的行为、`INNER JOIN` 悄悄滤掉的行、递归 CTE 在菱形继承下会不会把同一个祖先展开两次。`cargo check` 和 clippy 对这些一个字都不说。

碰了 `crates/utopia-store/` 里的 SQL，请把它设上再跑一遍：

```bash
export UTOPIA_DATABASE_URL=postgres://utopia:utopia@localhost:5432/utopia
cargo test --workspace
```

## 几条会被 review 拦下来的

**迁移编号别撞。** `migrations/` 按序号前滚。开 PR 前看一眼 `main` 上最新的号 —— 两个分支各写一个 `0011_` 已经发生过一次，合并之后谁都跑不起来。

**UI 文案进 i18n。** `web/src/i18n/en.ts` 与 `zh.ts` 两边都要加，不要在组件里硬编码字符串。

**代码注释写「为什么」。** 这个仓库的注释密度偏高，而且刻意记录踩过的坑（「第一版写的是『或』，结果 Elon Musk 那篇每 6KB 就取一张」）。跟着这个风格走 —— 复述代码在做什么的注释会被要求删掉。

**提交信息一句英文，说清动机。** 不写长 body。看一眼 `git log` 就知道调子。

**每个 workflow 自己声明 `permissions:`。** 仓库默认现在是读写——有一个 workflow 要把生成的图提交回来。不写 `permissions:` 块的 workflow 会继承那个默认，于是一个只需要读的任务悄悄拿到了写权限。按这个任务实际需要的最小集写：只做构建或测试的，写 `contents: read`。

## DCO：每个提交要签

我们用 [DCO](https://developercertificate.org/)（开发者原创声明），不用 CLA。你保留自己代码的著作权，只是声明你有权按 Apache-2.0 提交它。

用 `-s` 提交即可，git 会自动加上署名行：

```bash
git commit -s -m "Fix the thing"
```

提交末尾会多出：

```
Signed-off-by: 你的名字 <你的邮箱>
```

忘了签的话，最后一个提交用 `git commit --amend -s`，多个提交用 `git rebase --signoff HEAD~3`（数字换成实际条数），然后 `git push -f`。

署名用的名字和邮箱要是真实可联系的。

## 许可

提交即表示你的贡献按 [Apache-2.0](LICENSE) 发布。
