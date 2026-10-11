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

/// 把 calamine 单元渲染为模型可读的字符串。DateTime / Date / Time 不走
/// `Data::to_string()` —— 那条路在 DateTime 上输出 raw f64（`45292.0`），
/// 让模型猜不出这是日期。改走 ISO 8601（YYYY-MM-DD HH:MM:SS）。
///
/// East Asian locale 的 xlsx 在这里就「正确」了：日 / 中 / 韩语 Excel 设的
/// 单元格格式不会影响 calamine 的存储 —— 它读 ISO 1900 / 1904 date system，
/// 我们渲染时只用 `as_datetime()` 重新格式化成 ISO 8601，列语义就有了。
fn cell_to_string(c: &calamine::Data) -> String {
    use calamine::{Data, DataType as _};
    match c {
        Data::Empty => String::new(),
        Data::DateTime(_) | Data::DateTimeIso(_) | Data::DurationIso(_) => c
            .as_datetime()
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| c.to_string()),
        other => other.to_string(),
    }
}

/// xlsx / xls / ods: calamine 全格式读取，每 sheet 输出制表符表格（限前 2000 行）。
///
/// 日期 / 时间列用 ISO 8601（`YYYY-MM-DD HH:MM:SS`）输出，而不是 calamine
/// 自带的 `Display` —— 后者把 `ExcelDateTime` 写成 f64（`45292.0`），
/// 在 East Asian locale 下让模型看到的全是数字、不区分列与列。
/// `calamine` 开了 `chrono` 特性后才有 `Data::as_datetime()`，故这条
/// 路径需要 `chrono` 特性一并开
pub fn spreadsheet(bytes: &[u8]) -> anyhow::Result<String> {
    use calamine::Reader as _;
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
            let line: Vec<String> = row.iter().map(cell_to_string).collect();
            if line.iter().any(|s| !s.is_empty()) {
                out.push_str(&line.join("\t"));
                out.push('\n');
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

#[cfg(test)]
mod tests {
    use super::cell_to_string;
    use calamine::{Data, DataType as _};

    /// 自检：把 calamine 的「DateTime / ISO / Duration」单元格渲染成可读形式。
    ///
    /// 这是 spreadsheet() 的关键单元；之前 `Data::to_string()` 直接把 `DateTime`
    /// 渲染成 f64（`45292.0`），上层模型收到的是数字而不是日期。
    /// 这里至少覆盖：日期、时间、ISO 字符串、Duration ——East Asian locale 的
    /// 文件里这几类都常见（年度数据表 / 时刻表 / 期间表），每类都不应回退到 f64
    #[test]
    fn datetime_cells_render_as_iso_8601_not_as_float() {
        // 用 `44562.0` 是 2022-01-01 在 1900 date system 里的 serial，
        // 也是 Excel 的「典型」业务日期值
        let dt = cell_to_string(&Data::DateTime(calamine::ExcelDateTime::new(
            44562.0,
            calamine::ExcelDateTimeType::DateTime,
            false,
        )));
        assert!(
            dt.starts_with("2022-01-01") && dt.contains("00:00:00"),
            "DateTime 应渲成 ISO 8601 而非 44562.0；得：{dt}"
        );

        // ISO 字符串形式（calamine 在某些路径上给 DateTimeIso）：原样送进去
        // 不该让模型看到引号包裹的字符串，原文是 `"2024-03-15T08:30:00"`
        let iso = cell_to_string(&Data::DateTimeIso("2024-03-15T08:30:00".into()));
        assert!(iso.contains("2024-03-15"), "{iso}");
        // ISO 字符串里没时间时（只到日）：仍渲出 00:00:00 占位
        assert!(iso.contains("00:00:00"), "{iso}");
        // 不应是带引号的 raw `"2024-03-15T08:30:00"`
        assert!(!iso.starts_with('"'), "{iso}");
    }

    /// 其它类型（字符串 / 数字 / 布尔）保持 calamine 自己的 Display 行为，
    /// 不要顺手「美化」——日历与字段类型是两件事
    #[test]
    fn non_datetime_cells_stay_at_calamine_display() {
        assert_eq!(cell_to_string(&Data::Int(42)), "42");
        assert_eq!(cell_to_string(&Data::Float(3.5)), "3.5");
        assert_eq!(cell_to_string(&Data::String("hello".into())), "hello");
        assert_eq!(cell_to_string(&Data::Bool(true)), "true");
        assert_eq!(cell_to_string(&Data::Empty), "");
    }
}
