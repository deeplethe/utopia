# 0005 · 告警中心

- **状态**：规划中 · 未动工
- **成文**：2026-08-29
- **相关**：与 Review 队列（0001 P4）职责相邻，本文划边界

---

## 为什么

失败状态目前散在六处，各有各的字段，没有一个地方能一眼看全：

| 位置 | 记的是 |
|---|---|
| `jobs.status='failed'` + `jobs.last_error` | 任务失败 —— **没有任何界面** |
| `documents.status='failed'` | 摄入失败 |
| `documents.graph_status='failed'` | 抽取失败 |
| `sources.last_sync_status='failed'` | 源的最近一次同步失败 |
| `source_sync_runs.status='failed'` | 单次同步失败 |
| 日志 | 其余一切 |

而且这个问题**已经被局部修过一次**。迁移 `0021_graph_error.sql` 的第一句是：

> 抽取失败的原因此前只进日志与 `jobs.last_error`，文档上什么都不留，界面无从显示。

那次的修法是往 `documents` 上加错误字段。**这是打补丁**：每出现一类新的失败，就往对应的表上加一列。推演层、执行层、OCR 端点、湖仓连接都还没进来，每一个都会带来自己的失败面。照这个路子走下去，会有十几个错误字段散在十几张表上，而用户仍然没有"现在系统哪里不对"的入口。

真正伤人的不是失败本身，是**失败无声**。拖 100 份 PDF 进去、其中 12 份是扫描件，界面上 100 份全绿 —— 用户以为都进去了，直到某天问一个问题、答案里没有那份合同，而他不会想到去怀疑摄入环节。

---

## 边界：与 Review 队列的分工

两者都是"待处理的列表"，不划清会互相蚕食。

| | Review 队列 | 告警中心 |
|---|---|---|
| 性质 | 需要人**做决定** | 需要人**知道** |
| 例子 | 这两个实体合不合并、这条低置信事实要不要收 | 这份文档没进去、源连不上、端点挂了 |
| 不处理的后果 | 知识停在半路 | **你以为进去了，其实没有** |
| 谁产生 | 抽取与消解在拿不准时主动入队 | 任何执行路径在失败时上报 |

一句话：**Review 管知识的对错，告警管系统的死活。**

---

## 三个决定

### 1. 聚合：同一类未解决的只有一条

拖 100 份 PDF、12 份是扫描件，产生的是**一条**「12 份文档没有文本层」，点开看列表 —— 不是 12 条。同理「Notion 源连续 3 次同步失败」是一条，`last_seen` 往前推。

没有聚合的告警中心，两周后就没人看了。这不是优化，是能不能用的前提。

### 2. 自愈优先于人工关闭

`resolved_at` 由**产生方**清空：配好 OCR 端点、那 12 份重新处理成功，告警自己消失。人工"标记已解决"只是兜底。

做不到自愈的告警，用户很快学会无视它 —— 一旦养成无视的习惯，这个功能就废了，而且是不可逆的。

### 3. 读是各人的，解决是共享的

**这条推翻了一个更省事的方案，理由值得记下来。**

被否决的方案：一条告警被任何一个管理员点开，就对所有人标记已读。看起来省了重复劳动。

失效方式：

> 知识库有三个管理员。A 早上顺手点开看了一眼，没处理。这条告警从 B、C 的未读列表里**永远消失**了 —— 他们不知道曾经发生过这件事，而 A 想着"等会儿再说"。

这是共享已读的经典失效：**所有人都以为别人在处理**，而且发生之后没有任何痕迹能让人发现漏了。

根子在于把两件事合并了：**「已读」是我看没看过，「已解决」是事情完没完**。一个人读过不代表事情解决；而事情解决了，才是所有人都该从列表里移除它的时刻。

所以：

- **未读角标** = 我可见的、未解决的告警中我没读过的条数 —— 每人独立
- **告警消失** = `resolved_at` 落下，对所有人同时消失 —— 全局共享

代价只有一张两列小表。换来的是**没有人能替别人把一件事读掉**。GitHub 通知、Slack 未读都这么做，不是巧合。

---

## 可见性

`kb_id` 可空，两种语义：

```
kb_id IS NULL   系统级：LLM 端点不可达、数据库连接池打满、镜像版本不一致
                → 仅 users.is_admin

kb_id 有值      知识库级：解析失败、抽取失败、源同步失败
                → 该库中角色 ≥ min_role 的人
```

**不新写权限逻辑**：`access::kb_role()` 第一句就是 `if user.is_admin { return Ok(Some(Role::Owner)) }`，告警的可见性判定直接复用它，和 KB 路由走同一条鉴权链。系统管理员因此对知识库级告警也全通。

`min_role` 存在 alert 上而不是按 kind 硬编码，因为同一类告警在不同场景下该找的人不同：

- **配置类**（端点、权限、配额）→ admin
- **内容类**（解析失败、抽取失败、源同步）→ **editor 及以上**

内容类不能只给 admin：拿扫描件举例，admin 需要知道"该配 OCR 了"，但**上传那 12 份文件的人**更需要知道"你传的东西没进去"。只给 admin 的话，真正被影响的人反而看不见。

---

## 数据模型草案

```sql
CREATE TABLE alerts (
    id           UUID PRIMARY KEY,
    kb_id        UUID REFERENCES knowledge_bases(id) ON DELETE CASCADE,  -- NULL = 系统级
    severity     TEXT NOT NULL CHECK (severity IN ('info','warning','error')),
    kind         TEXT NOT NULL,   -- 'source.sync_failed' / 'llm.unreachable' / ...
    min_role     TEXT NOT NULL,   -- 见「可见性」
    subject_type TEXT,            -- document / source / system
    subject_ids  UUID[],          -- 聚合：同一 kind 下的所有对象
    detail       JSONB NOT NULL DEFAULT '{}',
    first_seen   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen    TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at  TIMESTAMPTZ
);

-- 聚合的实现关键：同类未解决的告警在库里只能有一条。
-- 新实例往 subject_ids 追加、更新 last_seen，而不是插新行。
CREATE UNIQUE INDEX alerts_open_kind_idx ON alerts (kb_id, kind)
    WHERE resolved_at IS NULL;

CREATE TABLE alert_reads (
    alert_id UUID NOT NULL REFERENCES alerts(id) ON DELETE CASCADE,
    user_id  UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    read_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (alert_id, user_id)
);
```

`kb_id IS NULL` 时那个部分唯一索引不生效（NULL 不参与唯一性判定），系统级告警的去重需要单独处理 —— 实现时用 `COALESCE(kb_id, '00000000-...')` 或一个独立的部分索引。**这是个已知的坑，先记在这里。**

**推送**复用现有的 `AppEvent` broadcast（当前只有 `document` / `review` 两种 kind，加一个 `alert`），SSE 通道是现成的。

---

## 第一刀的范围

**不先建空框架。**

迁移是最难回头的部分。空表建好、发布了，等真接入时发现 schema 不够用（聚合该用数组还是关联表、`min_role` 该在行上还是按 kind 硬编码），改迁移就得走升级路径。而现在还没打 tag，迁移可以推倒重来 —— 这个窗口不该浪费在一张没有数据流过的表上。

更要紧的是：**聚合、自愈、per-user 已读这三件最容易设计错的事，只有真有数据经过才验得出来。** 空框架把它们全留到了以后。

所以第一刀接**两条真实告警源**，各验一条权限路径，且都不需要新的检测逻辑：

| kind | 级别 | 验证什么 |
|---|---|---|
| `source.sync_failed` | KB 级 | 聚合（同源连续失败仍是一条）、自愈（下次成功即消失）、`min_role=editor` |
| `llm.unreachable` | 系统级 | `kb_id IS NULL` 那条去重路径、`is_admin` 可见性 |

两条加起来把 schema 的每个字段都跑过一遍真实数据。**如果设计有错，这一刀就会暴露**，而此时改迁移零成本。

UI 第一版可以粗：顶栏角标 + 一个列表页，能点进出问题的对象即可。

### 留到第二批

**`document.no_text_layer`（扫描件）** —— 它需要先写检测逻辑（判断解析结果为空），而且真正有用是在有了 OCR 端点之后：那时提示语才完整 ——「12 份文档没有文本层，配置 OCR 端点后可重新处理」，同时是错误说明和功能引导。

---

## 被否决的方案

**复用 `audit_events`。** 那张表是"谁做了什么"的台账，性质是不可变记录；告警是"出了什么事"，需要状态流转（未读 → 已读 → 已解决）和聚合。塞进同一张表会让台账不再是台账。

**不建表，把六处失败状态 union 起来查询展示。** 零迁移、零新概念，但存不下 per-user 已读，也表达不了自愈语义 —— 而那两件正是这个功能的核心价值。

**共享已读**。见上文「三个决定」第 3 条。
