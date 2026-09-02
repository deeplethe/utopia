//! 告警中心（0005）。**Review 管知识的对错，告警管系统的死活。**
//!
//! **一次故障一行，写完不再改。** 这张表刻意没有状态机：没有"已解决"，
//! 没有自愈，没有把多次故障并成一行。
//!
//! 曾经有过，代价是每种新告警都得自己实现一遍"怎么算修好了"——
//! `source.sync_failed` 有天然的成功信号（同步成功了），`llm.unreachable` 没有，
//! 就得为它单独造一个后台探针；第三种告警要造第三套，而漏写清除编译期看不出来，
//! 症状是告警永远亮着。更根本的是**那不是告警中心该回答的问题**：现在还坏不坏，
//! 来源页面上写着、文档状态里写着。告警的职责是让人去看一眼，不是当实时看板。
//!
//! 去重也不做。「同一个源连续 24 小时每小时失败」是 24 条不是 1 条——
//! 要写成 1 条就得判断"这是复发还是一直没好"，而那**在数据上无法区分**，
//! 除非引入时钟或者成功信号。漏报比行数贵得多，所以宁可多写行，
//! 靠 [`purge_older_than`] 收尾。

use sqlx::PgPool;
use utopia_core::models::{Role, User};
use utopia_core::AppResult;
use uuid::Uuid;

/// 告警 kind。**字符串写死在这里而不是散在调用点**：界面按它查措辞，
/// 拼错一个字母就会退回显示代号，而这种错编译期看不出来。
pub mod kind {
    /// 库级：某个来源同步失败。`min_role = editor`
    pub const SOURCE_SYNC_FAILED: &str = "source.sync_failed";
    /// 系统级：模型端点没给出可用的回答——连不上，或者连上了但回来的不是这个 API。
    /// 端点干净地回 4xx 不算：那说明它就是模型 API，只是密钥或配额不对。
    /// 配额那一种见 [`LLM_RATE_LIMITED`]
    pub const LLM_UNREACHABLE: &str = "llm.unreachable";
    /// 系统级：端点在限流，退避重试用尽后仍然过不去。`min_role = admin`
    ///
    /// **与 [`LLM_UNREACHABLE`] 分开，因为该做的事不同**：端点不可达要去查
    /// 网络或地址，配额打满要去降并发或升档，找的人和动作都不一样。
    ///
    /// severity 是 `warning` 不是 `error`：配额会自己恢复，端点挂了不会。
    ///
    /// 它补的是「退避重试」留下的那一半。重试之后不再丢数据，但一篇文档
    /// 真的被配额挡在外面时，没有这条告警就没有任何人知道——
    /// 实测一次跑测里 4 篇失败，一半是不可达（有告警），一半是限流（静默）。
    pub const LLM_RATE_LIMITED: &str = "llm.rate_limited";
    /// 系统级：账号付不起请求——欠费或套餐配额用尽。`min_role = admin`
    ///
    /// **`error` 而不是 `warning`，正因为它跟限流的区别是「会不会自己好」**：
    /// 配额到点重置，欠费不会。在有人去充值之前，这个部署的抽取与向量化
    /// 一直是停的。
    pub const LLM_OUT_OF_CREDIT: &str = "llm.out_of_credit";
    /// 库级：数据源挂上了，它的库表结构却没摄进来。`min_role = admin`
    ///
    /// **这一条描述的不是那次失败，是它留下的状态**：源挂着，而问数看不见它有
    /// 哪些表——`query_data` 照样入列，模型却只能瞎猜列名。挂载那一刻的报错
    /// 只有点按钮的人看得见，此后这个库就一直这样静默地缺着。
    pub const SCHEMA_SYNC_FAILED: &str = "data_source.schema_sync_failed";
    /// 库级：映射探索跑完了，一条口径都没提出来。`severity = info`，`min_role = editor`——
    /// 不是故障，是"你在等的那件事没有结果"，而页面上没有别的地方能说这句话（#223）
    pub const MAPPING_EXPLORATION_EMPTY: &str = "mapping.exploration_empty";
}

/// 一次故障。打包成结构体不只是为了参数个数——调用点写 `severity: "error"`
/// 比"第三个位置参数是 error"好读得多，而告警源只会越来越多。
pub struct NewAlert<'a> {
    /// None = 系统级
    pub kb_id: Option<Uuid>,
    pub severity: &'a str,
    /// 用 [`kind`] 里的常量，别写字面量
    pub kind: &'a str,
    pub min_role: Role,
    /// document / source / system
    pub subject_type: Option<&'a str>,
    pub subject_id: Option<Uuid>,
    /// 给人看的那份：名字、报错原文。**名字要在这里存一份**——
    /// 对象被删之后 `subject_id` 就解析不出名字了，而告警该留得住
    pub detail: serde_json::Value,
}

/// 记一次故障。就是一条 INSERT，没有冲突处理，没有回读。
pub async fn raise(pool: &PgPool, a: NewAlert<'_>) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO alerts
             (id, kb_id, severity, kind, min_role, subject_type, subject_id, detail)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(a.kb_id)
    .bind(a.severity)
    .bind(a.kind)
    .bind(a.min_role.as_str())
    .bind(a.subject_type)
    .bind(a.subject_id)
    .bind(a.detail)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

/// 保留期清理。**纯原子的代价就在这里**：一个坏掉的来源按小时同步，
/// 一天写 24 行；不清理这张表会长成第二个日志文件。
///
/// 一并删掉已读记录（外键 CASCADE）。
pub async fn purge_older_than(pool: &PgPool, days: i32) -> AppResult<u64> {
    let n = sqlx::query("DELETE FROM alerts WHERE created_at < now() - make_interval(days => $1)")
        .bind(days)
        .execute(pool)
        .await?;
    Ok(n.rows_affected())
}

/// 可见性谓词，**只写这一份**。列表、未读数、全部已读各查各的，
/// 但"谁能看见什么"是同一条规则；抄三遍迟早会漂。
///
/// `$1` = user_id、`$2` = is_admin、`$3` = 可见 kb 数组、`$4` = 对应角色的秩。
const VISIBLE: &str = "
    CASE
        WHEN a.kb_id IS NULL THEN $2::bool
        ELSE EXISTS (
            SELECT 1 FROM unnest($3::uuid[], $4::int[]) AS v(kb, rank)
            WHERE v.kb = a.kb_id
              AND v.rank >= CASE a.min_role
                    WHEN 'viewer' THEN 0
                    WHEN 'editor' THEN 1
                    WHEN 'admin'  THEN 2
                    ELSE 3 END)
    END";

/// 搜索匹配的是**库名、对象详情、kind 代号**，不是界面上那句标题。
///
/// 标题的措辞在客户端（0004：服务端不产出展示文案），所以服务端搜不到它。
/// 这不是将就：人会去搜的是来源名、库名、报错原文——那些语言中立、
/// 而且就在 detail 里。按类别找东西该用筛选，不是搜索框。
const SEARCH: &str = "
    ($5::text IS NULL
     OR a.kind ILIKE '%' || $5 || '%'
     OR COALESCE(k.name, '') ILIKE '%' || $5 || '%'
     OR a.detail::text ILIKE '%' || $5 || '%')";

/// 一组：**连着的**、同 `(kb, kind)` 的几次故障。
///
/// 存储是原子的（一次故障一行），折叠只影响读。分组在服务端做而不是前端，
/// 是因为**分页得按组分**：前端折叠的话一页只能取固定行数，一段连续故障
/// 跨了页边界就会断成两组，点一下也只标到边界为止。
#[derive(sqlx::FromRow)]
pub struct AlertGroup {
    pub kb_id: Option<Uuid>,
    pub kb_name: Option<String>,
    pub kind: String,
    /// 组里最重的那一档
    pub severity: String,
    /// 这一组几次
    pub count: i64,
    /// 其中我没读过的几次
    pub unread: i64,
    /// 组里最新与最早的时刻。**标已读按这个区间圈**，不用把 id 列表发给前端——
    /// 一组可能有几百条
    pub latest_at: chrono::DateTime<chrono::Utc>,
    pub earliest_at: chrono::DateTime<chrono::Utc>,
    /// 明细，最多 [`GROUP_LINES`] 条，新的在前
    pub lines: Vec<serde_json::Value>,
}

/// 一组里最多带回几条明细。面板里列不下更多，而一组可能有几百条——
/// 全发过来只是让首屏更慢
const GROUP_LINES: i64 = 5;

/// 一页分组，外加总组数。
pub struct GroupPage {
    pub items: Vec<AlertGroup>,
    pub total: i64,
}

/// 相邻同类折叠：两个 `row_number()` 相减（gaps and islands）。
///
/// 全局序号减去"同 (kb, kind) 内的序号"，连着的同类会得到同一个差值，
/// 中间插进别的故障就会让差值变化——于是差值就是组号。
/// `PARTITION BY kb_id` 把 NULL 当作相等，所以系统级告警自然归成一组。
const ISLANDS: &str = "
    SELECT v.*,
           row_number() OVER (ORDER BY v.created_at DESC, v.id DESC)
         - row_number() OVER (PARTITION BY v.kb_id, v.kind
                              ORDER BY v.created_at DESC, v.id DESC) AS grp
    FROM v";

/// 这个人能看见的告警，**按组分页**，时间倒序。
pub async fn list_groups(
    pool: &PgPool,
    user: &User,
    q: Option<&str>,
    limit: i64,
    offset: i64,
) -> AppResult<GroupPage> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    // 空串当没搜：前端清空搜索框时不该变成"搜一个空字符串"
    let q = q.map(str::trim).filter(|s| !s.is_empty());
    let base = format!(
        "WITH v AS (
             SELECT a.id, a.kb_id, k.name AS kb_name, a.severity, a.kind,
                    a.detail, a.created_at, (r.user_id IS NOT NULL) AS read
             FROM alerts a
             LEFT JOIN knowledge_bases k ON k.id = a.kb_id
             LEFT JOIN alert_reads r ON r.alert_id = a.id AND r.user_id = $1
             WHERE ({VISIBLE}) AND ({SEARCH})
         ),
         isl AS ({ISLANDS})"
    );
    let sql = format!(
        "{base}
         SELECT kb_id, max(kb_name) AS kb_name, kind,
                -- severity 按轻重取最大，不能按字典序：那样 warning 会压过 error
                CASE max(CASE severity WHEN 'error' THEN 3 WHEN 'warning' THEN 2 ELSE 1 END)
                    WHEN 3 THEN 'error' WHEN 2 THEN 'warning' ELSE 'info' END AS severity,
                count(*) AS count,
                count(*) FILTER (WHERE NOT read) AS unread,
                max(created_at) AS latest_at,
                min(created_at) AS earliest_at,
                (array_agg(detail ORDER BY created_at DESC))[1:{GROUP_LINES}] AS lines
         FROM isl
         GROUP BY kb_id, kind, grp
         ORDER BY max(created_at) DESC
         LIMIT $6 OFFSET $7"
    );
    let items: Vec<AlertGroup> = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .bind(q)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    // 总**组**数，不是总行数——翻页控件数的是组
    let count_sql =
        format!("{base} SELECT count(*) FROM (SELECT 1 FROM isl GROUP BY kb_id, kind, grp) g");
    let (total,): (i64,) = sqlx::query_as(&count_sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .bind(q)
        .fetch_one(pool)
        .await?;
    Ok(GroupPage { items, total })
}

/// 把一整组标已读。**按时间区间圈**，不按 id 列表——一组可能有几百条，
/// 把 id 全发给前端再发回来只是白跑一趟。
///
/// 可见性照查：不能让人靠猜 kind 把看不见的告警标掉。
pub async fn mark_group_read(
    pool: &PgPool,
    user: &User,
    kb_id: Option<Uuid>,
    kind: &str,
    from: chrono::DateTime<chrono::Utc>,
    to: chrono::DateTime<chrono::Utc>,
) -> AppResult<u64> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    let sql = format!(
        "INSERT INTO alert_reads (alert_id, user_id)
         SELECT a.id, $1 FROM alerts a
         WHERE ({VISIBLE})
           AND a.kind = $5
           -- IS NOT DISTINCT FROM：系统级告警的 kb_id 是 NULL，= 比不出来
           AND a.kb_id IS NOT DISTINCT FROM $6
           AND a.created_at BETWEEN $7 AND $8
         ON CONFLICT DO NOTHING"
    );
    let n = sqlx::query(&sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .bind(kind)
        .bind(kb_id)
        .bind(from)
        .bind(to)
        .execute(pool)
        .await?;
    Ok(n.rows_affected())
}

/// 我的未读数。
pub async fn unread_count(pool: &PgPool, user: &User) -> AppResult<i64> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    let sql = format!(
        "SELECT count(*) FROM alerts a
         LEFT JOIN alert_reads r ON r.alert_id = a.id AND r.user_id = $1
         WHERE ({VISIBLE}) AND r.user_id IS NULL"
    );
    let (n,): (i64,) = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// 把我能看见的全标已读。
///
/// 一条 SQL 而不是逐条 insert：逐条要先把列表取回来，而"能看见什么"
/// 已经在 [`VISIBLE`] 里写过一遍了，取回来再遍历等于把同一条规则用两次。
pub async fn mark_all_read(pool: &PgPool, user: &User) -> AppResult<u64> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    let sql = format!(
        "INSERT INTO alert_reads (alert_id, user_id)
         SELECT a.id, $1 FROM alerts a
         WHERE ({VISIBLE})
         ON CONFLICT DO NOTHING"
    );
    let n = sqlx::query(&sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .execute(pool)
        .await?;
    Ok(n.rows_affected())
}

/// 可见 KB 拆成两个平行数组：Postgres 没有元组数组的方便写法，
/// `unnest(a, b)` 两列并排展开是标准做法。
async fn visible(pool: &PgPool, user: &User) -> AppResult<(Vec<Uuid>, Vec<i32>)> {
    let roles = crate::access::visible_kb_roles(pool, user).await?;
    Ok(roles.into_iter().map(|(id, r)| (id, rank(r))).unzip())
}

/// 角色的序，用来跟 `alerts.min_role` 比大小。
/// 跟 [`Role`] 的 `PartialOrd` 同序，也跟 [`VISIBLE`] 里那个 CASE 同序——
/// 三处必须一致
fn rank(r: Role) -> i32 {
    match r {
        Role::Viewer => 0,
        Role::Editor => 1,
        Role::Admin => 2,
        Role::Owner => 3,
    }
}
