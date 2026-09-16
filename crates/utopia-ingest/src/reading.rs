//! 模型读出来的正文（0040）：拼好的一份文字、按出处分的段，以及每一小段在原文件里的位置。
//!
//! 扫描件交回的是一个个带页码和框的区域，录音交回的是一句句带起止时刻和说话人的分段。两者
//! 拼成正文之后按同一套规矩切块；切好之后，每块的锚点从它盖住的那几小段算出来：扫描件取框的
//! 外接矩形，录音取最早的开始、最晚的结束和出现过的说话人。
//!
//! 段比小段粗（扫描件一页一段，录音整段一段），因为一块只装一种出处（决定 2）：按小段分，
//! 一句话就是一块，块碎到抽取看不见上下文。

use crate::chunker::{chunk_segments, ChunkPiece};
use crate::provenance::Segment;
use serde_json::json;
use std::ops::Range;

/// 拼好的正文，连同出处和每一小段的位置
#[derive(Debug, Clone)]
pub struct Reading {
    pub text: String,
    pub segments: Vec<Segment>,
    pub(crate) regions: Vec<Region>,
}

/// 正文里的一小段，和它在原文件里的位置
#[derive(Debug, Clone)]
pub(crate) struct Region {
    pub range: Range<usize>,
    pub place: Place,
}

#[derive(Debug, Clone)]
pub(crate) enum Place {
    /// 扫描页上的一块：页码从 1 数，框是引擎给的（0–1000 归一化）
    Page { page: u64, bbox: Option<[f64; 4]> },
    /// 录音里的一句
    Time {
        start_ms: u64,
        end_ms: u64,
        speaker: String,
    },
}

impl Reading {
    /// 按段切块，再按每块盖住的小段写锚点
    pub fn chunk(&self, budget: usize) -> Vec<ChunkPiece> {
        let mut pieces = chunk_segments(&self.text, &self.segments, budget);
        for p in &mut pieces {
            let span = p.char_start as usize..p.char_end as usize;
            let Some(anchor) = p.provenance.anchor.as_mut() else {
                continue;
            };
            let covered = self
                .regions
                .iter()
                .filter(|r| r.range.start < span.end && span.start < r.range.end);
            let page = anchor["page"].as_u64();
            let mut bbox: Option<[f64; 4]> = None;
            let mut time: Option<(u64, u64)> = None;
            let mut speakers: Vec<&str> = Vec::new();
            for r in covered {
                match &r.place {
                    Place::Page { page: pg, bbox: b } if Some(*pg) == page => {
                        if let Some(b) = b {
                            bbox = Some(bbox.map_or(*b, |a| {
                                [
                                    a[0].min(b[0]),
                                    a[1].min(b[1]),
                                    a[2].max(b[2]),
                                    a[3].max(b[3]),
                                ]
                            }));
                        }
                    }
                    Place::Page { .. } => {}
                    Place::Time {
                        start_ms,
                        end_ms,
                        speaker,
                    } => {
                        time = Some(time.map_or((*start_ms, *end_ms), |(s, e)| {
                            (s.min(*start_ms), e.max(*end_ms))
                        }));
                        if !speakers.contains(&speaker.as_str()) {
                            speakers.push(speaker);
                        }
                    }
                }
            }
            if let Some(b) = bbox {
                anchor["bbox"] = json!(b);
            }
            if let Some((start, end)) = time {
                anchor["start_ms"] = json!(start);
                anchor["end_ms"] = json!(end);
                anchor["speaker"] = json!(speakers);
            }
        }
        pieces
    }
}
