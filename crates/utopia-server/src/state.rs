use sqlx::PgPool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use utopia_core::config::AppConfig;
use utopia_search::SearchIndex;
use uuid::Uuid;

/// 服务内事件（SSE 推送给前端做局部刷新）。
#[derive(Clone, Debug, serde::Serialize)]
pub struct AppEvent {
    pub kb_id: Uuid,
    /// document = 文档摄入/抽取状态变化；review = 审核队列变化
    pub kind: &'static str,
    pub document_id: Option<Uuid>,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jwt_secret: String,
    pub search: Arc<SearchIndex>,
    /// Charter（内置文档）内存索引：chat 的 search_docs 工具用
    pub docs: Arc<utopia_search::DocsIndex>,
    /// 原始文件字节的存取接缝（内容寻址，key = sha256）；当前实现为本地磁盘
    pub blob: Arc<dyn crate::blob::BlobStore>,
    pub open_registration: bool,
    /// 强制 Secure cookie（配置项）；未强制时按请求的 X-Forwarded-Proto 逐次判定
    pub cookie_secure: bool,
    /// worker 并发数：调度循环每轮热读——系统设置改动即时生效
    pub worker_concurrency: Arc<std::sync::atomic::AtomicUsize>,
    /// 按模型的并发闸门：后台任务调 LLM 前取许可。限额存库，改完即时生效
    pub model_gates: Arc<crate::llm_util::ModelGates>,
    pub events: broadcast::Sender<AppEvent>,
}

impl AppState {
    /// `jwt_secret` 由入口解析：环境变量给了就是它，否则是库里那条（首启时生成）。
    /// 不从 cfg 里取，是因为到这一步它必须已经是确定的一个值，而不是 Option。
    pub fn new(
        pool: PgPool,
        cfg: &AppConfig,
        search: Arc<SearchIndex>,
        jwt_secret: String,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        let data_dir = PathBuf::from(&cfg.data_dir);
        let blob = Arc::new(crate::blob::LocalBlobStore::new(data_dir.join("files")));
        Self {
            pool,
            jwt_secret,
            search,
            docs: Arc::new(crate::docs_corpus::build_index()),
            blob,
            open_registration: cfg.open_registration,
            cookie_secure: cfg.cookie_secure,
            worker_concurrency: Arc::new(std::sync::atomic::AtomicUsize::new(32)),
            model_gates: Arc::new(crate::llm_util::ModelGates::default()),
            events,
        }
    }

    /// 无订阅者时 send 返回 Err——正常情况，静默忽略。
    pub fn emit_document(&self, kb_id: Uuid, document_id: Uuid) {
        let _ = self.events.send(AppEvent {
            kb_id,
            kind: "document",
            document_id: Some(document_id),
        });
    }

    pub fn emit_review(&self, kb_id: Uuid) {
        let _ = self.events.send(AppEvent {
            kb_id,
            kind: "review",
            document_id: None,
        });
    }

    pub fn emit_source(&self, kb_id: Uuid) {
        let _ = self.events.send(AppEvent {
            kb_id,
            kind: "source",
            document_id: None,
        });
    }
}
