//! 文档块：把解析器出的 Markdown 读成**顶层块**的序列，每块带着自己在原文里的
//! 字节范围。分块器在这一层之上打包，而不是直接对着字符串找边界。
//!
//! 为什么要这一层：字符串里没有「这是一张表的第三行」这个信息，切分器只能靠
//! 换行和空行猜。猜出来的结果量过（召回台子，2026-09-13）：财报 35 块里 13 块
//! 从表格中间开始、没有表头，模型面对一行五个美元数只能猜哪列是哪季度；投票
//! 结果里一张表被原文的分页符劈成两半，后半截成了没有名字的孤表。
//!
//! 这里只做两件事：读出块，和把被分页符劈开的表接回去。切多大、怎么补头，
//! 归 `chunker`。

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// 一个顶层块是什么。范围一律对齐到整行，切出来的文本一个字不改。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// `#` 的个数
    Heading(u8),
    Paragraph,
    /// `head` 是表头行连同分隔行；`rows` 是体行，一行一个范围。
    /// 接回来的续表让 `head`/`rows` 不再连续，所以表的文本一律由这两样拼，
    /// 不从 `Block::range` 整段切
    Table {
        head: Range<usize>,
        rows: Vec<Range<usize>>,
    },
    /// 水平线。在这类文档里多半是**分页符**，不是章节的边界
    Rule,
    /// 列表、代码块、引用、原样 HTML：整块一个单元，不往里看
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub range: Range<usize>,
}

/// 读出顶层块，并把被分页符劈开的表接回去。
pub fn blocks(text: &str) -> Vec<Block> {
    join_continuations(text, parse(text))
}

fn parse(text: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut depth = 0usize;
    // 正在收的表：整表范围、表头范围、体行范围
    let mut table: Option<(Range<usize>, Option<Range<usize>>, Vec<Range<usize>>)> = None;
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    let kind = match &tag {
                        Tag::Heading { level, .. } => Some(Kind::Heading(*level as u8)),
                        Tag::Paragraph => Some(Kind::Paragraph),
                        Tag::Table(_) => {
                            table = Some((range.clone(), None, Vec::new()));
                            None
                        }
                        _ => Some(Kind::Other),
                    };
                    if let Some(kind) = kind {
                        out.push(Block {
                            kind,
                            range: lines(text, range),
                        });
                    }
                } else if depth == 1 {
                    if let Some((_, head, rows)) = table.as_mut() {
                        match tag {
                            Tag::TableHead => *head = Some(lines(text, range)),
                            Tag::TableRow => rows.push(lines(text, range)),
                            _ => {}
                        }
                    }
                }
                depth += 1;
            }
            Event::End(tag) => {
                depth = depth.saturating_sub(1);
                if depth == 0 && tag == TagEnd::Table {
                    if let Some((whole, head, rows)) = table.take() {
                        let whole = lines(text, whole);
                        // 表头连同它下面的分隔行：从表的开头到第一个体行之前
                        let head = match (head, rows.first()) {
                            (Some(_), Some(first)) => whole.start..line_before(text, first.start),
                            _ => whole.clone(),
                        };
                        out.push(Block {
                            kind: Kind::Table { head, rows },
                            range: whole,
                        });
                    }
                }
            }
            Event::Rule if depth == 0 => out.push(Block {
                kind: Kind::Rule,
                range: lines(text, range),
            }),
            _ => {}
        }
    }
    out
}

/// 被分页符劈开的表接回去。
///
/// 形状很具体：`表 A · 分隔线 · 表 B`，两张一样宽，而 B 的「表头」首格跟 A 某个
/// 体行的首格共享开头两个词——`Number of shares Abstaining` 接在
/// `Number of shares For / Against` 后面。那不是 B 的表头，是 A 的下一行，
/// 解析器只是因为分隔行在它上面才把它当成了表头。
///
/// 判据故意窄：只看行首的标签，不看数。「同宽、表头里有数字」会把两张按年份
/// 分列的财务表接成一张——`2025 | 2024` 也是数字。
fn join_continuations(text: &str, blocks: Vec<Block>) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    let mut i = 0usize;
    while i < blocks.len() {
        let b = blocks[i].clone();
        let joined = match (&b.kind, out.last()) {
            (
                Kind::Rule,
                Some(Block {
                    kind: Kind::Table { .. },
                    ..
                }),
            ) => match blocks.get(i + 1) {
                Some(Block {
                    kind: Kind::Table { head, rows },
                    range,
                }) if continues(text, out.last().unwrap(), head) => {
                    let Some(Block {
                        kind:
                            Kind::Table {
                                rows: prev_rows, ..
                            },
                        range: prev_range,
                    }) = out.last_mut()
                    else {
                        unreachable!()
                    };
                    // B 的「表头行」是 A 的一个体行；分隔行不要
                    prev_rows.push(first_line(text, head));
                    prev_rows.extend(rows.iter().cloned());
                    prev_range.end = range.end;
                    i += 2;
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if !joined {
            out.push(b);
            i += 1;
        }
    }
    out
}

fn continues(text: &str, prev: &Block, next_head: &Range<usize>) -> bool {
    let Kind::Table { head, rows } = &prev.kind else {
        return false;
    };
    let width = |r: &Range<usize>| cell_count(&text[first_line(text, r)]);
    if width(head) != width(next_head) {
        return false;
    }
    let lead = |r: &Range<usize>| {
        first_cell(&text[first_line(text, r)])
            .split_whitespace()
            .take(2)
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
    };
    let next = lead(next_head);
    next.len() == 2 && rows.iter().any(|r| lead(r) == next)
}

/// 范围对齐到整行：往前退到行首，往后走到行尾（不含换行）。
fn lines(text: &str, r: Range<usize>) -> Range<usize> {
    let start = text[..r.start.min(text.len())]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let mut end = r.end.min(text.len());
    while end > start && matches!(text.as_bytes()[end - 1], b'\n' | b' ' | b'\t') {
        end -= 1;
    }
    let mut end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    // 扩到行尾会把行尾的空格再收回来；块的文本不带尾随空白
    while end > start && matches!(text.as_bytes()[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    start..end
}

/// 上一行的末尾（不含换行）：一个范围结束在哪儿，下一段从哪儿起
fn line_before(text: &str, at: usize) -> usize {
    let mut end = at;
    while end > 0 && matches!(text.as_bytes()[end - 1], b'\n' | b' ' | b'\t') {
        end -= 1;
    }
    end
}

fn first_line(text: &str, r: &Range<usize>) -> Range<usize> {
    let end = text[r.start..r.end]
        .find('\n')
        .map_or(r.end, |i| r.start + i);
    r.start..end
}

fn cell_count(line: &str) -> usize {
    let t = line.trim();
    if t.starts_with('|') && t.ends_with('|') && t.len() > 1 {
        t[1..t.len() - 1].split('|').count()
    } else {
        0
    }
}

fn first_cell(line: &str) -> &str {
    let t = line.trim();
    t.trim_start_matches('|')
        .split('|')
        .next()
        .unwrap_or("")
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<String> {
        blocks(text)
            .into_iter()
            .map(|b| match b.kind {
                Kind::Heading(l) => format!("h{l}"),
                Kind::Paragraph => "p".into(),
                Kind::Table { rows, .. } => format!("table({})", rows.len()),
                Kind::Rule => "rule".into(),
                Kind::Other => "other".into(),
            })
            .collect()
    }

    #[test]
    fn a_table_keeps_its_header_and_rows_apart() {
        let text = "Intro.\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n\nAfter.";
        let b = blocks(text);
        assert_eq!(kinds(text), ["p", "table(2)", "p"]);
        let Kind::Table { head, rows } = &b[1].kind else {
            panic!()
        };
        assert_eq!(&text[head.clone()], "| a | b |\n| --- | --- |");
        assert_eq!(&text[rows[0].clone()], "| 1 | 2 |");
        assert_eq!(&text[rows[1].clone()], "| 3 | 4 |");
        assert_eq!(
            &text[b[1].range.clone()],
            "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |"
        );
    }

    /// 投票结果 8-K 里 Stephen C. Neal 那张表的形状：原文的分页符落在表中间，
    /// 后半截带着一条分隔行，解析器把「弃权」那一行当成了新表的表头。
    #[test]
    fn a_headerless_table_after_a_rule_continues_the_one_before() {
        let text = "| g. Stephen C. Neal |  |\n| --- | --- |\n| Number of shares For | 14,573 |\n| Number of shares Against | 2,234 |\n\n* * *\n\n| Number of shares Abstaining | 49,064 |\n| --- | --- |\n| Number of Broker Non-Votes | 2,829 |\n";
        assert_eq!(kinds(text), ["table(4)"]);
        let Kind::Table { rows, .. } = &blocks(text)[0].kind else {
            panic!()
        };
        let rows: Vec<&str> = rows.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(rows[2], "| Number of shares Abstaining | 49,064 |");
        assert_eq!(rows[3], "| Number of Broker Non-Votes | 2,829 |");
    }

    /// 两张按年份分列的表隔着分页符：表头是 `2025 | 2024` 这种数字，不接。
    #[test]
    fn two_tables_with_numeric_headers_stay_two_tables() {
        let text = "| Revenue | 2025 |\n| --- | --- |\n| Total | 10 |\n\n* * *\n\n| 2024 | 2023 |\n| --- | --- |\n| Total | 9 |\n";
        assert_eq!(kinds(text), ["table(1)", "rule", "table(1)"]);
    }

    #[test]
    fn headings_rules_and_lists_are_their_own_blocks() {
        let text = "# Title\n\nPara one.\n\n- a\n- b\n\n---\n\n## Sub\n\nPara two.";
        assert_eq!(kinds(text), ["h1", "p", "other", "rule", "h2", "p"]);
        let b = blocks(text);
        assert_eq!(&text[b[0].range.clone()], "# Title");
        assert_eq!(&text[b[2].range.clone()], "- a\n- b");
    }
}
