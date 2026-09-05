//! 来源同步任务：url（网页抓取）/ rss（订阅，pubDate → doc_time）/
//! github_issues / jira_issues（工单，更新时刻 → doc_time，正文里带状态变更史）/
//! s3（对象存储，LastModified → doc_time）。
//! 全部走 sha256 去重（重复内容静默跳过），新文档进标准摄入管道（process_document）。
//! folder 是纯容器（上传入内），api 是推送型——两者无拉取语义。

use crate::state::AppState;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use utopia_core::models::Source;
use utopia_core::models::SourceKind;
use uuid::Uuid;

/// 单次同步的新文档上限（防超长 feed/URL 列表拖垮任务）
const MAX_NEW_PER_SYNC: usize = 200;
const MAX_FEED_BYTES: usize = 4 * 1024 * 1024;
const RSS_MAX_INFLIGHT: i64 = 25;
const RSS_HYDRATION_ATTEMPTS: i32 = 5;

/// 抓取用的 User-Agent。reqwest 默认一个都不发，而维基百科明确拒绝匿名请求
/// （403），Cloudflare 前置的站点也普遍如此——URL 与 RSS 两类来源因此对一大批
/// 真实网站直接失效。自报家门也是爬虫礼节：站方能认出我们、能联系到我们。
pub(crate) const UA: &str = concat!(
    "Utopia/",
    env!("CARGO_PKG_VERSION"),
    " (+https://utopia.bi; self-hosted knowledge platform)"
);

/// 单次同步的产出统计（Moved/Unchanged 不计）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SyncStats {
    pub created: usize,
    pub updated: usize,
    pub discovered: usize,
    pub queued_for_content: usize,
    pub content_terminal: usize,
}

#[derive(Debug)]
struct RssObservation {
    key: String,
    title: String,
    article_url: Option<String>,
    summary: String,
    embedded_html: Option<String>,
    doc_time: Option<DateTime<Utc>>,
    has_usable_source: bool,
}

impl SyncStats {
    fn absorb(&mut self, action: IngestAction) {
        match action {
            IngestAction::Created => self.created += 1,
            IngestAction::Updated => self.updated += 1,
            _ => {}
        }
    }
    fn total(&self) -> usize {
        self.created + self.updated
    }
}

pub async fn sync_source(state: &AppState, source_id: Uuid) -> anyhow::Result<()> {
    let source = utopia_store::sources::get(&state.pool, source_id).await?;
    utopia_store::sources::mark_running(&state.pool, source_id).await?;
    let run_id = utopia_store::sources::start_run(&state.pool, source_id).await?;
    state.emit_source(source.kb_id);

    // 按枚举穷举：加一种来源就得在这里决定它怎么同步，编译器不放过漏掉的那一支
    let outcome = match SourceKind::parse(&source.kind) {
        Some(SourceKind::Url) => sync_urls(state, &source).await,
        Some(SourceKind::Rss) => sync_rss(state, &source).await,
        Some(SourceKind::Custom) => sync_custom(state, &source).await,
        Some(SourceKind::GithubIssues) => sync_github_issues(state, &source).await,
        Some(SourceKind::JiraIssues) => sync_jira_issues(state, &source).await,
        Some(SourceKind::S3 | SourceKind::AzureBlob | SourceKind::Gcs) => {
            sync_object_storage(state, &source).await
        }
        Some(SourceKind::Webdav) => sync_webdav(state, &source).await,
        Some(SourceKind::Notion) => sync_notion(state, &source).await,
        // 被动容器：folder / api / memory / upload 没有拉取语义
        Some(SourceKind::Folder | SourceKind::Api | SourceKind::Memory | SourceKind::Upload) => {
            Ok(SyncStats::default())
        }
        None => Err(anyhow::anyhow!("unknown source kind `{}`", source.kind)),
    };

    match outcome {
        Ok(stats) => {
            utopia_store::sources::finish_run(
                &state.pool,
                run_id,
                source_id,
                None,
                stats.created as i32,
                stats.updated as i32,
            )
            .await?;
            utopia_store::sources::finish_sync(&state.pool, source_id, None, stats.total() as i32)
                .await?;
            state.emit_source(source.kb_id);
            tracing::info!(
                %source_id,
                kind = %source.kind,
                created = stats.created,
                updated = stats.updated,
                discovered = stats.discovered,
                queued_for_content = stats.queued_for_content,
                content_terminal = stats.content_terminal,
                "来源同步完成"
            );
            Ok(())
        }
        Err(e) => {
            utopia_store::sources::finish_run(
                &state.pool,
                run_id,
                source_id,
                Some(&e.to_string()),
                0,
                0,
            )
            .await?;
            utopia_store::sources::finish_sync(&state.pool, source_id, Some(&e.to_string()), 0)
                .await?;
            state.emit_source(source.kb_id);
            Err(e)
        }
    }
}

/// 三路判定的结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IngestAction {
    Created,
    Updated,
    Moved,
    Unchanged,
    /// 墓碑：标记"不在来源中"（不删除文档，删不删由用户在 UI 决定）
    Tombstoned,
}

#[derive(Debug, Clone, Copy)]
pub struct IngestOutcome {
    pub action: IngestAction,
}

pub(crate) async fn write_blob(state: &AppState, sha256: &str, bytes: &[u8]) -> anyhow::Result<()> {
    state.blob.put(sha256, bytes).await
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 普通上传语义的推送（KB 级 /ingest → Uploads）：无来源、无身份键，
/// 不做三路判定——KB 内已有同内容时视为无操作，其余一律新建。
pub async fn ingest_upload(
    state: &AppState,
    kb_id: Uuid,
    filename: &str,
    mime: &str,
    bytes: &[u8],
    doc_time: Option<DateTime<Utc>>,
) -> anyhow::Result<IngestAction> {
    if bytes.is_empty() {
        return Ok(IngestAction::Unchanged);
    }
    let sha256 = sha_hex(bytes);
    write_blob(state, &sha256, bytes).await?;
    match utopia_store::documents::create(
        &state.pool,
        kb_id,
        filename,
        mime,
        bytes.len() as i64,
        &sha256,
        None,
        doc_time,
        None,
    )
    .await
    {
        Ok(doc) => {
            utopia_store::jobs::enqueue(
                &state.pool,
                "process_document",
                serde_json::json!({ "document_id": doc.id }),
            )
            .await?;
            state.emit_document(kb_id, doc.id);
            Ok(IngestAction::Created)
        }
        Err(utopia_core::AppError::Conflict(_)) => Ok(IngestAction::Unchanged),
        Err(e) => Err(e.into()),
    }
}

/// Existing callers only need the action; hydration also needs the stable document UUID.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_item(
    state: &AppState,
    kb_id: Uuid,
    source_id: Uuid,
    external_key: &str,
    filename: &str,
    mime: &str,
    bytes: &[u8],
    doc_time: Option<DateTime<Utc>>,
) -> anyhow::Result<IngestAction> {
    Ok(ingest_item_with_outcome(
        state,
        kb_id,
        source_id,
        external_key,
        filename,
        mime,
        bytes,
        doc_time,
    )
    .await?
    .action)
}

#[allow(clippy::too_many_arguments)]
pub async fn ingest_item_with_outcome(
    state: &AppState,
    kb_id: Uuid,
    source_id: Uuid,
    external_key: &str,
    filename: &str,
    mime: &str,
    bytes: &[u8],
    doc_time: Option<DateTime<Utc>>,
) -> anyhow::Result<IngestOutcome> {
    if bytes.is_empty() {
        return Ok(IngestOutcome {
            action: IngestAction::Unchanged,
        });
    }
    let sha256 = sha_hex(bytes);

    // 主判定：逻辑身份；兜底：迁移前的历史文档（无 key）按文件名认领并补键
    let mut existing =
        utopia_store::documents::find_by_external_key(&state.pool, source_id, external_key).await?;
    if existing.is_none() {
        if let Some(legacy) =
            utopia_store::documents::find_legacy_by_filename(&state.pool, source_id, filename)
                .await?
        {
            utopia_store::documents::adopt_external_key(&state.pool, legacy.id, external_key)
                .await?;
            // 认领时把旧内容记为版本 1（此前没有版本记录）
            utopia_store::documents::record_version(
                &state.pool,
                legacy.id,
                &legacy.sha256,
                legacy.size_bytes,
            )
            .await?;
            existing = Some(legacy);
        }
    }

    if let Some(doc) = existing {
        // Ordinary source reappearance retains upstream restore semantics,
        // including search and derivation repair, before comparing content.
        let revived = doc.deleted_at.is_some();
        if revived {
            let restored = utopia_store::documents::restore(&state.pool, kb_id, doc.id).await?;
            crate::api::documents_routes::reindex(state, &restored).await?;
            crate::api::documents_routes::settle_derivations(state, kb_id).await?;
            let _ = utopia_store::audit::record_opt(
                &state.pool,
                Some(kb_id),
                None,
                "document.restored",
                "document",
                Some(doc.id),
                serde_json::json!({"filename":restored.filename,"via":"sync"}),
            )
            .await;
        }
        if doc.sha256 == sha256 {
            if revived {
                state.emit_document(kb_id, doc.id);
                return Ok(IngestOutcome {
                    action: IngestAction::Updated,
                });
            }
            return Ok(IngestOutcome {
                action: IngestAction::Unchanged,
            });
        }
        // 变更：原地替换，旧版本入 document_versions，并与处理任务一起提交
        write_blob(state, &sha256, bytes).await?;
        utopia_store::documents::replace_content_and_enqueue_processing(
            &state.pool,
            doc.id,
            filename,
            mime,
            bytes.len() as i64,
            &sha256,
            doc_time,
        )
        .await?;
        state.emit_document(kb_id, doc.id);
        return Ok(IngestOutcome {
            action: IngestAction::Updated,
        });
    }

    // 同内容出现在新路径：识别为移动/改名，不重跑管道
    if let Some(doc) =
        utopia_store::documents::find_by_source_sha(&state.pool, source_id, &sha256).await?
    {
        utopia_store::documents::update_location(&state.pool, doc.id, filename, external_key)
            .await?;
        state.emit_document(kb_id, doc.id);
        return Ok(IngestOutcome {
            action: IngestAction::Moved,
        });
    }

    write_blob(state, &sha256, bytes).await?;
    match utopia_store::documents::create_with_version_and_processing(
        &state.pool,
        kb_id,
        filename,
        mime,
        bytes.len() as i64,
        &sha256,
        Some(source_id),
        doc_time,
        Some(external_key),
    )
    .await
    {
        Ok(doc) => {
            state.emit_document(kb_id, doc.id);
            Ok(IngestOutcome {
                action: IngestAction::Created,
            })
        }
        // KB 内已有同内容：不伪造新的身份，保留幂等结果
        Err(utopia_core::AppError::Conflict(_)) => Ok(IngestOutcome {
            action: IngestAction::Unchanged,
        }),
        Err(e) => Err(e.into()),
    }
}

async fn sync_urls(state: &AppState, source: &Source) -> anyhow::Result<SyncStats> {
    let urls: Vec<String> = source.config["urls"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    if urls.is_empty() {
        anyhow::bail!("url source is missing config.urls (a list of page URLs)");
    }

    let mut stats = SyncStats::default();
    let mut last_err: Option<String> = None;
    for url in urls.iter().take(MAX_NEW_PER_SYNC) {
        match fetch_page(url).await {
            Ok((filename, mime, bytes)) => {
                // 逻辑身份 = URL 本身：页面内容变了就原地替换（历史进版本表）
                let action = ingest_item(
                    state,
                    source.kb_id,
                    source.id,
                    url,
                    &filename,
                    &mime,
                    &bytes,
                    None,
                )
                .await?;
                stats.absorb(action);
            }
            Err(e) => {
                tracing::warn!(%url, error = %e, "抓取失败");
                last_err = Some(format!("{url}: {e}"));
            }
        }
    }
    // 部分失败：有产出则视为成功（错误进日志），全军覆没才报错
    if stats.total() == 0 {
        if let Some(err) = last_err {
            anyhow::bail!("{err}");
        }
    }
    // 配置列表即全集：不在列表里的文档标"不在来源中"（抓取失败不算——它仍被配置着）
    utopia_store::documents::reconcile_missing(&state.pool, source.id, &urls)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(stats)
}

/// 抓一个页面。地址是这个库的人自己填的，所以内网放行（`Reach::Operator`），
/// 但重定向不许把请求带进内网，响应体有上限（#330）。
async fn fetch_page(url: &str) -> anyhow::Result<(String, String, Vec<u8>)> {
    let page = crate::http_fetch::get(
        url,
        crate::http_fetch::Reach::Operator,
        crate::http_fetch::Limits::default(),
    )
    .await?;
    let mime = if page.mime == "application/octet-stream" {
        "text/html".to_string()
    } else {
        page.mime
    };
    // 身份仍然是**配置里那个 URL**（同一个页面换了地址不该变成新文档），
    // 但落到别处这件事值得留一行
    if page.final_url.as_str() != url {
        tracing::debug!(from = url, to = %page.final_url, "页面跟着重定向落在别处");
    }
    let filename = filename_from_url(url, &mime);
    Ok((filename, mime, page.bytes))
}

fn filename_from_url(url: &str, mime: &str) -> String {
    let stripped = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let mut slug: String = stripped
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    slug.truncate(120);
    let has_ext = slug
        .rsplit('.')
        .next()
        .map(|e| e.len() <= 5)
        .unwrap_or(false)
        && slug.contains('.')
        && !slug.ends_with('.');
    if !has_ext || mime.contains("html") {
        format!("{slug}.html")
    } else {
        slug
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn rss_article_link(entry: &feed_rs::model::Entry) -> Option<String> {
    entry.links.iter().find_map(|link| {
        let rel_is_alternate = link
            .rel
            .as_deref()
            .is_none_or(|rel| rel.eq_ignore_ascii_case("alternate"));
        if !rel_is_alternate {
            return None;
        }
        if link.media_type.as_deref().is_some_and(|media| {
            !media.eq_ignore_ascii_case("text/html")
                && !media.eq_ignore_ascii_case("application/xhtml+xml")
        }) {
            return None;
        }
        let parsed = reqwest::Url::parse(link.href.trim()).ok()?;
        match parsed.scheme() {
            "http" | "https" if parsed.host().is_some() => Some(parsed.to_string()),
            _ => None,
        }
    })
}

const RSS_EXTERNAL_KEY_MAX_BYTES: usize = 4_096;

fn rss_entry_key(entry: &feed_rs::model::Entry, article_url: Option<&str>) -> Option<String> {
    let identity = if !entry.id.trim().is_empty() {
        entry.id.trim()
    } else {
        article_url?.trim()
    };
    if identity.is_empty() {
        return None;
    }
    Some(bound_rss_identity(identity))
}

fn bound_rss_identity(identity: &str) -> String {
    if identity.len() <= RSS_EXTERNAL_KEY_MAX_BYTES {
        return identity.to_owned();
    }
    // Never truncate a publisher's GUID or URL: doing so silently merges
    // distinct feed items sharing the same prefix. A bounded digest retains
    // the complete identity material without violating the database bound.
    format!("rss:v1:sha256:{}", sha_hex(identity.as_bytes()))
}

fn content_is_substantive(markdown: &str, summary: &str, linked_page: bool) -> bool {
    crate::rss_full_content::quality_check(markdown, summary, linked_page).is_ok()
}

fn has_usable_rss_content(feed_usable: bool, article_url: Option<&str>) -> bool {
    feed_usable || article_url.is_some_and(|url| !url.trim().is_empty())
}

#[cfg(test)]
#[path = "rss_sync_contract_tests.rs"]
mod rss_sync_contract_tests;

async fn sync_rss(state: &AppState, source: &Source) -> anyhow::Result<SyncStats> {
    let feed_url = source.config["feed_url"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("rss source is missing config.feed_url"))?;
    let content_mode = utopia_store::sources::rss_content_mode(&source.config)?;
    let observation = if content_mode == utopia_store::sources::RSS_FULL_CONTENT_MODE {
        Some(
            utopia_store::rss_full_content::begin_feed_observation(
                &state.pool,
                source.id,
                &source.config,
            )
            .await?,
        )
    } else {
        None
    };

    let feed_body = crate::http_fetch::get(
        feed_url,
        crate::http_fetch::Reach::Operator,
        crate::http_fetch::Limits {
            max_bytes: MAX_FEED_BYTES,
            ..Default::default()
        },
    )
    .await?;
    let bytes = feed_body.bytes;
    // feed-rs normally synthesizes missing entry IDs from link + title (or a
    // random UUID). Neither is an application-level stable identity. Leave
    // missing IDs empty so rss_entry_key can use only the stable article URL.
    let feed = feed_rs::parser::Builder::new()
        .id_generator(|_, _, _| String::new())
        .build()
        .parse(&bytes[..])
        .map_err(|e| anyhow::anyhow!("Failed to parse feed: {e}"))?;

    let observations: Vec<RssObservation> = feed
        .entries
        .iter()
        .filter_map(|entry| {
            let title = entry
                .title
                .as_ref()
                .map(|t| t.content.trim().to_string())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| "untitled".into());
            let article_url = rss_article_link(entry);
            let key = rss_entry_key(entry, article_url.as_deref())?;
            let embedded_html = entry
                .content
                .as_ref()
                .and_then(|content| content.body.as_deref())
                .map(str::trim)
                .filter(|body| !body.is_empty())
                .map(|body| truncate_utf8(body, 2 * 1024 * 1024));
            let summary = entry
                .summary
                .as_ref()
                .map(|summary| summary.content.trim().to_string())
                .unwrap_or_default();
            let embedded_markdown = embedded_html
                .as_deref()
                .and_then(|html| crate::rss_full_content::normalize_feed_html(html).ok());
            let feed_usable = embedded_markdown
                .as_deref()
                .is_some_and(|markdown| content_is_substantive(markdown, &summary, false));
            let usable_source = has_usable_rss_content(feed_usable, article_url.as_deref());
            Some(RssObservation {
                key,
                title,
                article_url,
                summary,
                embedded_html,
                doc_time: entry.published.or(entry.updated),
                has_usable_source: usable_source,
            })
        })
        .collect();

    if let Some((activation, observed_at)) = observation {
        let entries: Vec<utopia_store::rss_full_content::NewEntry> = observations
            .iter()
            .map(|observation| utopia_store::rss_full_content::NewEntry {
                external_key: observation.key.clone(),
                title: truncate_utf8(&observation.title, 2_048),
                article_url: observation
                    .article_url
                    .as_deref()
                    .map(|url| truncate_utf8(url, 8_192)),
                summary: truncate_utf8(&observation.summary, 16_384),
                embedded_html: observation.embedded_html.clone(),
                doc_time: observation.doc_time,
                has_usable_source: observation.has_usable_source,
            })
            .collect();

        if activation.activation_state == "pending" {
            let discovered = utopia_store::rss_full_content::record_baseline(
                &state.pool,
                source.id,
                activation.activation_generation,
                &entries,
            )
            .await?;
            return Ok(SyncStats {
                discovered,
                ..SyncStats::default()
            });
        }
        if activation.activation_state != "active" {
            anyhow::bail!("RSS full-content activation is disabled");
        }

        let discovered = utopia_store::rss_full_content::discover_observed(
            &state.pool,
            source.id,
            activation.activation_generation,
            &entries,
            observed_at,
        )
        .await?;
        let queued = utopia_store::rss_full_content::claim_pending_and_enqueue(
            &state.pool,
            source.id,
            activation.activation_generation,
            RSS_MAX_INFLIGHT,
            RSS_HYDRATION_ATTEMPTS,
        )
        .await?;
        return Ok(SyncStats {
            discovered: discovered.discovered,
            queued_for_content: queued,
            content_terminal: discovered.terminal,
            ..SyncStats::default()
        });
    }

    let mut stats = SyncStats::default();
    for observation in observations.into_iter().take(MAX_NEW_PER_SYNC) {
        let RssObservation {
            key,
            title,
            article_url,
            summary,
            embedded_html,
            doc_time,
            ..
        } = observation;
        let body = embedded_html.unwrap_or(summary);
        let safe_title = escape_html(&title);
        let safe_link = article_url.as_deref().map(escape_html).unwrap_or_default();
        let html = format!(
            "<html><head><title>{safe_title}</title></head><body><h1>{safe_title}</h1>\n<p><a href=\"{safe_link}\">{safe_link}</a></p>\n{body}</body></html>",
        );
        let slug = {
            let slug = slugify(&title);
            if slug.is_empty() {
                "untitled".to_string()
            } else {
                slug
            }
        };
        let filename = format!("{slug}.html");
        let action = ingest_item(
            state,
            source.kb_id,
            source.id,
            &key,
            &filename,
            "text/html",
            html.as_bytes(),
            doc_time,
        )
        .await?;
        stats.absorb(action);
    }
    Ok(stats)
}

/// GitHub 工单：一张工单一篇文档，正文里带它的状态变更史。
///
/// 工单与评论走**仓库级 + `since`**（一次分页取全），事件走**逐工单**——
/// 不是不一致，是 `issues/events` 不支持 `since` 且会被 PR 事件淹没，
/// 拿真实仓库一跑就发现状态变更史悄悄空了。详见 [`crate::github_issues`]。
///
/// `doc_time` 取 `updated_at` 而不是 `created_at`：每次同步捕获的是"此刻这张
/// 工单是什么样"，认知时间该说这个状态是何时成立的。新增一条评论会改
/// `updated_at`，于是内容变了、记一个新版本、`doc_time` 也跟着走。
async fn sync_github_issues(state: &AppState, source: &Source) -> anyhow::Result<SyncStats> {
    let repo = source.config["repo"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("github_issues source is missing config.repo (owner/name)")
        })?;
    if repo.split('/').count() != 2 {
        anyhow::bail!("config.repo should look like owner/name, got {repo:?}");
    }
    let auth = source.config["auth_header"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    // PR 在 GitHub 的模型里也是 issue。默认排除——问"工单系统"要的是工单；
    // 但有些仓库的决策记录实际写在 PR 描述里，所以留了开关
    let include_prs = source.config["include_pull_requests"]
        .as_bool()
        .unwrap_or(false);

    // api.github.com 是固定的公网主机，所以按内容级的严格度收（`Reach::Content`）
    let http = crate::http_fetch::client_for(
        &reqwest::Url::parse("https://api.github.com/")?,
        crate::http_fetch::Reach::Content,
        crate::http_fetch::Limits::default(),
    )
    .await?;
    let base = format!("https://api.github.com/repos/{repo}");

    // 增量：GitHub 的 since 是"这之后更新过的"
    let mut issue_q: Vec<(&str, String)> = vec![("state", "all".into())];
    let mut comment_q: Vec<(&str, String)> = Vec::new();
    if let Some(t) = source.last_sync_at {
        issue_q.push(("since", t.to_rfc3339()));
        comment_q.push(("since", t.to_rfc3339()));
    }

    let issues: Vec<crate::github_issues::Issue> =
        crate::github_issues::fetch_all(&http, &format!("{base}/issues"), &issue_q, auth).await?;
    let comments: Vec<crate::github_issues::Comment> = crate::github_issues::fetch_all(
        &http,
        &format!("{base}/issues/comments"),
        &comment_q,
        auth,
    )
    .await?;
    let mut stats = SyncStats::default();
    for (issue, cs) in crate::github_issues::group_comments(&issues, &comments)
        .into_iter()
        .filter(|(i, _)| include_prs || i.pull_request.is_none())
        .take(MAX_NEW_PER_SYNC)
    {
        // 逐工单取事件。N 只是本轮要写入的工单数——首次同步等于总数，
        // 之后有 since 兜着通常是个位数
        let events = crate::github_issues::sort_events(
            crate::github_issues::fetch_all(
                &http,
                &format!("{base}/issues/{}/events", issue.number),
                &[],
                auth,
            )
            .await?,
        );
        let es: Vec<&crate::github_issues::Event> = events.iter().collect();
        let body = crate::github_issues::render(issue, &cs, &es);
        // 逻辑身份带上仓库：同一个知识库里接两个仓库时，#18 不会互相覆盖
        let key = format!("github:{repo}#{}", issue.number);
        let filename = format!("{}-{}.md", issue.number, slugify(&issue.title));
        let action = ingest_item(
            state,
            source.kb_id,
            source.id,
            &key,
            &filename,
            "text/markdown",
            body.as_bytes(),
            Some(issue.updated_at),
        )
        .await?;
        stats.absorb(action);
    }
    Ok(stats)
}

/// Jira 工单：一张工单一篇文档，正文里带**字段级**的变更史。
///
/// 比 GitHub 那条路省：`search?expand=changelog` 一次调用就带回工单本体、
/// 完整变更史与评论，**没有 N+1**。增量靠 JQL 的 `updated >= …` 表达，
/// 因为 Jira 没有 `since` 参数。详见 [`crate::jira_issues`]。
///
/// `doc_time` 取 `updated`，与 github_issues 同一口径：每次同步捕获的是
/// "此刻这张工单是什么样"，认知时间该说这个状态何时成立。
async fn sync_jira_issues(state: &AppState, source: &Source) -> anyhow::Result<SyncStats> {
    let base_url = source.config["base_url"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("jira_issues source is missing config.base_url"))?;
    let project = source.config["project"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("jira_issues source is missing config.project"))?;
    // 项目 key 直接拼进 JQL，所以不能是任意字符串。Jira 的 key 本身就限定
    // 字母数字加下划线——挡住它顺带挡住了 JQL 注入
    if !project.chars().all(|c| c.is_alphanumeric() || c == '_') {
        anyhow::bail!("config.project should be a Jira project key, got {project:?}");
    }
    let auth = source.config["auth_header"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let http = crate::http_fetch::client_for(
        &reqwest::Url::parse(base_url).map_err(|e| anyhow::anyhow!("Invalid base_url: {e}"))?,
        crate::http_fetch::Reach::Operator,
        crate::http_fetch::Limits::default().with_overall(std::time::Duration::from_secs(45)),
    )
    .await?;

    let jql = crate::jira_issues::jql(project, source.last_sync_at);
    let (issues, total) = crate::jira_issues::fetch_all(&http, base_url, &jql, auth).await?;
    // **截断了就说出来。** 一个跑了多年的项目动辄上万张工单，翻页上限意味着
    // 这一轮只覆盖了一段；不报的话界面上"同步完成"就是一句误导
    if total > issues.len() as i64 {
        tracing::warn!(
            source_id = %source.id,
            fetched = issues.len(),
            total,
            "Jira 结果被翻页上限截断，本轮只覆盖了一部分；下一轮的 JQL 窗口会接上"
        );
    }

    let mut stats = SyncStats::default();
    for issue in issues.iter().take(MAX_NEW_PER_SYNC) {
        let body = crate::jira_issues::render(issue);
        // 逻辑身份带上站点：一个知识库接两个 Jira 时，PROJ-1 不会互相覆盖
        let host = reqwest::Url::parse(base_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "jira".into());
        let key = format!("jira:{host}/{}", issue.key);
        let filename = format!(
            "{}-{}.md",
            issue.key,
            slugify(issue.fields.summary.as_deref().unwrap_or(""))
        );
        let action = ingest_item(
            state,
            source.kb_id,
            source.id,
            &key,
            &filename,
            "text/markdown",
            body.as_bytes(),
            issue.fields.updated.map(|t| t.0),
        )
        .await?;
        stats.absorb(action);
    }
    Ok(stats)
}

/// 标题 → 文件名安全的片段。与 RSS 那条路同一个口径（非字母数字换成 -，截断）。
fn slugify(title: &str) -> String {
    let mut s: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    s.truncate(60);
    s.trim_matches('-').to_string()
}

/// 自定义拉取器 —— Utopia Ingest Interface：
/// `GET {endpoint}?since=<上次同步 RFC3339>`（首次同步不带 since；可配 Authorization 头），
/// 响应 `{"items":[{"id":"稳定唯一ID","title":"文档名","content":"正文(纯文本/Markdown/HTML)",
///                  "doc_time":"RFC3339 可选","mime":"text/markdown 可选"}]}`。
/// id → external_key（custom:{id}），三路判定生效：同 id 同内容跳过、新内容原地更新。
async fn sync_custom(state: &AppState, source: &Source) -> anyhow::Result<SyncStats> {
    let endpoint = source.config["endpoint"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("custom source is missing config.endpoint"))?;
    let mut url =
        reqwest::Url::parse(endpoint).map_err(|e| anyhow::anyhow!("Invalid endpoint URL: {e}"))?;
    if let Some(t) = source.last_sync_at {
        url.query_pairs_mut().append_pair("since", &t.to_rfc3339());
    }

    // 端点是操作者自己填的，内网与回环都是正当目标；代理的取舍（回环走代理
    // 只会 502）和地址校验一起收进 `client_for`（#330）
    let http = crate::http_fetch::client_for(
        &url,
        crate::http_fetch::Reach::Operator,
        crate::http_fetch::Limits::default(),
    )
    .await?;
    let mut req = http.get(url);
    if let Some(auth) = source.config["auth_header"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
    {
        req = req.header(reqwest::header::AUTHORIZATION, auth.trim());
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} from endpoint", resp.status());
    }
    let body: serde_json::Value = resp.json().await?;
    let items = body["items"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Response is missing the items[] array"))?;

    let mut stats = SyncStats::default();
    let mut seen_keys: Vec<String> = Vec::new();
    for item in items.iter().take(MAX_NEW_PER_SYNC) {
        let Some(id) = item["id"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            tracing::warn!(source_id = %source.id, "custom item 缺少 id，跳过");
            continue;
        };
        let Some(content) = item["content"].as_str().filter(|s| !s.trim().is_empty()) else {
            tracing::warn!(source_id = %source.id, %id, "custom item 缺少 content，跳过");
            continue;
        };
        let title = item["title"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(id);
        let mime = item["mime"].as_str().unwrap_or("text/markdown");
        let doc_time = item["doc_time"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));
        let filename = ensure_extension(title, mime);
        let key = format!("custom:{id}");
        let action = ingest_item(
            state,
            source.kb_id,
            source.id,
            &key,
            &filename,
            mime,
            content.as_bytes(),
            doc_time,
        )
        .await?;
        stats.absorb(action);
        seen_keys.push(key);
    }
    // 增量响应（?since=）里缺席≠删除，不做全集对账；但：
    // 1) 再次出现的条目摘掉 missing 标记（失而复得）
    // 2) 显式墓碑 deleted[] 才标"不在来源中"——删不删文档由用户在 UI 决定
    if !seen_keys.is_empty() {
        utopia_store::documents::clear_missing_keys(&state.pool, source.id, &seen_keys)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
    }
    let tombstones: Vec<String> = body["deleted"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(|id| format!("custom:{}", id.trim()))
                .collect()
        })
        .unwrap_or_default();
    if !tombstones.is_empty() {
        let n = utopia_store::documents::mark_missing_keys(&state.pool, source.id, &tombstones)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        tracing::info!(source_id = %source.id, count = n, "custom 墓碑：标记不在来源中");
    }
    Ok(stats)
}

/// 对象存储同步：列前缀下的对象，逐个摄入。
///
/// `external_key` 用 `s3://bucket/key`，跟 `file:///` 与页面 URL 同一个约定：
/// 出处自描述。换了前缀但内容没变时，`ingest_item` 会认成「搬家」而不是新增，
/// 不会重跑一遍抽取。
///
/// **`doc_time` 取 `LastModified`，而它是写入时刻不是文档自身的时间。**
/// 一份 2019 年的合同今天传上去，时间线上会落在今天。对象存储没有更好的
/// 来源——除非文件名或正文里带日期，而那是抽取器的活。这一条与 `url` 源
/// 同病：`0013` 的第一条判据在这里只满足一半。
async fn sync_object_storage(state: &AppState, source: &Source) -> anyhow::Result<SyncStats> {
    let bucket = source.config["bucket"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} source is missing config.bucket", source.kind))?;
    let prefix = source.config["prefix"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let store = crate::object_storage::client(&source.kind, &source.config)?;
    let (objects, truncated) =
        crate::object_storage::fetch(&source.kind, store.as_ref(), bucket, prefix).await?;

    // 到顶不是错误，但必须说出来——否则「同步成功」下面藏着没进来的东西，
    // 而那正是 0005 说的失败无声
    if truncated {
        tracing::warn!(
            %bucket, prefix = prefix.unwrap_or(""),
            "对象数到达单次上限，其余留给下一次同步"
        );
    }

    let mut stats = SyncStats::default();
    for obj in objects {
        // **不猜 mime。** `utopia_ingest::parse` 先看魔数、再看扩展名，
        // 注释写着「扩展名可能撒谎」——在这里按文件名猜一个，只是多一个
        // 会撒谎的来源，而且要为此多一个依赖。`octet-stream` 是诚实的：
        // 我们拿到的就是一串字节，没看过里面是什么。
        let action = ingest_item(
            state,
            source.kb_id,
            source.id,
            &obj.external_key,
            &obj.filename,
            "application/octet-stream",
            &obj.bytes,
            obj.last_modified,
        )
        .await?;
        stats.absorb(action);
    }
    Ok(stats)
}

/// WebDAV 同步：逐层走目录，把文件摄进来。
///
/// `external_key` 用 `webdav://host/path`——同一台网盘换了挂载点仍是同一份
/// 文件，而不同网盘上的同名路径是两份。
async fn sync_webdav(state: &AppState, source: &Source) -> anyhow::Result<SyncStats> {
    let base = source.config["base_url"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("webdav source is missing config.base_url"))?;
    let root = source.config["path"].as_str().map(str::trim).unwrap_or("/");
    let user = source.config["username"].as_str().map(str::trim);
    let pass = source.config["password"].as_str().map(str::trim);
    let auth = match (user, pass) {
        (Some(u), Some(p)) if !u.is_empty() => Some((u, p)),
        _ => None,
    };

    let http = crate::http_fetch::client_for(
        &reqwest::Url::parse(base).map_err(|e| anyhow::anyhow!("Invalid base_url: {e}"))?,
        crate::http_fetch::Reach::Operator,
        crate::http_fetch::Limits::default().with_overall(std::time::Duration::from_secs(60)),
    )
    .await?;
    let (files, truncated) = crate::webdav::fetch(&http, base, root, auth).await?;
    if truncated {
        tracing::warn!(base, root, "文件数到达单次上限，其余留给下一次同步");
    }

    let mut stats = SyncStats::default();
    for f in files {
        // 不猜 mime，理由同对象存储：解析先看魔数
        let action = ingest_item(
            state,
            source.kb_id,
            source.id,
            &f.external_key,
            &f.filename,
            "application/octet-stream",
            &f.bytes,
            f.last_modified,
        )
        .await?;
        stats.absorb(action);
    }
    Ok(stats)
}

/// Notion 同步：把 integration 能看见的页面摄进来。
///
/// `doc_time` 取 `last_edited_time`——**这是页面自己的编辑时刻**，比对象存储
/// 那边的写入时刻实在：一份 2019 年的合同今天传进 S3 会落在今天，而 Notion
/// 页面的编辑时刻就是它内容变化的时刻。
async fn sync_notion(state: &AppState, source: &Source) -> anyhow::Result<SyncStats> {
    let token = source.config["token"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("notion source is missing config.token"))?;
    let query = source.config["query"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let (pages, truncated) = crate::notion::fetch(token, query).await?;
    if truncated {
        tracing::warn!("页面数到达单次上限，其余留给下一次同步");
    }

    let mut stats = SyncStats::default();
    for p in pages {
        let action = ingest_item(
            state,
            source.kb_id,
            source.id,
            &p.external_key,
            &p.filename,
            "text/markdown",
            p.text.as_bytes(),
            p.last_edited,
        )
        .await?;
        stats.absorb(action);
    }
    Ok(stats)
}

/// 标题无扩展名时按 mime 补一个，接入解析矩阵的分派。
fn ensure_extension(title: &str, mime: &str) -> String {
    let has_ext = title
        .rsplit('.')
        .next()
        .map(|e| e.len() <= 5 && e.len() >= 2 && !e.contains(' '))
        .unwrap_or(false)
        && title.contains('.');
    if has_ext {
        return title.to_string();
    }
    let ext = if mime.contains("html") {
        "html"
    } else if mime.contains("plain") {
        "txt"
    } else {
        "md"
    };
    format!("{title}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::{
        bound_rss_identity, has_usable_rss_content, rss_entry_key, RSS_EXTERNAL_KEY_MAX_BYTES,
    };

    #[test]
    fn summary_only_does_not_make_an_rss_entry_hydratable_without_an_article_link() {
        assert!(!has_usable_rss_content(false, None));
        assert!(has_usable_rss_content(false, Some("https://example.com/a")));
        assert!(has_usable_rss_content(true, None));
    }

    #[test]
    fn overlong_rss_identity_is_hashed_without_prefix_collisions_from_truncation() {
        let first = "a".repeat(RSS_EXTERNAL_KEY_MAX_BYTES) + "-one";
        let second = "a".repeat(RSS_EXTERNAL_KEY_MAX_BYTES) + "-two";
        let first = bound_rss_identity(&first);
        let second = bound_rss_identity(&second);
        assert_ne!(first, second);
        assert!(first.len() < RSS_EXTERNAL_KEY_MAX_BYTES);
    }

    #[test]
    fn parser_leaves_missing_entry_ids_empty_for_application_fallback() {
        let feed = feed_rs::parser::Builder::new()
            .id_generator(|_, _, _| String::new())
            .build()
            .parse(
                &br#"<?xml version="1.0"?><rss version="2.0"><channel><title>Test</title><link>https://example.com/</link><description>Test</description><item><title>Original</title><description>Summary</description></item></channel></rss>"#[..],
            )
            .expect("test feed should parse");
        let entry = &feed.entries[0];
        assert!(entry.id.is_empty());
        assert!(rss_entry_key(entry, None).is_none());
        assert!(rss_entry_key(entry, Some("https://example.com/article")).is_some());
    }
}
