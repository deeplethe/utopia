use crate::http_fetch::{self, FetchError, Limits, Reach};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::time::Duration;
use url::Url;
use utopia_ingest::html::{self, HtmlError};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ContentError {
    #[error("invalid article URL")]
    InvalidUrl,
    #[error("article destination is not public")]
    BlockedAddress,
    #[error("article DNS resolution failed")]
    DnsResolution,
    #[error("article connection failed")]
    Network,
    #[error("article request timed out")]
    Timeout,
    #[error("article response is too large")]
    TooLarge,
    #[error("article response is not HTML")]
    NotHtml,
    #[error("article response encoding is unsupported")]
    UnsupportedEncoding,
    #[error("article redirect downgraded HTTPS to HTTP")]
    BlockedRedirect,
    #[error("article redirect limit exceeded")]
    RedirectLimit,
    #[error("article response status is not acceptable")]
    HttpStatus { retryable: bool },
    #[error("HTML conversion failed: {0}")]
    Conversion(String),
    #[error("article page is a challenge or consent shell")]
    ChallengePage,
    #[error("article page requires unsupported video or player content")]
    VideoRequired,
    #[error("article content is not substantive")]
    NonSubstantive,
    #[error("article has no usable content source")]
    NoUsableSource,
    #[error("canonical Markdown is too large")]
    MarkdownTooLarge,
}

impl ContentError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_url",
            Self::BlockedAddress => "blocked_address",
            Self::DnsResolution => "dns_timeout",
            Self::Network => "connect_timeout",
            Self::Timeout => "connect_timeout",
            Self::TooLarge => "response_too_large",
            Self::NotHtml => "unsupported_content_type",
            Self::UnsupportedEncoding => "unsupported_content_type",
            Self::BlockedRedirect | Self::RedirectLimit => "blocked_redirect",
            Self::HttpStatus { retryable: true } => "http_retryable",
            Self::HttpStatus { retryable: false } => "http_denied",
            Self::Conversion(_) => "conversion_failed",
            Self::ChallengePage => "challenge_page",
            Self::VideoRequired => "video_required",
            Self::NonSubstantive => "non_substantive",
            Self::NoUsableSource => "non_substantive",
            Self::MarkdownTooLarge => "conversion_failed",
        }
    }

    pub(crate) fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::DnsResolution
                | Self::Network
                | Self::Timeout
                | Self::HttpStatus { retryable: true }
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FetchConfig {
    pub(crate) max_response_bytes: usize,
    pub(crate) max_redirects: usize,
    pub(crate) dns_timeout: Duration,
    pub(crate) connect_timeout: Duration,
    pub(crate) read_timeout: Duration,
    pub(crate) overall_timeout: Duration,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            max_response_bytes: 8 * 1024 * 1024,
            max_redirects: 5,
            dns_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(15),
            overall_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug)]
pub(crate) struct FetchedPage {
    pub(crate) final_url: Url,
    pub(crate) content_type: String,
    pub(crate) body: String,
}

pub(crate) async fn fetch_article(raw_url: &str) -> Result<FetchedPage, ContentError> {
    fetch_article_with_config(raw_url, FetchConfig::default()).await
}

const MAX_MARKDOWN_BYTES: usize = 2 * 1024 * 1024;
const HYDRATION_ATTEMPTS: i32 = 5;

#[derive(Debug)]
struct AcceptedContent {
    markdown: String,
    source: &'static str,
    final_url: Option<String>,
}

/// Durable job entry point. The job payload contains only UUIDs; all bounded
/// feed data comes from the ledger row, so a later replay does not depend on a
/// feed still exposing the item.
pub(crate) async fn hydrate_entry(
    state: &crate::state::AppState,
    hydration_job_id: i64,
    source_id: uuid::Uuid,
    entry_id: uuid::Uuid,
) -> anyhow::Result<()> {
    let entry = utopia_store::rss_full_content::get_entry(&state.pool, entry_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("RSS hydration entry was deleted"))?;
    if entry.source_id != source_id {
        return Err(anyhow::Error::new(utopia_core::Terminal).context("source_mismatch"));
    }
    let source = utopia_store::sources::get(&state.pool, source_id).await?;
    let Some(attempt) =
        utopia_store::rss_full_content::current_attempt(&state.pool, entry_id, hydration_job_id)
            .await?
    else {
        return Ok(());
    };
    state.emit_source(source.kb_id);

    let result = hydrate_content(&entry).await;
    let accepted = match result {
        Ok(content) => content,
        Err(error) => {
            let result = handle_hydration_failure(
                &state.pool,
                entry_id,
                hydration_job_id,
                attempt,
                error.into(),
            )
            .await;
            state.emit_source(source.kb_id);
            return result;
        }
    };
    let digest = sha256_hex(accepted.markdown.as_bytes());
    if let Err(error) =
        crate::ingest_sources::write_blob(state, &digest, accepted.markdown.as_bytes()).await
    {
        let result =
            handle_hydration_failure(&state.pool, entry_id, hydration_job_id, attempt, error).await;
        state.emit_source(source.kb_id);
        return result;
    }
    let completed = match utopia_store::rss_full_content::complete_hydration(
        &state.pool,
        entry_id,
        hydration_job_id,
        source.id,
        entry.activation_generation,
        source.kb_id,
        &entry.external_key,
        &format!("{}.md", hydration_filename(&entry.title)),
        "text/markdown",
        accepted.markdown.len() as i64,
        &digest,
        entry.doc_time,
        accepted.source,
        accepted.final_url.as_deref(),
    )
    .await
    {
        Ok(completed) => completed,
        Err(error) => {
            let result = handle_hydration_failure(
                &state.pool,
                entry_id,
                hydration_job_id,
                attempt,
                error.into(),
            )
            .await;
            state.emit_source(source.kb_id);
            return result;
        }
    };
    if let Some(document_id) = completed {
        state.emit_document(source.kb_id, document_id);
    }
    state.emit_source(source.kb_id);
    Ok(())
}

async fn hydrate_content(
    entry: &utopia_store::rss_full_content::Entry,
) -> Result<AcceptedContent, ContentError> {
    if let Some(embedded_html) = entry.embedded_html.as_deref() {
        if let Ok(markdown) = normalize_feed_html(embedded_html) {
            if quality_check(&markdown, &entry.summary, false).is_ok() {
                return canonical_content(markdown, "feed", None);
            }
        }
    }

    let Some(article_url) = entry.article_url.as_deref() else {
        return Err(ContentError::NoUsableSource);
    };
    let page = fetch_article(article_url).await?;
    let markdown = if page.content_type == "text/markdown" {
        normalize_markdown(&page.body)?
    } else {
        extract_article_markdown(&page.body, page.final_url.as_str())?
    };
    accept_linked_markdown(markdown, &entry.summary, Some(page.final_url.to_string()))
}

fn canonical_content(
    markdown: String,
    source: &'static str,
    final_url: Option<String>,
) -> Result<AcceptedContent, ContentError> {
    if markdown.is_empty() {
        return Err(ContentError::NonSubstantive);
    }
    if markdown.len() > MAX_MARKDOWN_BYTES {
        return Err(ContentError::MarkdownTooLarge);
    }
    Ok(AcceptedContent {
        markdown,
        source,
        final_url,
    })
}

fn accept_linked_markdown(
    markdown: String,
    summary: &str,
    final_url: Option<String>,
) -> Result<AcceptedContent, ContentError> {
    quality_check(&markdown, summary, true)?;
    canonical_content(markdown, "web", final_url)
}

async fn handle_hydration_failure(
    pool: &sqlx::PgPool,
    entry_id: uuid::Uuid,
    hydration_job_id: i64,
    attempt: i32,
    error: anyhow::Error,
) -> anyhow::Result<()> {
    let (code, retryable) = error
        .downcast_ref::<ContentError>()
        .map(|error| (error.code(), error.is_retryable()))
        .unwrap_or(("ingest_failed", true));
    let terminal = !retryable || attempt >= HYDRATION_ATTEMPTS;
    if utopia_store::rss_full_content::current_attempt(pool, entry_id, hydration_job_id)
        .await?
        .is_none()
    {
        return Ok(());
    }
    if terminal {
        return Err(anyhow::Error::new(utopia_core::Terminal).context(code));
    }
    Err(anyhow::anyhow!(code))
}

fn hydration_filename(title: &str) -> String {
    let filename: String = title
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { '-' })
        .take(80)
        .collect();
    let filename = filename.trim_matches('-');
    if filename.is_empty() {
        "untitled".into()
    } else {
        filename.into()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) async fn fetch_article_with_config(
    raw_url: &str,
    config: FetchConfig,
) -> Result<FetchedPage, ContentError> {
    let fetched = http_fetch::get(
        raw_url,
        Reach::Content,
        Limits {
            max_bytes: config.max_response_bytes,
            max_redirects: config.max_redirects,
            dns_timeout: config.dns_timeout,
            connect_timeout: config.connect_timeout,
            read_timeout: Some(config.read_timeout),
            overall_timeout: config.overall_timeout,
        },
    )
    .await
    .map_err(ContentError::from)?;
    read_page_response(fetched)
}

fn read_page_response(page: http_fetch::Fetched) -> Result<FetchedPage, ContentError> {
    if !matches!(
        page.mime.as_str(),
        "text/html" | "application/xhtml+xml" | "text/markdown"
    ) {
        return Err(ContentError::NotHtml);
    }
    if page.content_encoding.as_ref().is_some_and(|value| {
        value
            .to_str()
            .map(|value| !value.trim().is_empty() && !value.eq_ignore_ascii_case("identity"))
            .unwrap_or(true)
    }) {
        return Err(ContentError::UnsupportedEncoding);
    }
    Ok(FetchedPage {
        final_url: page.final_url,
        content_type: page.mime,
        body: utopia_ingest::decode_text(&page.bytes),
    })
}

impl From<FetchError> for ContentError {
    fn from(error: FetchError) -> Self {
        match error {
            FetchError::InvalidUrl(_) | FetchError::UnsupportedScheme(_) => Self::InvalidUrl,
            FetchError::BlockedAddress(_) => Self::BlockedAddress,
            FetchError::Dns(_) => Self::DnsResolution,
            FetchError::BlockedRedirect(_) => Self::BlockedRedirect,
            FetchError::RedirectLimit => Self::RedirectLimit,
            FetchError::TooLarge(_) => Self::TooLarge,
            FetchError::Status(status) => Self::HttpStatus {
                retryable: matches!(status, 401 | 403 | 408 | 425 | 429 | 500..=599),
            },
            FetchError::Transport(error) if error.is_timeout() => Self::Timeout,
            FetchError::Transport(_) => Self::Network,
        }
    }
}

impl From<HtmlError> for ContentError {
    fn from(error: HtmlError) -> Self {
        match error {
            HtmlError::Interstitial => Self::ChallengePage,
            HtmlError::Conversion(detail) => Self::Conversion(detail),
        }
    }
}

pub(crate) fn normalize_feed_html(raw: &str) -> Result<String, ContentError> {
    html::fragment_to_markdown(raw).map_err(Into::into)
}

pub(crate) fn extract_article_markdown(raw: &str, url: &str) -> Result<String, ContentError> {
    html::page_to_markdown(raw, Some(url)).map_err(Into::into)
}

pub(crate) fn normalize_markdown(markdown: &str) -> Result<String, ContentError> {
    let markdown = html::normalize_markdown(markdown)?;
    if markdown.is_empty() {
        return Err(ContentError::NonSubstantive);
    }
    Ok(markdown)
}

pub(crate) fn quality_check(
    markdown: &str,
    summary: &str,
    linked_page: bool,
) -> Result<(), ContentError> {
    if !is_substantive(markdown, summary, linked_page) {
        return Err(ContentError::NonSubstantive);
    }
    let lowered = markdown.to_ascii_lowercase();
    if html::looks_like_challenge_shell(&lowered) {
        return Err(ContentError::ChallengePage);
    }
    let link_count = markdown.matches("](").count();
    if lowered.contains("video player")
        || lowered.contains("watch the video")
        || (link_count > 20 && markdown.split_whitespace().count() < link_count * 8)
    {
        return Err(ContentError::VideoRequired);
    }
    Ok(())
}

fn is_substantive(markdown: &str, summary: &str, linked_page: bool) -> bool {
    let non_whitespace = markdown.chars().filter(|ch| !ch.is_whitespace()).count();
    // CJK prose has no word separators. Count each ideograph/syllable as a
    // lexical unit without lowering the English thresholds. A repeated handful
    // of navigation labels is not evidence of an article.
    let is_cjk = |ch: char| {
        matches!(ch as u32,
        0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff |
        0x20000..=0x323af | 0x3040..=0x30ff | 0xac00..=0xd7af)
    };
    let cjk_count = markdown.chars().filter(|ch| is_cjk(*ch)).count();
    let cjk_unique = markdown
        .chars()
        .filter(|ch| is_cjk(*ch))
        .collect::<HashSet<_>>()
        .len();
    if cjk_count >= 80 && cjk_unique < 20 {
        return false;
    }
    let words = cjk_count
        + markdown
            .split(is_cjk)
            .flat_map(str::split_whitespace)
            .count();
    let (min_words, min_chars) = if linked_page { (200, 1_200) } else { (80, 500) };
    if words < min_words && non_whitespace < min_chars {
        return false;
    }
    if summary.trim().is_empty() {
        return true;
    }
    let normalized = |value: &str| {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let body = normalized(markdown);
    let summary = normalized(summary);
    body != summary
        && !(body.starts_with(&summary)
            && body.len().saturating_sub(summary.len()) < if linked_page { 300 } else { 160 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn generic_html_and_rss_share_article_body_with_base_dependent_links() {
        let body = "Substantive reporting with detailed supporting evidence. ".repeat(80);
        let raw = format!("<html><head><title>Story</title></head><body><nav>Navigation noise</nav><article><h1>Story</h1><p>{body}</p><p><a href='related'>Related reporting</a></p></article><footer>Footer noise</footer></body></html>");
        let generic = utopia_ingest::parse("page.html", raw.as_bytes())
            .unwrap()
            .text;
        let rss = extract_article_markdown(&raw, "https://example.com/redirected/story").unwrap();
        for markdown in [&generic, &rss] {
            assert!(markdown.contains(body.trim()));
            assert!(markdown.contains("Story"));
            assert!(!markdown.contains("Navigation noise"));
            assert!(!markdown.contains("Footer noise"));
        }
        assert!(rss.contains("https://example.com/redirected/related"));
        let body_semantics = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            body_semantics(&generic),
            body_semantics(&rss.replace("https://example.com/redirected/related", "related"))
        );
    }

    #[tokio::test]
    async fn linked_response_decodes_gbk_and_utf8_like_uploads() {
        let expected = "中文新闻报道：记者调查城市交通建设，居民关注公共服务。";
        let gbk = vec![
            214, 208, 206, 196, 208, 194, 206, 197, 177, 168, 181, 192, 163, 186, 188, 199, 213,
            223, 181, 247, 178, 233, 179, 199, 202, 208, 189, 187, 205, 168, 189, 168, 201, 232,
            163, 172, 190, 211, 195, 241, 185, 216, 215, 162, 185, 171, 185, 178, 183, 254, 206,
            241, 161, 163,
        ];
        for bytes in [gbk, expected.as_bytes().to_vec()] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(bytes.clone(), "text/html"))
                .mount(&server)
                .await;
            let fetched = http_fetch::get(&server.uri(), Reach::Operator, Limits::default())
                .await
                .unwrap();
            let page = read_page_response(fetched).unwrap();
            assert_eq!(page.body, expected);
            assert_eq!(
                utopia_ingest::parse("text.txt", &bytes)
                    .unwrap()
                    .text
                    .trim(),
                page.body
            );
        }
    }

    #[test]
    fn direct_markdown_uses_shared_normalization() {
        let raw = "  # Story\n\n\n\n[bad](%6Aavascript:evil) [safe](https://example.com)  ";
        assert_eq!(
            normalize_markdown(raw).unwrap(),
            html::normalize_markdown(raw).unwrap()
        );
        assert!(!normalize_markdown(raw).unwrap().contains("%6Aavascript"));
        assert_eq!(normalize_markdown("  "), Err(ContentError::NonSubstantive));
    }

    #[test]
    fn article_with_login_chrome_is_not_an_interstitial() {
        let body = "Detailed reporting with plenty of supporting evidence. ".repeat(80);
        let html = format!("<form><input type='password'></form><article><p>{body}</p></article>");
        let markdown = extract_article_markdown(&html, "https://example.com/story").unwrap();
        assert!(markdown.contains(body.trim()));
        assert_eq!(quality_check(&markdown, "", true), Ok(()));
    }

    #[test]
    fn feed_html_is_normalized_to_markdown() {
        let html = "<article><h1>Article title</h1><p>Article body.</p></article>";
        let markdown = normalize_feed_html(html).expect("normalization should succeed");
        assert!(markdown.contains("# Article title"));
        assert!(markdown.contains("Article body."));
    }

    #[test]
    fn shared_response_adapter_keeps_rss_mime_encoding_and_retry_policy() {
        for mime in [
            "text/html",
            "application/xhtml+xml",
            "text/markdown",
            "text/plain",
            "application/octet-stream",
        ] {
            for encoding in [None, Some("identity"), Some("gzip"), Some("br")] {
                let result = read_page_response(http_fetch::Fetched {
                    final_url: Url::parse("https://example.com/final").unwrap(),
                    mime: mime.into(),
                    bytes: b"body".to_vec(),
                    content_encoding: encoding.map(reqwest::header::HeaderValue::from_static),
                });
                if matches!(mime, "text/plain" | "application/octet-stream") {
                    assert!(matches!(result, Err(ContentError::NotHtml)));
                } else if matches!(encoding, Some("gzip" | "br")) {
                    assert!(matches!(result, Err(ContentError::UnsupportedEncoding)));
                } else {
                    assert_eq!(
                        result.unwrap().final_url.as_str(),
                        "https://example.com/final"
                    );
                }
            }
        }
        for status in [401, 403, 408, 425, 429, 500, 503] {
            assert!(ContentError::from(FetchError::Status(status)).is_retryable());
        }
        assert!(!ContentError::from(FetchError::Status(404)).is_retryable());
        assert_eq!(
            ContentError::from(FetchError::TooLarge(8)).code(),
            "response_too_large"
        );
    }

    // Opt-in network acceptance, not an offline CI fixture. Endpoint contracts:
    // https://httpbingo.org/ documents /redirect-to and /html. Availability,
    // public DNS, TLS and the service's example body are external dependencies.
    // Run: cargo test -p utopia-server live_content_reach_rss -- --ignored --nocapture
    // No resolver override, proxy, private-address exception or alternate caller.
    #[tokio::test]
    #[ignore = "live public HTTPS acceptance; requires network and httpbingo.org"]
    async fn live_content_reach_rss() {
        let url = "https://httpbingo.org/redirect-to?url=%2Fhtml";
        let page = fetch_article_with_config(url, FetchConfig::default())
            .await
            .expect("Content reach must resolve/pin both public HTTPS hops and adapt HTML");
        assert_eq!(page.final_url.as_str(), "https://httpbingo.org/html");
        assert_eq!(page.content_type, "text/html");
        assert!(page.body.contains("Herman Melville - Moby-Dick"));
        let markdown = extract_article_markdown(&page.body, page.final_url.as_str()).unwrap();
        assert!(markdown.contains("portable forge"));
        assert_eq!(quality_check(&markdown, "", true), Ok(()));
        eprintln!(
            "Content reach accepted {}: {} bytes, extracted {} bytes",
            page.final_url,
            page.body.len(),
            markdown.len()
        );

        let too_small = FetchConfig {
            max_response_bytes: 32,
            ..FetchConfig::default()
        };
        assert!(matches!(
            fetch_article_with_config(url, too_small).await,
            Err(ContentError::TooLarge)
        ));
        let no_redirect = FetchConfig {
            max_redirects: 0,
            ..FetchConfig::default()
        };
        assert!(matches!(
            fetch_article_with_config(url, no_redirect).await,
            Err(ContentError::RedirectLimit)
        ));
    }

    #[tokio::test]
    async fn rss_fetch_cannot_reach_loopback() {
        assert!(matches!(
            fetch_article("http://127.0.0.1/article").await,
            Err(ContentError::BlockedAddress)
        ));
        let defaults = FetchConfig::default();
        assert_eq!(defaults.max_response_bytes, 8 * 1024 * 1024);
        assert_eq!(defaults.max_redirects, 5);
        assert_eq!(defaults.dns_timeout, Duration::from_secs(5));
        assert_eq!(defaults.connect_timeout, Duration::from_secs(10));
        assert_eq!(defaults.read_timeout, Duration::from_secs(15));
        assert_eq!(defaults.overall_timeout, Duration::from_secs(30));
    }

    #[test]
    fn rss_uses_shared_public_address_policy() {
        assert!(http_fetch::validate_content_url("https://192.0.1.1/article").is_ok());
        let source = include_str!("rss_full_content.rs");
        assert!(source.contains("http_fetch::get("));
        for duplicate in [
            "fetch_once",
            "resolve_public_addresses",
            "redirect_target",
            "is_public_ip",
        ] {
            assert!(
                !source.contains(&format!("fn {duplicate}(")),
                "duplicate guard {duplicate}"
            );
        }
    }

    #[test]
    fn article_url_policy_rejects_unsafe_shapes() {
        for raw in [
            "file:///etc/passwd",
            "ftp://example.com/article",
            "https://user:pass@example.com/article",
            "https://@example.com/article",
            "https://example.com:8443/article",
            "https://example.com/article#fragment",
            "http://127.0.0.1/article",
            "http://[::1]/article",
        ] {
            assert!(
                http_fetch::validate_content_url(raw).is_err(),
                "unsafe URL was accepted: {raw}"
            );
        }
        assert!(http_fetch::validate_content_url("https://example.com/article").is_ok());
    }

    #[test]
    fn feed_normalization_removes_active_and_image_content() {
        let html = r#"<p>Keep this text.</p><script>alert('x')</script><img src="https://example.com/a.png"><p><a href="javascript:alert(1)">bad link</a></p>"#;
        let markdown = normalize_feed_html(html).expect("normalization should succeed");
        assert!(markdown.contains("Keep this text."));
        assert!(!markdown.contains("alert"));
        assert!(!markdown.contains("!["));
        assert!(!markdown.contains("javascript:"));
    }

    #[test]
    fn feed_normalization_preserves_utf8_and_safe_links() {
        let html = r#"<p>Привет, мир — 你好。</p><p><a href="https://example.com/article">安全链接</a></p>"#;
        let markdown = normalize_feed_html(html).expect("normalization should succeed");
        assert!(markdown.contains("Привет, мир — 你好。"));
        assert!(markdown.contains("[安全链接](https://example.com/article)"));
    }

    #[test]
    fn article_extraction_drops_navigation_chrome() {
        let article_body = (0..240)
            .map(|index| format!("article-word-{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let html = format!(
            "<html><head><title>Story</title></head><body><nav>{}</nav><aside>{}</aside><article><h1>Story</h1><p>{article_body}</p></article></body></html>",
            "navigation-link ".repeat(200),
            "sidebar-link ".repeat(200),
        );
        let markdown = extract_article_markdown(&html, "https://example.com/story")
            .expect("readability should extract the article");
        assert!(markdown.contains("article-word-0"));
        assert!(!markdown.contains("navigation-link"));
        assert!(!markdown.contains("sidebar-link"));
    }

    #[test]
    fn article_url_policy_blocks_special_and_embedded_addresses() {
        for raw in [
            "http://10.0.0.1/article",
            "http://100.64.0.1/article",
            "http://169.254.1.1/article",
            "http://192.168.1.1/article",
            "http://224.0.0.1/article",
            "http://[::]/article",
            "http://[fc00::1]/article",
            "http://[fe80::1]/article",
            "http://[::ffff:192.168.1.1]/article",
            "http://[64:ff9b::c0a8:0101]/article",
            "http://[2002:c0a8:0101::]/article",
        ] {
            assert!(
                http_fetch::validate_content_url(raw).is_err(),
                "special address was accepted: {raw}"
            );
        }
        assert!(http_fetch::validate_content_url("http://[2001:4860:4860::8888]/article").is_ok());
    }

    #[test]
    fn cjk_news_is_substantive_without_whitespace() {
        let reporting = "记者走访城市公共交通建设现场，发现新的线路连接学校医院和居民社区。工程团队介绍施工进展，并公布环境监测数据。当地居民讨论出行需求，专家分析财政预算与长期维护成本。有关部门表示将继续收集意见，调整服务时间，保障不同地区乘客的基本需求。";
        let body: String = reporting.chars().cycle().take(600).collect();
        assert_eq!(body.chars().count(), 600);
        assert_eq!(quality_check(&body, "简短摘要", true), Ok(()));
        assert_eq!(quality_check(&body, "简短摘要", false), Ok(()));
        assert_eq!(
            quality_check(&body, &body, true),
            Err(ContentError::NonSubstantive)
        );
        assert_eq!(
            quality_check("记者报道城市交通建设", "", true),
            Err(ContentError::NonSubstantive)
        );
        assert!(quality_check(&"登录注册订阅".repeat(120), "", true).is_err());
        assert!(quality_check(&"short ".repeat(100), "", true).is_err());
        assert_eq!(quality_check(&"word ".repeat(200), "", true), Ok(()));
    }

    #[test]
    fn quality_policy_rejects_summary_shells_and_challenges() {
        let summary = "A short summary.";
        assert_eq!(
            quality_check(summary, summary, false),
            Err(ContentError::NonSubstantive)
        );

        let body = (0..210)
            .map(|index| format!("word{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            quality_check(&format!("{body} verify you are human"), "", true),
            Err(ContentError::ChallengePage)
        );
        assert_eq!(
            quality_check(&format!("{body} watch the video"), "", true),
            Err(ContentError::VideoRequired)
        );
        assert!(!ContentError::ChallengePage.is_retryable());
        assert!(!ContentError::VideoRequired.is_retryable());
        assert!(!ContentError::NonSubstantive.is_retryable());
        assert!(ContentError::Timeout.is_retryable());
        assert!(ContentError::HttpStatus { retryable: true }.is_retryable());
    }

    #[tokio::test]
    async fn summary_only_entries_are_not_marked_as_full_content() {
        let now = Utc::now();
        let entry = utopia_store::rss_full_content::Entry {
            id: Uuid::nil(),
            source_id: Uuid::nil(),
            activation_generation: 1,
            external_key: "summary-only".into(),
            title: "Summary only".into(),
            article_url: None,
            summary: "A short but useful RSS summary.".into(),
            embedded_html: None,
            doc_time: None,
            state: "hydrating".into(),
            hydration_job_id: None,
            attempt_count: 1,
            document_id: None,
            error_code: None,
            error_detail: None,
            first_seen_at: now,
            updated_at: now,
            completed_at: None,
        };
        assert!(matches!(
            hydrate_content(&entry).await,
            Err(ContentError::NoUsableSource)
        ));
    }

    #[test]
    fn generic_login_shells_fail_the_quality_gate() {
        let body = (0..210)
            .map(|index| format!("shell{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let shell = format!("# Login\nEmail\nPassword\n{body}");
        assert!(matches!(
            quality_check(&shell, "", true),
            Err(ContentError::ChallengePage)
        ));
    }

    #[test]
    fn private_nat64_prefix_is_blocked() {
        assert!(matches!(
            http_fetch::validate_content_url("http://[64:ff9b:1::1]/article"),
            Err(FetchError::BlockedAddress(_))
        ));
    }

    #[test]
    fn linked_markdown_must_pass_the_linked_page_quality_gate() {
        assert!(matches!(
            accept_linked_markdown("short shell".into(), "", None),
            Err(ContentError::NonSubstantive)
        ));
    }

    #[test]
    fn quality_policy_allows_an_article_that_discusses_paywalls_and_captchas() {
        let body = (0..210)
            .map(|index| format!("article{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let body = format!(
            "{body} This analysis compares paywall economics and captcha usability in publishing."
        );
        assert_eq!(quality_check(&body, "", true), Ok(()));
    }
}
