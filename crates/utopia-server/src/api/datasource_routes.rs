//! 问数数据源：系统层注册（admin，凭据只进不出）+ 知识库层挂载（库 admin）。
//! 挂载/手动刷新时把目标库的 schema 生成 markdown 摄入 KB（同 key 原地更新），
//! Chat 写 SQL 前可检索到表结构。查询执行的安全闸在刀 2（query_data 工具）。

use axum::extract::{Path, State};
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

fn require_admin(user: &utopia_core::models::User) -> Result<(), AppError> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// 系统层：注册/测试/删除
// ---------------------------------------------------------------------------

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    let sources = utopia_store::datasources::list(&state.pool).await?;
    Ok(Json(json!({ "data_sources": sources })))
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default = "default_engine")]
    pub engine: String,
    pub conn_string: String,
}
fn default_engine() -> String {
    "postgres".into()
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<CreateBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    let id = utopia_store::datasources::create(
        &state.pool,
        &body.name,
        &body.engine,
        &body.conn_string,
        user.id,
    )
    .await?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    utopia_store::datasources::delete(&state.pool, id).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn test(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    let (engine, conn) = utopia_store::datasources::engine_and_conn(&state.pool, id).await?;
    let ok = match crate::query_engine::engine_for(&engine, &conn) {
        Ok(eng) => eng.test().await.is_ok(),
        Err(_) => false,
    };
    utopia_store::datasources::record_test(&state.pool, id, ok).await?;
    Ok(Json(json!({ "ok": ok })))
}

// ---------------------------------------------------------------------------
// 知识库层：挂载/卸载/schema 刷新
// ---------------------------------------------------------------------------

pub async fn mounted(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let mounted = utopia_store::datasources::mounted(&state.pool, kb_id).await?;
    Ok(Json(json!({ "data_sources": mounted })))
}

/// 库 admin 从系统已注册的源里挑选挂载（列表给 name/summary，不含凭据）。
pub async fn mountable(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Admin).await?;
    let all = utopia_store::datasources::list(&state.pool).await?;
    Ok(Json(json!({ "data_sources": all })))
}

pub async fn mount(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, ds_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Admin).await?;
    utopia_store::datasources::mount(&state.pool, kb_id, ds_id).await?;
    // 挂载即摄取 schema：Chat 写 SQL 前能检索到表结构
    let synced = sync_schema_doc(&state, kb_id, ds_id)
        .await
        .map_err(AppError::Other)?;
    Ok(Json(json!({ "ok": true, "schema_tables": synced })))
}

pub async fn unmount(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, ds_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Admin).await?;
    utopia_store::datasources::unmount(&state.pool, kb_id, ds_id).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn sync_schema(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, ds_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Admin).await?;
    let synced = sync_schema_doc(&state, kb_id, ds_id)
        .await
        .map_err(AppError::Other)?;
    Ok(Json(json!({ "ok": true, "schema_tables": synced })))
}

/// Agentic 探索：后台任务读挂载源 schema，提议 指标/维度→字段 映射（低置信入 Review）。
pub async fn explore(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Admin).await?;
    if utopia_store::datasources::mounted(&state.pool, kb_id)
        .await?
        .is_empty()
    {
        return Err(AppError::invalid("no_data_sources", "No data sources mounted").into());
    }
    utopia_store::jobs::enqueue(&state.pool, "explore_mappings", json!({ "kb_id": kb_id })).await?;
    Ok(Json(json!({ "ok": true })))
}

/// 拉 information_schema 生成 markdown，走三路判定摄入（同 key 原地更新）。
/// 文档挂在 per-KB 的 "Data schemas" folder 来源下。
async fn sync_schema_doc(state: &AppState, kb_id: Uuid, ds_id: Uuid) -> anyhow::Result<usize> {
    const MAX_TABLES: usize = 200;
    let name = utopia_store::datasources::list(&state.pool)
        .await?
        .into_iter()
        .find(|d| d.id == ds_id)
        .map(|d| d.name)
        .ok_or_else(|| anyhow::anyhow!("Data source not found"))?;
    let (engine, conn) = utopia_store::datasources::engine_and_conn(&state.pool, ds_id).await?;
    let cols = crate::query_engine::engine_for(&engine, &conn)?
        .fetch_schema()
        .await?;

    let mut md = format!(
        "# Data source: {name}\n\nTables and columns available for SQL queries against this source.\n"
    );
    let mut current = String::new();
    let mut tables = 0usize;
    for c in &cols {
        let key = format!("{}.{}", c.schema, c.table);
        if key != current {
            if tables >= MAX_TABLES {
                md.push_str("\n(further tables omitted)\n");
                break;
            }
            current = key.clone();
            tables += 1;
            md.push_str(&format!("\n## {key}\n"));
        }
        md.push_str(&format!(
            "- {} ({}){}\n",
            c.column,
            c.data_type,
            c.comment
                .as_deref()
                .map(|x| format!(" — {x}"))
                .unwrap_or_default()
        ));
    }

    // per-KB "Data schemas" 容器来源（folder：纯容器语义）
    let folder = match sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM sources WHERE kb_id = $1 AND kind = 'folder' AND name = 'Data schemas'",
    )
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await?
    {
        Some((id,)) => id,
        None => {
            utopia_store::sources::create(
                &state.pool,
                kb_id,
                "folder",
                "Data schemas",
                &serde_json::json!({}),
                Some("database"),
                None,
                None,
            )
            .await?
            .id
        }
    };
    crate::ingest_sources::ingest_item(
        state,
        kb_id,
        folder,
        &format!("datasource:{ds_id}:schema"),
        &format!("{name}-schema.md"),
        "text/markdown",
        md.as_bytes(),
        None,
    )
    .await?;
    state.emit_source(kb_id);
    Ok(tables)
}
