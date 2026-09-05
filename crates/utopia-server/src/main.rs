mod adjudication;
mod alerting;
mod api;
mod auth;
mod blob;
mod bootstrap_ontology;
mod client_ctx;
mod docs_corpus;
mod error;
mod extraction;
mod github_issues;
mod http_fetch;
mod ingest_sources;
mod jira_issues;
mod live;
mod llm_util;
mod mappings;
mod notion;
mod object_storage;
mod ontology_index;
mod ontology_packs;
mod owl_import;
mod pack_alignment;
mod pipeline;
mod predicate_match;
mod query_engine;
mod rdf;
mod retrieval;
mod state;
mod type_resolution;
mod webdav;

use state::AppState;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use utopia_core::config::AppConfig;
use utopia_search::SearchIndex;
use uuid::Uuid;

/// 记忆那条路带着「谁说的」（0015、0026）；别的任务没有这两个字段，
/// 读到默认值本就该如此
fn payload_proposer(payload: &serde_json::Value) -> utopia_core::models::Proposer {
    let uuid = |key: &str| {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
    };
    utopia_core::models::Proposer {
        user_id: uuid("proposed_by"),
        token_id: uuid("proposed_token"),
    }
}

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

    // 凭据封印钥匙：环境变量优先，否则数据目录下的 secret.key（首次启动生成）。
    // **钥匙不进库**——库泄漏不等于凭据泄漏，是这一层的全部意义。空串按未设置处理，
    // 理由同下面的 JWT 密钥
    let secret_key = match cfg.secret_key.clone().filter(|s| !s.trim().is_empty()) {
        Some(text) => utopia_core::secrets::parse_key(&text).ok_or_else(|| {
            anyhow::anyhow!("UTOPIA_SECRET_KEY must be 32 bytes, as 64 hex characters or base64")
        })?,
        None => secret_key_file(std::path::Path::new(&cfg.data_dir))?,
    };
    utopia_core::secrets::init(secret_key);
    // 升级前落库的明文凭据在这里补封
    let sealed = utopia_store::sealing::backfill(&pool).await?;
    if sealed > 0 {
        tracing::info!(rows = sealed, "凭据补封完成");
    }

    let index_dir = std::path::Path::new(&cfg.data_dir).join("index");
    let search = Arc::new(SearchIndex::open(&index_dir)?);
    tracing::info!("全文索引就绪: {}", index_dir.display());

    // **索引落空就自己重建，不给按钮。**
    //
    // 索引是独立于数据库的一份文件：换机器、卷没挂上、目录损坏，任何一样都会让它
    // 落空。而落空之后检索只会静默回零——界面上看不出任何异样，用户会以为是
    // 「确实没有匹配」。这种失败不该靠人先意识到再去点一个按钮。
    //
    // 判据是「库里有分块而索引空着」，不是「数目对不对得上」：后者在正常运行中
    // 也会短暂不等（一篇文档正在索引），拿它当判据会让每次启动都重建一遍。
    reindex_if_empty(&pool, &search).await;

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
                // 任务失败时看一眼是不是模型端点连不上——那是系统级故障，
                // 而现在它只会留在 jobs.last_error 里，没有任何界面看得到
                //
                // 没救的失败在这里挂上标记，队列据此不再退避重试（#195）：
                // 判据在这一侧，因为 `utopia-store` 看不见 LLM 的错误类型
                let result = dispatch(&st, &job)
                    .await
                    .map_err(|e| match alerting::hopeless(&e) {
                        true => e.context(utopia_core::Terminal),
                        false => e,
                    });
                if let Err(e) = &result {
                    alerting::observe_job_failure(&st, &job, e).await;
                }
                result
            }
        },
    ));

    alerting::spawn_retention_sweep(state.clone());

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

    // 定时推理调度器（0002 R1）。
    //
    // **必须定时，不能只靠手点**：事实是持续变的——每篇文档抽取都在加边——而
    // 派生只在跑的那一刻算。不定时的话，下一篇文档进来之后图上的派生就是缺的，
    // 而这种缺失界面上看不出来（不是错，是新链没推）。
    //
    // 与来源同步共用一个节拍：每分钟扫一遍，到期的入队。真正的推导在任务里跑，
    // 不在这个循环里——一个大库全量重推可能要几秒，卡在调度循环里会拖住别的库
    let infer_state = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            match utopia_store::reasoning::due_for_inference(&infer_state.pool).await {
                Ok(due) => {
                    for kb_id in due {
                        if let Err(e) = utopia_store::jobs::enqueue(
                            &infer_state.pool,
                            "materialize_inferences",
                            serde_json::json!({ "kb_id": kb_id }),
                        )
                        .await
                        {
                            tracing::warn!(kb_id = %kb_id, error = %e, "推理任务入队失败");
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "扫描到期推理失败"),
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

/// 索引空着而库里有东西 → 从库里重建一遍。
///
/// 失败只告警不阻断启动：检索退化成「暂时搜不到」，而整个服务起不来是更坏的
/// 结果。日志里那一行说清了发生什么，下次重启还会再试。
async fn reindex_if_empty(pool: &sqlx::PgPool, search: &SearchIndex) {
    if !search.is_empty() {
        return;
    }
    let total = match utopia_store::documents::live_chunk_count(pool).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "数不出分块数，跳过索引重建");
            return;
        }
    };
    if total == 0 {
        return;
    }
    tracing::info!(chunks = total, "全文索引是空的，从库里重建");
    let rows = match utopia_store::documents::all_chunks_for_index(pool).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "读分块失败，索引未重建");
            return;
        }
    };
    // 按文档攒批再写：`reindex_document` 一次替换一篇文档的全部分块，
    // 逐条调会把前一条刚写的删掉
    let mut current: Option<(uuid::Uuid, uuid::Uuid)> = None;
    let mut batch: Vec<(String, String)> = Vec::new();
    let mut done = 0usize;
    let flush = |key: Option<(uuid::Uuid, uuid::Uuid)>, batch: &mut Vec<(String, String)>| {
        if let Some((kb, doc)) = key {
            if let Err(e) = search.reindex_document(&kb.to_string(), &doc.to_string(), batch) {
                tracing::warn!(error = %e, document = %doc, "重建索引时有一篇没写进去");
            }
        }
        batch.clear();
    };
    for (kb_id, doc_id, chunk_id, text) in rows {
        if current != Some((kb_id, doc_id)) {
            flush(current, &mut batch);
            current = Some((kb_id, doc_id));
        }
        batch.push((chunk_id.to_string(), text));
        done += 1;
    }
    // `reindex_document` 自己 commit，所以这里不必再提交一次
    flush(current, &mut batch);
    tracing::info!(chunks = done, "全文索引重建完成");
}
///
/// 任务分发：新任务类型在这里注册。
///
/// 单拎成函数而不是内联在闭包里，是为了让失败**有一个统一的出口**——
/// 上面那层要在每次失败时看一眼错误链，内联的话每个分支都得自己记得。
async fn dispatch(st: &state::AppState, job: &utopia_store::jobs::Job) -> anyhow::Result<()> {
    match job.kind.as_str() {
        "noop" => {
            tracing::info!(job_id = job.id, "noop 任务执行成功");
            Ok(())
        }
        "process_document" => {
            let id = payload_document_id(&job.payload)?;
            pipeline::process_document(st, id).await
        }
        "memory_ingest" => {
            let id = payload_document_id(&job.payload)?;
            pipeline::memory_ingest(st, id, payload_proposer(&job.payload)).await
        }
        "explore_mappings" => {
            let kb_id: Uuid = job
                .payload
                .get("kb_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("payload 缺少 kb_id"))?;
            mappings::explore_mappings(st, kb_id).await
        }
        // 定时重推（0002 R1）。**先记时间再推**——推导抛错时也不该让这个库
        // 在下一分钟被重新扫起来，那会变成一个每分钟失败一次的循环
        "materialize_inferences" => {
            let kb_id: Uuid = job
                .payload
                .get("kb_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("payload 缺少 kb_id"))?;
            utopia_store::reasoning::mark_inference_ran(&st.pool, kb_id).await?;
            let report = utopia_store::reasoning::materialize(&st.pool, kb_id).await?;
            // **到点对比**：`materialize` 本来就在拿这一轮算出来的对上库里现有的，
            // 所以「对比」不是新机制。这里只把那次对比的结果留下来给人看——
            // 没有变化时不写，免得台账被每小时一条「什么都没变」淹掉
            if report.inserted > 0 || report.invalidated > 0 {
                let _ = utopia_store::audit::record(
                    &st.pool,
                    Some(kb_id),
                    Uuid::nil(),
                    "inference.materialized",
                    "knowledge_base",
                    Some(kb_id),
                    serde_json::json!({
                        "scheduled": true,
                        "inserted": report.inserted,
                        "invalidated": report.invalidated,
                        "rules": report.rules,
                    }),
                )
                .await;
                st.emit_graph(kb_id);
            }
            Ok(())
        }
        "extract_document" => {
            let id = payload_document_id(&job.payload)?;
            extraction::extract_document(st, id, payload_proposer(&job.payload)).await
        }
        "bootstrap_ontology" => {
            let kb_id: Uuid = job
                .payload
                .get("kb_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("payload 缺少 kb_id"))?;
            bootstrap_ontology::bootstrap_ontology(st, kb_id).await
        }
        // 本体向量索引：**后台建，不卡请求**。
        // 一份 965 类的本体首次要嵌 2600 行，六到八分钟；放在
        // 交互请求里就是导入完之后第一个用到检索的人干等
        "embed_ontology" => {
            let kb_id: Uuid = job
                .payload
                .get("kb_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("payload 缺少 kb_id"))?;
            ontology_index::refresh(st, kb_id).await.map(|_| ())
        }
        "resolve_types" => {
            let kb_id: Uuid = job
                .payload
                .get("kb_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("payload 缺少 kb_id"))?;
            type_resolution::resolve_types_job(st, kb_id).await
        }
        "adjudicate_entities" => {
            let kb_id: Uuid = job
                .payload
                .get("kb_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("payload 缺少 kb_id"))?;
            adjudication::adjudicate_entities(st, kb_id).await
        }
        "sync_source" => {
            let source_id: Uuid = job
                .payload
                .get("source_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("payload 缺少 source_id"))?;
            ingest_sources::sync_source(st, source_id).await
        }
        other => anyhow::bail!("未知任务类型: {other}"),
    }
}

/// 数据目录下的封印钥匙：有就读，没有就生成一把写进去（unix 下 0600）。
/// 读出来解析不了就报错停下——静默生成一把新的会让库里已封的凭据全部打不开
fn secret_key_file(data_dir: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    let path = data_dir.join("secret.key");
    if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        return utopia_core::secrets::parse_key(&text).ok_or_else(|| {
            anyhow::anyhow!(
                "{} does not hold a 32-byte key (64 hex characters or base64)",
                path.display()
            )
        });
    }
    std::fs::create_dir_all(data_dir)?;
    let key = utopia_core::secrets::generate_key();
    std::fs::write(&path, utopia_core::secrets::key_to_hex(&key))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    tracing::info!(
        "生成了凭据封印钥匙: {}（备份数据目录时一起带走；UTOPIA_SECRET_KEY 可覆盖）",
        path.display()
    );
    Ok(key)
}
