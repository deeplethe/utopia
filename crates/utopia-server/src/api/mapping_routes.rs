//! 数据映射 API：口径的列表、改写与改版历史。
//!
//! 审批（`decide`）留在 `review_routes`——那条端点已经在写审计流水
//! （`mapping.decided`），换个界面调它即可，没有理由为了搬页面而搬端点。

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_core::AppError;
use uuid::Uuid;

use super::graph_routes::require_kb;
use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

/// 一页多少条。口径比审阅队列密（一行就是一个定义），一页给得起 25 条
const MAPPING_PAGE: i64 = 25;

#[derive(Deserialize)]
pub struct ListQuery {
    /// proposed | confirmed | rejected；缺省 = 全部
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// 一页口径 + 三种状态各多少。
///
/// **Viewer 就能看。** 口径是「这个数怎么算」，问数的答案直接由它决定——
/// 看得见答案却看不见口径，等于要人信一个不给看的算法。改（`revise`）
/// 才要 Editor。
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let status = q.status.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = status {
        if !matches!(s, "proposed" | "confirmed" | "rejected") {
            return Err(AppError::invalid(
                "bad_status",
                "status must be proposed, confirmed or rejected",
            )
            .into());
        }
    }
    let needle = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let limit = q.limit.unwrap_or(MAPPING_PAGE).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);

    let (items, total) =
        utopia_store::mappings::page(&state.pool, kb_id, status, needle, limit, offset).await?;
    let (proposed, confirmed, rejected) =
        utopia_store::mappings::status_counts(&state.pool, kb_id).await?;
    // 上一轮探索的账（#503）。**列表本身答不了「漏了多少」**——十一条提议对着
    // 一张八十列的宽表，与十一条刚好覆盖完一个小库，在 items 里长得一模一样
    let last_run = utopia_store::exploration_runs::recent(&state.pool, kb_id, 1)
        .await?
        .into_iter()
        .next();
    Ok(Json(json!({
        "items": items,
        "total": total,
        "counts": { "proposed": proposed, "confirmed": confirmed, "rejected": rejected },
        "last_run": last_run,
    })))
}

#[derive(Deserialize)]
pub struct ReviseReq {
    table_name: Option<String>,
    expr: Option<String>,
    sql: Option<String>,
    unit: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    derived: bool,
}

/// 改一条口径。
///
/// **在此之前 `mappings::revise` 是零调用的**：函数在、留痕表在、没有路由。
/// 于是口径确认之后就再没人改得动，也没人看得见——问数照着它算，人却够不着。
pub async fn revise(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, mapping_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<ReviseReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    // 空白等于没填：前端清空一个输入框传来的是 ""，落库该是 NULL 而不是空串，
    // 否则「有没有配 expr」这个判断要同时问 IS NULL 和 = ''
    let clean = |s: &Option<String>| -> Option<String> {
        s.as_deref()
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
    };
    let (table_name, expr, sql, unit, summary) = (
        clean(&req.table_name),
        clean(&req.expr),
        clean(&req.sql),
        clean(&req.unit),
        clean(&req.summary),
    );
    if table_name.is_none() && expr.is_none() && sql.is_none() {
        return Err(AppError::invalid(
            "empty_mapping",
            "A mapping needs at least one of table, expression or SQL",
        )
        .into());
    }
    utopia_store::mappings::revise(
        &state.pool,
        kb_id,
        mapping_id,
        table_name.as_deref(),
        expr.as_deref(),
        sql.as_deref(),
        unit.as_deref(),
        summary.as_deref(),
        req.derived,
        user.id,
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "mapping.revised",
        "concept_mapping",
        Some(mapping_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// 一条口径的改版历史。0006 说留痕是为了答得出「上季度这个数是怎么算的」，
/// 这是那句话的兑现处。
pub async fn revisions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, mapping_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let rows = utopia_store::mappings::revisions(&state.pool, kb_id, mapping_id).await?;
    Ok(Json(json!({ "revisions": rows })))
}

#[derive(Deserialize)]
pub struct RelevantQuery {
    q: String,
    k: Option<usize>,
}

/// 跟一个问题有关的口径，按相关度排——问数进提示词用的正是这一条检索（#574）。
///
/// 开成端点是为了两件事：测量台直接量 recall@k（那条对的口径在不在前 k 里），
/// 不用真的问一遍；页面以后能在问题旁边列出「用到的口径」。Viewer 就能看，
/// 与列表同一个理由——看得见答案却看不见口径，等于要人信一个不给看的算法。
pub async fn relevant(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<RelevantQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let kb = require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let k =
        q.k.unwrap_or(crate::mapping_index::DEFINITIONS_IN_PROMPT)
            .clamp(1, 50);
    let items = crate::mapping_index::relevant(&state, kb_id, kb.workspace_id, &q.q, k)
        .await
        .map_err(AppError::Other)?;
    Ok(Json(json!({ "items": items, "k": k })))
}

/// 空白等于没填：前端清空一个输入框传来的是 ""，落库该是 NULL 而不是空串。
fn clean(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

/// 一条口径至少得有表、表达式、SQL 之一，否则它不是一个可执行的定义。
/// `create`、`preview` 与 `revise` 判的是同一条。
fn definition_shape(
    table_name: &Option<String>,
    expr: &Option<String>,
    sql: &Option<String>,
) -> Result<(), AppError> {
    if table_name.is_none() && expr.is_none() && sql.is_none() {
        return Err(AppError::invalid(
            "empty_mapping",
            "A mapping needs at least one of table, expression or SQL",
        ));
    }
    Ok(())
}

/// 一条口径跑起来是哪句 SQL：给了 `sql` 用 `sql`，否则 `expr` + `table` 拼一句。
/// 测量台（`scripts/bench/mappings.mjs` 的 `proposalSql`）判分用的是同一个拼法，
/// 所以页面上预览到的数就是测量台会打分的数。
fn render_sql(
    table_name: &Option<String>,
    expr: &Option<String>,
    sql: &Option<String>,
) -> Option<String> {
    if let Some(s) = sql {
        return Some(s.trim_end_matches(';').trim().to_string());
    }
    match (expr, table_name) {
        (Some(e), Some(t)) => Some(format!("SELECT {e} FROM {t}")),
        _ => None,
    }
}

#[derive(Deserialize)]
pub struct CreateReq {
    concept: String,
    /// metric | dimension；缺省 metric
    #[serde(default = "default_kind")]
    kind: String,
    source: String,
    table_name: Option<String>,
    expr: Option<String>,
    sql: Option<String>,
    unit: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    derived: bool,
}
fn default_kind() -> String {
    "metric".into()
}

/// 人从零写一条口径（#562）。
///
/// 从前这张表只有探索一条来路。一个数据团队手上有自己的指标口径文档，却没有地方
/// 把它填进去——而口径进了问数的提示词，宽表语料从 1/18 到 17/18（#520）。缺的
/// 不是结构，是这扇门。
///
/// 概念仍落成 Metric / Dimension 类的实体——那是这张表的现状（`concept_id` 非空
/// 指向实体）。0035 要退役这个形状（#556），但保留这张表并往里渲染，人写的 SQL
/// 搬得过去。
pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<CreateReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let concept = req.concept.trim();
    if concept.is_empty() {
        return Err(AppError::invalid("empty_concept", "A definition needs a concept name").into());
    }
    if !matches!(req.kind.as_str(), "metric" | "dimension") {
        return Err(AppError::invalid("bad_kind", "kind must be metric or dimension").into());
    }
    let (table_name, expr, sql, unit, summary) = (
        clean(&req.table_name),
        clean(&req.expr),
        clean(&req.sql),
        clean(&req.unit),
        clean(&req.summary),
    );
    definition_shape(&table_name, &expr, &sql)?;
    // 源按名字点单，且必须是本库挂载的——与 `query_data` 同一条安全边界
    let mounted = utopia_store::datasources::mounted(&state.pool, kb_id).await?;
    let Some(ds) = mounted
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(req.source.trim()))
    else {
        return Err(AppError::invalid(
            "source_not_mounted",
            "That data source is not mounted on this knowledge base",
        )
        .into());
    };

    crate::mappings::ensure_concept_types(&state.pool, kb_id)
        .await
        .map_err(AppError::Other)?;
    let (type_id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM entity_types WHERE kb_id = $1 AND key = $2")
            .bind(kb_id)
            .bind(&req.kind)
            .fetch_one(&state.pool)
            .await?;
    // 概念实体走消解：同名归并，跟探索提的那些落在一处
    let resolved = utopia_store::resolution::resolve_mention(
        &state.pool,
        kb_id,
        Some(type_id),
        concept,
        None,
        None,
        None,
        &[],
    )
    .await?;
    let id = utopia_store::mappings::create(
        &state.pool,
        kb_id,
        resolved.entity_id,
        &ds.name,
        table_name.as_deref(),
        expr.as_deref(),
        sql.as_deref(),
        unit.as_deref(),
        summary.as_deref(),
        req.derived,
        user.id,
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "mapping.written",
        "concept_mapping",
        Some(id),
        json!({ "concept": concept, "source": ds.name }),
    )
    .await;
    Ok(Json(json!({ "id": id, "concept_id": resolved.entity_id })))
}

#[derive(Deserialize)]
pub struct PreviewReq {
    source: String,
    table_name: Option<String>,
    expr: Option<String>,
    sql: Option<String>,
}

/// 把一条口径过只读闸跑一遍，回第一行——写口径的人当场看到它算出来的数。
///
/// 这正是测量台判分的那个动作（#501 / #520：把定义跑一遍，跟 gold 比数），
/// 搬到页面上来。跑不通的定义无害，它失败得很响；跑得通而算错的才是全部风险，
/// 而看一眼数是人能做的唯一一道检查。
pub async fn preview(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<PreviewReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let (table_name, expr, sql) = (clean(&req.table_name), clean(&req.expr), clean(&req.sql));
    definition_shape(&table_name, &expr, &sql)?;
    let Some(rendered) = render_sql(&table_name, &expr, &sql) else {
        return Err(AppError::invalid(
            "empty_mapping",
            "A preview needs SQL, or an expression together with a table",
        )
        .into());
    };
    let mounted = utopia_store::datasources::mounted(&state.pool, kb_id).await?;
    let Some(ds) = mounted
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(req.source.trim()))
    else {
        return Err(AppError::invalid(
            "source_not_mounted",
            "That data source is not mounted on this knowledge base",
        )
        .into());
    };
    // 引擎的报错原样带回：列名拼错、语法错、超时是三件不同的事
    let out = super::tools::run_query(&state, ds.id, &rendered)
        .await
        .map_err(|e| AppError::invalid("preview_failed", e.to_string()))?;
    // 结果是 JSON Lines；第一行就是这条口径算出来的那个数（或那一组数）
    let rows: Vec<serde_json::Value> = out
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .take(5)
        .collect();
    Ok(Json(
        json!({ "sql": rendered, "row": rows.first(), "rows": rows }),
    ))
}

#[cfg(test)]
mod tests {
    use super::{definition_shape, render_sql};

    #[test]
    fn a_definition_needs_something_to_run() {
        let none: Option<String> = None;
        assert!(definition_shape(&none, &none, &none).is_err());
        assert!(definition_shape(&Some("t".into()), &none, &none).is_ok());
        assert!(definition_shape(&none, &Some("sum(x)".into()), &none).is_ok());
        assert!(definition_shape(&none, &none, &Some("SELECT 1".into())).is_ok());
    }

    #[test]
    fn preview_runs_what_the_bench_scores() {
        let none: Option<String> = None;
        // sql 优先，且去掉结尾的分号——闸门只放行单条语句
        assert_eq!(
            render_sql(
                &Some("t".into()),
                &Some("sum(x)".into()),
                &Some("SELECT 2;".into())
            )
            .as_deref(),
            Some("SELECT 2")
        );
        assert_eq!(
            render_sql(
                &Some("dw.orders".into()),
                &Some("sum(amt) / 100.0".into()),
                &none
            )
            .as_deref(),
            Some("SELECT sum(amt) / 100.0 FROM dw.orders")
        );
        // 只有表名不是一个可执行的口径
        assert_eq!(render_sql(&Some("t".into()), &none, &none), None);
    }
}
