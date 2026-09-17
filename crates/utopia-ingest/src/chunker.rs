//! 分块：把文档块打包成抽取一次能看见的单元。
//!
//! **块的职责是「抽取一次能看见的全部」。**模型只看这一块，切开的东西补不回来，
//! 所以块边界是抽取契约的一部分，不是文本处理的细节。从这条出发定的规矩：
//!
//! - 一行不拆；表头与表体不拆；说明句（表上面那句「结果如下：」）与它的表不拆；
//!   标题与它下面的第一段不拆。
//! - 一张表放得下就整张放。放不下才按行切，**每个续块重复面包屑、说明句和表头**
//!   ——这一条才是修「续块只剩一行五个美元数」的那一刀，换再聪明的切分器都不解决它。
//! - 预算按 token 算，不按字符。从前是 1200 字符，注释说「约 1000+ token」，那是中文；
//!   英文只有 300 上下，一张宽表放不下。现在两种语言拿到的是同一个预算。
//! - 水平线**不是**切点。从前的切分器偏爱在水平线上切，而这类文档里水平线是分页符：
//!   投票结果里一张表正是被它劈成了两半。
//! - 不重叠。块是语义单元，上下文靠面包屑和补头给，不靠把上一块的尾巴抄一遍。
//!
//! 每块的正文都是原文的切片，一个字不改；前缀（面包屑、说明句、表头）也是原文的
//! 切片，只是从别处抄来的。`char_start` / `char_end` 指正文那一段，不含前缀。

use crate::blocks::{blocks, Block, Kind};
use crate::provenance::{Provenance, Segment};
use std::ops::Range;
use std::sync::OnceLock;
use text_splitter::{ChunkConfig, TextSplitter};
use tiktoken_rs::CoreBPE;

#[derive(Debug, Clone)]
pub struct ChunkPiece {
    pub seq: i32,
    pub text: String,
    pub char_start: i32,
    pub char_end: i32,
    /// 这块所在的章节路径，`›` 分隔；没有标题的文档为 None
    pub heading: Option<String>,
    /// 这块文字从哪来（0040）。一块只有一种出处
    pub provenance: Provenance,
}

/// 一块的预算（cl100k token）。
///
/// **300，不是 1000。**1000 是旧字符预算的注释里写的本意，第一轮就量掉了：收购
/// 8-K 从九块变成两块，模型对着 4700 字符只写了七条事实——地址、电话、「published
/// in the SEC」——收购本身一条都没有。模型一次调用写出的事实数不随输入变长而变多，
/// 块一大它就挑最容易的几条写。300 是旧切法在英文上的实际大小，也是 43/45 那两轮
/// 量出来的条件；中文在这个数下拿到的上下文比从前少，还没量（0039 开放问题）。
pub const BUDGET_TOKENS: usize = 300;

/// 说明句最长多少字节还算说明句：表上面那一段要短、或者以冒号结尾，
/// 才当成表的一部分带着走；一整段分析不是说明句
const CAPTION_MAX_BYTES: usize = 200;
/// 表前最多几段短说明一起当说明句
const CAPTION_RUN_MAX: usize = 5;

/// 不到这么多 token 的一段（页码、脚注标记）不单独成块，并进上一块
const TINY_TOKENS: usize = 8;

pub fn chunk_text(text: &str) -> Vec<ChunkPiece> {
    chunk_with_budget(text, BUDGET_TOKENS)
}

fn bpe() -> &'static CoreBPE {
    static BPE: OnceLock<CoreBPE> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().expect("cl100k 的排名表随 crate 内嵌"))
}

fn tokens(s: &str) -> usize {
    bpe().encode_ordinary(s).len()
}

/// 打包的单元：标题贴着下一块走；表带着自己的说明句。
enum Unit {
    Heading {
        level: u8,
        range: Range<usize>,
    },
    Text(Range<usize>),
    Table {
        caption: Option<Range<usize>>,
        head: Range<usize>,
        rows: Vec<Range<usize>>,
    },
}

fn units(text: &str, blocks: Vec<Block>) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::with_capacity(blocks.len());
    let mut i = 0usize;
    while i < blocks.len() {
        let b = &blocks[i];
        match &b.kind {
            Kind::Heading(level) => out.push(Unit::Heading {
                level: *level,
                range: b.range.clone(),
            }),
            Kind::Rule => {}
            Kind::Paragraph => {
                // 紧跟着表的短段落是说明句，跟表走。**两条通用的补充**（召回台子，
                // 2026-09-13 充值后那一轮量出来的）：
                //
                // - 说明句和表之间隔着水平线也算紧跟。这类文档里水平线是分页符，
                //   第五项议案的「结果如下：」和它的表正好被一页隔开，说明句留在了
                //   上一块，表去了下一块。
                // - 一句说明引出的是**紧跟其后的一串表**，不只第一张。「选举十位董事
                //   提名人，结果如下：」后面是十张表，从前只有第一张带着它；黄仁勋
                //   那张在另一块里，四个票数都对，却没有一句话说这是在投什么，
                //   「当选董事」那条边就没了。一串到别的东西（段落、标题）出现为止。
                //
                // - 表前**连续几段**短说明一起算：财报的「NVIDIA CORPORATION」「CONDENSED
                //   CONSOLIDATED STATEMENTS OF INCOME」「(In millions)」「(Unaudited)」是四个
                //   段落，只带最后一段的话，模型看到的表不知道是哪张报表、什么单位。
                //   一串最多五段，中间不隔别的东西
                let caption_like = |blk: &Block| {
                    matches!(blk.kind, Kind::Paragraph)
                        && (blk.range.len() <= CAPTION_MAX_BYTES
                            || text[blk.range.clone()].trim_end().ends_with(':'))
                };
                let mut k = i + 1;
                while k - i < CAPTION_RUN_MAX && blocks.get(k).is_some_and(&caption_like) {
                    k += 1;
                }
                let caption = b.range.start..blocks[k - 1].range.end;
                let mut j = k;
                let mut attached = 0usize;
                if caption_like(b) {
                    loop {
                        while matches!(
                            blocks.get(j),
                            Some(Block {
                                kind: Kind::Rule,
                                ..
                            })
                        ) {
                            j += 1;
                        }
                        let Some(Block {
                            kind: Kind::Table { head, rows },
                            ..
                        }) = blocks.get(j)
                        else {
                            break;
                        };
                        out.push(Unit::Table {
                            caption: Some(caption.clone()),
                            head: head.clone(),
                            rows: rows.clone(),
                        });
                        attached += 1;
                        j += 1;
                    }
                }
                if attached == 0 {
                    // 不是说明句：这一段照常；后面那几段下一轮各自再看
                    out.push(Unit::Text(b.range.clone()));
                } else {
                    i = j - 1;
                }
            }
            Kind::Table { head, rows } => out.push(Unit::Table {
                caption: None,
                head: head.clone(),
                rows: rows.clone(),
            }),
            Kind::Other => out.push(Unit::Text(b.range.clone())),
        }
        i += 1;
    }
    out
}

/// 正文里的一项：一段文本，或一张（可能只剩几行的）表
enum Piece {
    Text(Range<usize>),
    Table {
        caption: Option<Range<usize>>,
        head: Range<usize>,
        rows: Vec<Range<usize>>,
    },
}

impl Piece {
    fn render(&self, text: &str) -> String {
        match self {
            Piece::Text(r) => text[r.clone()].to_string(),
            Piece::Table {
                caption,
                head,
                rows,
            } => {
                let mut s = String::new();
                if let Some(c) = caption {
                    s.push_str(&text[c.clone()]);
                    s.push_str("\n\n");
                }
                s.push_str(&text[head.clone()]);
                for r in rows {
                    s.push('\n');
                    s.push_str(&text[r.clone()]);
                }
                s
            }
        }
    }
    fn span(&self) -> Range<usize> {
        match self {
            Piece::Text(r) => r.clone(),
            Piece::Table {
                caption,
                head,
                rows,
            } => {
                let start = caption.as_ref().map_or(head.start, |c| c.start);
                let end = rows.last().map_or(head.end, |r| r.end);
                start..end
            }
        }
    }
}

struct Packer<'a> {
    text: &'a str,
    budget: usize,
    out: Vec<ChunkPiece>,
    /// 现在在哪一章：(层级, 标题行)
    path: Vec<(u8, Range<usize>)>,
    /// 出现了、还没贴到正文上的标题
    pending: Vec<Range<usize>>,
    /// 正在攒的块：前缀（章节标题）与正文
    prefix: Vec<Range<usize>>,
    body: Vec<Piece>,
    /// 正文按出处分成的段；正在攒的块属于 `current` 那一段
    segments: &'a [Segment],
    current: usize,
}

impl<'a> Packer<'a> {
    /// 位置 `at` 落在哪一段（段按位置排好序）
    fn segment_of(&self, at: usize) -> usize {
        self.segments
            .iter()
            .rposition(|s| s.range.start <= at)
            .unwrap_or(0)
    }

    /// 下一项要进 `seg` 那一段：跟正在攒的块不是同一段，先把那块收掉。
    /// 标题不跟着收——它属于接下来的正文，面包屑也照样往下传
    fn enter_segment(&mut self, seg: usize) {
        if seg != self.current {
            self.flush();
            self.current = seg;
        }
    }

    fn render(&self, prefix: &[Range<usize>], body: &[Piece]) -> String {
        let mut parts: Vec<String> = prefix
            .iter()
            .map(|r| self.text[r.clone()].to_string())
            .collect();
        // 同一块里一串表共用一句说明：说明只在第一张前面出现一次。
        // 十位董事的十张表挤在一块里，不该把「结果如下」念十遍
        let mut last_caption: Option<Range<usize>> = None;
        for p in body {
            match p {
                Piece::Table {
                    caption: Some(c),
                    head,
                    rows,
                } if last_caption.as_ref() == Some(c) => {
                    let bare = Piece::Table {
                        caption: None,
                        head: head.clone(),
                        rows: rows.clone(),
                    };
                    parts.push(bare.render(self.text));
                }
                Piece::Table { caption, .. } => {
                    last_caption = caption.clone();
                    parts.push(p.render(self.text));
                }
                Piece::Text(_) => {
                    last_caption = None;
                    parts.push(p.render(self.text));
                }
            }
        }
        parts.join("\n\n")
    }

    /// 新块的前缀：现在这一章的标题路径，去掉马上要作为正文出现的那几条
    fn prefix_now(&self) -> Vec<Range<usize>> {
        self.path
            .iter()
            .map(|(_, r)| r.clone())
            .filter(|r| !self.pending.contains(r))
            .collect()
    }

    fn fits(&self, prefix: &[Range<usize>], body: &[Piece]) -> bool {
        tokens(&self.render(prefix, body)) <= self.budget
    }

    /// 试着把一项放进当前块；放不下就先收当前块，再试一次空块。
    ///
    /// **极小的一项不单独成块。**新闻稿末尾有一行页码「4」，按预算它放不进已经
    /// 满了的上一块，于是自己成了一块——一个字符的块，抽取照样为它调一次模型。
    /// 几个 token 的东西并进上一块，超预算那几个 token 无所谓。
    fn place(&mut self, piece: Piece) -> Option<Piece> {
        if self.pending.is_empty() {
            if let Piece::Text(r) = &piece {
                if tokens(&self.text[r.clone()]) <= TINY_TOKENS {
                    if !self.body.is_empty() {
                        self.body.push(piece);
                        return None;
                    }
                    // 上一块已经发出去了（长段落走退路切分会立刻 flush）：接到它尾上。
                    // 只接同一种出处的——一行转写不能混进原文那一块里借它的可信度
                    let here = &self.segments[self.current].provenance;
                    if let Some(last) = self.out.last_mut().filter(|l| &l.provenance == here) {
                        last.text.push_str(
                            "

",
                        );
                        last.text.push_str(&self.text[r.clone()]);
                        last.char_end = last.char_end.max(r.end as i32);
                        return None;
                    }
                }
            }
        }
        let mut with_headings: Vec<Piece> = self.pending.iter().cloned().map(Piece::Text).collect();
        with_headings.push(piece);
        if self.body.is_empty() {
            self.prefix = self.prefix_now();
        }
        let mut candidate: Vec<Piece> = std::mem::take(&mut self.body);
        candidate.extend(with_headings);
        if self.fits(&self.prefix.clone(), &candidate) {
            self.body = candidate;
            self.pending.clear();
            return None;
        }
        // 放不下：把刚加的拆回来
        let n = candidate.len() - (self.pending.len() + 1);
        let mut rest = candidate.split_off(n);
        self.body = candidate;
        let piece = rest.pop().expect("刚放进去的那一项");
        if !self.body.is_empty() {
            self.flush();
            return self.place(piece);
        }
        Some(piece)
    }

    fn flush(&mut self) {
        if self.body.is_empty() {
            return;
        }
        let body = std::mem::take(&mut self.body);
        let prefix = std::mem::take(&mut self.prefix);
        let text = self.render(&prefix, &body);
        let start = body.iter().map(|p| p.span().start).min().unwrap_or(0);
        let end = body.iter().map(|p| p.span().end).max().unwrap_or(0);
        let heading = self.breadcrumb();
        self.out.push(ChunkPiece {
            seq: self.out.len() as i32,
            text,
            char_start: start as i32,
            char_end: end as i32,
            heading,
            provenance: self.segments[self.current].provenance.clone(),
        });
    }

    fn breadcrumb(&self) -> Option<String> {
        if self.path.is_empty() {
            return None;
        }
        Some(
            self.path
                .iter()
                .map(|(_, r)| self.text[r.clone()].trim_start_matches('#').trim())
                .collect::<Vec<_>>()
                .join(" › "),
        )
    }

    fn heading(&mut self, level: u8, range: Range<usize>) {
        while self.path.last().is_some_and(|(l, _)| *l >= level) {
            self.path.pop();
        }
        self.path.push((level, range.clone()));
        self.pending.push(range);
    }

    fn text_unit(&mut self, range: Range<usize>) {
        let Some(Piece::Text(range)) = self.place(Piece::Text(range)) else {
            return;
        };
        // 一段就超预算：退回按句切，每一小段自成一块，前缀照给
        self.prefix = self.prefix_now();
        let prefix_cost = tokens(&self.render(&self.prefix.clone(), &[]));
        let headings: Vec<Piece> = self.pending.drain(..).map(Piece::Text).collect();
        let heading_cost = tokens(&self.render(&[], &headings));
        let capacity = self
            .budget
            .saturating_sub(prefix_cost + heading_cost)
            .max(1);
        let config = ChunkConfig::new(capacity).with_sizer(bpe());
        let splitter = TextSplitter::new(config);
        let block = &self.text[range.clone()];
        let mut first = true;
        for (offset, piece) in splitter.chunk_indices(block) {
            let sub = range.start + offset..range.start + offset + piece.len();
            if first {
                self.body = headings
                    .iter()
                    .map(|h| match h {
                        Piece::Text(r) => Piece::Text(r.clone()),
                        _ => unreachable!(),
                    })
                    .collect();
                first = false;
            }
            self.body.push(Piece::Text(sub));
            self.prefix = self.prefix_now();
            self.flush();
        }
    }

    fn table_unit(
        &mut self,
        caption: Option<Range<usize>>,
        head: Range<usize>,
        rows: Vec<Range<usize>>,
    ) {
        let whole = Piece::Table {
            caption: caption.clone(),
            head: head.clone(),
            rows: rows.clone(),
        };
        let Some(_) = self.place(whole) else {
            return;
        };
        // 整张放不下：按行切，每一块都带说明句和表头。一行不拆；
        // 说明句 + 表头 + 一行还超预算的，照样出一块——没有更好的切法
        let mut rows = rows.into_iter().peekable();
        while rows.peek().is_some() {
            let mut taken: Vec<Range<usize>> = vec![rows.next().expect("peek 过")];
            while let Some(next) = rows.peek() {
                let mut trial = taken.clone();
                trial.push(next.clone());
                let piece = Piece::Table {
                    caption: caption.clone(),
                    head: head.clone(),
                    rows: trial,
                };
                let headings: Vec<Piece> = self.pending.iter().cloned().map(Piece::Text).collect();
                let mut body = headings;
                body.push(piece);
                if self.fits(&self.prefix_now(), &body) {
                    taken.push(rows.next().expect("peek 过"));
                } else {
                    break;
                }
            }
            self.prefix = self.prefix_now();
            self.body = self.pending.drain(..).map(Piece::Text).collect();
            self.body.push(Piece::Table {
                caption: caption.clone(),
                head: head.clone(),
                rows: taken,
            });
            self.flush();
        }
    }
}

pub fn chunk_with_budget(text: &str, budget: usize) -> Vec<ChunkPiece> {
    let whole = [Segment {
        range: 0..text.len(),
        provenance: Provenance::stated(),
    }];
    chunk_segments(text, &whole, budget)
}

/// 按出处分段的正文分块：块不跨段（0040 决定 2），面包屑跨段照传。
///
/// 读扫描件的引擎一页一段、转写一句一段地交回文字；它们拼成一份正文、按同一套规矩切，
/// 只是切点多了「出处变了」这一条。`segments` 按位置排好序、覆盖整份正文；
/// 空的时候当作整份都是原文
pub fn chunk_segments(text: &str, segments: &[Segment], budget: usize) -> Vec<ChunkPiece> {
    let whole = [Segment {
        range: 0..text.len(),
        provenance: Provenance::stated(),
    }];
    let segments = if segments.is_empty() {
        &whole[..]
    } else {
        segments
    };
    let mut p = Packer {
        text,
        budget,
        out: Vec::new(),
        path: Vec::new(),
        pending: Vec::new(),
        prefix: Vec::new(),
        body: Vec::new(),
        segments,
        current: 0,
    };
    for unit in units(text, blocks(text)) {
        let start = match &unit {
            Unit::Heading { range, .. } | Unit::Text(range) => range.start,
            Unit::Table { caption, head, .. } => caption.as_ref().map_or(head.start, |c| c.start),
        };
        // 标题不切段：它贴着下一项走，由下一项决定进哪一段
        if !matches!(unit, Unit::Heading { .. }) {
            p.enter_segment(p.segment_of(start));
        }
        match unit {
            Unit::Heading { level, range } => p.heading(level, range),
            Unit::Text(range) => p.text_unit(range),
            Unit::Table {
                caption,
                head,
                rows,
            } => p.table_unit(caption, head, rows),
        }
    }
    // 文档以标题收尾：标题自己成一块，总好过丢掉
    if !p.pending.is_empty() {
        p.prefix = p.prefix_now();
        p.body = p.pending.drain(..).map(Piece::Text).collect();
    }
    p.flush();
    p.out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一块只装一种出处（0040 决定 2）：原文与扫描页挨着，也分成两块；
    /// 面包屑跨段照传；页末那几个 token 的尾巴不接到别的出处那一块上
    #[test]
    fn a_chunk_holds_one_provenance_and_the_breadcrumb_crosses_it() {
        use crate::provenance::{Origin, Provenance, Segment};
        let stated = "# Lease

The tenant pays rent monthly.

";
        let page1 = "The rent is 1,000 per month.

";
        let page2 = "4

";
        let text = format!("{stated}{page1}{page2}");
        let ocr = |page: i64| Provenance {
            origin: Origin::Ocr,
            model: Some("mineru".into()),
            anchor: Some(serde_json::json!({ "page": page })),
        };
        let a = stated.len();
        let b = a + page1.len();
        let segments = vec![
            Segment {
                range: 0..a,
                provenance: Provenance::stated(),
            },
            Segment {
                range: a..b,
                provenance: ocr(1),
            },
            Segment {
                range: b..text.len(),
                provenance: ocr(2),
            },
        ];
        let chunks = chunk_segments(&text, &segments, BUDGET_TOKENS);
        let origins: Vec<_> = chunks.iter().map(|c| c.provenance.clone()).collect();
        assert_eq!(chunks.len(), 3, "{chunks:#?}");
        assert_eq!(origins[0], Provenance::stated());
        assert_eq!(origins[1], ocr(1));
        assert_eq!(origins[2], ocr(2), "the page number stays on its own page");
        assert!(chunks[1].text.contains("1,000"));
        assert!(!chunks[0].text.contains("1,000"));
        assert_eq!(chunks[1].heading.as_deref(), Some("Lease"));
        // 不分段的老路：整份都是原文
        assert!(chunk_with_budget(&text, BUDGET_TOKENS)
            .iter()
            .all(|c| c.provenance == Provenance::stated()));
    }

    fn table(n_rows: usize) -> String {
        let mut s = String::from("The results of the voting were as follows:\n\n| Nominee | For | Against |\n| --- | --- | --- |\n");
        for i in 0..n_rows {
            s.push_str(&format!("| Director {i} | {} | {} |\n", 1000 + i, 50 + i));
        }
        s
    }

    /// 财报里那 13 块的形状：表比预算大，续块必须还看得见表头和说明句。
    #[test]
    fn a_wide_table_is_split_by_rows_and_every_piece_carries_its_header_and_caption() {
        let text = format!("# Item 5.07\n\n{}", table(40));
        let pieces = chunk_with_budget(&text, 120);
        assert!(
            pieces.len() > 2,
            "40 行在 120 token 里必然切成好几块: {}",
            pieces.len()
        );
        for (i, p) in pieces.iter().enumerate() {
            assert!(
                p.text.contains("| Nominee | For | Against |"),
                "第 {i} 块没有表头:\n{}",
                p.text
            );
            assert!(
                p.text.contains("| --- | --- | --- |"),
                "第 {i} 块没有分隔行"
            );
            assert!(
                p.text
                    .contains("The results of the voting were as follows:"),
                "第 {i} 块没有说明句"
            );
            assert!(
                p.text.starts_with("# Item 5.07"),
                "第 {i} 块没有章节面包屑:\n{}",
                p.text
            );
            assert_eq!(p.heading.as_deref(), Some("Item 5.07"));
        }
        // 每一行只出现一次，一行不拆
        let all: String = pieces
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for i in 0..40 {
            assert_eq!(
                all.matches(&format!("| Director {i} |")).count(),
                1,
                "Director {i}"
            );
        }
    }

    #[test]
    fn a_table_that_fits_stays_whole_with_its_caption() {
        let text = format!("Intro paragraph.\n\n{}\nClosing.", table(3));
        let pieces = chunk_with_budget(&text, 1000);
        assert_eq!(pieces.len(), 1);
        let t = &pieces[0].text;
        assert!(t.contains("The results of the voting were as follows:\n\n| Nominee"));
        assert!(t.ends_with("Closing."));
    }

    /// 说明句已经在上一块的尾巴上、表放不进去了：说明句得跟着表走，不能留在上一块。
    #[test]
    fn a_caption_never_parts_from_its_table() {
        let filler = "Words about nothing in particular. ".repeat(12);
        let text = format!("{filler}\n\n{}", table(4));
        let pieces = chunk_with_budget(&text, 140);
        assert!(pieces.len() >= 2);
        let with_table = pieces
            .iter()
            .find(|p| p.text.contains("| Nominee |"))
            .expect("有表的那块");
        assert!(with_table
            .text
            .contains("The results of the voting were as follows:"));
        let before: Vec<&ChunkPiece> = pieces
            .iter()
            .filter(|p| !p.text.contains("| Nominee |"))
            .collect();
        for p in before {
            assert!(
                !p.text.contains("were as follows"),
                "说明句留在了没有表的块里:\n{}",
                p.text
            );
        }
    }

    #[test]
    fn a_chunk_opens_with_the_headings_it_lives_under() {
        let text = "# Report\n\n## Part A\n\nFirst.\n\n## Part B\n\nSecond.\n\n### B.1\n\nThird.";
        let pieces = chunk_with_budget(text, 1000);
        assert_eq!(pieces.len(), 1);
        assert!(pieces[0]
            .text
            .starts_with("# Report\n\n## Part A\n\nFirst."));
        assert_eq!(pieces[0].heading.as_deref(), Some("Report › Part B › B.1"));

        let pieces = chunk_with_budget(text, 14);
        let third = pieces.iter().find(|p| p.text.contains("Third.")).unwrap();
        assert!(
            third.text.starts_with("# Report\n\n## Part B\n\n### B.1"),
            "{}",
            third.text
        );
        assert_eq!(third.heading.as_deref(), Some("Report › Part B › B.1"));
        assert_eq!(
            &text[third.char_start as usize..third.char_end as usize],
            "### B.1\n\nThird."
        );
    }

    #[test]
    fn a_long_paragraph_still_splits_by_sentence_and_offsets_are_verbatim() {
        let text = "One sentence here. Another sentence follows it. ".repeat(30);
        let pieces = chunk_with_budget(&text, 40);
        assert!(pieces.len() > 1);
        for p in &pieces {
            assert_eq!(&text[p.char_start as usize..p.char_end as usize], p.text);
            assert!(tokens(&p.text) <= 40);
        }
    }

    #[test]
    fn chunks_are_numbered_from_zero_and_never_empty() {
        let text = "# T\n\nA.\n\n* * *\n\nB.\n\n| x | y |\n| --- | --- |\n| 1 | 2 |";
        let pieces = chunk_text(text);
        assert_eq!(pieces.len(), 1);
        for (i, p) in pieces.iter().enumerate() {
            assert_eq!(p.seq as usize, i);
            assert!(!p.text.trim().is_empty());
        }
        assert!(!pieces[0].text.contains("* * *"), "水平线不进块");
    }

    /// 新闻稿末尾的页码「4」：不单独成块，并进上一块，哪怕上一块已经满了。
    #[test]
    fn a_page_number_never_becomes_a_chunk_of_its_own() {
        let body = "A sentence that fills the budget nicely. ".repeat(6);
        let text = format!(
            "{body}

4"
        );
        let pieces = chunk_with_budget(&text, tokens(body.trim()));
        assert_eq!(
            pieces.len(),
            1,
            "{:?}",
            pieces.iter().map(|p| &p.text).collect::<Vec<_>>()
        );
        assert!(pieces[0].text.ends_with(
            "

4"
        ));
    }

    /// 第五项议案的形状：说明句 · 分页符 · 表。分页符不隔开说明句和它的表。
    #[test]
    fn a_page_break_between_a_caption_and_its_table_does_not_part_them() {
        let filler = "Words about nothing in particular. ".repeat(10);
        let text = format!(
            "{filler}\n\n5. Stockholders did not approve the proposal. The results of the voting were as follows:\n\n* * *\n\n| Number of shares For | 144 |\n| --- | --- |\n| Number of shares Against | 16 |\n"
        );
        let pieces = chunk_with_budget(&text, 110);
        let table = pieces
            .iter()
            .find(|p| p.text.contains("| Number of shares For |"))
            .expect("有表的那块");
        assert!(
            table
                .text
                .contains("The results of the voting were as follows:"),
            "{}",
            table.text
        );
        for p in pieces
            .iter()
            .filter(|p| !p.text.contains("| Number of shares For |"))
        {
            assert!(
                !p.text.contains("were as follows"),
                "说明句留在了没有表的块里:\n{}",
                p.text
            );
        }
    }

    /// 财报的形状：公司名、报表名、单位、「未经审计」四段短说明，然后是表。
    /// 四段都是说明句，切开的每一块都带着全部四段。
    #[test]
    fn a_run_of_short_paragraphs_before_a_table_is_its_caption() {
        let text = format!(
            "Prose that is long enough not to be a caption, going on about the quarter and the outlook and the products and the customers.\n\n\
             NVIDIA CORPORATION\n\nCONDENSED CONSOLIDATED STATEMENTS OF INCOME\n\n(In millions)\n\n(Unaudited)\n\n{}",
            table(30)
        );
        let pieces = chunk_with_budget(&text, 120);
        let with_table: Vec<&ChunkPiece> = pieces.iter().filter(|p| p.text.contains("| --- |")).collect();
        assert!(with_table.len() >= 2, "表该被切成几块: {}", pieces.len());
        for p in &with_table {
            assert!(p.text.contains("NVIDIA CORPORATION"), "每块都带公司名: {}", p.text);
            assert!(p.text.contains("STATEMENTS OF INCOME"), "每块都带报表名: {}", p.text);
            assert!(p.text.contains("(Unaudited)"), "每块都带最后一段: {}", p.text);
            assert!(!p.text.contains("Prose that is long"), "长段落不是说明句: {}", p.text);
        }
    }

    /// 十位董事的形状：一句说明、一串表。每张表不论落在哪一块都看得见那句说明；
    /// 同一块里的几张表，说明只念一遍。
    #[test]
    fn one_caption_introduces_the_whole_run_of_tables_after_it() {
        let mut text = String::from("Stockholders elected each of the director nominees. The results of the voting were as follows:\n\n");
        for (i, name) in [
            "Tench Coxe",
            "John Dabiri",
            "Jen-Hsun Huang",
            "Dawn Hudson",
            "Harvey Jones",
        ]
        .iter()
        .enumerate()
        {
            text.push_str(&format!(
                "| {name} |  |\n| --- | --- |\n| Number of shares For | {} |\n| Number of shares Against | {} |\n\n",
                1000 + i,
                50 + i
            ));
        }
        text.push_str(
            "Another matter entirely.\n\n| Unrelated | 1 |\n| --- | --- |\n| row | 2 |\n",
        );
        let pieces = chunk_with_budget(&text, 90);
        assert!(pieces.len() >= 3, "{}", pieces.len());
        let huang = pieces
            .iter()
            .find(|p| p.text.contains("Jen-Hsun Huang"))
            .expect("黄仁勋那块");
        assert!(
            huang.text.contains("director nominees"),
            "续块丢了说明句:\n{}",
            huang.text
        );
        for p in &pieces {
            assert!(
                p.text.matches("were as follows").count() <= 1,
                "同一块里说明念了不止一遍:\n{}",
                p.text
            );
        }
        // 一串到别的东西出现为止：后面那张无关的表不带这句说明
        let unrelated = pieces
            .iter()
            .find(|p| p.text.contains("| Unrelated |"))
            .unwrap();
        let before = &unrelated.text[..unrelated.text.find("| Unrelated |").unwrap()];
        assert!(!before.ends_with("follows:\n\n"), "{}", unrelated.text);
        assert!(unrelated.text.contains("Another matter entirely."));
    }

    #[test]
    fn a_rule_is_not_a_boundary() {
        let text = "Alpha.\n\n* * *\n\nBeta.";
        assert_eq!(chunk_with_budget(text, 1000).len(), 1);
    }
}
