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

/// docx：解压 word/document.xml。正文 w:t 取字、w:p 分段；表格（w:tbl）收成网格交给
/// `table::render_grid`，和 HTML 表走同一套渲染：w:gridSpan 跨列，上下合并（w:vMerge）的
/// 续格留空，格内段落的左缩进 w:ind 当内边距（小节行靠它折进标签）。套在格子里的表按格子
/// 文字处理。标题（有大纲级别的段落，见 [`docx_heading_styles`]）写成 Markdown 标题。
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
    // 再看它的样式；两样都只认第一次出现的，w:pPrChange 里记的是修订之前的样式，不算
    let mut paragraphs = 0usize;
    let mut para_start = 0usize;
    let mut own_level: Option<Option<u8>> = None;
    let mut style_level: Option<Option<u8>> = None;
    let mut in_revision = false;
    let attr = |e: &quick_xml::events::BytesStart<'_>, name: &str| -> Option<String> {
        e.attributes()
            .flatten()
            .find(|a| a.key.as_ref() == name)
            .map(|a| a.value.to_string())
    };
    loop {
        match reader.read_event() {
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
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                "w:t" => in_text = false,
                "w:p" => {
                    match cell.as_mut() {
                        Some(c) => c.0.push(' '),
                        None => {
                            // 标题写成一行 Markdown 标题：分块器靠它给每块开头补上所在的各级标题，
                            // 时间解释靠块里的标题行分节（0064 决定 1 按 cut 2 修订的那段）。没有
                            // 它，一份 Word 里各节的日期都算成第一节的
                            let level = own_level.unwrap_or(style_level.flatten());
                            if let Some(level) = level.filter(|_| paragraphs == 1) {
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
                            out.push('\n');
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

/// PPTX: extract a:t text in the presentation's logical slide order.
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
        let text = extract_xml_text(&xml, "a:t", "a:p", "a:br")?;
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
                            Data::DateTime(d) => excel_date(d),
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

fn read_zip_entry(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> anyhow::Result<String> {
    let mut entry = archive.by_name(name)?;
    let mut content = String::new();
    entry.read_to_string(&mut content)?;
    Ok(content)
}

/// 从 OOXML 里抽取 `text_tag`（如 a:t）内的文本，遇 `para_tag`（如 a:p）结束换行，
/// 遇 `break_tag`（如 a:br）也换行。
fn extract_xml_text(
    xml: &str,
    text_tag: &str,
    para_tag: &str,
    break_tag: &str,
) -> anyhow::Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == text_tag => in_text = true,
            // 段落里换的行（Shift+Enter）不是新段落，是两个 run 之间的一个 `<a:br>`。丢了它，
            // 标题「Q1」换行「2024」读成「Q12024」，季度和年份一起没了（Word 格子里的同一件事
            // 见 #813）
            Ok(Event::Start(e) | Event::Empty(e)) if e.name().as_ref() == break_tag => {
                out.push('\n');
            }
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
            Ok(Event::CData(t)) if in_text => {
                out.push_str(&t.xml_content(quick_xml::XmlVersion::Implicit1_0));
            }
            Ok(Event::GeneralRef(e)) if in_text => {
                let reference = format!("&{};", e.into_inner());
                match quick_xml::escape::unescape(&reference) {
                    Ok(s) => out.push_str(&s),
                    Err(_) => out.push_str(&reference),
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
