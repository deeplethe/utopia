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

#[cfg(test)]
mod reader_tests {
    use super::*;

    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D,
    ];
    const MP3: &[u8] = &[b'I', b'D', b'3', 4, 0, 0, 0, 0, 0, 0];
    const WAV: &[u8] = &[
        b'R', b'I', b'F', b'F', 0x24, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
    ];

    fn needs(filename: &str, bytes: &[u8]) -> Option<Reader> {
        parse(filename, bytes)
            .err()
            .and_then(|e| e.downcast_ref::<NeedsReader>().map(|n| n.reader))
    }

    fn unreadable(filename: &str, bytes: &[u8]) -> bool {
        parse(filename, bytes)
            .err()
            .is_some_and(|e| e.downcast_ref::<Unreadable>().is_some())
    }

    #[test]
    fn an_image_or_a_recording_waits_for_its_reader() {
        assert_eq!(needs("scan.png", PNG), Some(Reader::Ocr));
        // 扩展名撒谎：文件头说了算
        assert_eq!(needs("notes.txt", PNG), Some(Reader::Ocr));
        assert_eq!(needs("meeting.mp3", MP3), Some(Reader::Transcribe));
        assert_eq!(needs("meeting.wav", WAV), Some(Reader::Transcribe));
        // 认不出文件头时看扩展名
        assert_eq!(needs("photo.heic", &[0, 1, 2, 3]), Some(Reader::Ocr));
        assert_eq!(needs("call.amr", &[0, 1, 2, 3]), Some(Reader::Transcribe));
    }

    #[test]
    fn a_binary_is_not_decoded_as_text() {
        // 老式 .doc 这类：开头夹着 NUL，从前会解成乱码进库
        assert!(unreadable(
            "old.doc",
            &[0xD0, 0xCF, 0x11, 0xE0, 0, 0, 0, 0, b'x']
        ));
        assert!(unreadable(
            "clip.mp4",
            &[0, 0, 0, 0x18, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2']
        ));
        assert!(unreadable("empty.txt", b"   \n\n  "));
        // 真正的文本照读，UTF-16 带字节序标记的不算二进制
        assert!(parse("a.txt", "你好，世界".as_bytes()).is_ok());
        assert!(!looks_binary(&[0xFF, 0xFE, b'h', 0, b'i', 0]));
        // PDF 文本层抄来的正文夹着 NUL 但是合法 UTF-8：照读（#611）
        assert!(parse("layer.md", "Revenue grew\0 twelve percent.".as_bytes()).is_ok());
    }
}

mod blocks;
mod chunker;
pub mod html;
pub mod mineru;
pub mod ontology_rdf;
mod parsers;
pub mod provenance;
mod reading;
mod table;
pub mod transcript;

pub use chunker::{chunk_segments, chunk_text, chunk_with_budget, ChunkPiece, BUDGET_TOKENS};
/// Decode fetched text with the same encoding detection as file ingestion.
pub use parsers::plain_text as decode_text;
pub use provenance::{Origin, Provenance, Segment};
pub use reading::Reading;

/// 解析产物：纯文本 + 可选结构信息。
#[derive(Debug)]
pub struct ParsedDoc {
    pub text: String,
}

/// 读出字要靠哪一种模型（0040）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    /// 扫描件、图片：版面识别（MinerU 这类服务）
    Ocr,
    /// 录音：带说话人的转写
    Transcribe,
}

impl Reader {
    pub fn as_str(self) -> &'static str {
        match self {
            Reader::Ocr => "ocr",
            Reader::Transcribe => "transcribe",
        }
    }

    /// 报错里说缺的是什么
    fn wanted(self) -> &'static str {
        match self {
            Reader::Ocr => "a document-reading (OCR) service",
            Reader::Transcribe => "a transcription model that labels speakers",
        }
    }
}

/// 这份文件没有可以直接读的文字，要靠 [`Reader`] 那一种模型读。
///
/// **不是解析失败**：文件没坏，只是这一步轮不到文本解析器。从前图片和录音会掉进
/// 「按文本解码」那一支，解出一堆乱码、照样分块、嵌入、抽取；扫描件则报一句笼统的
/// 「没抽出文字」，重试三次。现在它们停在这里，交给配了模型的读取器，没配就降级并告警
#[derive(Debug, Clone, thiserror::Error)]
#[error("This {what} has no text layer; reading it needs {}", reader.wanted())]
pub struct NeedsReader {
    pub reader: Reader,
    /// 给人看的是什么文件：scanned PDF / image / recording
    pub what: &'static str,
}

/// 录音转写回来了，却分不出谁说的（0040 决定 5）。
///
/// 跟没配转写模型一样对待：不读，文档停下、告警说明原因。分不出说话人的会议记录，「张三说
/// 他三季度交付」和「李四说张三三季度交付」是同一行字，承诺会记到错的人头上——比什么都
/// 不抽更糟。换一个会标说话人的模型，存设置时它会重新排队
#[derive(Debug, Clone, thiserror::Error)]
#[error(
    "The transcription model did not say who spoke; a recording is read only with speaker labels"
)]
pub struct NoSpeakers;

/// 这份文件读不了，换什么模型也读不了（视频、可执行文件、老式二进制格式）。重试没用
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct Unreadable(pub String);

const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif",
];
const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "m4a", "flac", "ogg", "oga", "opus", "aac", "amr", "wma",
];
const VIDEO_EXTS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v", "wmv"];

/// 按文件头（认得出来的话）或扩展名判断这是不是要模型读的媒体文件。
///
/// 文件头优先：扩展名可能撒谎。认不出文件头的才看扩展名
fn media_kind(bytes: &[u8], ext: &str) -> Option<Result<NeedsReader, Unreadable>> {
    use infer::MatcherType;
    let by_header = infer::get(bytes).map(|t| t.matcher_type());
    let is = |m: MatcherType, exts: &[&str]| match by_header {
        Some(h) => h == m,
        None => exts.contains(&ext),
    };
    if is(MatcherType::Image, IMAGE_EXTS) {
        return Some(Ok(NeedsReader {
            reader: Reader::Ocr,
            what: "image",
        }));
    }
    if is(MatcherType::Audio, AUDIO_EXTS) {
        return Some(Ok(NeedsReader {
            reader: Reader::Transcribe,
            what: "recording",
        }));
    }
    if is(MatcherType::Video, VIDEO_EXTS) {
        return Some(Err(Unreadable(
            "Video files are not read yet; upload its audio track or a transcript".into(),
        )));
    }
    None
}

/// 按文本解码之前看一眼是不是二进制：开头夹着 NUL、**而且**不是合法的 UTF-8。
///
/// 两条都要：PDF 文本层抄出来的正文会夹 NUL，但它是合法的 UTF-8（#611，照读、剥掉 NUL）；
/// GBK 这类中文编码不是合法的 UTF-8，但不夹 NUL。老式 .doc、压缩包两条都占。
/// 带 UTF-16 字节序标记的不算
fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    let head = &bytes[..bytes.len().min(8192)];
    if !head.contains(&0) {
        return false;
    }
    // 截断可能切在一个多字节字符中间：只看完整的那一段是否合法
    match std::str::from_utf8(head) {
        Ok(_) => false,
        Err(e) => e.error_len().is_some(),
    }
}

/// 支持的格式（P1）：pdf / docx / xlsx·xls·ods / pptx / md / txt / html / csv / json / yaml / xml / log。
///
/// 要模型读的（扫描件、图片、录音）返回挂着 [`NeedsReader`] 的错误；读不了的挂 [`Unreadable`]
pub fn parse(filename: &str, bytes: &[u8]) -> anyhow::Result<ParsedDoc> {
    let ext = filename
        .rsplit('.')
        .next()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match media_kind(bytes, &ext) {
        Some(Ok(needs)) => return Err(needs.into()),
        Some(Err(unreadable)) => return Err(unreadable.into()),
        None => {}
    }

    // 魔数探测优先于扩展名（扩展名可能撒谎）
    let kind = infer::get(bytes).map(|t| t.extension()).unwrap_or("");

    let text = match (kind, ext.as_str()) {
        ("pdf", _) | (_, "pdf") => {
            let text = parsers::pdf(bytes)?;
            // 文本层是空的：扫描件，字在图里
            if text.trim().is_empty() {
                return Err(NeedsReader {
                    reader: Reader::Ocr,
                    what: "scanned PDF",
                }
                .into());
            }
            text
        }
        ("docx", _) | (_, "docx") => parsers::docx(bytes)?,
        ("xlsx", _) | (_, "xlsx") | (_, "xls") | (_, "ods") => parsers::spreadsheet(bytes)?,
        ("pptx", _) | (_, "pptx") => parsers::pptx(bytes)?,
        (_, "html") | (_, "htm") => parsers::html(bytes)?,
        (_, "csv") | (_, "tsv") => parsers::csv_text(bytes, ext == "tsv")?,
        // md/json/yaml/xml/log/txt 及一切未识别格式：按文本解码（编码探测覆盖 GBK 等）。
        // 解码之前先看是不是二进制：老式 .doc、压缩包、可执行文件解出来是乱码，
        // 从前照样分块、嵌入、抽取
        _ if looks_binary(bytes) => {
            return Err(
                Unreadable("This file is not in a format that can be read as text".into()).into(),
            )
        }
        _ => parsers::plain_text(bytes),
    };

    let text = normalize(&text);
    if text.trim().is_empty() {
        return Err(Unreadable("No text could be extracted from this file".into()).into());
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
