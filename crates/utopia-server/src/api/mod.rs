mod admin_routes;
mod agent;
mod alerts_routes;
mod auth_routes;
mod chat;
mod datasource_routes;
pub(crate) mod documents_routes;
mod events_routes;
mod export_routes;
mod graph_routes;
mod jobs_routes;
mod kbs;
mod mapping_routes;
mod mcp;
mod members_routes;
pub(crate) mod ontology_routes;
mod review_routes;
mod rig_model;
pub(crate) mod rule_routes;
mod search_routes;
mod settings_routes;
mod sources_routes;
#[cfg(test)]
mod static_files_tests;
mod token_routes;
mod tools;
mod tools_graph;
mod workspaces;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, Method};
use axum::routing::{any, get, patch, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use utopia_core::config::AppConfig;
use utopia_core::error::AppError;

use crate::error::ApiErr;

use crate::state::AppState;

const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;

/// 把构建产物挂上去。**单独一段、而且不带 state**，所以它可以脱开数据库单测：
/// 这一段的契约（哪条路 404、哪条路回首页、各自的缓存指示）已经破过两次，
/// 一次是升级白屏（#616），一次是把 404 也存一年。
fn with_static_files(mut app: Router, web_dist: &str) -> Router {
    /* SPA 托管。**分两条路，因为它们的失败方式不一样**：

    `/assets` 下面是构建产物，文件名带内容哈希。这里的 ServeDir **不带兜底**，
    所以缺文件就是 404。从前它和页面共用一个带 history fallback 的服务，于是
    升级之后旧哈希的请求拿到的是 `200 text/html` 的首页——浏览器按模块脚本
    解析一张网页，光凭 MIME 就拒绝执行，界面白屏（#616）。

    缓存指示按文件名本来的含义给：带哈希的产物换了内容就换名字，可以
    `immutable` 存一年；`index.html` 每次都要回源确认，否则它会指着一批
    已经不存在的哈希。少了这一条，升级要等浏览器的启发式缓存自己过期。 */
    let index = std::path::Path::new(web_dist).join("index.html");
    if index.exists() {
        /* 每条路各自套自己的头。**层要挂在这一段的 Router 上，不能挂在整个
        app 上**——挂在 app 上，API 的响应也会跟着被扣上 `immutable`。 */
        let assets = Router::new()
            .fallback_service(ServeDir::new(std::path::Path::new(web_dist).join("assets")))
            /* **只有拿到东西的那一次才 `immutable`。**这个头从前是无条件套的，
            于是 `/assets/<不存在的文件>` 的 404 也带着「存一年」——浏览器（和
            中间的缓存）会把「这个文件不存在」记一年。而资源文件名带哈希，
            一次部署之后请求的正是新名字：上一秒刚缓存下来的那条 404 会让
            新版本的 js 在那台机器上整整一年拿不到。这正是 #616 要防的那条链，
            只是发生在另一头。拿不到的那一次给 `no-cache`，下一次照常回源。 */
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                |res: &axum::response::Response| {
                    Some(if res.status().is_success() {
                        HeaderValue::from_static("public, max-age=31536000, immutable")
                    } else {
                        HeaderValue::from_static("no-cache")
                    })
                },
            ));
        let spa = Router::new()
            .fallback_service(ServeDir::new(web_dist).fallback(ServeFile::new(index)))
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            ));
        app = app.nest("/assets", assets).fallback_service(spa);
        tracing::info!("已托管前端产物: {}", web_dist);
    }
    app
}

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
        // 已停用的账号：没有这一条，恢复就够不着（那个人从所有列表里消失）
        .route("/users/deactivated", get(members_routes::deactivated_users))
        .route(
            "/kbs/{id}",
            patch(kbs::update).get(kbs::get_one).delete(kbs::delete),
        )
        // 四个页面的空状态共用的一步判断（#313）
        .route("/kbs/{id}/readiness", get(kbs::readiness))
        .route("/kbs/{id}/members", get(kbs::members))
        .route("/kbs/{id}/audit", get(kbs::audit_log))
        // 整库导出为 RDF（0020）。viewer 就能导：能看见的东西本来就能一条条抄走，
        // 拦在这里只是让诚实的人多花点力气
        .route("/kbs/{id}/export", get(export_routes::export))
        // 失败任务回队列（#216）：库内给 Editor，全局给管理员
        .route("/kbs/{id}/jobs/failed", get(jobs_routes::failed_in_kb))
        .route("/kbs/{id}/jobs/requeue", post(jobs_routes::requeue_in_kb))
        .route("/jobs/requeue", post(jobs_routes::requeue_all))
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
        // 静态段排在 `{id}` 前面：存之前先试一次连接，不落库
        .route(
            "/admin/data-sources/test",
            post(datasource_routes::test_conn),
        )
        .route(
            "/admin/data-sources/{id}/test",
            post(datasource_routes::test),
        )
        .route(
            "/admin/data-sources/{id}/grants",
            get(datasource_routes::grants),
        )
        .route(
            "/admin/data-sources/{id}/grants/{workspace_id}",
            axum::routing::put(datasource_routes::grant).delete(datasource_routes::revoke),
        )
        .route("/kbs/{id}/mcp", post(mcp::handle))
        .route(
            "/me/tokens",
            get(token_routes::list).post(token_routes::issue),
        )
        .route(
            "/me/tokens/{token_id}",
            axum::routing::delete(token_routes::revoke),
        )
        .route(
            "/kbs/{id}/mappings",
            get(mapping_routes::list).post(mapping_routes::create),
        )
        // 静态段排在 `{mapping_id}` 前面：先跑一遍看数，不落库
        .route("/kbs/{id}/mappings/preview", post(mapping_routes::preview))
        // 跟一个问题有关的口径（问数用的同一条检索）
        .route("/kbs/{id}/mappings/relevant", get(mapping_routes::relevant))
        .route(
            "/kbs/{id}/mappings/{mapping_id}",
            axum::routing::patch(mapping_routes::revise),
        )
        .route(
            "/kbs/{id}/mappings/{mapping_id}/revisions",
            get(mapping_routes::revisions),
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
        // 一键重试：抽取失败往往是成批的（模型端点断了一阵，那段时间进来的全挂）
        .route(
            "/kbs/{id}/documents/retry-failed",
            post(documents_routes::retry_failed),
        )
        .route(
            "/kbs/{id}/extraction-drops",
            get(documents_routes::extraction_drops),
        )
        .route("/kbs/{id}/ontology", get(ontology_routes::get))
        // 业务规则（0021）：读规则要 Viewer，写要 Editor
        .route(
            "/kbs/{id}/rules",
            get(rule_routes::list).post(rule_routes::create),
        )
        .route("/kbs/{id}/rules/run", post(rule_routes::run_now))
        .route(
            "/kbs/{id}/rules/{rule_id}",
            patch(rule_routes::update).delete(rule_routes::delete),
        )
        .route(
            "/kbs/{id}/rules/{rule_id}/matches",
            get(rule_routes::matches),
        )
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
        // 声明来晚了（#341）：谁的哪一端挂着两个以上开放值；补上声明后把账对一遍
        .route(
            "/kbs/{id}/ontology/uniqueness",
            get(ontology_routes::uniqueness_candidates),
        )
        .route(
            "/kbs/{id}/ontology/relation-types/{type_id}/reconcile",
            post(ontology_routes::reconcile_relation_type),
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
        // 刷新页面后重新接上正在生成的那个回答（见 `live`）
        .route(
            "/kbs/{id}/conversations/{conversation_id}/stream",
            get(chat::reattach),
        )
        .route("/kbs/{id}/conversations", get(chat::list_conversations))
        .route(
            "/kbs/{id}/conversations/{conversation_id}",
            get(chat::conversation_detail)
                .patch(chat::rename_conversation)
                .delete(chat::delete_conversation),
        )
        .route(
            "/documents/{id}",
            get(documents_routes::detail).delete(documents_routes::delete),
        )
        // 撤销删除（#268）：删除是墓碑，所以有得撤
        .route("/documents/{id}/restore", post(documents_routes::restore))
        // 真删（#268 下半）：只对已删除的开放，库管理员
        .route("/documents/{id}/purge", post(documents_routes::purge))
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
        // 人工修正有效区间（302）：与实体的 PATCH 对称，走账本不原地改
        .route(
            "/kbs/{id}/facts/{fact_id}",
            patch(graph_routes::update_fact_time),
        )
        // 派生事实的证明（0002 R2）：前提按顺序展开到原句
        .route(
            "/kbs/{id}/derived/{derived_id}/proof",
            get(graph_routes::derived_proof),
        )
        // 没落地的派生的证明（0017 §3）：前提在违规的 path 里
        .route(
            "/kbs/{id}/violations/{violation_id}/proof",
            get(graph_routes::blocked_proof),
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
        .route("/kbs/{id}/review/summary", get(review_routes::summary))
        .route("/kbs/{id}/review/batch", post(review_routes::batch))
        .route(
            "/kbs/{id}/review/agent/{decision_id}",
            post(review_routes::agent_answer),
        )
        // 记忆抽出、等人点头的事实（0015）：按句取、逐条裁
        .route(
            "/kbs/{id}/review/pending",
            get(review_routes::pending_for_chunk),
        )
        .route(
            "/kbs/{id}/review/pending/{pending_id}",
            post(review_routes::decide_pending),
        )
        .route("/kbs/{id}/review/{review_id}", post(review_routes::decide))
        // 语义层映射的表态（0011）。跟消解审核并排——都是「引擎提议、人裁决」
        .route(
            "/kbs/{id}/review/mappings/{mapping_id}",
            post(review_routes::decide_mapping),
        )
        // 一致性检查（0002 R0）：跑一遍，与裁决一处违规。
        // 检查本身是纯计算,同步跑
        .route(
            "/kbs/{id}/consistency/check",
            post(review_routes::run_consistency_check),
        )
        .route(
            "/kbs/{id}/review/violations/{violation_id}",
            post(review_routes::decide_violation),
        )
        .route(
            "/kbs/{id}/review/defects/{defect_id}",
            post(review_routes::decide_defect),
        )
        // R1 物化推导。受 KB 上的 materialize_inferences 开关约束
        .route(
            "/kbs/{id}/inference/run",
            post(review_routes::run_inference),
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

    /* **`/api` 下面认不出来的路径是 404，不是首页。**
    下面那个 history fallback 是给页面路由用的：刷新 /graph 要拿到 index.html。
    可它挂在**整个应用**上，于是拼错或已经删掉的接口也落进同一张网，回的是
    `200 text/html`。客户端那头 `res.json()` 抛 "Unexpected token '<'"，
    报错的位置离原因十万八千里；对着 MCP 与 RDF 那两个对外契约的集成方更糟，
    通用 HTTP 客户端看到 200 当成功，解析器看到的是一张网页。
    给嵌套的 API 路由自己一个兜底，形状与其余错误一致（error.rs 里
    `NotFound` 就映射成 404）。 */
    let api = api.fallback(|| async { ApiErr(AppError::NotFound) });
    let mut app = Router::new()
        .nest("/api/v1", api)
        /* 版本号写错、或者干脆忘了写的请求（`/api/kbs`、`/api/v2/...`）落不进上面
        那个嵌套，还是会掉到首页去。**`/api` 底下没有页面**，所以整条前缀都收住；
        静态段比通配更具体，`/api/v1/...` 仍然走上面那个路由。 */
        .route("/api/{*rest}", any(|| async { ApiErr(AppError::NotFound) }))
        .layer(cors);

    app = with_static_files(app, &cfg.web_dist);

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
