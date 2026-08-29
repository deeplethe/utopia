//! 告警中心（0005）。**Review 管知识的对错，告警管系统的死活。**
//!
//! 三条不变量，任何新告警源都得遵守：
//! 1. **聚合**——同一 `(kb_id, kind)` 未解决的只有一行。12 份扫描件是一条告警。
//! 2. **自愈**——`resolved_at` 由产生方清空，不靠人点"已解决"。
//! 3. **已读逐人、解决全局**——没有人能替别人把一件事读掉。

use sqlx::PgPool;
use utopia_core::models::{Alert, AlertView, Role, User};
use utopia_core::AppResult;
use uuid::Uuid;

/// 告警 kind。**字符串写死在这里而不是散在调用点**：升报与自愈必须用同一个词，
/// 拼错一个字母的后果是告警永远关不掉，而这种错编译期看不出来。
pub mod kind {
    /// 库级：某个来源同步失败。聚合到源、`min_role = editor`
    pub const SOURCE_SYNC_FAILED: &str = "source.sync_failed";
    /// 系统级：模型端点连不上（DNS / 连接 / 超时）。
    /// 跟"端点回了 4xx"是两回事——那是密钥、配额、模型名的问题，另算一类
    pub const LLM_UNREACHABLE: &str = "llm.unreachable";
}

/// 一次上报的内容。打包成结构体不只是为了参数个数——调用点写
/// `severity: "error"` 比"第三个位置参数是 error"好读得多，而告警源只会越来越多。
pub struct NewAlert<'a> {
    /// None = 系统级
    pub kb_id: Option<Uuid>,
    pub severity: &'a str,
    /// 用 [`kind`] 里的常量，别写字面量
    pub kind: &'a str,
    pub min_role: Role,
    pub subject_type: Option<&'a str>,
    /// 聚合的那个对象。系统级告警没有对象，传 None
    pub subject: Option<Uuid>,
    /// **扁的、按 subject id 索引**——jsonb 的 `||` 是浅合并，
    /// 嵌一层的话第二个对象上报时会整个盖掉第一个的详情
    pub detail: serde_json::Value,
}

/// 上报一条。同 `(kb_id, kind)` 已有未解决的就**并进去**：追加 subject、
/// 推 `last_seen`、合并 detail，不插新行也不改 `first_seen`。
///
/// **有新对象加入时清掉这条的已读。** 不这么做的话，"3 个源失败"读过之后
/// 第 4 个源坏了没有任何信号——聚合就成了藏信息。反过来，同一个对象反复失败
/// 不清已读，否则一个抖动的源能把角标刷成常亮，而那是通往"学会无视"的路。
/// 分界线是**有没有新东西**，不是**有没有新事件**。
///
/// 返回 `(alert_id, changed)`。`changed` = 新建的，或者有新对象加入——
/// 调用方据此决定要不要推事件。
pub async fn raise(pool: &PgPool, a: NewAlert<'_>) -> AppResult<(Uuid, bool)> {
    let NewAlert {
        kb_id,
        severity,
        kind,
        min_role,
        subject_type,
        subject,
        detail,
    } = a;
    // CTE 里的 prev 看到的是语句执行**前**的快照，所以能拿到旧的 subject_ids——
    // ON CONFLICT DO UPDATE 的 RETURNING 只给得到新值。
    // xmax = 0 是"这一行是本次 INSERT 插进去的"的判据：走哪条路都会 RETURNING，
    // 光看返回值分不出新旧
    let row: (Uuid, bool, bool) = sqlx::query_as(
        "WITH prev AS (
             SELECT subject_ids FROM alerts
             WHERE kind = $4 AND resolved_at IS NULL
               AND COALESCE(kb_id, '00000000-0000-0000-0000-000000000000'::uuid)
                 = COALESCE($2, '00000000-0000-0000-0000-000000000000'::uuid)
         ),
         up AS (
             INSERT INTO alerts
                 (id, kb_id, severity, kind, min_role, subject_type, subject_ids, detail)
             VALUES ($1, $2, $3, $4, $5, $6,
                     CASE WHEN $7::uuid IS NULL THEN '{}'::uuid[] ELSE ARRAY[$7::uuid] END,
                     $8)
             ON CONFLICT (COALESCE(kb_id, '00000000-0000-0000-0000-000000000000'::uuid), kind)
                 WHERE resolved_at IS NULL
             DO UPDATE SET
                 last_seen = now(),
                 severity  = EXCLUDED.severity,
                 detail    = alerts.detail || EXCLUDED.detail,
                 -- 数组去重合并：同一个源反复失败不该在 subject_ids 里堆一串重复
                 subject_ids = ARRAY(
                     SELECT DISTINCT unnest(alerts.subject_ids || EXCLUDED.subject_ids))
             RETURNING id, (xmax = 0) AS is_new
         )
         SELECT up.id, up.is_new,
                -- `= ANY(子查询)` 会被当成集合形式，拿 uuid 去比 uuid[]。
                -- 要的是这个 id 在不在那一行的数组里，所以走 EXISTS 加数组版 ANY
                $7::uuid IS NOT NULL
                AND NOT EXISTS (SELECT 1 FROM prev WHERE $7::uuid = ANY(prev.subject_ids))
                AS subject_added
         FROM up",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(severity)
    .bind(kind)
    .bind(min_role.as_str())
    .bind(subject_type)
    .bind(subject)
    .bind(detail)
    .fetch_one(pool)
    .await?;
    let (id, is_new, subject_added) = row;
    if subject_added && !is_new {
        // 有新东西坏了，所有人都该重新看一眼
        sqlx::query("DELETE FROM alert_reads WHERE alert_id = $1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok((id, is_new || subject_added))
}

/// 某个对象好了。**不是整条告警好了**——一个库里三个源一起失败聚成一条，
/// 其中一个恢复只该把它从 `subject_ids` 里摘掉；数组空了才算解决。
///
/// 这条语义 0005 没写，但不定它聚合就是假的：要么一个恢复就把另外两个的
/// 告警一起关掉（谎报太平），要么永远关不掉（用户学会无视）。
///
/// 返回 true = 这次把整条告警解决了。
pub async fn clear_subject(
    pool: &PgPool,
    kb_id: Option<Uuid>,
    kind: &str,
    subject: Uuid,
) -> AppResult<bool> {
    let done: Option<(bool,)> = sqlx::query_as(
        "UPDATE alerts SET
             subject_ids = array_remove(subject_ids, $3),
             -- detail 也得摘掉这一条。只摘 subject_ids 的话，界面照样把
             -- 已经好了的那个源列成失败——detail 才是给人看的那一份
             detail = detail - $3::text,
             resolved_at = CASE WHEN array_remove(subject_ids, $3) = '{}'::uuid[]
                                THEN now() ELSE NULL END
         WHERE kind = $2 AND resolved_at IS NULL
           AND COALESCE(kb_id, '00000000-0000-0000-0000-000000000000'::uuid)
             = COALESCE($1, '00000000-0000-0000-0000-000000000000'::uuid)
           AND $3 = ANY(subject_ids)
         RETURNING resolved_at IS NOT NULL",
    )
    .bind(kb_id)
    .bind(kind)
    .bind(subject)
    .fetch_optional(pool)
    .await?;
    Ok(done.is_some_and(|(d,)| d))
}

/// 整条解决。给没有 subject 的告警用（系统级：端点回来了）。
pub async fn resolve(pool: &PgPool, kb_id: Option<Uuid>, kind: &str) -> AppResult<bool> {
    let n = sqlx::query(
        "UPDATE alerts SET resolved_at = now()
         WHERE kind = $2 AND resolved_at IS NULL
           AND COALESCE(kb_id, '00000000-0000-0000-0000-000000000000'::uuid)
             = COALESCE($1, '00000000-0000-0000-0000-000000000000'::uuid)",
    )
    .bind(kb_id)
    .bind(kind)
    .execute(pool)
    .await?;
    Ok(n.rows_affected() > 0)
}

/// 某个 kind 现在有没有未解决的。探针据此决定要不要跑——
/// 健康时一次网络调用都不发。
pub async fn is_open(pool: &PgPool, kb_id: Option<Uuid>, kind: &str) -> AppResult<bool> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM alerts
         WHERE kind = $2 AND resolved_at IS NULL
           AND COALESCE(kb_id, '00000000-0000-0000-0000-000000000000'::uuid)
             = COALESCE($1, '00000000-0000-0000-0000-000000000000'::uuid)",
    )
    .bind(kb_id)
    .bind(kind)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// 可见性谓词，**只写这一份**。列表与未读数各查各的，但"谁能看见什么"
/// 是同一条规则；抄两遍迟早会漂。
///
/// `$1` = user_id（这里不用，但两个查询都按同样的序号绑）、`$2` = is_admin、
/// `$3` = 可见 kb 数组、`$4` = 对应角色的秩。
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

/// 一页告警，外加符合条件的总数。
pub struct AlertPage {
    pub items: Vec<AlertView>,
    /// 满足过滤条件的总数，不受 limit/offset 影响——翻页控件要靠它
    pub total: i64,
}

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

/// 这个人能看见的告警，一页。
///
/// 可见性判定**一次做完**：系统级看 `is_admin`，库级看 `visible_kb_roles`
/// 给出的有效角色够不够 `min_role`。推送那一路不判权限（它不带数据），
/// 全部落在这里一处。
pub async fn list_for_user(
    pool: &PgPool,
    user: &User,
    include_resolved: bool,
    q: Option<&str>,
    limit: i64,
    offset: i64,
) -> AppResult<AlertPage> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    // 空串当没搜：前端清空搜索框时不该变成"搜一个空字符串"
    let q = q.map(str::trim).filter(|s| !s.is_empty());
    let where_ = format!("({VISIBLE}) AND ({SEARCH}) AND ($6::bool OR a.resolved_at IS NULL)");
    let sql = format!(
        "SELECT a.id, a.kb_id, k.name AS kb_name, a.severity, a.kind,
                a.subject_type, a.subject_ids, a.detail,
                a.first_seen, a.last_seen, a.resolved_at,
                (r.user_id IS NOT NULL) AS read
         FROM alerts a
         LEFT JOIN knowledge_bases k ON k.id = a.kb_id
         LEFT JOIN alert_reads r ON r.alert_id = a.id AND r.user_id = $1
         WHERE {where_}
         ORDER BY (a.resolved_at IS NULL) DESC, a.last_seen DESC
         LIMIT $7 OFFSET $8"
    );
    let items: Vec<AlertView> = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .bind(q)
        .bind(include_resolved)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    // 总数单独查：翻页控件要知道有几页，而 LIMIT 之后就数不出来了。
    // 条件必须跟上面一模一样，所以 where_ 是同一个字符串
    let count_sql = format!(
        "SELECT count(*) FROM alerts a
         LEFT JOIN knowledge_bases k ON k.id = a.kb_id
         LEFT JOIN alert_reads r ON r.alert_id = a.id AND r.user_id = $1
         WHERE {where_}"
    );
    let (total,): (i64,) = sqlx::query_as(&count_sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .bind(q)
        .bind(include_resolved)
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

/// 我的未读数：可见的、未解决的、我没读过的。
///
/// 单独查而不是 `list_for_user().filter()`：角标每次开页都要，
/// 而未解决的告警可以攒到几百条
pub async fn unread_count(pool: &PgPool, user: &User) -> AppResult<i64> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    let sql = format!(
        "SELECT count(*) FROM alerts a
         LEFT JOIN alert_reads r ON r.alert_id = a.id AND r.user_id = $1
         WHERE ({VISIBLE}) AND a.resolved_at IS NULL AND r.user_id IS NULL"
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

/// 把我能看见的、未解决的全标已读。
///
/// 一条 SQL 而不是逐条 insert：逐条要先把列表取回来，而"能看见什么"
/// 已经在 [`VISIBLE`] 里写过一遍了，取回来再遍历等于把同一条规则用两次。
/// 已解决的不动——它们本来就不在未读里。
pub async fn mark_all_read(pool: &PgPool, user: &User) -> AppResult<u64> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    let sql = format!(
        "INSERT INTO alert_reads (alert_id, user_id)
         SELECT a.id, $1 FROM alerts a
         WHERE ({VISIBLE}) AND a.resolved_at IS NULL
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

pub async fn get(pool: &PgPool, id: Uuid) -> AppResult<Option<Alert>> {
    let row = sqlx::query_as("SELECT * FROM alerts WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}
