//! 业务规则的增删改查（0021 / #277）。
//!
//! **写规则要 Editor**：它是本体的一部分——一条规则改一次，全库的结论跟着变，
//! 与改一条公理同一个量级。

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_store::business_rules::{ConclusionInput, ConditionInput};
use uuid::Uuid;

use super::graph_routes::require_kb;
use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RuleReq {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub subject_type_id: Uuid,
    /// typing | attribute
    pub conclusion: String,
    #[serde(default)]
    pub conclude_type_id: Option<Uuid>,
    #[serde(default)]
    pub conclude_predicate_id: Option<Uuid>,
    #[serde(default)]
    pub conclude_value: Option<serde_json::Value>,
    /// 算出来的结论那棵树（0032）：`conclusion = "computed"` 时给
    #[serde(default)]
    pub conclude_expr: Option<serde_json::Value>,
    #[serde(default)]
    pub join_predicate_id: Option<Uuid>,
    pub conditions: Vec<ConditionInput>,
}

#[derive(Deserialize)]
pub struct RulePatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// 给了就整组替换；不给就不动条件
    #[serde(default)]
    pub conditions: Option<Vec<ConditionInput>>,
    /// 结论也整组替换：三格互相定义，只改一格会留下半截状态
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub conclude_type_id: Option<Uuid>,
    #[serde(default)]
    pub conclude_predicate_id: Option<Uuid>,
    #[serde(default)]
    pub conclude_value: Option<serde_json::Value>,
    #[serde(default)]
    pub conclude_expr: Option<serde_json::Value>,
    #[serde(default)]
    pub join_predicate_id: Option<Uuid>,
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let rules = utopia_store::business_rules::list(&state.pool, kb_id).await?;
    Ok(Json(json!({ "rules": rules })))
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<RuleReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let id = utopia_store::business_rules::create(
        &state.pool,
        kb_id,
        &req.name,
        &req.description,
        req.subject_type_id,
        &req.conclusion,
        req.conclude_type_id,
        req.conclude_predicate_id,
        req.conclude_value.clone(),
        req.conclude_expr.clone(),
        req.join_predicate_id,
        &req.conditions,
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "rule.created",
        "attribute_rule",
        Some(id),
        json!({ "name": req.name, "conclusion": req.conclusion }),
    )
    .await;
    Ok(Json(json!({ "id": id })))
}

pub async fn update(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, rule_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<RulePatch>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let conclusion = req.conclusion.as_ref().map(|kind| ConclusionInput {
        kind: kind.clone(),
        type_id: req.conclude_type_id,
        predicate_id: req.conclude_predicate_id,
        value: req.conclude_value.clone(),
        expr: req.conclude_expr.clone(),
        join_predicate_id: req.join_predicate_id,
    });
    utopia_store::business_rules::update(
        &state.pool,
        kb_id,
        rule_id,
        req.name.as_deref(),
        req.description.as_deref(),
        req.enabled,
        req.conditions.as_deref(),
        conclusion.as_ref(),
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "rule.updated",
        "attribute_rule",
        Some(rule_id),
        json!({ "enabled": req.enabled, "conditions_replaced": req.conditions.is_some() }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, rule_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    utopia_store::business_rules::delete(&state.pool, kb_id, rule_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "rule.deleted",
        "attribute_rule",
        Some(rule_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// 一条规则此刻标了谁。界面上那个数字点开就是这份列表。
pub async fn matches(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, rule_id)): Path<(Uuid, Uuid)>,
    axum::extract::Query(q): axum::extract::Query<MatchQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let per = q.per.unwrap_or(20).clamp(1, 100);
    let page = q.page.unwrap_or(0).max(0);
    let (rows, total) =
        utopia_store::business_rules::matches(&state.pool, kb_id, rule_id, per, page * per).await?;
    Ok(Json(json!({ "matches": rows, "total": total })))
}

/// 一条规则的定义史（0060）：每一版说了什么、从什么时候到什么时候、此刻凭它成立几条
pub async fn versions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, rule_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let versions = utopia_store::business_rules::versions(&state.pool, kb_id, rule_id).await?;
    Ok(Json(json!({ "versions": versions })))
}

#[derive(Deserialize)]
pub struct MatchQuery {
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub per: Option<i64>,
}

/// 现在就跑一遍，把这条规则的结论算出来。
///
/// **不等下一个物化周期**：写规则的人想立刻看见它推出了什么，而周期默认一小时。
/// 走的是同一个 `materialize`——预览与正式是同一条路，不另写一份会漂移的逻辑。
pub async fn run_now(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let report = utopia_store::reasoning::materialize(&state.pool, kb_id).await?;
    state.emit_graph(kb_id);
    Ok(Json(json!({
        "rules": report.attribute_rules,
        "hits": report.rule_hits,
        "capped": report.rule_capped,
        "inserted": report.inserted,
        "invalidated": report.invalidated,
        // 链跑了几轮，以及有没有跑满上限就停（0030）。**跑满得说出来**——
        // 没接上的那一环与「不满足」在结果里长得一模一样
        "rounds": report.rule_rounds,
        "rounds_capped": report.rule_rounds_capped,
    })))
}
