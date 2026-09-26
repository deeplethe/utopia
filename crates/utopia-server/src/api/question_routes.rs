//! 能力问题（0061 决定 1）与本体代理（决定 2、3）的接口。
//! 看 = viewer；改 = editor：问题是本体的验收标准，提案采纳直接改本体。

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_core::AppError;
use utopia_store::competency_questions as questions;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::ontology_agent;
use crate::state::AppState;

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<Vec<questions::Question>>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    Ok(Json(questions::list(&state.pool, kb_id).await?))
}

#[derive(Deserialize)]
pub struct CreateQuestionReq {
    pub question: String,
    #[serde(default)]
    pub expected_answer: Option<String>,
    #[serde(default)]
    pub needs: Option<serde_json::Value>,
}

/// 人提的问题一落地就是 accepted：它就是人对这个库的要求
pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<CreateQuestionReq>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Editor).await?;
    let text = req.question.trim();
    if text.is_empty() {
        return Err(AppError::invalid("question_required", "question is required").into());
    }
    let id = questions::create(
        &state.pool,
        kb_id,
        questions::NewQuestion {
            question: text,
            expected_answer: req
                .expected_answer
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            needs: req.needs.as_ref(),
            origin: "person",
            status: "accepted",
            created_by: Some(user.id),
        },
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "competency_question.created",
        "competency_question",
        Some(id),
        json!({ "question": text }),
    )
    .await;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct UpdateQuestionReq {
    #[serde(default)]
    pub question: Option<String>,
    /// 空串 = 清掉
    #[serde(default)]
    pub expected_answer: Option<String>,
    #[serde(default)]
    pub needs: Option<serde_json::Value>,
    /// accepted | rejected | retired
    #[serde(default)]
    pub status: Option<String>,
}

pub async fn update(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, qid)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateQuestionReq>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Editor).await?;
    if let Some(q) = req.question.as_deref() {
        if q.trim().is_empty() {
            return Err(AppError::invalid("question_required", "question is required").into());
        }
    }
    if req.question.is_some() || req.expected_answer.is_some() || req.needs.is_some() {
        questions::update(
            &state.pool,
            kb_id,
            qid,
            req.question.as_deref().map(str::trim),
            req.expected_answer.as_deref().map(str::trim),
            req.needs.as_ref(),
        )
        .await?;
    }
    if let Some(status) = req.status.as_deref() {
        questions::set_status(&state.pool, kb_id, qid, status).await?;
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, qid)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Editor).await?;
    questions::delete(&state.pool, kb_id, qid).await?;
    Ok(Json(json!({ "ok": true })))
}

/// 叫代理来看一眼：排一份任务，马上回。结果落在 `ontology_proposals`，界面从那里读
pub async fn propose(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Editor).await?;
    let queued = utopia_store::jobs::enqueue_unless_queued(
        &state.pool,
        "propose_ontology",
        json!({ "kb_id": kb_id }),
    )
    .await?;
    Ok(Json(json!({ "queued": queued.is_some() })))
}

#[derive(Deserialize)]
pub struct AdoptReq {
    pub section: String,
    pub key: String,
    #[serde(default, flatten)]
    pub edits: ontology_agent::AdoptEdits,
}

/// 采纳一条提案（人改过的几格随请求来）：建元素、标记、排对齐
pub async fn adopt(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<AdoptReq>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Editor).await?;
    let id =
        ontology_agent::adopt(&state, kb_id, &req.section, &req.key, req.edits, user.id).await?;
    Ok(Json(json!({ "id": id })))
}
