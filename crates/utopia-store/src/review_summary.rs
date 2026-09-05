//! 审核台的总览（#377）：等着办的、办过的、库的成色。
//!
//! 与 [`crate::review::counts`] 同一个前提——**数数不取数**，每一段都是
//! 聚合查询，不把行拉下来在 Rust 里数。七档队列的 WHERE 与 `counts` 逐字
//! 相同（`unconfirmed` 那段抽成了常量共用），否则左栏的数和总览的数迟早
//! 分叉。
//!
//! 「办过的」读的是审计台账：review./fact./conflict./merge. 四族动作，与
//! 决策流水（`audit::review_history`）同一个筛子。没有 actor 的那些是后台
//! 自裁（攒批裁决），单独数出来——「AI 替你办了多少」本身就是一个数。

use chrono::{DateTime, Duration, NaiveDate, Utc};
use sqlx::PgPool;
use std::collections::BTreeMap;
use utopia_core::models::{
    ActionCount, ActorCount, DecidedDay, DecidedWindow, QueueWait, ReviewDecided, ReviewHealth,
    ReviewSummary, ReviewWaiting,
};
use utopia_core::AppResult;
use uuid::Uuid;

use crate::review::{LOW_CONFIDENCE_BELOW, UNCONFIRMED_FACT};

/// 「办过的」按天看多少天
pub const DAILY_DAYS: i64 = 14;
/// 「谁办的」最多列几个人
const TOP_ACTORS: i64 = 5;

/// 与 `audit::review_history` 同一个筛子：review 域的四族动作
const DECISION_ACTIONS: &str = "(e.action LIKE 'review.%' OR e.action LIKE 'fact.%'
     OR e.action LIKE 'conflict.%' OR e.action LIKE 'merge.%')";

pub async fn summary(pool: &PgPool, kb_id: Uuid) -> AppResult<ReviewSummary> {
    let (waiting, decided, health) = tokio::try_join!(
        waiting(pool, kb_id),
        decided(pool, kb_id),
        health(pool, kb_id),
    )?;
    Ok(ReviewSummary {
        waiting,
        decided,
        health,
    })
}

#[derive(sqlx::FromRow)]
struct WaitingRow {
    pending: i64,
    pending_oldest: Option<DateTime<Utc>>,
    duplicates: i64,
    duplicates_oldest: Option<DateTime<Utc>>,
    conflicts: i64,
    conflicts_oldest: Option<DateTime<Utc>>,
    unconfirmed: i64,
    unconfirmed_oldest: Option<DateTime<Utc>>,
    lowconf: i64,
    lowconf_oldest: Option<DateTime<Utc>>,
    violations: i64,
    violations_oldest: Option<DateTime<Utc>>,
    defects: i64,
    defects_oldest: Option<DateTime<Utc>>,
}

async fn waiting(pool: &PgPool, kb_id: Uuid) -> AppResult<ReviewWaiting> {
    // 每档一对 count/min，一条查询端回来。「最老」按各表自己的时钟：待办事实与
    // 重复、冲突是 created_at，公理与本体缺陷是 detected_at，两种事实队列是
    // 事实自己的 recorded_at——都是「从什么时候起就在等」
    let sql = format!(
        "SELECT
           (SELECT count(*) FROM pending_facts WHERE kb_id = $1) AS pending,
           (SELECT min(created_at) FROM pending_facts WHERE kb_id = $1) AS pending_oldest,
           (SELECT count(*) FROM resolution_reviews
             WHERE kb_id = $1 AND status = 'pending') AS duplicates,
           (SELECT min(created_at) FROM resolution_reviews
             WHERE kb_id = $1 AND status = 'pending') AS duplicates_oldest,
           (SELECT count(*) FROM fact_conflicts
             WHERE kb_id = $1 AND status = 'open') AS conflicts,
           (SELECT min(created_at) FROM fact_conflicts
             WHERE kb_id = $1 AND status = 'open') AS conflicts_oldest,
           (SELECT count(*) FROM facts f
             WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND {unconfirmed}) AS unconfirmed,
           (SELECT min(f.recorded_at) FROM facts f
             WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND {unconfirmed}) AS unconfirmed_oldest,
           (SELECT count(*) FROM facts
             WHERE kb_id = $1 AND invalidated_at IS NULL
               AND confidence < $2 AND derived_by_rule IS NULL) AS lowconf,
           (SELECT min(recorded_at) FROM facts
             WHERE kb_id = $1 AND invalidated_at IS NULL
               AND confidence < $2 AND derived_by_rule IS NULL) AS lowconf_oldest,
           (SELECT count(*) FROM axiom_violations
             WHERE kb_id = $1 AND status = 'open') AS violations,
           (SELECT min(detected_at) FROM axiom_violations
             WHERE kb_id = $1 AND status = 'open') AS violations_oldest,
           (SELECT count(*) FROM ontology_defects
             WHERE kb_id = $1 AND status = 'open') AS defects,
           (SELECT min(detected_at) FROM ontology_defects
             WHERE kb_id = $1 AND status = 'open') AS defects_oldest",
        unconfirmed = UNCONFIRMED_FACT,
    );
    let r: WaitingRow = sqlx::query_as(&sql)
        .bind(kb_id)
        .bind(LOW_CONFIDENCE_BELOW)
        .fetch_one(pool)
        .await?;
    let wait = |count: i64, oldest_at: Option<DateTime<Utc>>| QueueWait { count, oldest_at };
    Ok(ReviewWaiting {
        pending: wait(r.pending, r.pending_oldest),
        duplicates: wait(r.duplicates, r.duplicates_oldest),
        conflicts: wait(r.conflicts, r.conflicts_oldest),
        unconfirmed: wait(r.unconfirmed, r.unconfirmed_oldest),
        lowconf: wait(r.lowconf, r.lowconf_oldest),
        violations: wait(r.violations, r.violations_oldest),
        defects: wait(r.defects, r.defects_oldest),
    })
}

#[derive(sqlx::FromRow)]
struct ActionRow {
    action: String,
    automatic: bool,
    last_7d: i64,
    last_30d: i64,
}

#[derive(sqlx::FromRow)]
struct ActorRow {
    actor_id: Option<Uuid>,
    actor_label: Option<String>,
    last_7d: i64,
    last_30d: i64,
}

#[derive(sqlx::FromRow)]
struct DayRow {
    day: NaiveDate,
    count: i64,
}

async fn decided(pool: &PgPool, kb_id: Uuid) -> AppResult<ReviewDecided> {
    // 两个窗口一趟查完：按 30 天筛，7 天的用 FILTER 另数一遍
    let by_action: Vec<ActionRow> = sqlx::query_as(&format!(
        "SELECT e.action, (e.actor_id IS NULL) AS automatic,
                count(*) FILTER (WHERE e.created_at >= now() - interval '7 days') AS last_7d,
                count(*) AS last_30d
           FROM audit_events e
          WHERE e.kb_id = $1 AND {DECISION_ACTIONS}
            AND e.created_at >= now() - interval '30 days'
          GROUP BY e.action, automatic
          ORDER BY last_30d DESC, e.action"
    ))
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    let by_actor: Vec<ActorRow> = sqlx::query_as(&format!(
        "SELECT e.actor_id, min(e.actor_label) AS actor_label,
                count(*) FILTER (WHERE e.created_at >= now() - interval '7 days') AS last_7d,
                count(*) AS last_30d
           FROM audit_events e
          WHERE e.kb_id = $1 AND {DECISION_ACTIONS}
            AND e.created_at >= now() - interval '30 days'
          GROUP BY e.actor_id
          ORDER BY last_30d DESC
          LIMIT $2"
    ))
    .bind(kb_id)
    .bind(TOP_ACTORS)
    .fetch_all(pool)
    .await?;
    let days: Vec<DayRow> = sqlx::query_as(&format!(
        "SELECT (e.created_at AT TIME ZONE 'UTC')::date AS day, count(*) AS count
           FROM audit_events e
          WHERE e.kb_id = $1 AND {DECISION_ACTIONS}
            AND e.created_at >= (now() AT TIME ZONE 'UTC')::date - $2::int
          GROUP BY day"
    ))
    .bind(kb_id)
    .bind((DAILY_DAYS - 1) as i32)
    .fetch_all(pool)
    .await?;

    let window = |seven: bool| {
        let pick = |a: &ActionRow| if seven { a.last_7d } else { a.last_30d };
        let mut merged: BTreeMap<&str, i64> = BTreeMap::new();
        let mut total = 0;
        let mut automatic = 0;
        for a in &by_action {
            let n = pick(a);
            if n == 0 {
                continue;
            }
            total += n;
            if a.automatic {
                automatic += n;
            }
            *merged.entry(a.action.as_str()).or_default() += n;
        }
        let mut by_action: Vec<ActionCount> = merged
            .into_iter()
            .map(|(action, count)| ActionCount {
                action: action.to_string(),
                count,
            })
            .collect();
        by_action.sort_by(|x, y| y.count.cmp(&x.count).then_with(|| x.action.cmp(&y.action)));
        let mut by_actor: Vec<ActorCount> = by_actor
            .iter()
            .map(|r| ActorCount {
                actor_id: r.actor_id,
                label: r.actor_label.clone(),
                count: if seven { r.last_7d } else { r.last_30d },
            })
            .filter(|r| r.count > 0)
            .collect();
        by_actor.sort_by_key(|r| std::cmp::Reverse(r.count));
        DecidedWindow {
            total,
            automatic,
            by_action,
            by_actor,
        }
    };

    // 补齐没有决定的那些天——柱子要等距，缺一天图就说谎
    let by_day: BTreeMap<NaiveDate, i64> = days.into_iter().map(|d| (d.day, d.count)).collect();
    let today = Utc::now().date_naive();
    let daily = (0..DAILY_DAYS)
        .rev()
        .map(|back| {
            let day = today - Duration::days(back);
            DecidedDay {
                day,
                count: by_day.get(&day).copied().unwrap_or(0),
            }
        })
        .collect();

    Ok(ReviewDecided {
        last_7d: window(true),
        last_30d: window(false),
        daily,
    })
}

#[derive(sqlx::FromRow)]
struct HealthRow {
    facts: i64,
    low_confidence: i64,
    unconfirmed: i64,
    contested: i64,
}

async fn health(pool: &PgPool, kb_id: Uuid) -> AppResult<ReviewHealth> {
    // 只数在世的事实（invalidated_at IS NULL）：作废的不算库的成色。
    // 「有争议」= 挂在一条还开着的冲突的任一端
    let sql = format!(
        "SELECT count(*) AS facts,
                count(*) FILTER (WHERE f.confidence < $2 AND f.derived_by_rule IS NULL) AS low_confidence,
                count(*) FILTER (WHERE {unconfirmed}) AS unconfirmed,
                count(*) FILTER (WHERE EXISTS (
                    SELECT 1 FROM fact_conflicts c
                     WHERE c.status = 'open'
                       AND (c.old_fact_id = f.id OR c.new_fact_id = f.id))) AS contested
           FROM facts f
          WHERE f.kb_id = $1 AND f.invalidated_at IS NULL",
        unconfirmed = UNCONFIRMED_FACT,
    );
    let r: HealthRow = sqlx::query_as(&sql)
        .bind(kb_id)
        .bind(LOW_CONFIDENCE_BELOW)
        .fetch_one(pool)
        .await?;
    Ok(ReviewHealth {
        facts: r.facts,
        low_confidence: r.low_confidence,
        unconfirmed: r.unconfirmed,
        contested: r.contested,
    })
}
