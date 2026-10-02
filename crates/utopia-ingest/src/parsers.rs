//! 各格式解析器。全部输出纯文本（结构用 markdown 风格标题保留）。

use anyhow::Context;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::{Cursor, Read, Write};
use std::process::Command;

const SPREADSHEET_ROW_LIMIT: usize = 2_000;
const CSV_RECORD_LIMIT: usize = 10_000;

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

/// PDF 的文字层。字体的 ToUnicode 常把字形指到长得一样的部首、连字码位，`pdf_extract`
/// 读出来的照收：换回它们显示的那个字（[`crate::lookalikes`]）
pub fn pdf(bytes: &[u8]) -> anyhow::Result<String> {
    let text = text_layer(bytes)?;
    Ok(match crate::lookalikes::plain(&text) {
        std::borrow::Cow::Borrowed(_) => text,
        std::borrow::Cow::Owned(plain) => plain,
    })
}

/// 取文字层。取不出来时交给外面的 `pdftotext`——它是另一个进程，所以那边再崩也带不走
/// 一个工作线程。
///
/// 回退也空手而归时，[`draws_text`] 决定该说哪句话：这份文件没画过字，那是扫描件，交给
/// OCR 那条路（`NeedsReader`）；画了字却一个都没取到，那是我们读不了它，带着第一个解析器
/// 的原话往外抛。报错的那句话值得较真：说成「没有文字层」会把人支去配一个 OCR 服务，而
/// 这份文件的文字层好端端地在那儿（#739：同名的 `pdftotext` 有两个实现，Xpdf 那个和缺了
/// CJK CMap 数据的 Poppler 都会静静地返回空）
fn text_layer(bytes: &[u8]) -> anyhow::Result<String> {
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

/// docx：解压 word/document.xml。正文 w:t 取字；不在表格里的 w:p 是一段，段末留空行；
/// 段内的 w:br/w:cr 仍是单换行。表格（w:tbl）收成网格交给 `table::grid_or_lines`，和 HTML 表
/// 走同一套渲染（不成表的网格按行写出字来）：w:gridSpan 跨列，上下合并（w:vMerge）的续格留空，格内段落的左缩进 w:ind
/// 当内边距（小节行靠它折进标签）。套在格子里的表按格子文字处理。标题（有大纲级别的段落，
/// 见 [`docx_heading_styles`]）写成 Markdown 标题。
pub fn docx(bytes: &[u8]) -> anyhow::Result<String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("Malformed docx structure")?;
    let xml =
        read_zip_entry(&mut archive, "word/document.xml").context("Malformed docx structure")?;
    // 样式表缺了或读不了，正文照读，只是认不出靠样式定的标题
    let headings = read_zip_entry(&mut archive, "word/styles.xml")
        .ok()
        .and_then(|styles| docx_heading_styles(&styles).ok())
        .unwrap_or_default();
    docx_xml_to_text(&xml, &headings)
}

/// 哪些段落样式是标题、第几级（样式 id → 1–9）。
///
/// Word 的标题就是有大纲级别的段落：导航窗格和目录都按它列，样式定义里写 `w:outlineLvl`
/// （从 0 数，9 是正文）。样式 id 靠不住——英文 Word 写 `Heading1`，中文 Word 写 `1`、`2`，
/// 正文样式是 `a`——内置样式的名字倒是一律写英文的 `heading 1`。所以按这个次序认：样式
/// 自己的 `w:outlineLvl`，其次名字是 `heading N`，再次顺着 `w:basedOn` 往上找（自定义的
/// 标题样式多半基于内置标题）。`Title`、`toc 1` 没有大纲级别，不算标题
pub(crate) fn docx_heading_styles(
    xml: &str,
) -> anyhow::Result<std::collections::HashMap<String, u8>> {
    #[derive(Default)]
    struct Style {
        name: Option<String>,
        based_on: Option<String>,
        outline: Option<u8>,
    }
    let attr = |e: &quick_xml::events::BytesStart<'_>, name: &str| -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == name)
            .map(|a| a.value.to_string())
    };
    let mut reader = Reader::from_str(xml);
    let mut styles: std::collections::HashMap<String, Style> = std::collections::HashMap::new();
    let mut current: Option<(String, Style)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == "w:style" => {
                current = (attr(&e, "w:type").as_deref() == Some("paragraph"))
                    .then(|| attr(&e, "w:styleId"))
                    .flatten()
                    .map(|id| (id, Style::default()));
            }
            Ok(Event::End(e)) if e.name().as_ref() == "w:style" => {
                if let Some((id, style)) = current.take() {
                    styles.insert(id, style);
                }
            }
            Ok(Event::Start(e) | Event::Empty(e)) => {
                if let Some((_, style)) = current.as_mut() {
                    match e.name().as_ref() {
                        "w:name" => style.name = attr(&e, "w:val"),
                        "w:basedOn" => style.based_on = attr(&e, "w:val"),
                        "w:outlineLvl" => {
                            style.outline = attr(&e, "w:val").and_then(|v| v.parse().ok());
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => anyhow::bail!("XML parse error: {e}"),
            _ => {}
        }
    }
    let level_of = |id: &str| -> Option<u8> {
        let mut id = id;
        // basedOn 最多往上走十步：Word 自己不会链这么长，写坏了成环的也就此打住
        for _ in 0..10 {
            let style = styles.get(id)?;
            if let Some(outline) = style.outline {
                return outline_level(outline);
            }
            if let Some(n) = style.name.as_deref().and_then(heading_number) {
                return Some(n);
            }
            id = style.based_on.as_deref()?;
        }
        None
    };
    Ok(styles
        .keys()
        .filter_map(|id| Some((id.clone(), level_of(id)?)))
        .collect())
}

/// 大纲级别 0–8 是第 1–9 级标题，9 是正文
fn outline_level(outline: u8) -> Option<u8> {
    (outline <= 8).then_some(outline + 1)
}

/// 内置标题样式的名字 `heading 1` … `heading 9`，大小写不论
fn heading_number(name: &str) -> Option<u8> {
    let name = name.trim().to_ascii_lowercase();
    let n: u8 = name.strip_prefix("heading")?.trim().parse().ok()?;
    (1..=9).contains(&n).then_some(n)
}

type GridRow = Vec<(String, usize, u32)>;

pub(crate) fn docx_xml_to_text(
    xml: &str,
    headings: &std::collections::HashMap<String, u8>,
) -> anyhow::Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    // 套了几层 w:tbl；只有最外层收成网格
    let mut depth = 0usize;
    let mut rows: Vec<GridRow> = Vec::new();
    let mut row: GridRow = Vec::new();
    let mut cell: Option<(String, usize, u32)> = None;
    // 正文里的一段（不在表格里、不是文本框里套着的段）是不是标题。`paragraphs` 数套了几层
    // w:p，`para_start` 是这一段在 out 里开始的位置。级别先看段落自己写的 w:outlineLvl，
    // 再看它的样式；两样都只认第一次出现的，w:pPrChange 里记的是修订之前的样式，不算。
    // 另一个栈记每段开始的位置，用来在段末把空段丢掉、非空段之间只留一个空行。文本框会
    // 在宿主段里再套 w:p，栈让内层段落也能各自收尾，而标题判定仍只看最外层。
    let mut paragraphs = 0usize;
    let mut para_start = 0usize;
    let mut paragraph_starts: Vec<usize> = Vec::new();
    let mut own_level: Option<Option<u8>> = None;
    let mut style_level: Option<Option<u8>> = None;
    let mut in_revision = false;
    let attr = |e: &quick_xml::events::BytesStart<'_>, name: &str| -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == name)
            .map(|a| a.value.to_string())
    };
    // 每个 mc:AlternateContent 是否已经读过一种画法；读过之后，其余画法整棵跳过，
    // `skipping` 是跳过的那棵子树还有几层没收口
    let mut alternates: Vec<bool> = Vec::new();
    let mut skipping = 0usize;
    loop {
        let event = reader.read_event();
        if skipping > 0 {
            match event {
                Ok(Event::Start(_)) => skipping += 1,
                Ok(Event::End(_)) => skipping -= 1,
                Ok(Event::Eof) => break,
                Err(e) => anyhow::bail!("XML parse error: {e}"),
                _ => {}
            }
            continue;
        }
        match event {
            // Word 2010 起一个文本框写两遍：`mc:Choice` 里是 wps 形状，`mc:Fallback` 里是给
            // 旧版 Word 的 VML，框里的字各有一份。两份都读，同一段字就进来两遍，抽取也各算
            // 一遍。照标记兼容（markup compatibility）的规矩只取一种画法：第一个 Choice，
            // 其后的 Choice 和 Fallback 不读
            Ok(Event::Start(e)) if e.name().as_ref() == "mc:AlternateContent" => {
                alternates.push(false);
            }
            Ok(Event::End(e)) if e.name().as_ref() == "mc:AlternateContent" => {
                alternates.pop();
            }
            Ok(Event::Start(e)) if matches!(e.name().as_ref(), "mc:Choice" | "mc:Fallback") => {
                match alternates.last_mut() {
                    Some(taken) if *taken => skipping = 1,
                    Some(taken) => *taken = true,
                    None => {}
                }
            }
            Ok(Event::Start(e) | Event::Empty(e))
                if matches!(e.name().as_ref(), "w:pStyle" | "w:outlineLvl") =>
            {
                if paragraphs == 1 && cell.is_none() && !in_revision {
                    if e.name().as_ref() == "w:pStyle" {
                        style_level.get_or_insert_with(|| {
                            attr(&e, "w:val").and_then(|id| headings.get(&id).copied())
                        });
                    } else {
                        own_level.get_or_insert_with(|| {
                            attr(&e, "w:val")
                                .and_then(|v| v.parse().ok())
                                .and_then(outline_level)
                        });
                    }
                }
            }
            Ok(Event::Start(e)) if e.name().as_ref() == "w:pPrChange" => in_revision = true,
            Ok(Event::End(e)) if e.name().as_ref() == "w:pPrChange" => in_revision = false,
            Ok(Event::Start(e) | Event::Empty(e))
                if matches!(e.name().as_ref(), "w:br" | "w:cr") =>
            {
                // Cells are flattened later, but an explicit break still separates
                // text: dropping it turns two readings such as 10 and 20 into 1020.
                match cell.as_mut() {
                    Some(c) => c.0.push(' '),
                    None => out.push('\n'),
                }
            }
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "w:t" => in_text = true,
                "w:p" => {
                    paragraphs += 1;
                    if cell.is_none() {
                        paragraph_starts.push(out.len());
                    }
                    if paragraphs == 1 && cell.is_none() {
                        para_start = out.len();
                        own_level = None;
                        style_level = None;
                    }
                }
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
                // Word 的不断行连字符（Ctrl+Shift+-）不是 w:t 里的字，是一个元素：页面上照样画出
                // 连字符，只是不在这里折行。丢了它，2024‑01‑15 读成 20240115，010‑62345678 读成
                // 01062345678。写成普通连字符，日期和号码才认得出来
                "w:noBreakHyphen" => match cell.as_mut() {
                    Some(c) => c.0.push('-'),
                    None => out.push('-'),
                },
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                "w:t" => in_text = false,
                "w:p" => {
                    match cell.as_mut() {
                        Some(c) => c.0.push(' '),
                        None => {
                            let start = paragraph_starts.pop().unwrap_or(out.len());
                            // 标题写成一行 Markdown 标题：分块器靠它给每块开头补上所在的各级标题，
                            // 时间解释靠块里的标题行分节（0064 决定 1 按 cut 2 修订的那段）。没有
                            // 它，一份 Word 里各节的日期都算成第一节的
                            let level = own_level.unwrap_or(style_level.flatten());
                            if let Some(level) = level.filter(|_| paragraph_starts.is_empty()) {
                                let title = out[para_start..]
                                    .split_whitespace()
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                if !title.is_empty() {
                                    out.truncate(para_start);
                                    out.push_str(&"#".repeat(usize::from(level.min(6))));
                                    out.push(' ');
                                    out.push_str(&title);
                                }
                            }
                            // 一个 Word 段落是一个 Markdown 段。收尾时先去掉段内末尾的空白，
                            // 空段不留痕；非空段只补一个空行，w:br 产生的段内单换行不受影响。
                            let end = start + out[start..].trim_end().len();
                            if end == start {
                                out.truncate(start);
                            } else {
                                out.truncate(end);
                                out.push_str("\n\n");
                            }
                        }
                    }
                    paragraphs = paragraphs.saturating_sub(1);
                }
                "w:tc" if depth == 1 => {
                    if let Some(c) = cell.take() {
                        row.push(c);
                    }
                }
                "w:tr" if depth == 1 => rows.push(std::mem::take(&mut row)),
                "w:tbl" => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        if let Some(md) = crate::table::grid_or_lines(&rows, false) {
                            if !out.is_empty() {
                                let end = out.trim_end().len();
                                out.truncate(end);
                                out.push_str("\n\n");
                            }
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
            Ok(Event::CData(t)) if in_text => {
                let s = t.xml_content(quick_xml::XmlVersion::Implicit1_0);
                match cell.as_mut() {
                    Some(c) => c.0.push_str(&s),
                    None => out.push_str(&s),
                }
            }
            // 字符引用是独立事件，丢掉它会把 R&amp;D 这样的正文变成 RD。
            // 未识别的引用保留原样，不能因此让整篇导入失败。
            Ok(Event::GeneralRef(e)) if in_text => {
                let reference = format!("&{};", e.into_inner());
                let s = quick_xml::escape::unescape(&reference)
                    .unwrap_or(std::borrow::Cow::Borrowed(&reference));
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

/// PPTX: extract a:t text in the presentation's logical slide order. Tables under a:tbl
/// become Markdown grids through the same renderer as DOCX and spreadsheet tables.
pub fn pptx(bytes: &[u8]) -> anyhow::Result<String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes.to_vec())).context("Failed to unzip pptx")?;
    let slides = if let Some(names) = pptx_order(&mut archive)? {
        names
            .into_iter()
            .enumerate()
            .map(|(i, name)| (i as u32 + 1, name))
            .collect()
    } else {
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

        slides
    };

    let mut out = String::new();
    for (num, name) in slides {
        let xml = pptx_part(&mut archive, &name)?;
        let text = pptx_xml_to_text(&xml)?;
        if !text.trim().is_empty() {
            out.push_str(&format!("\n## Slide {num}\n{text}\n"));
        }
    }
    Ok(out)
}

const PPT_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PACKAGE_REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

fn pptx_part(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> anyhow::Result<String> {
    let mut xml = String::new();
    archive
        .by_name(name)
        .with_context(|| format!("Missing PPTX part: {name}"))?
        .read_to_string(&mut xml)?;
    Ok(xml)
}

// Relationship targets are package URIs, never paths to open or URLs to fetch.
fn pptx_target(source: &str, target: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        !target.contains([':', '\\', '?', '#']) && !target.starts_with("//"),
        "Invalid PPTX part target"
    );
    let target = percent_encoding::percent_decode_str(target).decode_utf8()?;
    anyhow::ensure!(
        !target.contains([':', '\\', '?', '#', '\0']) && !target.starts_with("//"),
        "Invalid PPTX part target"
    );
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        source
            .rsplit_once('/')
            .map(|(dir, _)| dir.split('/').collect())
            .unwrap_or_default()
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                anyhow::ensure!(parts.pop().is_some(), "PPTX target escapes package");
            }
            _ => parts.push(part),
        }
    }
    anyhow::ensure!(!parts.is_empty(), "Empty PPTX part target");
    Ok(parts.join("/"))
}

// Only the requested relationship type is returned. Other types (notes, masters,
// hyperlinks) cannot become slides. Reject ambiguity rather than silently losing pages.
fn pptx_relationships(
    xml: &str,
    kind: &str,
    source: &str,
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    use quick_xml::name::{Namespace, ResolveResult};
    let mut reader = quick_xml::NsReader::from_str(xml);
    let mut depth = 0;
    let mut root = false;
    let mut ids = std::collections::HashSet::new();
    let mut targets = std::collections::HashMap::new();
    loop {
        let event = reader.read_event()?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let (ns, local) = reader.resolver().resolve_element(e.name());
                let package = ns == ResolveResult::Bound(Namespace(PACKAGE_REL_NS));
                if depth == 0 {
                    anyhow::ensure!(
                        !root && package && local.as_ref() == "Relationships",
                        "Invalid PPTX relationships root"
                    );
                    root = true;
                } else if depth == 1 && package && local.as_ref() == "Relationship" {
                    let attrs: std::collections::HashMap<_, _> = e
                        .attributes()
                        .map(|a| {
                            let a = a?;
                            Ok((
                                a.key.as_ref().to_string(),
                                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                                    .into_owned(),
                            ))
                        })
                        .collect::<anyhow::Result<_>>()?;
                    let id = attrs.get("Id").context("PPTX relationship has no Id")?;
                    anyhow::ensure!(ids.insert(id.clone()), "Duplicate PPTX relationship Id");
                    let ty = attrs.get("Type").context("PPTX relationship has no Type")?;
                    if ty == &format!("{REL_NS}/{kind}")
                        || ty
                            == &format!(
                                "http://purl.oclc.org/ooxml/officeDocument/relationships/{kind}"
                            )
                    {
                        anyhow::ensure!(
                            attrs.get("TargetMode").is_none_or(|m| m == "Internal"),
                            "External PPTX {kind} relationship is unsupported"
                        );
                        let target = attrs
                            .get("Target")
                            .context("PPTX relationship has no Target")?;
                        targets.insert(id.clone(), pptx_target(source, target)?);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => depth -= 1,
            Event::Eof => {
                anyhow::ensure!(root && depth == 0, "Incomplete PPTX relationships");
                break;
            }
            _ => {}
        }
    }
    Ok(targets)
}

fn pptx_slide_ids(xml: &str) -> anyhow::Result<Vec<String>> {
    use quick_xml::name::{Namespace, ResolveResult};
    let mut reader = quick_xml::NsReader::from_str(xml);
    let mut depth = 0;
    let mut root = false;
    let mut in_list = false;
    let mut seen_list = false;
    let mut ids = Vec::new();
    loop {
        let event = reader.read_event()?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let (ns, local) = reader.resolver().resolve_element(e.name());
                let presentation = matches!(
                    ns,
                    ResolveResult::Bound(Namespace(
                        PPT_NS | "http://purl.oclc.org/ooxml/presentationml/main"
                    ))
                );
                if depth == 0 {
                    anyhow::ensure!(
                        !root && presentation && local.as_ref() == "presentation",
                        "Invalid PPTX presentation root"
                    );
                    root = true;
                } else if depth == 1 && presentation && local.as_ref() == "sldIdLst" {
                    anyhow::ensure!(!seen_list, "Duplicate PPTX slide list");
                    seen_list = true;
                    in_list = matches!(event, Event::Start(_));
                } else if depth == 2 && in_list {
                    anyhow::ensure!(
                        presentation && local.as_ref() == "sldId",
                        "Invalid PPTX slide-list entry"
                    );
                    let mut id = None;
                    for a in e.attributes() {
                        let a = a?;
                        let (ns, local) = reader.resolver().resolve_attribute(a.key);
                        if local.as_ref() == "id"
                            && matches!(
                                ns,
                                ResolveResult::Bound(Namespace(
                                    REL_NS
                                        | "http://purl.oclc.org/ooxml/officeDocument/relationships"
                                ))
                            )
                        {
                            anyhow::ensure!(id.is_none(), "Duplicate PPTX slide relationship");
                            id = Some(
                                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                                    .into_owned(),
                            );
                        }
                    }
                    ids.push(id.context("PPTX slide has no relationship id")?);
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 1 {
                    in_list = false;
                }
            }
            Event::Eof => {
                anyhow::ensure!(root && depth == 0, "Incomplete PPTX presentation");
                break;
            }
            _ => {}
        }
    }
    Ok(ids)
}

fn pptx_order(
    archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
) -> anyhow::Result<Option<Vec<String>>> {
    let has_root = archive.file_names().any(|n| n == "_rels/.rels");
    let main = if has_root {
        let xml = pptx_part(archive, "_rels/.rels")?;
        let mut roots = pptx_relationships(&xml, "officeDocument", "")?.into_values();
        let main = roots
            .next()
            .context("PPTX has no presentation relationship")?;
        anyhow::ensure!(roots.next().is_none(), "PPTX has multiple presentations");
        main
    } else if archive.file_names().any(|n| n == "ppt/presentation.xml") {
        "ppt/presentation.xml".to_string()
    } else {
        // Preserve the existing behavior for legacy partial packages. If a
        // manifest exists, errors must never fall back to guessed filename order.
        return Ok(None);
    };
    let ids = pptx_slide_ids(&pptx_part(archive, &main)?)?;
    if ids.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let (dir, file) = main.rsplit_once('/').unwrap_or(("", main.as_str()));
    let rels = if dir.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{dir}/_rels/{file}.rels")
    };
    let relationships = pptx_relationships(&pptx_part(archive, &rels)?, "slide", &main)?;
    let slides = ids
        .into_iter()
        .map(|id| {
            relationships
                .get(&id)
                .cloned()
                .with_context(|| format!("Missing PPTX slide relationship: {id}"))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Some(slides))
}

/// xlsx / xls / ods: calamine 全格式读取，每 sheet 输出制表符表格（限前 2000 行）。
pub fn spreadsheet(bytes: &[u8]) -> anyhow::Result<(String, Vec<crate::ParseWarning>)> {
    use calamine::Reader as _;
    let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec()))
        .context("Failed to open spreadsheet")?;
    let mut out = String::new();
    let mut warnings = Vec::new();
    for sheet_name in workbook.sheet_names() {
        let Ok(range) = workbook.worksheet_range(&sheet_name) else {
            continue;
        };
        if range.is_empty() {
            continue;
        }
        // xlsx/xls 把 merge 单独给出；xlsb/ods 的 calamine reader 没有这一步，按无 merge 读。
        let merges = match &mut workbook {
            calamine::Sheets::Xlsx(book) => book
                .merge_cells_by_sheet_name(&sheet_name)
                .unwrap_or_default(),
            calamine::Sheets::Xls(book) => book
                .merge_cells_by_sheet_name(&sheet_name)
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        out.push_str(&format!("\n# Sheet: {sheet_name}\n\n"));
        let rows_total = range.height();
        if rows_total > SPREADSHEET_ROW_LIMIT {
            let rows_read = SPREADSHEET_ROW_LIMIT;
            warnings.push(crate::ParseWarning {
                kind: crate::ParseWarning::SPREADSHEET_ROWS_TRUNCATED,
                detail: serde_json::json!({
                    "sheet": sheet_name.clone(),
                    "rows_read": rows_read,
                    "rows_total": rows_total,
                    "rows_omitted": rows_total - rows_read,
                }),
            });
        }
        // 每张表按网格渲染成带列头的 Markdown 表；一格字都没有的表退回制表符分隔
        let (rows, headers) = spreadsheet_grid(&range, &merges);
        match crate::table::render_grid_with_headers(&rows, headers) {
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
    Ok((out, warnings))
}

#[derive(Clone)]
struct GridMerge {
    start: (usize, usize),
    end: (usize, usize),
    text: String,
}

/// 把 calamine 的矩形 range 和 merge 信息铺成 [`GridRow`]。横向 merge 变成一格的
/// span，纵向 merge 把锚点的值带到覆盖的每一行；anchor 之外的值不再单独成格。
fn spreadsheet_grid(
    range: &calamine::Range<calamine::Data>,
    merges: &[calamine::Dimensions],
) -> (Vec<GridRow>, Option<std::ops::Range<usize>>) {
    let Some((start_row, start_col)) = range.start() else {
        return (Vec::new(), None);
    };
    let height = range.height().min(SPREADSHEET_ROW_LIMIT);
    let width = range.width();
    if height == 0 || width == 0 {
        return (Vec::new(), None);
    }
    let end_row = start_row + height as u32 - 1;
    let end_col = start_col + width as u32 - 1;

    let mut regions = Vec::new();
    for merge in merges {
        if merge.start.0 > merge.end.0
            || merge.start.1 > merge.end.1
            || merge.end.0 < start_row
            || merge.end.1 < start_col
            || merge.start.0 > end_row
            || merge.start.1 > end_col
        {
            continue;
        }
        let start = (
            (merge.start.0.max(start_row) - start_row) as usize,
            (merge.start.1.max(start_col) - start_col) as usize,
        );
        let end = (
            (merge.end.0.min(end_row) - start_row) as usize,
            (merge.end.1.min(end_col) - start_col) as usize,
        );
        regions.push(GridMerge {
            start,
            end,
            text: spreadsheet_cell_text(range.get(start)),
        });
    }

    let mut owner = vec![usize::MAX; height * width];
    for (index, region) in regions.iter().enumerate() {
        for row in region.start.0..=region.end.0 {
            for col in region.start.1..=region.end.1 {
                owner[row * width + col] = index;
            }
        }
    }

    let mut rows: Vec<GridRow> = Vec::with_capacity(height);
    for row in 0..height {
        let mut cells = GridRow::new();
        let mut col = 0usize;
        while col < width {
            let index = owner[row * width + col];
            if index == usize::MAX {
                cells.push((spreadsheet_cell_text(range.get((row, col))), 1, 0));
                col += 1;
                continue;
            }
            let region = &regions[index];
            if col == region.start.1 {
                cells.push((region.text.clone(), region.end.1 - region.start.1 + 1, 0));
                col = region.end.1 + 1;
            } else {
                col += 1;
            }
        }
        rows.push(cells);
    }

    let headers = spreadsheet_headers(&regions, &rows);
    (rows, headers)
}

fn spreadsheet_cell_text(cell: Option<&calamine::Data>) -> String {
    match cell {
        None | Some(calamine::Data::Empty) => String::new(),
        Some(calamine::Data::DateTime(d)) => excel_date(d),
        Some(calamine::Data::Float(f)) => excel_number(*f),
        Some(other) => other.to_string(),
    }
}

/// 表头从第一排至少两格有字的行开始；这一块里纵向 merge 伸到的行也是表头。
fn spreadsheet_headers(regions: &[GridMerge], rows: &[GridRow]) -> Option<std::ops::Range<usize>> {
    let start = rows
        .iter()
        .position(|row| row.iter().filter(|(text, _, _)| !text.is_empty()).count() >= 2)?;
    let mut end = start + 1;
    while let Some(next) = regions
        .iter()
        .filter(|region| region.start.0 >= start && region.start.0 < end && region.end.0 >= end)
        .map(|region| region.end.0 + 1)
        .max()
    {
        end = next;
    }
    Some(start..end.min(rows.len()))
}

/// 日期格按它显示的样子写。
///
/// calamine 按格子挂的数字格式认出了日期，交来的却还是 Excel 存的那个数：从 1899-12-30
/// 起的天数（1904 纪年的工作簿从 1904-01-01 起），小数部分是一天里的时刻。`Data` 的
/// Display 原样写这个数，2024-01-15 就成了 45306——文档里没有这个日期了，时间抽取看不见
/// 它，模型只当它是个量；同一天在 1904 纪年的工作簿里还是另一个数（43844）。
///
/// 写成 ISO：整天只写日期，带时刻的加上时刻，整数部分是 0 的只写时刻（`h:mm` 一类格式），
/// 累计时长（`[h]:mm:ss`）写累计的时分秒。显示用的格式 calamine 不交出来，所以只显示年月
/// 的格子也写到日。出了 Excel 日历的数照原样写
fn excel_date(d: &calamine::ExcelDateTime) -> String {
    let value = d.as_f64();
    if d.is_duration() {
        let seconds = (value.abs() * 86_400.0).round() as u64;
        let sign = if value < 0.0 { "-" } else { "" };
        return format!(
            "{sign}{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        );
    }
    // Excel 的日历止于 9999-12-31（序数 2958465）；负数它自己也只显示 ####
    if !(0.0..2_958_466.0).contains(&value) {
        return value.to_string();
    }
    let (year, month, day, hour, minute, second, _) = d.to_ymd_hms_milli();
    let time = if second == 0 {
        format!("{hour:02}:{minute:02}")
    } else {
        format!("{hour:02}:{minute:02}:{second:02}")
    };
    if value < 1.0 {
        return time;
    }
    let date = format!("{year:04}-{month:02}-{day:02}");
    if (hour, minute, second) == (0, 0, 0) {
        date
    } else {
        format!("{date} {time}")
    }
}

/// 数字格按 Excel 认的精度写：15 位有效数字。
///
/// Excel 比较、显示一个数都只看 15 位有效数字，公式的缓存值却按双精度的 17 位写进文件：
/// `=0.1+0.2` 存成 0.30000000000000004，`=1.1*1.1-1` 存成 0.21000000000000019，Excel 里
/// 看到的是 0.3 和 0.21。原样写出来，正文、引文和抽出来的值都带着这截二进制的尾巴。先舍到
/// 15 位有效数字，再写最短的十进制；本来就干净的数（19.9、1200、45306）一个字不变
fn excel_number(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    format!("{value:.14e}")
        .parse::<f64>()
        .unwrap_or(value)
        .to_string()
}

/// Decode before conversion so legacy HTML encodings remain supported.
pub fn html(bytes: &[u8]) -> anyhow::Result<String> {
    Ok(crate::html::page_to_markdown(&plain_text(bytes), None)?)
}

pub fn csv_text(bytes: &[u8], tsv: bool) -> anyhow::Result<(String, Vec<crate::ParseWarning>)> {
    let decoded = plain_text(bytes);
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(if tsv { b'\t' } else { b',' })
        .flexible(true)
        .has_headers(false)
        .from_reader(decoded.as_bytes());
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut records_total = 0usize;
    for record in reader.records() {
        let record = record?;
        records_total += 1;
        if rows.len() < CSV_RECORD_LIMIT {
            rows.push(record.iter().map(str::to_string).collect());
        }
    }
    let mut warnings = Vec::new();
    if records_total > CSV_RECORD_LIMIT {
        let records_read = rows.len();
        warnings.push(crate::ParseWarning {
            kind: crate::ParseWarning::CSV_RECORDS_TRUNCATED,
            detail: serde_json::json!({
                "records_read": records_read,
                "records_total": records_total,
                "records_omitted": records_total - records_read,
            }),
        });
    }
    // 第一条记录是列头（csv 的惯例）；渲染不出表时退回竖线分隔的行
    let text = match crate::table::render_records(&rows) {
        Some(md) => md + "\n",
        None => {
            rows.iter()
                .map(|r| r.iter().map(String::as_str).collect::<Vec<_>>().join(" | "))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        }
    };
    Ok((text, warnings))
}

// ---- 工具 ----

fn read_zip_entry(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> anyhow::Result<String> {
    let mut entry = archive.by_name(name)?;
    let mut content = String::new();
    entry.read_to_string(&mut content)?;
    Ok(content)
}

/// 从 PPTX 的 slide XML 里读文字。表外的 `a:p`/`a:br` 仍按原来的换行规则；
/// `a:tbl` 里的每个 `a:tr` 收成一行，每个 `a:tc` 收成一格，格内段落用空格连接。
fn pptx_xml_to_text(xml: &str) -> anyhow::Result<String> {
    #[derive(Default)]
    struct TableCell {
        text: String,
        span: usize,
        horizontal_merge: bool,
        vertical_merge: bool,
    }

    fn attr(e: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == name)
            .map(|a| a.value.to_string())
    }

    fn truthy(e: &quick_xml::events::BytesStart<'_>, name: &str) -> bool {
        matches!(attr(e, name).as_deref(), Some("1" | "true" | "on"))
    }

    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    let mut table_depth = 0usize;
    let mut rows: Vec<GridRow> = Vec::new();
    let mut row: GridRow = Vec::new();
    let mut cell: Option<TableCell> = None;
    let mut first_is_header = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "a:tbl" => {
                    table_depth += 1;
                    if table_depth == 1 {
                        rows.clear();
                        row.clear();
                        cell = None;
                        first_is_header = false;
                    }
                }
                "a:tblPr" if table_depth == 1 => {
                    first_is_header = truthy(&e, "firstRow");
                }
                "a:tr" if table_depth == 1 => row.clear(),
                "a:tc" if table_depth == 1 => {
                    cell = Some(TableCell {
                        span: 1,
                        ..TableCell::default()
                    });
                }
                "a:tcPr" if table_depth == 1 => {
                    if let Some(c) = cell.as_mut() {
                        if let Some(span) = attr(&e, "gridSpan").and_then(|v| v.parse().ok()) {
                            c.span = span;
                        }
                        c.horizontal_merge = truthy(&e, "hMerge");
                        c.vertical_merge = truthy(&e, "vMerge");
                    }
                }
                "a:t" => {
                    if cell.is_some() || table_depth == 0 {
                        in_text = true;
                    }
                }
                "a:br" => match cell.as_mut() {
                    Some(c) => c.text.push(' '),
                    None if table_depth == 0 => out.push('\n'),
                    _ => {}
                },
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "a:tblPr" if table_depth == 1 => {
                    first_is_header = truthy(&e, "firstRow");
                }
                "a:tcPr" if table_depth == 1 => {
                    if let Some(c) = cell.as_mut() {
                        if let Some(span) = attr(&e, "gridSpan").and_then(|v| v.parse().ok()) {
                            c.span = span;
                        }
                        c.horizontal_merge = truthy(&e, "hMerge");
                        c.vertical_merge = truthy(&e, "vMerge");
                    }
                }
                "a:br" => match cell.as_mut() {
                    Some(c) => c.text.push(' '),
                    None if table_depth == 0 => out.push('\n'),
                    _ => {}
                },
                "a:tab" => {
                    if let Some(c) = cell.as_mut() {
                        c.text.push(' ');
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                "a:t" => in_text = false,
                "a:p" => match cell.as_mut() {
                    Some(c) => c.text.push(' '),
                    None if table_depth == 0 => out.push('\n'),
                    _ => {}
                },
                "a:tc" if table_depth == 1 => {
                    if let Some(mut c) = cell.take() {
                        if !c.horizontal_merge {
                            if c.vertical_merge {
                                c.text.clear();
                            }
                            row.push((c.text, c.span.max(1), 0));
                        }
                    }
                }
                "a:tr" if table_depth == 1 => rows.push(std::mem::take(&mut row)),
                "a:tbl" => {
                    table_depth = table_depth.saturating_sub(1);
                    if table_depth == 0 {
                        if let Some(md) = crate::table::grid_or_lines(&rows, first_is_header) {
                            out.push('\n');
                            out.push_str(&md);
                            out.push_str("\n\n");
                        }
                        rows.clear();
                        row.clear();
                        cell = None;
                        first_is_header = false;
                    }
                }
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => {
                let text = t.xml_content(quick_xml::XmlVersion::Implicit1_0);
                match cell.as_mut() {
                    Some(c) => c.text.push_str(&text),
                    None => out.push_str(&text),
                }
            }
            Ok(Event::CData(t)) if in_text => {
                let text = t.xml_content(quick_xml::XmlVersion::Implicit1_0);
                match cell.as_mut() {
                    Some(c) => c.text.push_str(&text),
                    None => out.push_str(&text),
                }
            }
            Ok(Event::GeneralRef(e)) if in_text => {
                let reference = format!("&{};", e.into_inner());
                let text = quick_xml::escape::unescape(&reference)
                    .unwrap_or(std::borrow::Cow::Borrowed(&reference));
                match cell.as_mut() {
                    Some(c) => c.text.push_str(&text),
                    None => out.push_str(&text),
                }
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
        let text = docx_xml_to_text(DOC, &Default::default()).unwrap();
        assert!(text.starts_with("Segment results\n\n"), "{text}");
        assert!(
            text.contains(
                "| Segment | Revenue |\n| --- | --- |\n| Cloud | $1,200 |\n| Devices | $300 |"
            ),
            "{text}"
        );
        assert!(text.ends_with("After the table.\n\n"), "{text}");
    }

    /// 套在格子里的表不单独成表：它的字算外层格子的字
    #[test]
    fn a_nested_docx_table_is_its_cells_text() {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:tbl>
<w:tr><w:tc><w:p><w:r><w:t>Region</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Notes</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>North</w:t></w:r></w:p></w:tc><w:tc><w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>7</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:tc></w:tr>
</w:tbl></w:body></w:document>"#;
        let text = docx_xml_to_text(xml, &Default::default()).unwrap();
        assert_eq!(text.matches("| --- |").count(), 1, "{text}");
        assert!(text.contains("| North | inner 7 |"), "{text}");
    }

    /// 标题样式按大纲级别认，其次按内置名字，再顺着 basedOn 找；样式 id 不作数
    #[test]
    fn heading_styles_are_known_by_outline_level_then_name_then_parent() {
        let styles = r#"<w:styles xmlns:w="x">
<w:style w:type="paragraph" w:default="1" w:styleId="a"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/><w:basedOn w:val="a"/><w:pPr><w:keepNext/><w:outlineLvl w:val="0"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="2"><w:name w:val="heading 2"/><w:basedOn w:val="a"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="Heading 3"/></w:style>
<w:style w:type="paragraph" w:styleId="ReportSection"><w:name w:val="Report Section"/><w:basedOn w:val="2"/></w:style>
<w:style w:type="paragraph" w:styleId="Outlined"><w:name w:val="Outlined"/><w:pPr><w:outlineLvl w:val="3"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Demoted"><w:name w:val="Demoted"/><w:basedOn w:val="1"/><w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="a3"><w:name w:val="Title"/><w:basedOn w:val="a"/></w:style>
<w:style w:type="paragraph" w:styleId="TOC1"><w:name w:val="toc 1"/><w:basedOn w:val="a"/></w:style>
<w:style w:type="character" w:styleId="10"><w:name w:val="Heading 1 Char"/><w:basedOn w:val="a0"/></w:style>
<w:style w:type="paragraph" w:styleId="Loop1"><w:name w:val="Loop 1"/><w:basedOn w:val="Loop2"/></w:style>
<w:style w:type="paragraph" w:styleId="Loop2"><w:name w:val="Loop 2"/><w:basedOn w:val="Loop1"/></w:style>
</w:styles>"#;
        let levels = docx_heading_styles(styles).unwrap();
        let mut found: Vec<(&str, u8)> = levels.iter().map(|(id, l)| (id.as_str(), *l)).collect();
        found.sort();
        assert_eq!(
            found,
            [
                ("1", 1),
                ("2", 2),
                ("Heading3", 3),
                ("Outlined", 4),
                ("ReportSection", 2)
            ]
        );
    }

    /// csv 的第一条记录是列头，哪怕列头是年份
    #[test]
    fn a_csv_is_a_table_whose_first_record_is_the_header() {
        let text = csv_text(b"Item,2025,2024\nRevenue,10,8\nCost,4,3\n", false)
            .unwrap()
            .0;
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
        .unwrap()
        .0;
        assert!(text.starts_with("| no | name | score |"), "{text}");
        assert!(text.contains("| 1 |  |  |"), "{text}");
        assert!(text.contains("| 2 | Ada | 9 |"), "{text}");
    }

    /// 一格数字都没有的 csv 也是表，字段仍按原列分开
    #[test]
    fn a_csv_of_words_is_still_a_table() {
        let text = csv_text(b"name,role\nAda,engineer\nGrace,admiral\n", false)
            .unwrap()
            .0;
        assert!(
            text.starts_with("| name | role |\n| --- | --- |\n| Ada | engineer |"),
            "{text}"
        );
    }

    #[test]
    fn a_missing_email_does_not_attach_the_name_to_the_next_record() {
        let text = csv_text(b"name,email\nAlice,\nBob,bob@example.com\n", false)
            .unwrap()
            .0;
        assert_eq!(
            text,
            "| name | email |\n| --- | --- |\n| Alice |  |\n| Bob | bob@example.com |\n"
        );
    }

    #[test]
    fn sparse_delimited_records_keep_their_rows_and_columns() {
        let csv = "name,email,city\nAlice,,\n,,\n,bob@example.com,\n,,Sydney\nCarol,,Melbourne\nDave,dave@example.com,\nEve,,\n,,\n";
        let expected = "| name | email | city |\n| --- | --- | --- |\n| Alice |  |  |\n|  | bob@example.com |  |\n|  |  | Sydney |\n| Carol |  | Melbourne |\n| Dave | dave@example.com |  |\n| Eve |  |  |\n";
        for tsv in [false, true] {
            let input = if tsv {
                csv.replace(',', "\t")
            } else {
                csv.to_string()
            };
            assert_eq!(csv_text(input.as_bytes(), tsv).unwrap().0, expected);
        }
    }

    #[test]
    fn a_single_text_column_keeps_the_header_and_each_record() {
        for tsv in [false, true] {
            assert_eq!(
                csv_text(b"name\nAlice\nBob\n", tsv).unwrap().0,
                "| name |\n| --- |\n| Alice |\n| Bob |\n"
            );
        }
    }

    #[test]
    fn delimited_fields_keep_their_columns_whether_text_or_numbers() {
        for (csv, expected) in [
            (
                "name,role,team\nAda,engineer,platform\nGrace,admiral,navy\n",
                "| name | role | team |\n| --- | --- | --- |\n| Ada | engineer | platform |\n| Grace | admiral | navy |\n",
            ),
            (
                "name,role,score\nAda,engineer,9\nGrace,admiral,8\n",
                "| name | role | score |\n| --- | --- | --- |\n| Ada | engineer | 9 |\n| Grace | admiral | 8 |\n",
            ),
        ] {
            for tsv in [false, true] {
                let input = if tsv {
                    csv.replace(',', "\t")
                } else {
                    csv.to_string()
                };
                assert_eq!(csv_text(input.as_bytes(), tsv).unwrap().0, expected);
            }
        }
    }

    #[test]
    fn a_delimited_header_can_have_empty_columns_and_records_can_vary_in_width() {
        let csv = "name,,email\nAlice\nBob,,bob@example.com,extra\n,,eve@example.com\n";
        let expected = "| name |  | email |  |\n| --- | --- | --- | --- |\n| Alice |  |  |  |\n| Bob |  | bob@example.com | extra |\n|  |  | eve@example.com |  |\n";
        for tsv in [false, true] {
            let input = if tsv {
                csv.replace(',', "\t")
            } else {
                csv.to_string()
            };
            assert_eq!(csv_text(input.as_bytes(), tsv).unwrap().0, expected);
        }
        for (csv, expected) in [
            (
                "name,\nAlice,alice@example.com\n",
                "| name |  |\n| --- | --- |\n| Alice | alice@example.com |\n",
            ),
            (
                ",email\n,alice@example.com\n",
                "|  | email |\n| --- | --- |\n|  | alice@example.com |\n",
            ),
        ] {
            for tsv in [false, true] {
                let input = if tsv {
                    csv.replace(',', "\t")
                } else {
                    csv.to_string()
                };
                assert_eq!(csv_text(input.as_bytes(), tsv).unwrap().0, expected);
            }
        }
    }

    #[test]
    fn symbols_and_quoted_text_stay_in_their_delimited_fields() {
        let expected = "| kind | symbol | value |\n| --- | --- | --- |\n| price, retail | $ | 10 |\n| rate | % | 20 |\n| pipe \\| and line | hello | 30 |\n";
        for (input, tsv) in [
            (
                "kind,symbol,value\n\"price, retail\",$,10\nrate,%,20\n\"pipe | and\nline\",hello,30\n",
                false,
            ),
            (
                "kind\tsymbol\tvalue\n\"price, retail\"\t$\t10\nrate\t%\t20\n\"pipe | and\nline\"\thello\t30\n",
                true,
            ),
        ] {
            assert_eq!(csv_text(input.as_bytes(), tsv).unwrap().0, expected);
        }
    }
}
