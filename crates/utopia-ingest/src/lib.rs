//! utopia-ingest: 解析矩阵 + 分块。
//! 原则：文本层 Rust 原生解决（快、零依赖）；扫描件/复杂版式后续走 docling sidecar。

#[cfg(test)]
mod html_tests {
    #[test]
    fn html_fragment_preserves_scoped_content() {
        let raw = "<nav>Scoped feed navigation</nav><p>Short <em>entry</em>.</p>";
        let markdown = super::html::fragment_to_markdown(raw).unwrap();
        assert!(markdown.contains("Scoped feed navigation"));
        assert!(markdown.contains("*entry*"));
    }
    #[test]
    fn html_readability_failure_uses_raw_conversion() {
        let raw = "<p>Short <strong>fallback</strong> body.</p>";
        // An invalid base URL deterministically fails Readability setup.
        assert!(dom_smoothie::Readability::new(raw, Some("not a URL"), None).is_err());
        assert_eq!(
            super::html::page_to_markdown(raw, Some("not a URL")).unwrap(),
            super::html::fragment_to_markdown(raw).unwrap()
        );
    }
    #[test]
    fn html_empty_output_remains_parse_error() {
        assert!(super::parse("empty.html", b"<html><body></body></html>").is_err());
    }
    #[test]
    fn html_interstitial_errors_are_stable_for_pages_and_fragments() {
        for raw in [
            "<form><input type='email'></form>",
            "<video data-player='player'></video>",
        ] {
            assert_eq!(
                super::html::page_to_markdown(raw, Some("not a URL")),
                Err(super::html::HtmlError::Interstitial)
            );
            assert_eq!(
                super::html::fragment_to_markdown(raw),
                Err(super::html::HtmlError::Interstitial)
            );
        }
    }
    #[test]
    fn html_markdown_normalization_removes_unsafe_destinations() {
        let normalized = super::html::normalize_markdown("  [bad](javascript:evil) [encoded](%64ata:text/plain,evil) [safe](https://example.com)\n\n\n\n  ").unwrap();
        assert_eq!(normalized, "bad encoded [safe](https://example.com)");
    }
    #[test]
    fn html_interstitial_is_not_raw_fallback() {
        let html = "<form><input type='password'></form><p>Readable shell</p>";
        assert!(super::parse("page.html", html.as_bytes()).is_err());
    }
    #[test]
    fn html_long_login_only_body_is_rejected_after_extraction_and_on_fallback() {
        let raw = format!("<html><body><h1>Sign in to continue</h1><form><input type='email'><input type='password'></form><p>{}</p></body></html>", "Account access requires verification of your identity before proceeding. ".repeat(40));
        for base in [Some("https://example.com/login"), Some("not a URL")] {
            assert_eq!(
                super::html::page_to_markdown(&raw, base),
                Err(super::html::HtmlError::Interstitial)
            );
        }
    }

    #[test]
    fn html_substantive_fallback_survives_newsletter_controls() {
        let body =
            "Detailed reporting on the community with sources and supporting evidence. ".repeat(30);
        let raw = format!("<form><input type='email'></form><article><p>{body}</p></article>");
        assert!(dom_smoothie::Readability::new(raw.as_str(), Some("not a URL"), None).is_err());
        let markdown = super::html::page_to_markdown(&raw, Some("not a URL")).unwrap();
        assert!(markdown.contains(body.trim()));
    }

    #[test]
    fn html_articles_survive_newsletter_and_login_chrome() {
        let body = "Reporting on password security with detailed supporting evidence. ".repeat(80);
        for chrome in [
            "<aside><form><label>Newsletter</label><input type='email'></form></aside>",
            "<aside><form><label>Login</label><input type='password'></form></aside>",
        ] {
            let raw = format!("<html><body>{chrome}<article><h1>Security reporting</h1><p>{body}</p></article></body></html>");
            let parsed = super::parse("page.html", raw.as_bytes()).unwrap();
            assert!(parsed.text.contains(body.trim()));
        }
    }

    #[test]
    fn html_page_removes_chrome() {
        let body = "Substantive reporting with detailed evidence. ".repeat(80);
        let html = format!("<html><head><title>Story</title></head><body><nav>Navigation noise</nav><article><h1>Story</h1><p>{body}</p></article><footer>Footer noise</footer></body></html>");
        let parsed = super::parse("page.html", html.as_bytes()).unwrap();
        assert!(parsed.text.contains("Story"));
        assert!(parsed.text.contains(body.trim()));
        assert!(!parsed.text.contains("Navigation noise"), "{}", parsed.text);
        assert!(!parsed.text.contains("Footer noise"));
    }
    #[test]
    fn html_preserves_markdown_structure() {
        let parsed = super::parse(
            "page.html",
            b"<article><h1>Story</h1><p>Useful <strong>body</strong>.</p></article>",
        )
        .unwrap();
        assert!(parsed.text.contains("**body**"), "{}", parsed.text);
    }
}

mod chunker;
pub mod html;
pub mod ontology_rdf;
mod parsers;

pub use chunker::{chunk_text, ChunkPiece};
/// Decode fetched text with the same encoding detection as file ingestion.
pub use parsers::plain_text as decode_text;

/// 解析产物：纯文本 + 可选结构信息。
#[derive(Debug)]
pub struct ParsedDoc {
    pub text: String,
}

/// 支持的格式（P1）：pdf / docx / xlsx·xls·ods / pptx / md / txt / html / csv / json / yaml / xml / log
pub fn parse(filename: &str, bytes: &[u8]) -> anyhow::Result<ParsedDoc> {
    let ext = filename
        .rsplit('.')
        .next()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    // 魔数探测优先于扩展名（扩展名可能撒谎）
    let kind = infer::get(bytes).map(|t| t.extension()).unwrap_or("");

    let text = match (kind, ext.as_str()) {
        ("pdf", _) | (_, "pdf") => parsers::pdf(bytes)?,
        ("docx", _) | (_, "docx") => parsers::docx(bytes)?,
        ("xlsx", _) | (_, "xlsx") | (_, "xls") | (_, "ods") => parsers::spreadsheet(bytes)?,
        ("pptx", _) | (_, "pptx") => parsers::pptx(bytes)?,
        (_, "html") | (_, "htm") => parsers::html(bytes)?,
        (_, "csv") | (_, "tsv") => parsers::csv_text(bytes, ext == "tsv")?,
        // md/json/yaml/xml/log/txt 及一切未识别格式：按文本解码（编码探测覆盖 GBK 等）
        _ => parsers::plain_text(bytes),
    };

    let text = normalize(&text);
    if text.trim().is_empty() {
        anyhow::bail!("No text could be extracted (possibly a scanned or empty file)");
    }
    Ok(ParsedDoc { text })
}

/// 压缩连续空白行，统一换行符。
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out
}
