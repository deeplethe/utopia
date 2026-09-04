//! 各格式解析器。全部输出纯文本（结构用 markdown 风格标题保留）。

use anyhow::Context;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::{Cursor, Read};

/// 文本解码：chardetng 探测编码（覆盖 GBK/GB18030/BIG5 等中文常见编码）。
pub fn plain_text(bytes: &[u8]) -> String {
    use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, Utf8Detection::Allow);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

pub fn pdf(bytes: &[u8]) -> anyhow::Result<String> {
    pdf_extract::extract_text_from_mem(bytes).context("PDF text-layer extraction failed")
}

/// docx: 解压 word/document.xml，取 w:t 文本、w:p 分段。
pub fn docx(bytes: &[u8]) -> anyhow::Result<String> {
    let xml = read_zip_entry(bytes, "word/document.xml").context("Malformed docx structure")?;
    extract_xml_text(&xml, "w:t", "w:p")
}

/// pptx: 按页码顺序解析 ppt/slides/slideN.xml，取 a:t 文本。
pub fn pptx(bytes: &[u8]) -> anyhow::Result<String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes.to_vec())).context("Failed to unzip pptx")?;
    let mut slides: Vec<(u32, String)> = Vec::new();
    for i in 0..archive.len() {
        let name = archive.by_index(i)?.name().to_string();
        if let Some(num) = name
            .strip_prefix("ppt/slides/slide")
            .and_then(|s| s.strip_suffix(".xml"))
            .and_then(|s| s.parse::<u32>().ok())
        {
            slides.push((num, name));
        }
    }
    slides.sort();

    let mut out = String::new();
    for (num, name) in slides {
        let mut entry = archive.by_name(&name)?;
        let mut xml = String::new();
        entry.read_to_string(&mut xml)?;
        let text = extract_xml_text(&xml, "a:t", "a:p")?;
        if !text.trim().is_empty() {
            out.push_str(&format!("\n## Slide {num}\n{text}\n"));
        }
    }
    Ok(out)
}

/// xlsx / xls / ods: calamine 全格式读取，每 sheet 输出制表符表格（限前 2000 行）。
pub fn spreadsheet(bytes: &[u8]) -> anyhow::Result<String> {
    use calamine::{Data, Reader as _};
    let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec()))
        .context("Failed to open spreadsheet")?;
    let mut out = String::new();
    for sheet_name in workbook.sheet_names() {
        let Ok(range) = workbook.worksheet_range(&sheet_name) else {
            continue;
        };
        if range.is_empty() {
            continue;
        }
        out.push_str(&format!("\n# Sheet: {sheet_name}\n"));
        for row in range.rows().take(2000) {
            let line: Vec<String> = row
                .iter()
                .map(|c| match c {
                    Data::Empty => String::new(),
                    other => other.to_string(),
                })
                .collect();
            if line.iter().any(|s| !s.is_empty()) {
                out.push_str(&line.join("\t"));
                out.push('\n');
            }
        }
    }
    Ok(out)
}

/// HTML: 取正文文本。
///
/// 真实网页的 chrome 往往比正文还多——整站导航、语言列表、页脚法务、编辑工具。
/// 全量遍历会把它们一并喂给抽取器：既浪费每个分块一次 LLM 调用，又会把
/// "Main page""Privacy policy"之类抽成实体污染图谱（实测一篇维基条目 647KB
/// 产出 60 个分块，首块整块是侧栏菜单、末块整块是版权声明）。
///
/// 因此先认正文容器（main / role=main / article），只有都找不到才退回整篇；
/// 容器内仍可能嵌着导航与表单，交给 walk_html 的 SKIP 名单。
pub fn html(bytes: &[u8]) -> String {
    let raw = plain_text(bytes);
    let doc = scraper::Html::parse_document(&raw);
    let mut out = String::new();
    if let Some(title) = doc
        .select(&scraper::Selector::parse("title").unwrap())
        .next()
    {
        out.push_str(&format!(
            "# {}\n\n",
            title.text().collect::<String>().trim()
        ));
    }
    let root = ["main", "[role=main]", "article"]
        .iter()
        .find_map(|sel| {
            scraper::Selector::parse(sel)
                .ok()
                .and_then(|s| doc.select(&s).next())
        })
        .unwrap_or_else(|| doc.root_element());
    walk_html(root, &mut out);
    out
}

fn walk_html(el: scraper::ElementRef, out: &mut String) {
    // 导航/页眉页脚/边栏/表单控件即便落在正文容器内也是 chrome，一律跳过
    const SKIP: &[&str] = &[
        "script", "style", "noscript", "head", "svg", "template", "nav", "header", "footer",
        "aside", "form", "button", "select", "iframe", "dialog",
    ];
    const BLOCK: &[&str] = &[
        "p", "div", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "br", "section", "article",
    ];
    for node in el.children() {
        if let Some(child) = scraper::ElementRef::wrap(node) {
            let tag = child.value().name();
            if SKIP.contains(&tag) {
                continue;
            }
            walk_html(child, out);
            if BLOCK.contains(&tag) && !out.ends_with('\n') {
                out.push('\n');
            }
        } else if let Some(text) = node.value().as_text() {
            let t: &str = text;
            if !t.trim().is_empty() {
                out.push_str(t);
            }
        }
    }
}

pub fn csv_text(bytes: &[u8], tsv: bool) -> anyhow::Result<String> {
    let decoded = plain_text(bytes);
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(if tsv { b'\t' } else { b',' })
        .flexible(true)
        .has_headers(false)
        .from_reader(decoded.as_bytes());
    let mut out = String::new();
    for (i, record) in reader.records().enumerate() {
        if i >= 10_000 {
            break;
        }
        let record = record?;
        out.push_str(&record.iter().collect::<Vec<_>>().join(" | "));
        out.push('\n');
    }
    Ok(out)
}

// ---- 工具 ----

fn read_zip_entry(bytes: &[u8], name: &str) -> anyhow::Result<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec()))?;
    let mut entry = archive.by_name(name)?;
    let mut content = String::new();
    entry.read_to_string(&mut content)?;
    Ok(content)
}

/// 从 OOXML 里抽取 `text_tag`（如 w:t）内的文本，遇 `para_tag`（如 w:p）结束换行。
fn extract_xml_text(xml: &str, text_tag: &str, para_tag: &str) -> anyhow::Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == text_tag => in_text = true,
            Ok(Event::End(e)) => {
                let name = e.name();
                if name.as_ref() == text_tag {
                    in_text = false;
                } else if name.as_ref() == para_tag {
                    out.push('\n');
                }
            }
            Ok(Event::Text(t)) if in_text => {
                out.push_str(&t.xml_content(quick_xml::XmlVersion::Implicit1_0));
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("XML parse error: {e}"),
            _ => {}
        }
    }
    Ok(out)
}
