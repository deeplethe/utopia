mod adjudication;
mod api;
mod auth;
mod blob;
mod bootstrap_ontology;
mod client_ctx;
mod docs_corpus;
mod error;
mod extraction;
mod ingest_sources;
mod llm_util;
mod mappings;
mod ontology_index;
mod owl_import;
mod pipeline;
mod query_engine;
mod retrieval;
mod state;

use state::AppState;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use utopia_core::config::AppConfig;
use utopia_search::SearchIndex;
use uuid::Uuid;

fn payload_document_id(payload: &serde_json::Value) -> anyhow::Result<Uuid> {
    payload
        .get("document_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("payload 缺少 document_id"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,utopia=debug".into()),
        )
        .init();

    let cfg = AppConfig::load()?;

    // 迁移要建表建触发器，运行时不需要那些权限。两者分开，应用才能用一个
    // 只读写业务表、对台账只增不改的受限角色连库。迁移池用完立即释放，
    // 那个高权限连接不在运行期常驻。
    let migration_url = cfg.migration_url().to_string();
    let separate_migration_role = cfg.migration_url.is_some();
    {
        let mig_pool = utopia_store::db::connect(&migration_url, Some(2)).await?;
        utopia_store::db::migrate(&mig_pool).await?;
        mig_pool.close().await;
    }
    if separate_migration_role {
        tracing::info!("数据库迁移完成（迁移身份与运行身份分离）");
    } else {
        tracing::info!("数据库迁移完成");
    }

    let pool = utopia_store::db::connect(&cfg.database_url, cfg.db_max_connections).await?;

    let index_dir = std::path::Path::new(&cfg.data_dir).join("index");
    let search = Arc::new(SearchIndex::open(&index_dir)?);
    tracing::info!("全文索引就绪: {}", index_dir.display());

    // JWT 密钥：环境变量优先（轮换、多实例显式对齐走这条），否则用库里那条；
    // 库里也没有就现生成一条存进去。生成放在这里而不是 store 里，是因为
    // OsRng 已经随 argon2 在 server 的依赖里，store 不必为此多一个依赖。
    // 空串按未设置处理：compose 里写 ${UTOPIA_JWT_SECRET:-} 时环境变量是存在但为空的，
    // 照字面读会得到 Some("")——一个所有部署都相同的空密钥，比默认值更糟。
    let jwt_secret = match cfg.jwt_secret.clone().filter(|s| !s.trim().is_empty()) {
        Some(s) => s,
        None => {
            let secret =
                utopia_store::access::ensure_jwt_secret(&pool, &auth::generate_jwt_secret())
                    .await?;
            tracing::info!("JWT 密钥取自部署设置（未显式配置 UTOPIA_JWT_SECRET）");
            secret
        }
    };

    let state = AppState::new(pool.clone(), &cfg, search, jwt_secret);

    // worker 并发数：系统设置持久化，启动时装载；运行中经同一 AtomicUsize 热调
    let n = utopia_store::access::worker_concurrency(&pool)
        .await
        .unwrap_or(32);
    state.worker_concurrency.store(
        n.clamp(1, 256) as usize,
        std::sync::atomic::Ordering::Relaxed,
    );

    // 任务分发：新任务类型在这里注册
    let worker_state = state.clone();
    tokio::spawn(utopia_store::jobs::run_worker(
        pool,
        state.worker_concurrency.clone(),
        move |job| {
            let st = worker_state.clone();
            async move {
                match job.kind.as_str() {
                    "noop" => {
                        tracing::info!(job_id = job.id, "noop 任务执行成功");
                        Ok(())
                    }
                    "process_document" => {
                        let id = payload_document_id(&job.payload)?;
                        pipeline::process_document(&st, id).await
                    }
                    "memory_ingest" => {
                        let id = payload_document_id(&job.payload)?;
                        pipeline::memory_ingest(&st, id).await
                    }
                    "explore_mappings" => {
                        let kb_id: Uuid = job
                            .payload
                            .get("kb_id")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(|| anyhow::anyhow!("payload 缺少 kb_id"))?;
                        mappings::explore_mappings(&st, kb_id).await
                    }
                    "extract_document" => {
                        let id = payload_document_id(&job.payload)?;
                        extraction::extract_document(&st, id).await
                    }
                    "bootstrap_ontology" => {
                        let kb_id: Uuid = job
                            .payload
                            .get("kb_id")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(|| anyhow::anyhow!("payload 缺少 kb_id"))?;
                        bootstrap_ontology::bootstrap_ontology(&st, kb_id).await
                    }
                    "adjudicate_entities" => {
                        let kb_id: Uuid = job
                            .payload
                            .get("kb_id")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(|| anyhow::anyhow!("payload 缺少 kb_id"))?;
                        adjudication::adjudicate_entities(&st, kb_id).await
                    }
                    "sync_source" => {
                        let source_id: Uuid = job
                            .payload
                            .get("source_id")
                            .and_then(|v| v.as_str())
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(|| anyhow::anyhow!("payload 缺少 source_id"))?;
                        ingest_sources::sync_source(&st, source_id).await
                    }
                    other => anyhow::bail!("未知任务类型: {other}"),
                }
            }
        },
    ));

    // 定时摄入调度器：每分钟扫一次到期来源，入队同步任务
    let sched_state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            match utopia_store::sources::due_sources(&sched_state.pool).await {
                Ok(due) => {
                    for s in due {
                        match utopia_store::sources::mark_queued(&sched_state.pool, s.id).await {
                            Ok(true) => {
                                if let Err(e) = utopia_store::jobs::enqueue(
                                    &sched_state.pool,
                                    "sync_source",
                                    serde_json::json!({ "source_id": s.id }),
                                )
                                .await
                                {
                                    tracing::warn!(source_id = %s.id, error = %e, "同步任务入队失败");
                                }
                            }
                            Ok(false) => {}
                            Err(e) => tracing::warn!(source_id = %s.id, error = %e, "标记入队失败"),
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "扫描到期来源失败"),
            }
        }
    });

    let app = api::router(state, &cfg);
    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    tracing::info!("Utopia 服务启动于 http://{}", cfg.bind_addr);

    // 浏览器可能把 localhost 解析为 ::1（IPv6）——配置为 IPv4 地址时补一个同端口的
    // IPv6 回环监听，避免「找不到 localhost」。绑定失败（端口被占/无 IPv6）仅告警。
    if let Ok(addr) = cfg.bind_addr.parse::<std::net::SocketAddrV4>() {
        let v6_addr = format!("[::1]:{}", addr.port());
        match tokio::net::TcpListener::bind(&v6_addr).await {
            Ok(v6_listener) => {
                tracing::info!("同时监听 http://{v6_addr}");
                let app_v6 = app.clone();
                tokio::spawn(async move {
                    let svc = app_v6.into_make_service_with_connect_info::<std::net::SocketAddr>();
                    if let Err(e) = axum::serve(v6_listener, svc).await {
                        tracing::warn!(error = %e, "IPv6 监听退出");
                    }
                });
            }
            Err(e) => tracing::warn!(error = %e, "IPv6 回环绑定失败（不影响 IPv4）"),
        }
    }

    // with_connect_info：审计需要真实的 TCP 对端地址。直连部署时它是唯一真值——
    // X-Forwarded-For 那些头此时并不存在，且本就不可轻信。
    let svc = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, svc).await?;
    Ok(())
}
