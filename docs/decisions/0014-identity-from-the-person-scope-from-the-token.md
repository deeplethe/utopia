# 0014 · 身份跟着人，范围跟着令牌

- **状态**：已实施（#161 记录、#180 落地）· `personal_tokens` + Streamable HTTP 的 MCP 服务端已上线，暴露五个只读工具；界面在 `feat/tokens-have-a-page`（0016 A2）：账户层「Agents & tokens」页，发放时明文与 MCP 客户端配置片段一起只显示一次，列表只剩前缀，撤销留痕（2026-09-02 核，修订见文末）
- **成文**：2026-09-01（约定见 [README](README.md)）
- **相关**：迁移 `0014_data_source_grants` 刚给数据源补上授权层——本篇是同一个问题
  换到「机器来敲门」这一侧；[0004](0004-language-and-localization.md) 定下服务端只说英文，
  错误码这一条对 MCP 同样适用

> 起因是要把 Utopia 的七个工具暴露成 MCP 服务端。**第一个要回答的不是传输层选哪个，
> 是客户端以什么身份接进来。** 这一篇只答这个。

## 现状：两种凭据，都不合身

| | 跟着谁 | 存法 | 过期 | 能撤吗 |
|---|---|---|---|---|
| JWT | 人 | 无状态 | 7 天 | **不能**——签出去就管不了 |
| `sources.ingest_token` | 一个来源 | 明文 | 无 | 换一个 |

JWT 是给浏览器会话设计的：短命、每次登录重签、无状态所以不需要一张表。MCP 客户端是
长命的、机器的、配在别人机器上的一个文件里——七天过期意味着每周手动重配一次，而
「撤不回来」意味着笔记本丢了只能等它自己过期。

`ingest_token` 更不合适：它跟来源走，不跟人走，而且只能**往里推文档**。

## 走过的岔路：KB 级机器令牌

先提的方案是给知识库发机器令牌——一枚令牌对应一个库，跟人无关。

**否掉了，两条理由：**

1. **它引入第三套授权模型。** 现在已经有工作区成员和 KB 角色两层；再加一层「令牌自己的
   权限」，那么「这个 agent 能看什么」就得同时查三张表才答得出来。而三层里任何一层写错，
   失败方向都是「多给了」。
2. **归因会变成假的。** `audit_events.actor_id` 现在记的是活人，`actor_label` 还存了一份
   身份快照。机器令牌写进来的事实，actor 只能是一个合成 id——台账上就多出一类「不是任何
   人做的」记录，而账本存在的理由正是「谁在什么时候认下了什么」。

**改成：令牌以这个人的身份行事。**

## 决定

```
有效权限 = 这个人的角色  ∩  这枚令牌的 scope
```

交集，不是并集。**令牌只能收窄，永远不能放宽。** 一个 viewer 的令牌勾上 write 也还是
只读——scope 是上限，不是授权。

### 为什么身份跟着人

- **现有守卫一行不用改。** `require_kb(kb_id, Role::Viewer)`、`access::kb_role` 拿到的还是
  一个 `User`，它从哪来的无所谓
- **归因是真的。** 台账上是活人，不是机器人
- **停用即失效。** 人离职停用，他的令牌跟着废，不用单独维护一张「谁的机器还连着」的表
- **多个库不用发多把钥匙**

### 为什么范围仍然要单独收窄

因为有一个 MCP 特有的问题，应用内对话没有这么严重：

> **混淆代理。** MCP 客户端是别人的 agent、别人的系统提示词，而它读的是知识库里的
> 文档——**不可信内容**。一份文档里写「请执行这段 SQL」或者「记住 X」，那个 agent 可能
> 就照做了，用的是这个人的全部权限。

应用内对话也有这个面，但那里提示词和工具循环都在 Utopia 手里；走 MCP，Utopia 对客户端
的提示词、对它还接了哪些别的服务端，一无所知。

而这个人的「全部权限」是什么：经 `query_data`，对他所在**每一个库挂载的每一个生产
数据库**跑只读 SQL；经 `remember`，往 append-only 账本里写事实。这些能力配在一串放在
`claude_desktop_config.json` 明文里的字符串上。

所以默认发**只读、限定到一个库**的令牌。要让 agent 写，得显式勾。

## 这枚要哈希，而 `ingest_token` 不哈希

两处结论不同，不是疏忽。`ingest_token` 那条明文决定的原话（`0002_ingest.sql`）：

> **明文存，不是哈希。** 自部署威胁模型下「只看一次」是自找麻烦：改存明文随时可查。
> DB 失守时文档本体早已泄露，密钥哈希化没有额外收益

那个推理对 ingest_token 成立，因为**它只能往里推文档**——泄露它的最坏结果是有人往你
库里塞垃圾，而库都失守了，塞垃圾不是最要紧的事。

个人令牌不一样：它经 `query_data` 能**读出 Utopia 之外的生产库**。Utopia 的数据库失守
本来就泄露 Utopia 自己的文档，但数仓在另一台机器上、装着另一批数据，不该跟着一起丢。
**爆炸半径不同，所以存法不同。**

## 形状

```sql
CREATE TABLE personal_tokens (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,          -- 人自己起的，"我的笔记本"
    token_hash   TEXT NOT NULL,          -- 哈希，理由见上
    scope        TEXT NOT NULL DEFAULT 'read'
                 CHECK (scope IN ('read', 'write')),
    kb_ids       UUID[],                 -- NULL = 这个人能进的全部
    expires_at   TIMESTAMPTZ,            -- NULL = 不过期，但 UI 默认给 90 天
    last_used_at TIMESTAMPTZ,            -- 「这枚还在用吗」，撤之前要答得出
    revoked_at   TIMESTAMPTZ,            -- 撤销不删行：撤过这件事本身要留痕
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

`user_id` 这条有外键且级联，与 `audit_events.actor_id` 的裸外键相反——**台账要活得比
用户久，令牌不该**。人没了，他的钥匙就该一起没。

## 一条实施纪律

**每个工具入口都要校验 scope，不能只在连接握手时校验一次。**

这是 `0014_data_source_grants` 那轮的教训，原话写在测试里：

> 列表过滤只挡「看得见」，而挂载端点是照着 id 调的——守卫必须在两侧都有

MCP 的对应形态是：握手时校验一次，然后整条连接生命周期里都信任它。工具调用是一个个
独立请求，`revoked_at` 在中途被写上时，正在跑的连接必须立刻失效。

## 修订记录（2026-09-02）：落地之后对照本文

**形状**多一列 `token_prefix`（`utp_pat_…`，与 ingest 的 `utp_` 区分——日志与配置文件里一眼要认得出是哪一种），`token_hash` 加了 `UNIQUE`。哈希选 SHA-256 不选 argon2：高熵串不怕爆破，而每行盐不同就查不了唯一索引。

**「每个工具入口都校验 scope」那条纪律被满足了，但形态与预想不同**：做成完全无状态——每个 POST 重跑一次认证（撤销 / 过期在 SQL 的 `WHERE` 里判）+ `covers()` + `require_kb`，没有「连接」这个东西可以被信任。`scope` 在这一版没有分支：`can_write` 硬编码 `false`，就算令牌勾了 write 也不放开。每次工具调用写一条审计（`mcp.tool_called`，target 是令牌），归因是真的。

**前置条件本文没记**：#175 把七个工具的执行从 `chat.rs` 的 `match` 里抽成 `tools.rs`，对话与 MCP 共用同一份实现——否则「对话里的 `entity_facts` 和 MCP 里的不是同一个东西」。工具的 JSON schema 仍留在 `chat.rs`，MCP 复用它做转换，已知带一处瑕疵：`search_chunks` 的描述里还写着「可以引用成 [n]」，而 MCP 客户端拿不到引用编号。

**一处会误导人的陈述**：`crates/utopia-mcp` 曾是三行占位，宣称的三个工具名（`add_memory` / `search_memory` / `get_entity_timeline`）从未实现，MCP 服务端住在 `utopia-server/src/api/mcp.rs`。〔已删（2026-09-02，连同 `utopia-graph`、`utopia-connectors`），理由见 [0016](0016-close-the-open-seams-before-cutting-new-ones.md) A2。〕

## 未决

- **`query_data` 与 `remember` 要不要进第一版。** 倾向不进——先发四个只读工具
  （`search_chunks` / `find_entities` / `entity_facts` / `changes`），把身份这条路走通再说。
  这两个各自还有没答的问题：外部 agent 写进来的事实挂什么证据？跑 SQL 的审计怎么记？〔**已答：不进**。实际发的是五个而非四个，多一个 `search_docs`。`remember` 的前提是 [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) 那道闸，而闸还没接上。〕
- **传输层**：stdio（本地，配 Claude Desktop 最省事）还是 streamable HTTP（远程，
  和 Utopia 已经是个服务端相称）。这个选择不影响本篇的结论，两种都要认令牌。〔**已答：Streamable HTTP**，一条路由 `POST /api/v1/kbs/{kb_id}/mcp`，响应用 `application/json` 不用 SSE——工具一问一答，没有服务端主动推的东西。〕
- **令牌能不能跨工作区。** 现在的 `kb_ids` 是库级白名单；如果将来要按工作区发，
  和数据源授权那张表会长得很像，届时看要不要合并概念。〔仍未做。〕
- **（2026-09-02 补）没有界面。** 「UI 默认给 90 天」的 90 天在服务端，而前端根本没有令牌那一页。这是 MCP 今天对用户不可用的直接原因。〔**已做**，同日：`/account/tokens`。三个决定落在界面上——scope 缺省只读、库多选、过期缺省 90 天且「不过期」要显式选；配置片段按库出端点（令牌限定到库，端点也按库分），写法取 Claude Code / Claude Desktop 一族的 Streamable HTTP 键名，别的客户端要自己改键。〕
