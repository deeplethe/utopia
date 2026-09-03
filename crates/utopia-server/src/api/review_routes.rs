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

/// 一页多少条。**服务端的默认，不是上限**——前端可以要更少，多则被 clamp 挡住
const REVIEW_PAGE: i64 = 10;

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

#[derive(Deserialize)]
pub struct ReviewQuery {
    /// 要看哪一档。缺省 duplicates
    #[serde(default)]
    pub queue: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

/// 审核队列：**计数与内容分开取**。
///
/// 从前一次把八个队列全端回来，每个固定 100 条，前端再客户端分页——于是左栏
/// 的徽标是截断后的数字（库里 164 条低置信事实，界面写 100），而第十一页之后
/// 的东西界面上不存在。
///
/// 现在：计数每次都回（八个 COUNT 一条查询，走的是与列表同一套 WHERE），
/// 内容只回当前那一档的一页。切换分档或翻页各发一次请求，代价是多几次往返，
/// 换来的是**数字不再骗人**，而且一个有十万条待办的库也翻得到底。
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<ReviewQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let counts = utopia_store::review::counts(&state.pool, kb_id).await?;
    let queue = q.queue.as_deref().unwrap_or("duplicates");
    // 上限挡住「limit=1000000 把库拖垮」，同时留出足够一页的余量
    let limit = q.limit.unwrap_or(REVIEW_PAGE).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);

    let items = match queue {
        // 记忆抽出、等人点头的事实（0015）。排第一：它是人自己说的话
        "pending" => json!(utopia_store::pending::list(&state.pool, kb_id, limit, offset).await?),
        "duplicates" => {
            json!(utopia_store::resolution::list_reviews(&state.pool, kb_id, limit, offset).await?)
        }
        "conflicts" => {
            json!(utopia_store::temporal::list_conflicts(&state.pool, kb_id, limit, offset).await?)
        }
        "unconfirmed" => {
            json!(utopia_store::graph::stale_facts(&state.pool, kb_id, limit, offset).await?)
        }
        "lowconf" => json!(
            utopia_store::graph::low_confidence_facts(
                &state.pool,
                kb_id,
                utopia_store::review::LOW_CONFIDENCE_BELOW,
                limit,
                offset,
            )
            .await?
        ),
        "mappings" => {
            json!(utopia_store::mappings::proposed(&state.pool, kb_id, limit, offset).await?)
        }
        "violations" => {
            json!(
                utopia_store::reasoning::open_violations(&state.pool, kb_id, limit, offset).await?
            )
        }
        "defects" => {
            json!(utopia_store::reasoning::open_defects(&state.pool, kb_id, limit, offset).await?)
        }
        "merges" => {
            json!(utopia_store::resolution::list_merges(&state.pool, kb_id, limit, offset).await?)
        }
        // 认不出的档名当成契约错误报出来，而不是悄悄回空——悄悄回空会让前端
        // 拼错一个字母之后看到「这一档清空了」
        other => {
            return Err(utopia_core::AppError::invalid(
                "unknown_queue",
                format!("no review queue named {other}"),
            )
            .into())
        }
    };

    Ok(Json(json!({
        "counts": counts,
        "queue": queue,
        "items": items,
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

#[derive(Deserialize)]
pub struct DecideMappingReq {
    /// confirmed | rejected
    pub status: String,
}

/// 对一条语义层映射表态（0011）。
///
/// **改状态不删行**：确认发生过、拒绝也发生过。而拒绝留痕当下就有用——
/// 下一轮探索会再次算出被拒绝过的那条，`propose` 的 `WHERE status = 'proposed'`
/// 据此不把它刷回待看。
pub async fn decide_mapping(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, mapping_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<DecideMappingReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    if !matches!(req.status.as_str(), "confirmed" | "rejected") {
        return Err(utopia_core::AppError::invalid(
            "bad_status",
            "status must be confirmed or rejected",
        )
        .into());
    }
    utopia_store::mappings::decide(&state.pool, kb_id, mapping_id, &req.status, user.id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "mapping.decided",
        "concept_mapping",
        Some(mapping_id),
        json!({ "status": req.status }),
    )
    .await;
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct DecideViolationReq {
    /// fact_retracted | fact_closed | axiom_relaxed | accepted
    pub resolution: String,
    /// `fact_closed` 必填：旧断言在哪一天结束
    #[serde(default)]
    pub close_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 人裁决一处公理违规。
///
/// **三个出路而不是两个。** `axiom_relaxed` 是这一档独有的：矛盾可能出在定义
/// 而不是数据——用户导的本体把某个属性声明成反对称，而他自己的语料里那关系
/// 其实双向。这时该改的是本体，不是二十条事实。
///
/// 端点只记决定，不替人执行。撤事实走 `reject_fact`，改公理走本体页——
/// 那两个动作各有自己的权限与台账，塞进这里会变成一个什么都能干的端点。
pub async fn decide_violation(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, violation_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<DecideViolationReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    if !matches!(
        req.resolution.as_str(),
        "fact_retracted" | "fact_closed" | "axiom_relaxed" | "accepted"
    ) {
        return Err(utopia_core::AppError::invalid(
            "bad_resolution",
            "resolution 只能是 fact_retracted、fact_closed、axiom_relaxed 或 accepted",
        )
        .into());
    }
    let row: Option<(String, Uuid)> = sqlx::query_as(
        "SELECT kind, left_fact FROM axiom_violations
          WHERE id = $1 AND kb_id = $2 AND status = 'open'",
    )
    .bind(violation_id)
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(utopia_core::AppError::Db)?;
    let Some((kind, left)) = row else {
        return Err(utopia_core::AppError::NotFound.into());
    };
    // 派生撞断言那一类（0017）的修法就在卡片上，端点替人执行：撤旧断言、或给它一个
    // 结束日期。其它几类仍只记决定——那些卡片上两条都是断言，撤哪条端点判不了
    let repaired = kind == "derived_contradiction";
    match (repaired, req.resolution.as_str()) {
        (true, "fact_retracted") => {
            let snap = fact_snapshot(&state, kb_id, left).await;
            utopia_store::graph::reject_fact(&state.pool, kb_id, left).await?;
            if let Some(d) = snap {
                let _ = utopia_store::audit::record(
                    &state.pool,
                    Some(kb_id),
                    user.id,
                    "fact.reject",
                    "fact",
                    Some(left),
                    d,
                )
                .await;
            }
        }
        (true, "fact_closed") => {
            let Some(at) = req.close_at else {
                return Err(utopia_core::AppError::invalid(
                    "close_at_required",
                    "fact_closed 要给出结束日期",
                )
                .into());
            };
            let open: Option<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM facts
                  WHERE id = $1 AND invalidated_at IS NULL AND valid_to IS NULL",
            )
            .bind(left)
            .fetch_optional(&state.pool)
            .await
            .map_err(utopia_core::AppError::Db)?;
            if open.is_none() {
                return Err(utopia_core::AppError::invalid(
                    "not_open",
                    "这条断言已有结束日期，或已被撤",
                )
                .into());
            }
            let snap = fact_snapshot(&state, kb_id, left).await;
            utopia_store::temporal::close_superseded(&state.pool, left, at, "day").await?;
            if let Some(mut d) = snap {
                d["valid_to"] = json!(at.to_rfc3339());
                let _ = utopia_store::audit::record(
                    &state.pool,
                    Some(kb_id),
                    user.id,
                    "fact.close",
                    "fact",
                    Some(left),
                    d,
                )
                .await;
            }
        }
        (false, "fact_closed") => {
            return Err(utopia_core::AppError::invalid(
                "bad_resolution",
                "fact_closed 只用于 derived_contradiction",
            )
            .into());
        }
        _ => {}
    }
    utopia_store::reasoning::decide(&state.pool, kb_id, violation_id, &req.resolution, user.id)
        .await?;
    // 路清了就让派生落地，人不必再去点一次「推一遍」。撤与闭合把断言挪开了，
    // 认可则在 materialize 里放行
    if repaired {
        utopia_store::reasoning::materialize(&state.pool, kb_id).await?;
    }
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "violation.decided",
        "axiom_violation",
        Some(violation_id),
        json!({ "resolution": req.resolution }),
    )
    .await;
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

/// 手动跑一遍一致性检查。
///
/// **同步跑而不是排任务**：这是纯计算，没有模型调用也没有网络——一个几万条
/// 事实的库跑下来是毫秒级。排进任务队列只会让人点完按钮盯着一个"排队中"，
/// 而队列真正要解决的是"这活儿要跑几分钟"。
///
/// `predicates_with_axioms` 一并回给前端：**零和零的含义不同**。没有公理时
/// 结论是"没有判据"，界面该说"先导一份带公理的本体"，而不是"未发现矛盾"。
pub async fn run_consistency_check(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let report = utopia_store::reasoning::run(&state.pool, kb_id).await?;
    // 本体自洽性一并算。两者判据同源（都是本体声明的公理），分两个按钮
    // 只会让人点两次
    let onto = utopia_store::reasoning::check_ontology(&state.pool, kb_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "consistency.checked",
        "knowledge_base",
        Some(kb_id),
        json!({
            "edges": report.edges,
            "predicates_with_axioms": report.predicates_with_axioms,
            "found": report.found,
            "inserted": report.inserted,
            "cleared": report.cleared,
            "defects_found": onto.found,
        }),
    )
    .await;
    state.emit_review(kb_id);
    Ok(Json(json!({
        "edges": report.edges,
        "predicates_with_axioms": report.predicates_with_axioms,
        "found": report.found,
        "inserted": report.inserted,
        "cleared": report.cleared,
        // 本体自己那一档单独回。**不加进 found**：两个数不是一类东西，
        // 加起来之后「3 处矛盾」既可能是三条事实抵触，也可能是本体自己写反了三处
        "classes": onto.classes,
        "defects_found": onto.found,
        "defects_new": onto.inserted,
    })))
}

#[derive(Deserialize)]
pub struct DecideDefectReq {
    /// fixed | accepted
    pub resolution: String,
}

/// 人对一处本体缺陷表态。
///
/// **两个出路而不是三个**：本体缺陷压根没看数据，所以没有「数据错了」这一条。
pub async fn decide_defect(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, defect_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<DecideDefectReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    if !matches!(req.resolution.as_str(), "fixed" | "accepted") {
        return Err(utopia_core::AppError::invalid(
            "bad_resolution",
            "resolution 只能是 fixed 或 accepted",
        )
        .into());
    }
    utopia_store::reasoning::decide_defect(&state.pool, kb_id, defect_id, &req.resolution, user.id)
        .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "defect.decided",
        "ontology_defect",
        Some(defect_id),
        json!({ "resolution": req.resolution }),
    )
    .await;
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

/// 跑一遍推理（R1）。
///
/// **受 `materialize_inferences` 开关约束。** 默认关，因为这一步往图里加东西，
/// 而 0001 判据 2 说「本体是引导不是执法」——声明可能是错的，不该在用户没表态时
/// 就按它改图。开关关着时不静默跳过：回一个明确的错，界面才说得出为什么没动。
///
/// 同步跑，与一致性检查同一个理由：纯计算，没有模型调用也没有网络。
#[derive(Deserialize)]
pub struct PendingQuery {
    pub chunk_id: Uuid,
}

/// 一句记忆抽出的全部待确认项——对话里那张确认卡按这个取（0015）。
/// Viewer 也能看：看得见提议、看不见按钮，与 Review 页同一口径
pub async fn pending_for_chunk(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<PendingQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let items = utopia_store::pending::for_chunk(&state.pool, kb_id, q.chunk_id).await?;
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct DecidePendingBody {
    /// `confirm` 进账本；`reject` 记进 `rejected_facts`，下一轮重抽不再提
    pub action: String,
}

/// 人对一条待确认事实点头或摇头。
///
/// 两个动作都进决策台账，快照自包含——行删了之后台账上还读得出当时确认的是什么。
/// 确认走与抽取相同的那条路（事实 + 证据 + 时态对账），所以一句「Mira 交给 Devin 了」
/// 点头之后，Mira 那条会像从文档里抽出来时一样被闭合
pub async fn decide_pending(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, pending_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<DecidePendingBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    match body.action.as_str() {
        "confirm" => {
            let done = utopia_store::pending::confirm(&state.pool, kb_id, pending_id).await?;
            let _ = utopia_store::audit::record(
                &state.pool,
                Some(kb_id),
                user.id,
                "fact.nod_confirmed",
                "fact",
                Some(done.fact_id),
                done.snapshot,
            )
            .await;
            state.emit_pending(kb_id);
            state.emit_review(kb_id);
            state.emit_graph(kb_id);
            Ok(Json(json!({
                "ok": true,
                "fact_id": done.fact_id,
                "created": done.created,
                "conflicts": done.conflicts,
            })))
        }
        "reject" => {
            let snap = utopia_store::pending::reject(&state.pool, kb_id, pending_id, Some(user.id))
                .await?;
            let _ = utopia_store::audit::record(
                &state.pool,
                Some(kb_id),
                user.id,
                "fact.nod_rejected",
                "pending_fact",
                Some(pending_id),
                snap,
            )
            .await;
            state.emit_pending(kb_id);
            state.emit_review(kb_id);
            Ok(Json(json!({ "ok": true })))
        }
        other => Err(utopia_core::AppError::invalid(
            "unknown_action",
            format!("action must be confirm or reject, got {other}"),
        )
        .into()),
    }
}

pub async fn run_inference(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let on: bool =
        sqlx::query_scalar("SELECT materialize_inferences FROM knowledge_bases WHERE id = $1")
            .bind(kb_id)
            .fetch_one(&state.pool)
            .await
            .map_err(utopia_core::AppError::Db)?;
    if !on {
        return Err(utopia_core::AppError::invalid(
            "inference_off",
            "materialized inference is off for this knowledge base",
        )
        .into());
    }
    let report = utopia_store::reasoning::materialize(&state.pool, kb_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "inference.materialized",
        "knowledge_base",
        Some(kb_id),
        json!({
            "rules": report.rules,
            "edges": report.edges,
            "derived": report.derived,
            "inserted": report.inserted,
            "invalidated": report.invalidated,
            "capped": report.capped,
        }),
    )
    .await;
    state.emit_graph(kb_id);
    Ok(Json(json!({
        "rules": report.rules,
        "edges": report.edges,
        "derived": report.derived,
        "inserted": report.inserted,
        "invalidated": report.invalidated,
        "capped": report.capped,
    })))
}
