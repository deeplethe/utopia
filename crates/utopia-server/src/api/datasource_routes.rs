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
    // 引擎跟着 scheme 走，界面只有一个连接串输入框；body.engine 只为兼容旧调用留着
    let engine = crate::query_engine::engine_from_conn(&body.conn_string).ok_or_else(|| {
        utopia_core::AppError::invalid(
            "unsupported_conn_scheme",
            format!(
                "Connection string must start with one of: postgres://, trino://, databricks://, snowflake:// (engines: {})",
                crate::query_engine::ENGINES.join(", ")
            ),
        )
    })?;
    let _ = &body.engine;
    // 连接串的形状在登记时就校验（缺令牌、缺 warehouse……），错误信息里带写法；
    // 否则要等到「测试」才知道，而那一步只回 ok:false
    crate::query_engine::engine_for(engine, &body.conn_string)
        .map_err(|e| utopia_core::AppError::invalid("bad_conn_string", e.to_string()))?;
    let id = utopia_store::datasources::create(
        &state.pool,
        &body.name,
        engine,
        &body.conn_string,
        user.id,
    )
    .await?;
    // **登记完就能挂。** 授权是按工作区的（0014），可工作区在界面上已经隐形——
    // 单租户部署只有一个，谁也没见过它的名字。从前登记完还要在卡片上先
    // 「授权给工作区」，等于让人授权一件从没见过的东西，挂载时只得到一句
    // 「未授权」。所以登记人所在的每个工作区一并授权；要收窄，卡片上仍能撤销
    for ws in utopia_store::workspaces::list_for_user(&state.pool, user.id).await? {
        utopia_store::datasources::grant(&state.pool, id, ws.id, user.id).await?;
    }
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

/// 库 admin 能挂哪些（列表给 name/summary，不含凭据）。
///
/// **只列授权给本工作区的（0014）。** 从前这里返回 `datasources::list`——
/// 全部署每一个源，于是任何库的管理员都看得见并挂得上任意生产库。
pub async fn mountable(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let kb = require_kb(&state, &user, kb_id, Role::Admin).await?;
    let granted =
        utopia_store::datasources::granted_to_workspace(&state.pool, kb.workspace_id).await?;
    Ok(Json(json!({ "data_sources": granted })))
}

pub async fn mount(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, ds_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Admin).await?;
    // **列表过滤不是守卫。** 那只挡「看得见」，而这个端点是照着 id 调的——
    // 谁都能自己拼一个 uuid 打过来。授权在这里再查一次
    if !utopia_store::datasources::is_granted(&state.pool, kb_id, ds_id).await? {
        return Err(AppError::invalid(
            "source_not_granted",
            "This data source is not available to this workspace",
        )
        .into());
    }
    utopia_store::datasources::mount(&state.pool, kb_id, ds_id).await?;
    // 挂载即摄取 schema：Chat 写 SQL 前能检索到表结构
    //
    // **这一步失败不能回报成挂载失败。** 上面那行已经写进 kb_data_sources 了，
    // 源是真挂着的；从前这里 `?` 出去回 500，人以为没挂上，实际挂上了——
    // 而问数看不见它有哪些表。改成：照实说挂载成了，schema 没成，并且
    // 报进告警中心，因为那之后就是一个静默的缺失状态（0009）
    match sync_schema_doc(&state, kb_id, ds_id).await {
        Ok(synced) => Ok(Json(json!({ "ok": true, "schema_tables": synced }))),
        Err(e) => {
            let name = source_name(&state, ds_id).await;
            crate::alerting::observe_schema_sync_failure(&state, kb_id, ds_id, &name, &e).await;
            Ok(Json(json!({
                "ok": true,
                "schema_tables": 0,
                "schema_error": e.to_string(),
            })))
        }
    }
}

/// 告警要留住名字：源被删之后 `subject_id` 就解析不出名字了。查不到时给占位符
/// 而不是让告警本身失败——**报警路径上的失败不该淹掉它要报的那件事**。
async fn source_name(state: &AppState, ds_id: Uuid) -> String {
    utopia_store::datasources::list(&state.pool)
        .await
        .ok()
        .and_then(|all| all.into_iter().find(|d| d.id == ds_id).map(|d| d.name))
        .unwrap_or_else(|| ds_id.to_string())
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

/// 手动刷新结构。
///
/// **这里照旧把错误回给调用方**——点按钮的人正看着，而且什么都没半途发生。
/// 但同样报一条告警：留下的后果与挂载失败时一模一样（源挂着、表结构是旧的
/// 或空的），而点按钮的人未必是需要知道这件事的人。
pub async fn sync_schema(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, ds_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Admin).await?;
    match sync_schema_doc(&state, kb_id, ds_id).await {
        Ok(synced) => Ok(Json(json!({ "ok": true, "schema_tables": synced }))),
        Err(e) => {
            let name = source_name(&state, ds_id).await;
            crate::alerting::observe_schema_sync_failure(&state, kb_id, ds_id, &name, &e).await;
            Err(AppError::Other(e).into())
        }
    }
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
        "# Data source: {name}\n\nEngine: {engine}. Tables and columns available for SQL queries against this source; write SQL in this engine's dialect.\n"
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

// ---------------------------------------------------------------------------
// 系统层：授权（0014）
//
// 授权与挂载是两层，各有各的主人：
//   授权 = 系统管理员说「这个源可以给哪些工作区用」  ← 这里
//   挂载 = KB 管理员说「我这个库挂哪几个」          ← 上面那组
// 两层都是多对多。挂载只能在授权过的集合里挑。
// ---------------------------------------------------------------------------

/// 这个源授权给了哪些工作区。
pub async fn grants(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    let rows = utopia_store::datasources::grants_for_source(&state.pool, id).await?;
    let workspaces: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    Ok(Json(json!({ "workspaces": workspaces })))
}

pub async fn grant(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, workspace_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    utopia_store::datasources::grant(&state.pool, id, workspace_id, user.id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "data_source.granted",
        "data_source",
        Some(id),
        json!({ "workspace_id": workspace_id }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// 收回授权。**连同该工作区里已挂上的一起卸掉**——只删授权行的话，
/// 问数读的还是 `kb_data_sources`，撤销就不生效。返回卸掉了几个，
/// 好让界面说得出「顺带卸了 3 个库」而不是悄悄断人家的连接。
pub async fn revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, workspace_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    let unmounted = utopia_store::datasources::revoke(&state.pool, id, workspace_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "data_source.revoked",
        "data_source",
        Some(id),
        json!({ "workspace_id": workspace_id, "unmounted": unmounted }),
    )
    .await;
    Ok(Json(json!({ "ok": true, "unmounted": unmounted })))
}
