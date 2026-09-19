//! 各格式解析器。全部输出纯文本（结构用 markdown 风格标题保留）。

use anyhow::Context;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::{Cursor, Read, Write};
use std::process::Command;

/// 文本解码：chardetng 探测编码（覆盖 GBK/GB18030/BIG5 等中文常见编码）。
pub fn plain_text(bytes: &[u8]) -> String {
    use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, Utf8Detection::Allow);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

/// 这份 PDF 的内容流里有没有画出字形：任何一个画字的算子（`Tj`、`TJ`、`'`、`"`）
/// 带着非空的串就算。
///
/// **这是「扫描件」与「我们读不了」之间唯一靠得住的分界。** 两种情况下 `pdftotext` 都是
/// 空输出、退出码 0：一张扫描的图本来就没有字可取，而一份用 `GBK-EUC-H` 这类预定义 CMap
/// 的文件是取字的人不认那张表（#739）。读不出结构就算作没有——宁可让一份文件多走一趟 OCR，
/// 也不要把它挡在库外
fn draws_text(bytes: &[u8]) -> bool {
    let Ok(doc) = lopdf::Document::load_mem(bytes) else {
        return false;
    };
    let drawn = |object: &lopdf::Object| match object {
        lopdf::Object::String(s, _) => !s.is_empty(),
        lopdf::Object::Array(items) => items
            .iter()
            .any(|i| matches!(i, lopdf::Object::String(s, _) if !s.is_empty())),
        _ => false,
    };
    let pages: Vec<lopdf::ObjectId> = doc.page_iter().collect();
    pages.into_iter().any(|id| {
        doc.get_page_content(id)
            .ok()
            .and_then(|data| lopdf::content::Content::decode(&data).ok())
            .is_some_and(|content| {
                content.operations.iter().any(|op| {
                    matches!(op.operator.as_str(), "Tj" | "TJ" | "'" | "\"")
                        && op.operands.iter().any(drawn)
                })
            })
    })
}

/// PDF 的文字层。取不出来时交给外面的 `pdftotext`——它是另一个进程，所以那边再崩也带不走
/// 一个工作线程。
///
/// 回退也空手而归时，[`draws_text`] 决定该说哪句话：这份文件没画过字，那是扫描件，交给
/// OCR 那条路（`NeedsReader`）；画了字却一个都没取到，那是我们读不了它，带着第一个解析器
/// 的原话往外抛。报错的那句话值得较真：说成「没有文字层」会把人支去配一个 OCR 服务，而
/// 这份文件的文字层好端端地在那儿（#739：同名的 `pdftotext` 有两个实现，Xpdf 那个和缺了
/// CJK CMap 数据的 Poppler 都会静静地返回空）
pub fn pdf(bytes: &[u8]) -> anyhow::Result<String> {
    let extracted = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes));
    let original_error = match extracted {
        Ok(Ok(text)) if !text.trim().is_empty() => return Ok(text),
        Ok(Ok(_)) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(panic) => Some(
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    panic
                        .downcast_ref::<&str>()
                        .map(|message| (*message).to_owned())
                })
                .unwrap_or_else(|| "PDF parser panicked".to_owned()),
        ),
    };

    let fallback = pdf_with_poppler(bytes);
    if let Ok(text) = &fallback {
        if !text.trim().is_empty() {
            return Ok(text.clone());
        }
    }
    // The first extractor read the file and found no text layer: the OCR path takes it from here.
    let Some(original) = original_error else {
        return Ok(String::new());
    };
    // It could not read the file. A file that draws no glyphs is a scan even so.
    if !draws_text(bytes) {
        return Ok(String::new());
    }
    match fallback {
        Ok(_) => anyhow::bail!(
            "PDF text-layer extraction failed: {original}; the fallback read no text from a file \
that draws it (is pdftotext Poppler, with its CJK CMap data?)"
        ),
        Err(error) => anyhow::bail!(
            "PDF text-layer extraction failed: {original}; Poppler fallback failed: {error:#}"
        ),
    }
}

fn pdf_with_poppler(bytes: &[u8]) -> anyhow::Result<String> {
    let mut input = tempfile::NamedTempFile::new().context("Could not create a temporary PDF")?;
    input
        .write_all(bytes)
        .context("Could not write the temporary PDF")?;
    input.flush().context("Could not flush the temporary PDF")?;

    let output = Command::new("pdftotext")
        .args(["-enc", "UTF-8", "-nopgbrk"])
        .arg(input.path())
        .arg("-")
        .output()
        .context("Could not run pdftotext")?;
    if !output.status.success() {
        anyhow::bail!(
            "pdftotext exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("pdftotext returned invalid UTF-8")
}

/// docx：解压 word/document.xml。正文 w:t 取字、w:p 分段；表格（w:tbl）收成网格交给
/// `table::render_grid`，和 HTML 表走同一套渲染：w:gridSpan 跨列，上下合并（w:vMerge）的
/// 续格留空，格内段落的左缩进 w:ind 当内边距（小节行靠它折进标签）。套在格子里的表按格子
/// 文字处理。
pub fn docx(bytes: &[u8]) -> anyhow::Result<String> {
    let xml = read_zip_entry(bytes, "word/document.xml").context("Malformed docx structure")?;
    docx_xml_to_text(&xml)
}

type GridRow = Vec<(String, usize, u32)>;

pub(crate) fn docx_xml_to_text(xml: &str) -> anyhow::Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    // 套了几层 w:tbl；只有最外层收成网格
    let mut depth = 0usize;
    let mut rows: Vec<GridRow> = Vec::new();
    let mut row: GridRow = Vec::new();
    let mut cell: Option<(String, usize, u32)> = None;
    let attr = |e: &quick_xml::events::BytesStart<'_>, name: &str| -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == name)
            .map(|a| a.value.to_string())
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "w:t" => in_text = true,
                "w:tbl" => {
                    depth += 1;
                    if depth == 1 {
                        rows.clear();
                    }
                }
                "w:tr" if depth == 1 => row = Vec::new(),
                "w:tc" if depth == 1 => cell = Some((String::new(), 1, 0)),
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "w:gridSpan" => {
                    if let Some(c) = cell.as_mut() {
                        if let Some(v) = attr(&e, "w:val").and_then(|v| v.parse::<usize>().ok()) {
                            c.1 = v.max(1);
                        }
                    }
                }
                // 格子里第一段的左缩进（twips，1/20 pt）
                "w:ind" => {
                    if let Some(c) = cell.as_mut().filter(|c| c.0.is_empty()) {
                        if let Some(v) = attr(&e, "w:left")
                            .or_else(|| attr(&e, "w:start"))
                            .and_then(|v| v.parse::<u32>().ok())
                        {
                            c.2 = v / 20;
                        }
                    }
                }
                "w:tab" => match cell.as_mut() {
                    Some(c) => c.0.push(' '),
                    None => out.push(' '),
                },
                "w:br" | "w:cr" if cell.is_none() => out.push('\n'),
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                "w:t" => in_text = false,
                "w:p" => match cell.as_mut() {
                    Some(c) => c.0.push(' '),
                    None => out.push('\n'),
                },
                "w:tc" if depth == 1 => {
                    if let Some(c) = cell.take() {
                        row.push(c);
                    }
                }
                "w:tr" if depth == 1 => rows.push(std::mem::take(&mut row)),
                "w:tbl" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        if let Some(md) = crate::table::render_grid(&rows, false) {
                            out.push('\n');
                            out.push_str(&md);
                            out.push_str("\n\n");
                        }
                        rows.clear();
                    }
                }
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => {
                let s = t.xml_content(quick_xml::XmlVersion::Implicit1_0);
                match cell.as_mut() {
                    Some(c) => c.0.push_str(&s),
                    None => out.push_str(&s),
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("XML parse error: {e}"),
            _ => {}
        }
    }
    Ok(out)
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
        out.push_str(&format!("\n# Sheet: {sheet_name}\n\n"));
        // 每张表按网格渲染成带列头的 Markdown 表；一格字都没有的表退回制表符分隔
        let rows: Vec<GridRow> = range
            .rows()
            .take(2000)
            .map(|row| {
                row.iter()
                    .map(|c| {
                        let text = match c {
                            Data::Empty => String::new(),
                            other => other.to_string(),
                        };
                        (text, 1, 0)
                    })
                    .collect::<GridRow>()
            })
            .filter(|line| line.iter().any(|(s, _, _)| !s.is_empty()))
            .collect();
        match crate::table::render_grid(&rows, true) {
            Some(md) => {
                out.push_str(&md);
                out.push('\n');
            }
            None => {
                for line in &rows {
                    let cells: Vec<&str> = line.iter().map(|(s, _, _)| s.as_str()).collect();
                    out.push_str(&cells.join("\t"));
                    out.push('\n');
                }
            }
        }
    }
    Ok(out)
}

/// Decode before conversion so legacy HTML encodings remain supported.
pub fn html(bytes: &[u8]) -> anyhow::Result<String> {
    Ok(crate::html::page_to_markdown(&plain_text(bytes), None)?)
}

pub fn csv_text(bytes: &[u8], tsv: bool) -> anyhow::Result<String> {
    let decoded = plain_text(bytes);
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(if tsv { b'\t' } else { b',' })
        .flexible(true)
        .has_headers(false)
        .from_reader(decoded.as_bytes());
    let mut rows: Vec<GridRow> = Vec::new();
    for (i, record) in reader.records().enumerate() {
        if i >= 10_000 {
            break;
        }
        let record = record?;
        rows.push(record.iter().map(|s| (s.to_string(), 1, 0)).collect());
    }
    // 第一条记录是列头（csv 的惯例）；渲染不出表时退回竖线分隔的行
    Ok(match crate::table::render_grid(&rows, true) {
        Some(md) => md + "\n",
        None => {
            rows.iter()
                .map(|r| {
                    r.iter()
                        .map(|(s, _, _)| s.as_str())
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        }
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0"?><w:document xmlns:w="x"><w:body>
<w:p><w:r><w:t>Segment results</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>Segment</w:t></w:r></w:p></w:tc><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>Revenue</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>Cloud</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>$</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>1,200</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>Devices</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>$</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>300</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:t>After the table.</w:t></w:r></w:p></w:body></w:document>"#;

    /// docx 的表：跨列的列头按列拼，"$" 并回数字，表前后的段落照旧
    #[test]
    fn a_docx_table_is_rendered_under_its_headings() {
        let text = docx_xml_to_text(DOC).unwrap();
        assert!(text.starts_with("Segment results\n"), "{text}");
        assert!(
            text.contains(
                "| Segment | Revenue |\n| --- | --- |\n| Cloud | $1,200 |\n| Devices | $300 |"
            ),
            "{text}"
        );
        assert!(text.ends_with("After the table.\n"), "{text}");
    }

    /// 套在格子里的表不单独成表：它的字算外层格子的字
    #[test]
    fn a_nested_docx_table_is_its_cells_text() {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:tbl>
<w:tr><w:tc><w:p><w:r><w:t>Region</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Notes</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>North</w:t></w:r></w:p></w:tc><w:tc><w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>7</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:tc></w:tr>
</w:tbl></w:body></w:document>"#;
        let text = docx_xml_to_text(xml).unwrap();
        assert_eq!(text.matches("| --- |").count(), 1, "{text}");
        assert!(text.contains("| North | inner 7 |"), "{text}");
    }

    /// csv 的第一条记录是列头，哪怕列头是年份
    #[test]
    fn a_csv_is_a_table_whose_first_record_is_the_header() {
        let text = csv_text(b"Item,2025,2024\nRevenue,10,8\nCost,4,3\n", false).unwrap();
        assert_eq!(
            text,
            "| Item | 2025 | 2024 |\n| --- | --- | --- |\n| Revenue | 10 | 8 |\n| Cost | 4 | 3 |\n"
        );
    }

    /// 列头之后只填了一个序号的行也是记录，不是列头的第二行
    #[test]
    fn a_lone_number_row_after_the_header_is_a_record() {
        let text = csv_text(
            b"no,name,score
1,,
2,Ada,9
",
            false,
        )
        .unwrap();
        assert!(text.starts_with("| no | name | score |"), "{text}");
        assert!(text.contains("| 1 |  |  |"), "{text}");
        assert!(text.contains("| 2 | Ada | 9 |"), "{text}");
    }

    /// 一格数字都没有的 csv 也是表：标签列是最左那格
    #[test]
    fn a_csv_of_words_is_still_a_table() {
        let text = csv_text(b"name,role\nAda,engineer\nGrace,admiral\n", false).unwrap();
        assert!(
            text.starts_with("| name | role |\n| --- | --- |\n| Ada | engineer |"),
            "{text}"
        );
    }
}
