//! 审计日志：谁在何时对什么做了什么。纯审计——只记录与展示，不承载回滚等衍生功能。
//! 记录失败绝不影响业务操作（调用方一律 `let _ =`）。

use sqlx::PgPool;
use utopia_core::models::AuditEventView;
use utopia_core::AppResult;
use uuid::Uuid;

pub async fn record(
    pool: &PgPool,
    kb_id: Option<Uuid>,
    actor_id: Uuid,
    action: &str,
    target_kind: &str,
    target_id: Option<Uuid>,
    detail: serde_json::Value,
) -> AppResult<()> {
    record_opt(
        pool,
        kb_id,
        Some(actor_id),
        action,
        target_kind,
        target_id,
        detail,
    )
    .await
}

/// 请求的来源信息。由 HTTP 层在每个请求外层 scope 进 [`CLIENT`]，`record_opt`
/// 自行读取——否则 25 个调用点每一个都要多带两个与业务无关的参数。
///
/// 后台任务（攒批裁决、定时同步）不在任何请求之内，读到的是 None，本就该如此：
/// 那些动作确实没有客户端。
#[derive(Debug, Clone, Default)]
pub struct ClientContext {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

tokio::task_local! {
    pub static CLIENT: ClientContext;
}

/// 取当前请求的来源；不在请求上下文中（后台任务）时返回全空。
fn client_context() -> ClientContext {
    CLIENT.try_with(|c| c.clone()).unwrap_or_default()
}

/// 无人类操作者的系统事件（如 AI 裁决自动合并）走这里：actor 为 NULL。
pub async fn record_opt(
    pool: &PgPool,
    kb_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    action: &str,
    target_kind: &str,
    target_id: Option<Uuid>,
    detail: serde_json::Value,
) -> AppResult<()> {
    let ctx = client_context();
    // 身份快照：用户日后被删除时（0025 之后行会留下），台账仍认得出是谁，而不是
    // 只剩一串 UUID。此刻查是可靠的——操作正在发生，人还在。
    let actor_label: Option<String> = match actor_id {
        Some(id) => sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    sqlx::query(
        "INSERT INTO audit_events
            (id, kb_id, actor_id, action, target_kind, target_id, detail,
             client_ip, user_agent, actor_label)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(actor_id)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(detail)
    .bind(ctx.ip)
    .bind(ctx.user_agent)
    .bind(actor_label)
    .execute(pool)
    .await?;
    Ok(())
}

/// 审核决策台账：只取 review 域的动作（review./fact./conflict./merge.），服务端分页。
pub async fn review_history(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<AuditEventView>, i64)> {
    const COND: &str = "e.kb_id = $1 AND (e.action LIKE 'review.%' OR e.action LIKE 'fact.%'
                        OR e.action LIKE 'conflict.%' OR e.action LIKE 'merge.%')";
    let rows: Vec<AuditEventView> = sqlx::query_as(&format!(
        "SELECT e.id, e.action, e.target_kind, e.target_id, e.detail,
                u.display_name AS actor_name, e.created_at
         FROM audit_events e LEFT JOIN users u ON u.id = e.actor_id
         WHERE {COND} ORDER BY e.created_at DESC LIMIT $2 OFFSET $3"
    ))
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let (total,): (i64,) =
        sqlx::query_as(&format!("SELECT count(*) FROM audit_events e WHERE {COND}"))
            .bind(kb_id)
            .fetch_one(pool)
            .await?;
    Ok((rows, total))
}

/// 一个知识库的审计台账，**带分页与筛选**。
///
/// 从前是固定最近 100 条、无分页无筛选——而台账是合规材料，「只看得到最近
/// 一百条」等于查不了历史。三个筛选是按真实的查法挑的：
///
/// - `action`：查一类动作（「谁改过类型」「有哪些拒绝」）。前缀匹配而不是
///   全等，因为动作名本身分层（`entity.retyped` / `entity.renamed`），
///   传 `entity.` 就能把一族捞出来
/// - `actor`：查一个人做过什么。合规审计最常见的问题
/// - `since` / `until`：查一段时间。事故复盘要的就是这个
///
/// 一并回总数，否则分页器不知道有几页——而「不知道有几页」正是这一档
/// 从前那个 100 的翻版。
#[allow(clippy::too_many_arguments)]
pub async fn list_for_kb(
    pool: &PgPool,
    kb_id: Uuid,
    action: Option<&str>,
    actor: Option<Uuid>,
    since: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<AuditEventView>, i64)> {
    // 四个筛选都写成「参数为空就不生效」，这样一条 SQL 覆盖全部组合——
    // 拼字符串会在这里长出十六个分支，而每个分支都是一次注入面
    const WHERE: &str = "WHERE e.kb_id = $1
           AND ($2::text IS NULL OR e.action LIKE $2 || '%')
           AND ($3::uuid IS NULL OR e.actor_id = $3)
           AND ($4::timestamptz IS NULL OR e.created_at >= $4)
           AND ($5::timestamptz IS NULL OR e.created_at < $5)";

    let rows: Vec<AuditEventView> = sqlx::query_as(&format!(
        "SELECT e.id, e.action, e.target_kind, e.target_id, e.detail,
                u.display_name AS actor_name, e.created_at
         FROM audit_events e LEFT JOIN users u ON u.id = e.actor_id
         {WHERE}
         ORDER BY e.created_at DESC LIMIT $6 OFFSET $7"
    ))
    .bind(kb_id)
    .bind(action)
    .bind(actor)
    .bind(since)
    .bind(until)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let (total,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM audit_events e {WHERE}"))
        .bind(kb_id)
        .bind(action)
        .bind(actor)
        .bind(since)
        .bind(until)
        .fetch_one(pool)
        .await?;
    Ok((rows, total))
}

/// 这个库的台账里出现过哪些动作。**筛选下拉要按实际有的填**——列一个
/// 全部动作的硬编码清单，用户会看到一堆这个库从来没发生过的选项。
pub async fn actions_for_kb(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT DISTINCT action FROM audit_events WHERE kb_id = $1 ORDER BY 1")
            .bind(kb_id)
            .fetch_all(pool)
            .await?,
    )
}
