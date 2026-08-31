//! 来源同步任务：url（网页抓取）/ rss（订阅，pubDate → doc_time）/
//! github_issues / jira_issues（工单，更新时刻 → doc_time，正文里带状态变更史）。
//! 全部走 sha256 去重（重复内容静默跳过），新文档进标准摄入管道（process_document）。
//! folder 是纯容器（上传入内），api 是推送型——两者无拉取语义。

use crate::state::AppState;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use utopia_core::models::Source;
use uuid::Uuid;

/// 单次同步的新文档上限（防超长 feed/URL 列表拖垮任务）
const MAX_NEW_PER_SYNC: usize = 200;

/// 抓取用的 User-Agent。reqwest 默认一个都不发，而维基百科明确拒绝匿名请求
/// （403），Cloudflare 前置的站点也普遍如此——URL 与 RSS 两类来源因此对一大批
/// 真实网站直接失效。自报家门也是爬虫礼节：站方能认出我们、能联系到我们。
const UA: &str = concat!(
    "Utopia/",
    env!("CARGO_PKG_VERSION"),
    " (+https://utopia.bi; self-hosted knowledge platform)"
);

/// 单次同步的产出统计（Moved/Unchanged 不计）。
#[derive(Debug, Default, Clone, Copy)]
pub struct SyncStats {
    pub created: usize,
    pub updated: usize,
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

    let outcome = match source.kind.as_str() {
        "url" => sync_urls(state, &source).await,
        "rss" => sync_rss(state, &source).await,
        "custom" => sync_custom(state, &source).await,
        "github_issues" => sync_github_issues(state, &source).await,
        "jira_issues" => sync_jira_issues(state, &source).await,
        // folder / api 无拉取语义
        _ => Ok(SyncStats::default()),
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
            tracing::info!(%source_id, kind = %source.kind, created = stats.created, updated = stats.updated, "来源同步完成");
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

async fn write_blob(state: &AppState, sha256: &str, bytes: &[u8]) -> anyhow::Result<()> {
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

/// 身份感知摄入：按 (source, external_key) 三路判定——
/// 新增（建文档）/ 变更（原地替换 + 版本记录 + 重跑管道）/ 未变（跳过）；
/// 同内容换路径识别为移动（只改身份，不重跑）。external_key 为 URI 形态
/// （file:/// 相对路径、页面 URL、rss guid、api:{id}），出处自描述，
/// 也为 P5 SPARQL 投影的文档 IRI 提前对齐。
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
    if bytes.is_empty() {
        return Ok(IngestAction::Unchanged);
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
        if doc.sha256 == sha256 {
            return Ok(IngestAction::Unchanged);
        }
        // 变更：原地替换，旧版本入 document_versions（blob 内容寻址不删，回放有料）
        write_blob(state, &sha256, bytes).await?;
        utopia_store::documents::replace_content(
            &state.pool,
            doc.id,
            filename,
            mime,
            bytes.len() as i64,
            &sha256,
            doc_time,
        )
        .await?;
        utopia_store::documents::record_version(&state.pool, doc.id, &sha256, bytes.len() as i64)
            .await?;
        utopia_store::jobs::enqueue(
            &state.pool,
            "process_document",
            serde_json::json!({ "document_id": doc.id }),
        )
        .await?;
        state.emit_document(kb_id, doc.id);
        return Ok(IngestAction::Updated);
    }

    // 同内容出现在新路径：识别为移动/改名，不重跑管道
    if let Some(doc) =
        utopia_store::documents::find_by_source_sha(&state.pool, source_id, &sha256).await?
    {
        utopia_store::documents::update_location(&state.pool, doc.id, filename, external_key)
            .await?;
        state.emit_document(kb_id, doc.id);
        return Ok(IngestAction::Moved);
    }

    write_blob(state, &sha256, bytes).await?;
    match utopia_store::documents::create(
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
            utopia_store::documents::record_version(
                &state.pool,
                doc.id,
                &sha256,
                bytes.len() as i64,
            )
            .await?;
            utopia_store::jobs::enqueue(
                &state.pool,
                "process_document",
                serde_json::json!({ "document_id": doc.id }),
            )
            .await?;
            state.emit_document(kb_id, doc.id);
            Ok(IngestAction::Created)
        }
        // KB 内已有同内容（如手动上传过同一文件）：不重复摄入
        Err(utopia_core::AppError::Conflict(_)) => Ok(IngestAction::Unchanged),
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

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(UA)
        .build()?;
    let mut stats = SyncStats::default();
    let mut last_err: Option<String> = None;
    for url in urls.iter().take(MAX_NEW_PER_SYNC) {
        match fetch_page(&http, url).await {
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

async fn fetch_page(
    http: &reqwest::Client,
    url: &str,
) -> anyhow::Result<(String, String, Vec<u8>)> {
    let resp = http.get(url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/html")
        .split(';')
        .next()
        .unwrap_or("text/html")
        .to_string();
    let bytes = resp.bytes().await?.to_vec();
    let filename = filename_from_url(url, &mime);
    Ok((filename, mime, bytes))
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

async fn sync_rss(state: &AppState, source: &Source) -> anyhow::Result<SyncStats> {
    let feed_url = source.config["feed_url"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("rss source is missing config.feed_url"))?;

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(UA)
        .build()?;
    let resp = http.get(feed_url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} fetching feed", resp.status());
    }
    let bytes = resp.bytes().await?;
    let feed = feed_rs::parser::parse(&bytes[..])
        .map_err(|e| anyhow::anyhow!("Failed to parse feed: {e}"))?;

    let mut stats = SyncStats::default();
    for entry in feed.entries.iter().take(MAX_NEW_PER_SYNC) {
        let title = entry
            .title
            .as_ref()
            .map(|t| t.content.clone())
            .unwrap_or_else(|| "untitled".into());
        let link = entry
            .links
            .first()
            .map(|l| l.href.clone())
            .unwrap_or_default();
        // 逻辑身份：feed 规范的 guid（通常已是 permalink/urn），缺失时退条目链接
        let key = if !entry.id.trim().is_empty() {
            entry.id.trim().to_string()
        } else if !link.is_empty() {
            link.clone()
        } else {
            format!("entry:{}", sha_hex(title.as_bytes()))
        };
        let body = entry
            .content
            .as_ref()
            .and_then(|c| c.body.clone())
            .or_else(|| entry.summary.as_ref().map(|s| s.content.clone()))
            .unwrap_or_default();
        // 条目发布时间 → 文档时间：时态抽取吃到真实时间戳（本平台的差异化正在于此）
        let doc_time = entry.published.or(entry.updated);

        let html = format!(
            "<html><head><title>{}</title></head><body><h1>{}</h1>\n<p><a href=\"{}\">{}</a></p>\n{}</body></html>",
            title, title, link, link, body
        );
        let mut slug: String = title
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        slug.truncate(80);
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

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(UA)
        .build()?;
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

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .user_agent(UA)
        .build()?;

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

    // loopback 端点不走系统代理：代理对回环地址只会 502，本机服务必须直连
    let loopback = url
        .host_str()
        .map(|h| {
            h.eq_ignore_ascii_case("localhost") || h == "127.0.0.1" || h == "::1" || h == "[::1]"
        })
        .unwrap_or(false);
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(UA);
    if loopback {
        builder = builder.no_proxy();
    }
    let http = builder.build()?;
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
