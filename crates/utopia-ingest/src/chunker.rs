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
                // 紧跟着一张表的短段落是它的说明句，跟表走
                let caption_like = b.range.len() <= CAPTION_MAX_BYTES
                    || text[b.range.clone()].trim_end().ends_with(':');
                match blocks.get(i + 1) {
                    Some(Block {
                        kind: Kind::Table { head, rows },
                        ..
                    }) if caption_like => {
                        out.push(Unit::Table {
                            caption: Some(b.range.clone()),
                            head: head.clone(),
                            rows: rows.clone(),
                        });
                        i += 1;
                    }
                    _ => out.push(Unit::Text(b.range.clone())),
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
}

impl<'a> Packer<'a> {
    fn render(&self, prefix: &[Range<usize>], body: &[Piece]) -> String {
        let mut parts: Vec<String> = prefix
            .iter()
            .map(|r| self.text[r.clone()].to_string())
            .collect();
        parts.extend(body.iter().map(|p| p.render(self.text)));
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
                    // 上一块已经发出去了（长段落走退路切分会立刻 flush）：接到它尾上
                    if let Some(last) = self.out.last_mut() {
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
            loop {
                let Some(next) = rows.peek() else { break };
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
    let mut p = Packer {
        text,
        budget,
        out: Vec::new(),
        path: Vec::new(),
        pending: Vec::new(),
        prefix: Vec::new(),
        body: Vec::new(),
    };
    for unit in units(text, blocks(text)) {
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

    #[test]
    fn a_rule_is_not_a_boundary() {
        let text = "Alpha.\n\n* * *\n\nBeta.";
        assert_eq!(chunk_with_budget(text, 1000).len(), 1);
    }
}
