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
use utopia_core::models::{AlertView, Role, User};
use utopia_core::AppResult;
use uuid::Uuid;

/// 告警 kind。**字符串写死在这里而不是散在调用点**：界面按它查措辞，
/// 拼错一个字母就会退回显示代号，而这种错编译期看不出来。
pub mod kind {
    /// 库级：某个来源同步失败。`min_role = editor`
    pub const SOURCE_SYNC_FAILED: &str = "source.sync_failed";
    /// 系统级：模型端点没给出可用的回答——连不上，或者连上了但回来的不是这个 API。
    /// 端点干净地回 4xx 不算：那说明它就是模型 API，只是密钥或配额不对
    pub const LLM_UNREACHABLE: &str = "llm.unreachable";
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

/// 一页告警，外加符合条件的总数。
pub struct AlertPage {
    pub items: Vec<AlertView>,
    /// 满足过滤条件的总数，不受 limit/offset 影响——翻页控件要靠它
    pub total: i64,
}

/// 这个人能看见的告警，一页，**按时间倒序**。
///
/// 只有这一种排序。分组是界面的事：面板把连着的同 `(kb, kind)` 折成一条
/// 「12 份文档没有文本层」。服务端不分组，因为分组一旦进了分页就得按组分页，
/// 而那要么把一整组的明细全发过来，要么再开一个"取某组明细"的端点——
/// 为了一个折叠效果不值得。
///
/// 可见性判定**一次做完**：系统级看 `is_admin`，库级看 `visible_kb_roles`
/// 给出的有效角色够不够 `min_role`。推送那一路不判权限（它不带数据），
/// 全部落在这里一处。
pub async fn list_for_user(
    pool: &PgPool,
    user: &User,
    q: Option<&str>,
    limit: i64,
    offset: i64,
) -> AppResult<AlertPage> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    // 空串当没搜：前端清空搜索框时不该变成"搜一个空字符串"
    let q = q.map(str::trim).filter(|s| !s.is_empty());
    let where_ = format!("({VISIBLE}) AND ({SEARCH})");
    let sql = format!(
        "SELECT a.id, a.kb_id, k.name AS kb_name, a.severity, a.kind,
                a.subject_type, a.subject_id, a.detail, a.created_at,
                (r.user_id IS NOT NULL) AS read
         FROM alerts a
         LEFT JOIN knowledge_bases k ON k.id = a.kb_id
         LEFT JOIN alert_reads r ON r.alert_id = a.id AND r.user_id = $1
         WHERE {where_}
         ORDER BY a.created_at DESC
         LIMIT $6 OFFSET $7"
    );
    let items: Vec<AlertView> = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .bind(q)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    // 总数单独查：翻页控件要知道有几页，而 LIMIT 之后就数不出来了。
    // 条件必须跟上面一模一样，所以 where_ 是同一个字符串
    let count_sql = format!(
        "SELECT count(*) FROM alerts a
         LEFT JOIN knowledge_bases k ON k.id = a.kb_id
         WHERE {where_}"
    );
    let (total,): (i64,) = sqlx::query_as(&count_sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .bind(q)
        .fetch_one(pool)
        .await?;
    Ok(AlertPage { items, total })
}

/// 某一条在不在这个人的可见范围内。标记已读之前查一次——
/// 不能让人靠猜 id 在 `alert_reads` 里留下一条他本不该有的记录。
pub async fn is_visible(pool: &PgPool, user: &User, alert_id: Uuid) -> AppResult<bool> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    let sql = format!("SELECT 1 FROM alerts a WHERE a.id = $5 AND ({VISIBLE})");
    let row: Option<(i32,)> = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .bind(alert_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
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

/// 标记已读。行写完不再变，所以已读也是一次性的：读过就是读过。
pub async fn mark_read(pool: &PgPool, alert_id: Uuid, user_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO alert_reads (alert_id, user_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(alert_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
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
