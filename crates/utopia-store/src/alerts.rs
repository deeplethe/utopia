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

/// 上报一条。同 `(kb_id, kind)` 已有未解决的就**并进去**：追加 subject、
/// 推 `last_seen`、合并 detail，不插新行也不改 `first_seen`。
///
/// 返回 `(alert_id, is_new)`。`is_new` 给调用方决定要不要推事件——
/// 一个源每 5 分钟失败一次，不该每次都把所有人的角标点亮。
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
    // 冲突目标是那个 COALESCE 表达式索引，所以 ON CONFLICT 也得写成表达式。
    // xmax = 0 是"这一行是本次 INSERT 插进去的"的判据——ON CONFLICT DO UPDATE
    // 无论走哪条路都 RETURNING，光看返回值分不出新旧
    let row: (Uuid, bool) = sqlx::query_as(
        "INSERT INTO alerts
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
         RETURNING id, (xmax = 0)",
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
    Ok(row)
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

/// 这个人能看见的告警。
///
/// 可见性判定**一次做完**：系统级看 `is_admin`，库级看 `visible_kb_roles`
/// 给出的有效角色够不够 `min_role`。推送那一路不判权限（它不带数据），
/// 全部落在这里一处。
pub async fn list_for_user(
    pool: &PgPool,
    user: &User,
    include_resolved: bool,
    limit: i64,
) -> AppResult<Vec<AlertView>> {
    let (kb_ids, kb_roles) = visible(pool, user).await?;
    let sql = format!(
        "SELECT a.id, a.kb_id, k.name AS kb_name, a.severity, a.kind,
                a.subject_type, a.subject_ids, a.detail,
                a.first_seen, a.last_seen, a.resolved_at,
                (r.user_id IS NOT NULL) AS read
         FROM alerts a
         LEFT JOIN knowledge_bases k ON k.id = a.kb_id
         LEFT JOIN alert_reads r ON r.alert_id = a.id AND r.user_id = $1
         WHERE ({VISIBLE})
           AND ($5::bool OR a.resolved_at IS NULL)
         ORDER BY (a.resolved_at IS NULL) DESC, a.last_seen DESC
         LIMIT $6"
    );
    let rows: Vec<AlertView> = sqlx::query_as(&sql)
        .bind(user.id)
        .bind(user.is_admin)
        .bind(&kb_ids)
        .bind(&kb_roles)
        .bind(include_resolved)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(rows)
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
