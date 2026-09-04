//! 整库导出为 RDF（0020）。
//!
//! **边查边发**：一页事实序列化完就把攒下的字节送出去，不在内存里拼出整份文件。
//! 一个十万条事实的库正是最需要导出的那种库，也正是「先拼成一个 String」会
//! 把服务打死的那种库。
//!
//! 中途出错只能截断——HTTP 头早就发出去了。所以错误进日志，而客户端拿到的是
//! 一份短了一截的文件；这比先攒后发要好，那种做法在同样的库上根本发不出来。

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use utopia_core::models::Role;
use utopia_core::AppError;
use uuid::Uuid;

use super::graph_routes::require_kb;
use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::rdf::{self, Format, Names, SharedBuf, Sink};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ExportQuery {
    /// `turtle`（缺省）或 `jsonld`
    #[serde(default)]
    pub format: Option<String>,
    /// 造 IRI 用的对外地址。不给就用 URN——稳定，且不假装自己知道部署在哪
    #[serde(default)]
    pub base: Option<String>,
}

pub async fn export(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<ExportQuery>,
) -> ApiResult<Response> {
    let kb = require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let format = Format::parse(q.format.as_deref()).ok_or_else(|| {
        AppError::Validation("Unknown `format` (expected turtle or jsonld)".into())
    })?;
    let names = Names::new(kb_id, q.base.as_deref()).map_err(AppError::Validation)?;

    // 导出是一次「整个库离开这台机器」的动作，台账要记下（0014 的同一条理由）
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "kb.exported",
        "knowledge_base",
        Some(kb_id),
        serde_json::json!({ "format": format.extension() }),
    )
    .await;

    let pool = state.pool.clone();
    let stream = async_stream::try_stream! {
        let buf = SharedBuf::default();
        let mut sink = Sink::new(format, buf.clone());

        let classes = utopia_store::export::classes(&pool, kb_id).await.map_err(io)?;
        let relations = utopia_store::export::relations(&pool, kb_id).await.map_err(io)?;
        let vocab = rdf::vocabulary(&names, &classes, &relations);
        for c in &classes {
            rdf::emit_class(&mut sink, &vocab, c)?;
        }
        for r in &relations {
            rdf::emit_relation(&mut sink, &vocab, r)?;
        }
        yield axum::body::Bytes::from(buf.take());

        let mut after = None;
        loop {
            let page = utopia_store::export::documents_page(&pool, kb_id, after).await.map_err(io)?;
            let Some(last) = page.last() else { break };
            after = Some(last.id);
            for d in &page {
                rdf::emit_document(&mut sink, &names, d)?;
            }
            yield axum::body::Bytes::from(buf.take());
        }

        let mut after = None;
        loop {
            let page = utopia_store::export::entities_page(&pool, kb_id, after).await.map_err(io)?;
            let Some(last) = page.last() else { break };
            after = Some(last.id);
            for e in &page {
                rdf::emit_entity(&mut sink, &names, &vocab, e)?;
            }
            yield axum::body::Bytes::from(buf.take());
        }

        // 现行三元组按「导出这一刻」判定，整份文件用同一个 now：
        // 边发边取当前时间的话，同一份文件的前后两半会按两个不同的现在写
        let now = chrono::Utc::now();
        let mut after = None;
        loop {
            let page = utopia_store::export::facts_page(&pool, kb_id, after).await.map_err(io)?;
            let Some(last) = page.last() else { break };
            after = Some(last.id);
            for f in &page {
                rdf::emit_fact(&mut sink, &names, &vocab, f, now)?;
            }
            yield axum::body::Bytes::from(buf.take());
        }

        let mut after = None;
        loop {
            let page = utopia_store::export::derived_page(&pool, kb_id, after).await.map_err(io)?;
            let Some(last) = page.last() else { break };
            after = Some(last.id);
            for d in &page {
                rdf::emit_derived(&mut sink, &names, &vocab, d)?;
            }
            yield axum::body::Bytes::from(buf.take());
        }

        sink.finish()?;
        yield axum::body::Bytes::from(buf.take());
    };

    let filename = format!("{}.{}", slug(&kb.name), format.extension());
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(format.content_type()),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok((
        StatusCode::OK,
        headers,
        Body::from_stream(Box::pin(stream)
            as std::pin::Pin<
                Box<
                    dyn futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>>
                        + Send,
                >,
            >),
    )
        .into_response())
}

/// 库里出的错在流中间只能变成 io 错误——响应头已经发出去了，改不了状态码。
fn io(e: AppError) -> std::io::Error {
    tracing::error!(error = %e, "导出中断");
    std::io::Error::other(e.to_string())
}

/// 文件名用的短名。ASCII 之外的字符不进 Content-Disposition 的 filename，
/// 中文库名会在那里变成一串问号
fn slug(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "export".into()
    } else {
        trimmed
    }
}
