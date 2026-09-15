//! 一段文字从哪来（0040）：原文写的、从扫描页上认出来的、从录音里转出来的、还是模型看图说的。
//!
//! 分块之后的一切只吃文字，分不出一句话是写在文件里的，还是模型转述的。这四种不是同等的证据：
//! 前三种是「原话，照读」，人能对着页面或录音核对；第四种是模型对一张图的转述，没有原话可对。
//! 所以来源跟着分块走，不跟着证据行走——证据引用的就是分块，抽取读的也是分块。

use std::ops::Range;

/// 文字的来源。数据库里存 [`Origin::as_str`] 那个词
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// 文件里写着的
    Stated,
    /// 从扫描页、截图上认出来的字（可能认错一个数字）
    Ocr,
    /// 录音转写（名字最容易听错，而名字正是抽取要抽的）
    Transcribed,
    /// 模型对一张图的描述：没有人写过、说过这句话
    Described,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Stated => "stated",
            Origin::Ocr => "ocr",
            Origin::Transcribed => "transcribed",
            Origin::Described => "described",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "stated" => Some(Origin::Stated),
            "ocr" => Some(Origin::Ocr),
            "transcribed" => Some(Origin::Transcribed),
            "described" => Some(Origin::Described),
            _ => None,
        }
    }
}

/// 一段文字的出处：来源、读出它的引擎或模型、以及指回原文件字节的锚点。
///
/// 锚点的形状由来源决定（数据库上有 CHECK）：原文没有锚点（`char_start` / `char_end`
/// 已经给出位置）；扫描页是 `{"page": n}`，引擎给了框就带 `"bbox"`；录音是
/// `{"start_ms", "end_ms", "speaker"}`；图片描述指到页和图
#[derive(Debug, Clone, PartialEq)]
pub struct Provenance {
    pub origin: Origin,
    pub model: Option<String>,
    pub anchor: Option<serde_json::Value>,
}

impl Provenance {
    pub fn stated() -> Self {
        Provenance {
            origin: Origin::Stated,
            model: None,
            anchor: None,
        }
    }
}

/// 正文里的一段，连同它的出处。读扫描件、录音的引擎一页、一段地交回文字，拼成一份正文
/// 交给分块；分块按段切开，**一块只装一种出处**（0040 决定 2）
#[derive(Debug, Clone)]
pub struct Segment {
    /// 在拼好的正文里的字节区间
    pub range: Range<usize>,
    pub provenance: Provenance,
}
