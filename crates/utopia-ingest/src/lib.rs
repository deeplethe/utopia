//! utopia-ingest: 解析矩阵 + 分块。
//! 原则：文本层 Rust 原生解决（快、零依赖）；扫描件/复杂版式后续走 docling sidecar。

mod chunker;
pub mod ontology_rdf;
mod parsers;

pub use chunker::{chunk_text, ChunkPiece};

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
        (_, "html") | (_, "htm") => parsers::html(bytes),
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
