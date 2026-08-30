mod admin_routes;
mod alerts_routes;
mod auth_routes;
mod chat;
mod datasource_routes;
mod documents_routes;
mod events_routes;
mod graph_routes;
mod kbs;
mod members_routes;
pub(crate) mod ontology_routes;
mod review_routes;
mod search_routes;
mod settings_routes;
mod sources_routes;
mod workspaces;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, Method};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use utopia_core::config::AppConfig;

use crate::state::AppState;

const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;

pub fn router(state: AppState, cfg: &AppConfig) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/auth/register", post(auth_routes::register))
        .route("/auth/login", post(auth_routes::login))
        .route("/auth/logout", post(auth_routes::logout))
        // 告警中心：跨库，不挂在 /kbs/{id} 下面
        .route("/alerts", get(alerts_routes::list))
        .route("/alerts/unread", get(alerts_routes::unread))
        .route("/alerts/read-all", post(alerts_routes::mark_all_read))
        .route("/alerts/read-group", post(alerts_routes::mark_group_read))
        .route("/alerts/events", get(alerts_routes::stream))
        .route(
            "/auth/me",
            get(auth_routes::me).patch(auth_routes::update_me),
        )
        .route("/auth/password", post(auth_routes::change_password))
        .route(
            "/workspaces",
            get(workspaces::list).post(workspaces::create),
        )
        .route(
            "/workspaces/{id}",
            get(workspaces::get_one)
                .patch(workspaces::rename)
                .delete(workspaces::delete),
        )
        .route("/workspaces/{id}/kbs", get(kbs::list).post(kbs::create))
        // 建库界面用的静态清单，与具体工作区无关
        .route("/ontology-packs", get(kbs::list_packs))
        .route("/workspaces/{id}/my-kbs", get(kbs::my_kbs))
        .route(
            "/workspaces/{id}/settings",
            get(settings_routes::get).put(settings_routes::put),
        )
        .route(
            "/workspaces/{id}/settings/test",
            post(settings_routes::test),
        )
        .route("/workspaces/{id}/members", get(members_routes::list))
        .route(
            "/workspaces/{id}/members/{user_id}",
            axum::routing::put(members_routes::set_role).delete(members_routes::remove),
        )
        .route("/users", get(members_routes::org_users))
        .route(
            "/kbs/{id}",
            patch(kbs::update).get(kbs::get_one).delete(kbs::delete),
        )
        .route("/kbs/{id}/members", get(kbs::members))
        .route("/kbs/{id}/audit", get(kbs::audit_log))
        .route(
            "/kbs/{id}/members/{user_id}",
            axum::routing::put(kbs::set_member).delete(kbs::remove_member),
        )
        .route(
            "/admin/deployment",
            get(admin_routes::get_deployment).put(admin_routes::put_deployment),
        )
        .route("/admin/users", post(admin_routes::create_user))
        // 停用 / 恢复账号（见 `users.deactivated_at`）。DELETE 的语义是「这个人不再有访问权」,
        // 而不是「这一行没了」——归因照旧查得到
        .route(
            "/admin/users/{id}",
            axum::routing::delete(admin_routes::deactivate_user)
                .post(admin_routes::reactivate_user),
        )
        .route(
            "/admin/data-sources",
            get(datasource_routes::list).post(datasource_routes::create),
        )
        .route(
            "/admin/data-sources/{id}",
            axum::routing::delete(datasource_routes::delete),
        )
        .route(
            "/admin/data-sources/{id}/test",
            post(datasource_routes::test),
        )
        .route("/kbs/{id}/data-sources", get(datasource_routes::mounted))
        .route(
            "/kbs/{id}/data-sources/available",
            get(datasource_routes::mountable),
        )
        .route(
            "/kbs/{id}/data-sources/{ds_id}",
            axum::routing::put(datasource_routes::mount).delete(datasource_routes::unmount),
        )
        .route(
            "/kbs/{id}/data-sources/{ds_id}/sync-schema",
            post(datasource_routes::sync_schema),
        )
        .route(
            "/kbs/{id}/data-sources/explore",
            post(datasource_routes::explore),
        )
        .route(
            "/kbs/{id}/documents",
            get(documents_routes::list)
                .post(documents_routes::upload)
                .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route(
            "/kbs/{id}/extraction-drops",
            get(documents_routes::extraction_drops),
        )
        .route("/kbs/{id}/ontology", get(ontology_routes::get))
        .route(
            "/kbs/{id}/ontology/type-resolution/preview",
            post(ontology_routes::type_resolution_preview),
        )
        .route(
            "/kbs/{id}/ontology/type-resolution",
            post(ontology_routes::type_resolution_apply),
        )
        .route(
            "/kbs/{id}/ontology/type-resolution/approve",
            post(ontology_routes::approve_refinement),
        )
        .route(
            "/kbs/{id}/ontology/type-resolution/{batch_id}",
            axum::routing::delete(ontology_routes::type_resolution_undo),
        )
        .route(
            "/kbs/{id}/ontology/entity-types",
            post(ontology_routes::create_entity_type),
        )
        .route(
            "/kbs/{id}/ontology/entity-types/{type_id}",
            patch(ontology_routes::update_entity_type).delete(ontology_routes::delete_entity_type),
        )
        .route(
            "/kbs/{id}/ontology/entity-types/{type_id}/entities",
            get(ontology_routes::list_entity_instances),
        )
        .route(
            "/kbs/{id}/ontology/relation-types",
            post(ontology_routes::create_relation_type),
        )
        .route(
            "/kbs/{id}/ontology/relation-types/{type_id}",
            patch(ontology_routes::update_relation_type)
                .delete(ontology_routes::delete_relation_type),
        )
        .route(
            "/kbs/{id}/ontology/misses/dismiss",
            post(ontology_routes::dismiss_miss),
        )
        .route(
            "/kbs/{id}/ontology/misses/restore",
            post(ontology_routes::restore_miss),
        )
        .route("/kbs/{id}/ontology/suggest", post(ontology_routes::suggest))
        // 上次算出来、还没人表态的那些（见 `ontology_proposals`）。刷新页面靠它，不必重跑模型
        .route(
            "/kbs/{id}/ontology/proposals",
            get(ontology_routes::stored_proposals).post(ontology_routes::decide_proposal),
        )
        // OWL 导入：预览与落库分开两个端点，绝不让上传即改本体。
        // 两者跑同一个 plan——分开的代码路径会分叉，而分叉意味着确认之后
        // 发生的事与刚看过的不一样
        .route(
            "/kbs/{id}/ontology/imports",
            get(ontology_routes::list_imports)
                .post(ontology_routes::apply_import)
                .layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/kbs/{id}/ontology/imports/preview",
            post(ontology_routes::preview_import).layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route(
            "/kbs/{id}/ontology/proposed-predicates",
            get(ontology_routes::proposed_predicates),
        )
        .route(
            "/kbs/{id}/ontology/auto-extension",
            get(ontology_routes::last_auto_extension),
        )
        .route(
            "/kbs/{id}/ontology/adopt-predicate",
            post(ontology_routes::adopt_predicate),
        )
        .route(
            "/kbs/{id}/ontology/adopt-predicate/{batch_id}",
            axum::routing::delete(ontology_routes::unadopt_predicate),
        )
        .route("/kbs/{id}/search", post(search_routes::search))
        .route("/kbs/{id}/chat", post(chat::chat))
        .route("/kbs/{id}/conversations", get(chat::list_conversations))
        .route(
            "/kbs/{id}/conversations/{conversation_id}",
            get(chat::conversation_detail).delete(chat::delete_conversation),
        )
        .route(
            "/documents/{id}",
            get(documents_routes::detail).delete(documents_routes::delete),
        )
        .route("/documents/{id}/extract", post(graph_routes::extract))
        .route("/kbs/{id}/graph/overview", get(graph_routes::overview))
        .route(
            "/kbs/{id}/graph/neighborhood",
            get(graph_routes::neighborhood),
        )
        .route("/kbs/{id}/entities", get(graph_routes::search_entities))
        .route(
            "/kbs/{id}/entities/{entity_id}",
            get(graph_routes::entity_detail).patch(graph_routes::update_entity),
        )
        .route(
            "/kbs/{id}/entities/{entity_id}/history",
            get(graph_routes::entity_history),
        )
        .route(
            "/kbs/{id}/facts/{fact_id}/evidence",
            get(graph_routes::fact_evidence),
        )
        .route("/kbs/{id}/events", get(events_routes::kb_events))
        .route(
            "/kbs/{id}/sources",
            get(sources_routes::list).post(sources_routes::create),
        )
        .route(
            "/kbs/{id}/sources/{source_id}",
            patch(sources_routes::update).delete(sources_routes::delete),
        )
        .route(
            "/kbs/{id}/sources/{source_id}/sync",
            post(sources_routes::sync_now),
        )
        .route(
            "/kbs/{id}/sources/{source_id}/runs",
            get(sources_routes::runs),
        )
        .route(
            "/kbs/{id}/sources/{source_id}/re-extract",
            post(sources_routes::re_extract),
        )
        .route("/kbs/{id}/graph/rebuild", post(graph_routes::rebuild))
        .route(
            "/kbs/{id}/sources/{source_id}/missing/cleanup",
            post(sources_routes::cleanup_missing),
        )
        .route(
            "/documents/{id}/extractions",
            get(documents_routes::extractions),
        )
        .route("/kbs/{id}/ingest", post(sources_routes::ingest))
        // api 来源推送：来源专属密钥认证（Bearer），无会话
        .route("/sources/{source_id}/ingest", post(sources_routes::push))
        .route(
            "/kbs/{id}/sources/{source_id}/token",
            get(sources_routes::get_token),
        )
        .route(
            "/kbs/{id}/sources/{source_id}/rotate-token",
            post(sources_routes::rotate_token),
        )
        .route("/kbs/{id}/review", get(review_routes::list))
        .route("/kbs/{id}/review/history", get(review_routes::history))
        .route("/kbs/{id}/review/{review_id}", post(review_routes::decide))
        // 语义层映射的表态（0011）。跟消解审核并排——都是「引擎提议、人裁决」
        .route(
            "/kbs/{id}/review/mappings/{mapping_id}",
            post(review_routes::decide_mapping),
        )
        .route(
            "/kbs/{id}/facts/{fact_id}/confirm",
            post(review_routes::confirm_fact),
        )
        .route(
            "/kbs/{id}/facts/{fact_id}/reject",
            post(review_routes::reject_fact),
        )
        .route(
            "/kbs/{id}/facts/{fact_id}/close",
            post(review_routes::close_fact),
        )
        .route(
            "/kbs/{id}/merges/{merge_id}/revert",
            post(review_routes::revert_merge),
        )
        .route(
            "/kbs/{id}/conflicts/{conflict_id}",
            post(review_routes::resolve_conflict),
        )
        .route(
            "/kbs/{id}/entities/merge",
            post(review_routes::manual_merge),
        )
        .route(
            "/documents/{id}/reprocess",
            post(documents_routes::reprocess),
        )
        .route("/jobs/noop", post(jobs_noop))
        .with_state(state);

    // 开发环境 CORS：Vite dev server 携带 cookie 跨端口访问
    let cors = CorsLayer::new()
        .allow_origin("http://localhost:5173".parse::<HeaderValue>().unwrap())
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::PUT,
        ])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
        .allow_credentials(true);

    let mut app = Router::new().nest("/api/v1", api).layer(cors);

    // SPA 托管：产物存在则挂载，history fallback 到 index.html
    let index = std::path::Path::new(&cfg.web_dist).join("index.html");
    if index.exists() {
        let serve = ServeDir::new(&cfg.web_dist).fallback(ServeFile::new(index));
        app = app.fallback_service(serve);
        tracing::info!("已托管前端产物: {}", cfg.web_dist);
    }

    // 最外层：先把请求来源放进 task-local，之后任何一层写审计都读得到
    app.layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(crate::client_ctx::capture))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "name": "utopia", "version": env!("CARGO_PKG_VERSION") }))
}

/// P0 队列验证端点：入队一个 noop 任务（后续里程碑移除）。
async fn jobs_noop(
    axum::extract::State(state): axum::extract::State<AppState>,
    _user: crate::auth::AuthUser,
) -> crate::error::ApiResult<Json<serde_json::Value>> {
    let id = utopia_store::jobs::enqueue(&state.pool, "noop", json!({})).await?;
    Ok(Json(json!({ "job_id": id })))
}
