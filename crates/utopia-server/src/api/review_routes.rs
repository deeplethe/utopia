//! 审核队列 API：消解疑似重复对 + 低置信事实 + 合并日志/回滚。

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use uuid::Uuid;

use super::graph_routes::require_kb;
use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

const LOW_CONFIDENCE_BELOW: f32 = 0.75;

/// 冲突双方的三元组快照：(旧主语, 旧宾语, 新主语, 新宾语, 谓词标签)。
type ConflictSnapshot = (String, Option<String>, String, Option<String>, String);

/// 事实快照（决策台账用）：reject 后事实从图里消失，台账必须自包含展示文本。
/// 一律在动作执行前取。
async fn fact_snapshot(state: &AppState, kb_id: Uuid, fact_id: Uuid) -> Option<serde_json::Value> {
    // 宾语可能是实体或字面值；结构化字面值优先取 summary（与队列卡片同一显示口径）
    let row: Option<(String, Option<String>, Option<String>, f32)> = sqlx::query_as(
        "SELECT s.canonical_name, COALESCE(r.label, fact_surface_predicate(f.id)),
                COALESCE(o.canonical_name, f.object_value ->> 'summary',
                         f.object_value #>> '{}'), f.confidence
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.id = $1 AND f.kb_id = $2",
    )
    .bind(fact_id)
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    row.map(|(s, p, o, c)| json!({ "subject": s, "predicate": p, "object": o, "confidence": c }))
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let reviews = utopia_store::resolution::list_reviews(&state.pool, kb_id).await?;
    let facts =
        utopia_store::graph::low_confidence_facts(&state.pool, kb_id, LOW_CONFIDENCE_BELOW, 100)
            .await?;
    let merges = utopia_store::resolution::list_merges(&state.pool, kb_id, 30).await?;
    let conflicts = utopia_store::temporal::list_conflicts(&state.pool, kb_id).await?;
    let unconfirmed = utopia_store::graph::stale_facts(&state.pool, kb_id, 100).await?;
    Ok(Json(json!({
        "reviews": reviews, "facts": facts, "merges": merges,
        "conflicts": conflicts, "unconfirmed": unconfirmed
    })))
}

#[derive(Deserialize)]
pub struct CloseFactBody {
    pub valid_to: chrono::DateTime<chrono::Utc>,
}

/// 人工闭合一条事实的有效区间（"这事在某时结束了"）——走作废+改写，
/// 与自动闭合同一机制，账本可回放。
pub async fn close_fact(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, fact_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<CloseFactBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    // 归属与状态校验：本 KB 的、未作废、开放区间的事实才可闭合
    let ok: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM facts
         WHERE id = $1 AND kb_id = $2 AND invalidated_at IS NULL AND valid_to IS NULL",
    )
    .bind(fact_id)
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(utopia_core::AppError::Db)?;
    if ok.is_none() {
        return Err(utopia_core::AppError::NotFound.into());
    }
    let snap = fact_snapshot(&state, kb_id, fact_id).await;
    // 人在界面上选的是一个日期，所以闭合点按日
    utopia_store::temporal::close_superseded(&state.pool, fact_id, body.valid_to, "day").await?;
    if let Some(mut d) = snap {
        d["valid_to"] = json!(body.valid_to.to_rfc3339());
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            "fact.close",
            "fact",
            Some(fact_id),
            d,
        )
        .await;
    }
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ConflictBody {
    /// close | keep | reject_new
    pub action: String,
    /// close 且新事实无起点时必填
    #[serde(default)]
    pub close_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 时态冲突裁决（S3：自动闭合拿不准的那些）。
pub async fn resolve_conflict(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, conflict_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<ConflictBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    // 快照双方三元组：裁决会作废/改写事实，先抄后动
    let snap: Option<ConflictSnapshot> = sqlx::query_as(
        "SELECT os.canonical_name, oo.canonical_name, ns.canonical_name, no_.canonical_name,
                r.label
         FROM fact_conflicts c
         JOIN facts fo ON fo.id = c.old_fact_id
         JOIN facts fn_ ON fn_.id = c.new_fact_id
         JOIN entities os ON os.id = fo.subject_id
         LEFT JOIN entities oo ON oo.id = fo.object_id
         JOIN entities ns ON ns.id = fn_.subject_id
         LEFT JOIN entities no_ ON no_.id = fn_.object_id
         JOIN relation_types r ON r.id = fo.predicate_id
         WHERE c.id = $1 AND c.kb_id = $2",
    )
    .bind(conflict_id)
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    utopia_store::temporal::resolve_conflict(
        &state.pool,
        kb_id,
        conflict_id,
        &body.action,
        body.close_at,
    )
    .await?;
    if let Some((os, oo, ns, no, pred)) = snap {
        let action = match body.action.as_str() {
            "close" => "conflict.close_old",
            "keep" => "conflict.keep_both",
            _ => "conflict.reject_new",
        };
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            action,
            "conflict",
            Some(conflict_id),
            json!({
                "predicate": pred,
                "old_subject": os, "old_object": oo,
                "new_subject": ns, "new_object": no,
                "close_at": body.close_at.map(|t| t.to_rfc3339()),
            }),
        )
        .await;
    }
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct DecideBody {
    /// merge | keep
    pub action: String,
}

pub async fn decide(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, review_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<DecideBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let snap: Option<(String, String, f32)> = sqlx::query_as(
        "SELECT a.canonical_name, b.canonical_name, rr.score
         FROM resolution_reviews rr
         JOIN entities a ON a.id = rr.left_id
         JOIN entities b ON b.id = rr.right_id
         WHERE rr.id = $1 AND rr.kb_id = $2",
    )
    .bind(review_id)
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    utopia_store::resolution::decide_review(&state.pool, kb_id, review_id, &body.action, user.id)
        .await?;
    if let Some((l, r, score)) = snap {
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            if body.action == "merge" {
                "review.merge"
            } else {
                "review.keep"
            },
            "review",
            Some(review_id),
            json!({ "left": l, "right": r, "score": score }),
        )
        .await;
    }
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

pub async fn confirm_fact(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, fact_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let snap = fact_snapshot(&state, kb_id, fact_id).await;
    utopia_store::graph::confirm_fact(&state.pool, kb_id, fact_id).await?;
    if let Some(d) = snap {
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            "fact.confirm",
            "fact",
            Some(fact_id),
            d,
        )
        .await;
    }
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

pub async fn reject_fact(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, fact_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let snap = fact_snapshot(&state, kb_id, fact_id).await;
    utopia_store::graph::reject_fact(&state.pool, kb_id, fact_id).await?;
    if let Some(d) = snap {
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            "fact.reject",
            "fact",
            Some(fact_id),
            d,
        )
        .await;
    }
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

pub async fn revert_merge(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, merge_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let snap: Option<(String, String)> = sqlx::query_as(
        "SELECT s.canonical_name, t.canonical_name
         FROM entity_merges m
         JOIN entities s ON s.id = m.source_id
         JOIN entities t ON t.id = m.target_id
         WHERE m.id = $1 AND m.kb_id = $2",
    )
    .bind(merge_id)
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    utopia_store::resolution::revert_merge(&state.pool, kb_id, merge_id).await?;
    if let Some((s, t)) = snap {
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            "merge.revert",
            "merge",
            Some(merge_id),
            json!({ "source": s, "target": t }),
        )
        .await;
    }
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct ManualMergeBody {
    pub source: Uuid,
    pub target: Uuid,
}

/// 手动合并（实体面板"Merge into…"入口）。
pub async fn manual_merge(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(body): Json<ManualMergeBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let snap: Option<(String,)> = sqlx::query_as(
        "SELECT s.canonical_name || ' → ' || t.canonical_name
         FROM entities s, entities t WHERE s.id = $1 AND t.id = $2",
    )
    .bind(body.source)
    .bind(body.target)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
    let merge_id = utopia_store::resolution::merge_entities(
        &state.pool,
        kb_id,
        body.source,
        body.target,
        Some(user.id),
        "manual merge",
    )
    .await?;
    if let Some((pair,)) = snap {
        let (s, t) = pair.split_once(" → ").unwrap_or((pair.as_str(), ""));
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            "merge.manual",
            "merge",
            Some(merge_id),
            json!({ "source": s, "target": t }),
        )
        .await;
    }
    state.emit_review(kb_id);
    Ok(Json(json!({ "merge_id": merge_id })))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    pub page: i64,
    #[serde(default = "default_history_per")]
    pub per: i64,
}

fn default_history_per() -> i64 {
    20
}

/// 决策台账：review 域的审计事件，服务端分页。
pub async fn history(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let per = q.per.clamp(1, 100);
    let page = q.page.max(0);
    let (events, total) =
        utopia_store::audit::review_history(&state.pool, kb_id, per, page * per).await?;
    Ok(Json(json!({ "events": events, "total": total })))
}
