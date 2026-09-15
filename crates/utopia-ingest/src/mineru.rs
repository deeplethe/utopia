//! MinerU 读出来的版面（`content_list`）→ 一份正文 + 按页的出处段（0040 第二刀）。
//!
//! MinerU 先认版面、再逐区域认字，交回一串按阅读顺序排好的区域：正文、标题、表、公式、
//! 图注，每个带 `page_idx` 和 `bbox`（映射到 0–1000）。这里把它们拼成分块器认得的
//! Markdown——标题还是标题，表还是表，于是扫描件的块跟原生文档按同一套规矩切：表头
//! 跟着续块走、标题贴着下一段。
//!
//! **段按页分，不按区域分。**一块只装一种出处（决定 2），而每个区域的框都不一样；按区域
//! 分段，一段话一块，块就碎到抽取看不见上下文。页是人核对时翻到的那个单位，所以一页一
//! 段，块切好之后再把它盖住的那几个区域的框并成一个，写进锚点。
//!
//! 页码从 1 数（`page_idx` + 1）：锚点是给人翻页用的，查看器的 `#page=` 也从 1 数。

use crate::chunker::{chunk_segments, ChunkPiece};
use crate::provenance::{Origin, Provenance, Segment};
use serde_json::{json, Value};
use std::ops::Range;

/// 版面上的辅助区域：页眉、页脚、页码。每页重复一遍，进了正文只会让每块都多一行噪声。
/// 页脚注和旁注不在这里——合同的脚注里写着条款
const CHROME: &[&str] = &["header", "footer", "page_number"];

/// 拼好的正文，连同每页的出处和每个区域的位置
#[derive(Debug, Clone)]
pub struct Reading {
    pub text: String,
    pub segments: Vec<Segment>,
    /// 每个区域在正文里的字节区间、所在页、框
    regions: Vec<Region>,
}

#[derive(Debug, Clone)]
struct Region {
    range: Range<usize>,
    page: u64,
    bbox: Option<[f64; 4]>,
}

impl Reading {
    /// 按页分段切块，再给每块的锚点补上它盖住的区域的外框
    pub fn chunk(&self, budget: usize) -> Vec<ChunkPiece> {
        let mut pieces = chunk_segments(&self.text, &self.segments, budget);
        for p in &mut pieces {
            let span = p.char_start as usize..p.char_end as usize;
            let Some(anchor) = p.provenance.anchor.as_mut() else {
                continue;
            };
            let page = anchor["page"].as_u64();
            let covered = self
                .regions
                .iter()
                .filter(|r| {
                    Some(r.page) == page && r.range.start < span.end && span.start < r.range.end
                })
                .filter_map(|r| r.bbox);
            if let Some(b) = covered.reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            }) {
                anchor["bbox"] = json!(b);
            }
        }
        pieces
    }
}

/// 读 `content_list`。`model` 记在出处上：哪个版本、哪个后端读的
pub fn reading(content_list: &Value, model: &str) -> Reading {
    let mut text = String::new();
    let mut regions: Vec<Region> = Vec::new();
    for item in content_list.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let kind = item["type"].as_str().unwrap_or("text");
        if CHROME.contains(&kind) {
            continue;
        }
        let Some(body) = render(kind, item) else {
            continue;
        };
        let body = without_nul(&body);
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        let start = text.len();
        text.push_str(body);
        regions.push(Region {
            range: start..text.len(),
            page: item["page_idx"].as_u64().unwrap_or(0) + 1,
            bbox: bbox(&item["bbox"]),
        });
    }

    // 一页一段，段首尾相接盖满整份正文：段从这一页第一个区域开始，到下一页第一个区域为止
    let mut segments: Vec<Segment> = Vec::new();
    for r in &regions {
        if segments.last().is_some_and(|s| {
            s.provenance.anchor.as_ref().map(|a| &a["page"]) == Some(&json!(r.page))
        }) {
            continue;
        }
        if let Some(prev) = segments.last_mut() {
            prev.range.end = r.range.start;
        }
        segments.push(Segment {
            range: r.range.start..text.len(),
            provenance: Provenance {
                origin: Origin::Ocr,
                model: Some(model.to_string()),
                anchor: Some(json!({ "page": r.page })),
            },
        });
    }
    if let Some(first) = segments.first_mut() {
        first.range.start = 0;
    }
    Reading {
        text,
        segments,
        regions,
    }
}

/// 一个区域写成 Markdown。认不出的类型有字就照字收，没字就跳过
fn render(kind: &str, item: &Value) -> Option<String> {
    let s = |key: &str| item[key].as_str().map(str::trim).filter(|t| !t.is_empty());
    let lines = |key: &str| -> Vec<String> {
        item[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::trim).filter(|t| !t.is_empty()))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let with_notes = |caption: &str, body: Option<String>, footnote: &str| -> Option<String> {
        let parts: Vec<String> = lines(caption)
            .into_iter()
            .chain(body)
            .chain(lines(footnote))
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    };
    match kind {
        "text" => {
            let t = s("text")?;
            match item["text_level"].as_u64().unwrap_or(0) {
                0 => Some(t.to_string()),
                // 标题要单独一行才认得出；认字认出来的换行并成空格
                n => Some(format!(
                    "{} {}",
                    "#".repeat(n.min(6) as usize),
                    t.split_whitespace().collect::<Vec<_>>().join(" ")
                )),
            }
        }
        "table" => {
            let body = s("table_body").map(|html| {
                crate::html::fragment_to_markdown(html)
                    .map(|md| md.trim().to_string())
                    .unwrap_or_else(|_| html.to_string())
            });
            with_notes("table_caption", body, "table_footnote")
        }
        "image" | "chart" => with_notes("image_caption", None, "image_footnote"),
        "code" => with_notes(
            "code_caption",
            s("code_body").map(|c| format!("```\n{c}\n```")),
            "code_footnote",
        ),
        "list" => {
            let items = lines("list_items");
            (!items.is_empty()).then(|| items.join("\n"))
        }
        _ => s("text").map(String::from),
    }
}

fn bbox(v: &Value) -> Option<[f64; 4]> {
    let a = v.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let mut out = [0.0; 4];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x.as_f64()?;
    }
    Some(out)
}

/// 认字认出来的 NUL 跟 PDF 文本层里的一样要剥（#611）
fn without_nul(s: &str) -> String {
    s.replace('\0', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan() -> Value {
        json!([
            { "type": "header", "text": "ACME CONFIDENTIAL", "page_idx": 0, "bbox": [0, 0, 1000, 30] },
            { "type": "text", "text": "Lease Agreement", "text_level": 1, "page_idx": 0, "bbox": [100, 50, 900, 90] },
            { "type": "text", "text": "The tenant is Beta Robotics.", "page_idx": 0, "bbox": [100, 100, 900, 140] },
            { "type": "text", "text": "Rent is paid monthly.", "page_idx": 0, "bbox": [100, 150, 800, 190] },
            { "type": "page_number", "text": "1", "page_idx": 0, "bbox": [480, 970, 520, 990] },
            { "type": "table", "table_caption": ["Schedule of rent"],
              "table_body": "<table><tr><td>Year</td><td>Rent</td></tr><tr><td>2026</td><td>1,000</td></tr></table>",
              "table_footnote": [], "page_idx": 1, "bbox": [100, 60, 900, 300] },
            { "type": "image", "img_path": "images/stamp.png", "image_caption": [], "page_idx": 1, "bbox": [600, 800, 900, 950] },
            { "type": "text", "text": "Signed by both parties.", "page_idx": 1, "bbox": [100, 320, 900, 360] }
        ])
    }

    #[test]
    fn a_scan_reads_as_markdown_without_page_chrome() {
        let r = reading(&scan(), "mineru 2.5.4 vlm");
        assert!(r
            .text
            .starts_with("# Lease Agreement\n\nThe tenant is Beta Robotics."));
        assert!(
            !r.text.contains("ACME CONFIDENTIAL"),
            "the header repeats on every page"
        );
        assert!(!r.text.contains("\n\n1\n\n"), "the page number is not text");
        assert!(
            r.text.contains("Schedule of rent\n\n| Year | Rent |"),
            "{}",
            r.text
        );
        assert!(r.text.contains("| 2026 | 1,000 |"));
        assert!(
            !r.text.contains("stamp.png"),
            "an image without a caption says nothing yet"
        );
    }

    #[test]
    fn pages_are_segments_that_cover_the_text() {
        let r = reading(&scan(), "mineru 2.5.4 vlm");
        assert_eq!(r.segments.len(), 2);
        assert_eq!(r.segments[0].range.start, 0);
        assert_eq!(r.segments[0].range.end, r.segments[1].range.start);
        assert_eq!(r.segments[1].range.end, r.text.len());
        assert!(r.text[r.segments[1].range.clone()].starts_with("Schedule of rent"));
        assert_eq!(r.segments[1].provenance.anchor, Some(json!({ "page": 2 })));
    }

    #[test]
    fn a_chunk_stays_on_its_page_and_boxes_what_it_covers() {
        let r = reading(&scan(), "mineru 2.5.4 vlm");
        let pieces = r.chunk(300);
        assert_eq!(pieces.len(), 2, "{pieces:#?}");
        let first = &pieces[0];
        assert_eq!(first.provenance.origin, Origin::Ocr);
        assert_eq!(first.provenance.model.as_deref(), Some("mineru 2.5.4 vlm"));
        assert_eq!(
            first.provenance.anchor,
            Some(json!({ "page": 1, "bbox": [100.0, 50.0, 900.0, 190.0] }))
        );
        assert!(first.text.contains("Rent is paid monthly."));
        let second = &pieces[1];
        assert_eq!(second.provenance.anchor.as_ref().unwrap()["page"], json!(2));
        assert!(second.text.contains("Signed by both parties."));
        assert_eq!(
            second.heading.as_deref(),
            Some("Lease Agreement"),
            "the breadcrumb crosses the page"
        );
        for p in &pieces {
            assert!(p
                .text
                .ends_with(&r.text[p.char_start as usize..p.char_end as usize]));
        }
    }

    #[test]
    fn an_empty_or_odd_list_reads_as_nothing() {
        assert!(reading(&json!([]), "m").text.is_empty());
        assert!(reading(&json!({"not": "a list"}), "m").segments.is_empty());
        let r = reading(
            &json!([{ "type": "equation", "text": "$$E = mc^2$$", "page_idx": 3 }]),
            "m",
        );
        assert_eq!(r.text, "$$E = mc^2$$");
        assert_eq!(
            r.chunk(300)[0].provenance.anchor,
            Some(json!({ "page": 4 }))
        );
    }
}
