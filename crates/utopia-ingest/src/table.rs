//! 表格从 DOM 直接渲染成带真表头的 Markdown 表（0039/0040 那一层的活）。
//!
//! 为什么不交给 htmd：SEC/XBRL 报表的表一个 `<th>` 都没有。标题行（公司名、报表名、
//! 单位）是 colspan 跨全宽的普通行，列头（"July 26," / "2026"）拆在两行里，每个数字前后
//! 各有一格 "$" 或 ")"，列与列之间是空的占位格。此前的补丁把第一行提成表头，于是被切开
//! 的每一块表头都是「| NVIDIA CORPORATION | | |」，真列头一块都没带；模型面对
//! 「Accounts receivable, net | 63,059 | 38,466」只能写出「短语 = 数字」或者不写。财报
//! 长文在 #743 下 56 条错说法里 38 条、64 条读不通里 43 条出自这里。
//!
//! 这里只看结构，不认任何词：跨列格、空列、只含符号的格、只有一格的行、行标签的左内
//! 边距、格子里有没有数字和字母。做四件事：标题行提成表前的说明句；多行列头按列拼成
//! 一行；只有标签没有值的行当成小节，折进后面每行的标签里（「Current assets ›
//! Accounts receivable, net」）；"$"、")" 这类符号格并回相邻的数字格。渲染出的表交给
//! `blocks` / `chunker`，它们本来就让说明句和表头跟着每一块走。
//!
//! 只处理最外层且不含内层表的表；套着的表留给 htmd 原路。
//!
//! docx、电子表格和 csv 的表不经过 DOM：解析器把格子收成网格交给 [`render_grid`]，之后的
//! 分类与渲染和 HTML 表一模一样。电子表格和 csv 的第一排（至少两格有字的那排）按惯例是
//! 列头，哪怕列头是年份这种数字。

use dom_query::{Document, Selection};

const PLACEHOLDER: &str = "UTOPIATABLE";
const PATH_SEP: &str = " › ";

/// 一格：文字、跨了几列、左内边距（pt，取整）、在网格里的起始列。
#[derive(Clone, Debug, Default)]
struct Cell {
    text: String,
    span: usize,
    pad: u32,
    col: usize,
}

#[derive(Clone, Debug)]
struct Row {
    cells: Vec<Cell>,
    /// 网格总宽（含跨列）
    width: usize,
}

impl Row {
    /// 覆盖第 k 列的那个非空格
    fn covering(&self, k: usize) -> Option<&Cell> {
        self.cells
            .iter()
            .find(|c| !c.text.is_empty() && c.col <= k && k < c.col + c.span)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Skip,
    Caption,
    Header,
    Section,
    Data,
}

/// 把最外层且不含内层表的表换成占位段落；返回改过的 HTML 和每个占位对应的 Markdown。
pub fn lift_tables(html: &str) -> (String, Vec<String>) {
    let doc = Document::from(html);
    let mut rendered: Vec<String> = Vec::new();
    let mut targets: Vec<Selection<'_>> = Vec::new();
    // 上一张表的列头：分页符把一张报表劈成两个 <table> 时，后半张没有表头，
    // 列数又一样，就是同一张表的续表，接着用前一张的列头
    let mut last_headers: Vec<String> = Vec::new();
    for tbl in doc.select("table").iter() {
        let nested_in = tbl.nodes().first().is_some_and(|n| {
            n.ancestors(None)
                .iter()
                .any(|a| a.node_name().as_deref() == Some("table"))
        });
        if nested_in || tbl.select("table").exists() {
            continue;
        }
        if let Some((md, headers)) = render_table(&tbl, &last_headers) {
            rendered.push(md);
            targets.push(tbl);
            last_headers = headers;
        }
    }
    if rendered.is_empty() {
        return (html.to_string(), rendered);
    }
    for (i, tbl) in targets.iter().enumerate() {
        tbl.replace_with_html(format!("<p>{PLACEHOLDER}{i}</p>"));
    }
    (doc.html().to_string(), rendered)
}

/// 把 htmd 转出的 Markdown 里的占位行换回渲染好的表。
pub fn restore_tables(markdown: &str, tables: &[String]) -> String {
    if tables.is_empty() {
        return markdown.to_string();
    }
    let extra: usize = tables.iter().map(String::len).sum();
    let mut out = String::with_capacity(markdown.len() + extra);
    for line in markdown.lines() {
        let hit = line
            .trim()
            .strip_prefix(PLACEHOLDER)
            .and_then(|rest| rest.parse::<usize>().ok())
            .and_then(|i| tables.get(i));
        match hit {
            Some(md) => {
                out.push('\n');
                out.push_str(md);
                out.push('\n');
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

fn hidden(style: &str) -> bool {
    let s: String = style.chars().filter(|c| !c.is_whitespace()).collect();
    s.to_ascii_lowercase().contains("display:none")
}

/// 左内边距：`padding-left: 12pt`、`text-indent: 6pt`，或 `padding: a b c d` 的第四个值。
fn padding_left(style: &str) -> u32 {
    let lower = style.to_ascii_lowercase();
    let number_at = |s: &str| -> Option<f32> {
        let s = s.trim_start();
        let end = s
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(s.len());
        s[..end].parse::<f32>().ok()
    };
    for key in ["padding-left:", "text-indent:"] {
        if let Some(i) = lower.find(key) {
            if let Some(v) = number_at(&lower[i + key.len()..]) {
                return v.round() as u32;
            }
        }
    }
    if let Some(i) = lower.find("padding:") {
        let rest = lower[i + "padding:".len()..]
            .split(';')
            .next()
            .unwrap_or("");
        let parts: Vec<&str> = rest.split_whitespace().collect();
        let left = match parts.len() {
            4 => parts.get(3),
            2 | 3 => parts.get(1),
            1 => parts.first(),
            _ => None,
        };
        if let Some(v) = left.and_then(|p| number_at(p)) {
            return v.round() as u32;
        }
    }
    0
}

fn clean(text: &str) -> String {
    text.replace('\u{a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn attr_usize(cell: &Selection<'_>, name: &str) -> usize {
    cell.attr(name)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .max(1)
}

fn is_numeric(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_digit()) && !text.chars().any(char::is_alphabetic)
}

/// 该并回邻格的符号格：货币符号和左括号贴到后一格前面，右括号和百分号贴到前一格后面。
/// 破折号那类「空值」占位符不算——它自己就是这一格的内容，并过去会把整行的值挪位
fn merge_direction(text: &str) -> Option<bool> {
    let mut chars = text.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if first == '(' || is_currency(first) {
        Some(true)
    } else if first == ')' || first == '%' {
        Some(false)
    } else {
        None
    }
}

fn is_currency(c: char) -> bool {
    matches!(c, '$' | '€' | '£' | '¥' | '₩' | '₹' | '¢' | '￥')
}

/// 读出网格：按 `tr` 逐行，跨行的格顺着往下带。
fn grid(tbl: &Selection<'_>) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // (列, 格, 还剩几行)
    let mut carry: Vec<(usize, Cell, usize)> = Vec::new();
    for tr in tbl.select("tr").iter() {
        let mut cells: Vec<Cell> = Vec::new();
        let mut col = 0usize;
        for cell in tr.select("td, th").iter() {
            let style = cell
                .attr("style")
                .map(|s| s.to_string())
                .unwrap_or_default();
            if hidden(&style) {
                continue;
            }
            place_carried(&mut col, &mut cells, &mut carry);
            let span = attr_usize(&cell, "colspan");
            let rowspan = attr_usize(&cell, "rowspan");
            let c = Cell {
                text: clean(&cell.text()),
                span,
                pad: padding_left(&style),
                col,
            };
            if rowspan > 1 {
                carry.push((col, c.clone(), rowspan - 1));
            }
            col += span;
            cells.push(c);
        }
        place_carried(&mut col, &mut cells, &mut carry);
        rows.push(Row { cells, width: col });
    }
    rows
}

fn place_carried(col: &mut usize, cells: &mut Vec<Cell>, carry: &mut Vec<(usize, Cell, usize)>) {
    while let Some(pos) = carry.iter().position(|(c, _, _)| *c == *col) {
        let (_, mut cell, left) = carry[pos].clone();
        cell.col = *col;
        *col += cell.span;
        cells.push(cell);
        if left <= 1 {
            carry.remove(pos);
        } else {
            carry[pos].2 = left - 1;
        }
    }
}

/// 一行是什么：只看非空格的个数、位置、跨度，和格子里有没有数字。
fn kind(row: &Row, headers_seen: bool, data_seen: bool, table_width: usize) -> Kind {
    let nonempty: Vec<&Cell> = row.cells.iter().filter(|c| !c.text.is_empty()).collect();
    let Some(first) = nonempty.first() else {
        return Kind::Skip;
    };
    if nonempty.len() == 1 {
        let wide = first.span >= 2 && first.span * 2 >= table_width.max(2);
        if is_numeric(&first.text) {
            return if data_seen { Kind::Data } else { Kind::Header };
        }
        if !headers_seen && !data_seen {
            // 表头和数据都还没来：靠左或跨宽的一格文字是标题，不靠左的是一根列头
            return if wide || first.col == 0 {
                Kind::Caption
            } else {
                Kind::Header
            };
        }
        return Kind::Section;
    }
    let label = !is_numeric(&first.text);
    let values_after = nonempty[1..].iter().any(|c| is_numeric(&c.text));
    // 有标签跟着数字的是数据行；最左一格就有内容的整排数字也是（年份列的表）。
    // 最左几格空着、全是数字的一排（"2026 | 2026"）是列头的第二行
    if values_after && (label || first.col == 0 || data_seen) {
        return Kind::Data;
    }
    if !data_seen {
        return Kind::Header;
    }
    if label {
        // 数据行之后又一排没有数字的格：当成一条小节
        return Kind::Section;
    }
    Kind::Data
}

/// 把只含符号的格并回邻格："$" 和 "(" 贴到后一个非空格前面，")" 和 "%" 贴到前一个后面。
fn merge_symbols(row: &mut Row) {
    let n = row.cells.len();
    for i in 0..n {
        let Some(opening) = merge_direction(&row.cells[i].text) else {
            continue;
        };
        let sym = row.cells[i].text.clone();
        let target = if opening {
            (i + 1..n).find(|&j| !row.cells[j].text.is_empty())
        } else {
            (0..i).rev().find(|&j| !row.cells[j].text.is_empty())
        };
        let Some(j) = target else { continue };
        if opening {
            row.cells[j].text = format!("{sym}{}", row.cells[j].text);
        } else {
            row.cells[j].text = format!("{}{sym}", row.cells[j].text);
        }
        row.cells[i].text.clear();
    }
}

fn escape(text: &str) -> String {
    text.replace('|', "\\|")
}

/// 缩进深度：先看标签格前面有几根空列（Workiva 报表用空格子缩进），再看左内边距
/// （另一些用 padding-left）。两者合成一个数，列在前、内边距在后
fn depth_of(cell: &Cell) -> u32 {
    cell.col as u32 * 1000 + cell.pad
}

/// 一张表 → 说明句（可选）+ 带真表头的 Markdown 表，连同它用的列头（给续表接着用）。
/// 表里什么都没有时返回 None。
fn render_table(tbl: &Selection<'_>, inherited: &[String]) -> Option<(String, Vec<String>)> {
    render_rows(grid(tbl), inherited, None)
}

/// 别的解析器用的入口：一行是若干 (文字, 跨几列, 左内边距 pt)。`first_is_header` 为真时，
/// 第一排至少两格有字的行按列头算（电子表格、csv 的惯例），其余行照常分类。
pub(crate) fn render_grid(
    rows: &[Vec<(String, usize, u32)>],
    first_is_header: bool,
) -> Option<String> {
    let rows: Vec<Row> = rows
        .iter()
        .map(|cells| {
            let mut col = 0usize;
            let mut out = Vec::with_capacity(cells.len());
            for (text, span, pad) in cells {
                let span = (*span).max(1);
                out.push(Cell {
                    text: clean(text),
                    span,
                    pad: *pad,
                    col,
                });
                col += span;
            }
            Row {
                cells: out,
                width: col,
            }
        })
        .collect();
    let forced = first_is_header
        .then(|| {
            rows.iter()
                .position(|r| r.cells.iter().filter(|c| !c.text.is_empty()).count() >= 2)
        })
        .flatten();
    render_rows(rows, &[], forced).map(|(md, _)| md)
}

fn render_rows(
    mut rows: Vec<Row>,
    inherited: &[String],
    forced_header: Option<usize>,
) -> Option<(String, Vec<String>)> {
    let width = rows.iter().map(|r| r.width).max().unwrap_or(0);
    if width == 0 {
        return None;
    }
    let mut captions: Vec<String> = Vec::new();
    let mut headers: Vec<Row> = Vec::new();
    // (小节深度, 小节名, 已经见过比它深的行)。
    // 一个小节被同深度的数据行结束，只在它已经有过更深的子行之后：资产负债表里
    // 「Current assets:」的子行缩进了，同深度的「Property and equipment」是下一项；
    // 现金流量表里「Cash flows from operating activities:」的子行没缩进，同深度的
    // 「Net income」就是它的第一行
    let mut sections: Vec<(u32, String, bool)> = Vec::new();
    // (标签路径, 该行)
    let mut data: Vec<(String, Row)> = Vec::new();
    let (mut headers_seen, mut data_seen) = (false, false);
    for (i, row) in rows.iter_mut().enumerate() {
        let filled = row.cells.iter().filter(|c| !c.text.is_empty()).count();
        let k = match forced_header {
            Some(h) if i == h => Kind::Header,
            // 列头之后的行是记录，哪怕一格数字都没有；只填了一个数字的行（序号列）也是记录，
            // 只填了一个词的行才按小节算
            Some(h)
                if i > h
                    && (filled >= 2
                        || row
                            .cells
                            .iter()
                            .find(|c| !c.text.is_empty())
                            .is_some_and(|c| is_numeric(&c.text))) =>
            {
                Kind::Data
            }
            _ => kind(row, headers_seen, data_seen, width),
        };
        match k {
            Kind::Skip => {}
            Kind::Caption => {
                if let Some(c) = row.cells.iter().find(|c| !c.text.is_empty()) {
                    captions.push(c.text.clone());
                }
            }
            Kind::Header => {
                merge_symbols(row);
                headers.push(row.clone());
                headers_seen = true;
            }
            Kind::Section => {
                let Some(cell) = row.cells.iter().find(|c| !c.text.is_empty()) else {
                    continue;
                };
                let depth = depth_of(cell);
                while sections.last().is_some_and(|(d, _, _)| *d >= depth) {
                    sections.pop();
                }
                sections.push((
                    depth,
                    cell.text.trim_end_matches([':', '：']).to_string(),
                    false,
                ));
            }
            Kind::Data => {
                merge_symbols(row);
                let label_cell = row.cells.iter().find(|c| !c.text.is_empty());
                let depth = label_cell.map(depth_of).unwrap_or(0);
                while sections
                    .last()
                    .is_some_and(|(d, _, deeper)| *d > depth || (*d == depth && *deeper))
                {
                    sections.pop();
                }
                if let Some(top) = sections.last_mut() {
                    if top.0 < depth {
                        top.2 = true;
                    }
                }
                let path: Vec<&str> = sections.iter().map(|(_, s, _)| s.as_str()).collect();
                data.push((path.join(PATH_SEP), row.clone()));
                data_seen = true;
            }
        }
    }
    // 全是列头没有一行数据的表不存在：一格数字都没有的表，第一排是列头，其余是数据行
    if data.is_empty() && headers.len() >= 2 {
        let rest = headers.split_off(1);
        data.extend(rest.into_iter().map(|r| (String::new(), r)));
    }
    if data.is_empty() && headers.is_empty() {
        return None;
    }

    // 标签列 = 第一根出现过数字的列左边的所有列；一个数字都没有的表，标签列只有最左那格。
    // 值列 = 标签列右边、数据行里出现过内容的列（表头有字但整列没数据的列不算）
    let label_end: usize = (0..width)
        .find(|&k| {
            data.iter()
                .any(|(_, r)| r.covering(k).is_some_and(|c| is_numeric(&c.text)))
        })
        .or_else(|| {
            data.iter()
                .filter_map(|(_, r)| {
                    r.cells
                        .iter()
                        .find(|c| !c.text.is_empty())
                        .map(|c| c.col + c.span)
                })
                .min()
        })
        .or_else(|| {
            headers
                .iter()
                .filter_map(|h| h.cells.iter().find(|c| !c.text.is_empty()).map(|c| c.col))
                .min()
        })
        .unwrap_or(0);
    let has_content = |k: usize| -> bool {
        if data.is_empty() {
            headers.iter().any(|h| h.covering(k).is_some())
        } else {
            data.iter().any(|(_, r)| r.covering(k).is_some())
        }
    };
    let value_cols: Vec<usize> = (label_end..width).filter(|&k| has_content(k)).collect();
    let value_cols: Vec<Vec<usize>> = collapse_spans(value_cols, &headers, &data);
    // 一根值列是一组网格列：取这一行落在这组里的第一个非空格
    let at = |r: &Row, group: &[usize]| -> String {
        group
            .iter()
            .find_map(|&k| r.covering(k))
            .map(|c| c.text.clone())
            .unwrap_or_default()
    };
    // 一行的标签：标签列里的格子按出现顺序拼起来
    let own_label = |r: &Row| -> String {
        let mut parts: Vec<&str> = Vec::new();
        for c in r
            .cells
            .iter()
            .filter(|c| !c.text.is_empty() && c.col < label_end)
        {
            if parts.last() != Some(&c.text.as_str()) {
                parts.push(c.text.as_str());
            }
        }
        parts.join(" ")
    };
    let column_header = |group: &[usize]| -> String {
        let mut parts: Vec<String> = Vec::new();
        for h in &headers {
            let t = at(h, group);
            if !t.is_empty() && parts.last() != Some(&t) {
                parts.push(t);
            }
        }
        parts.join(" ")
    };
    let mut header_texts: Vec<String> = value_cols.iter().map(|g| column_header(g)).collect();
    if headers.is_empty() && !data.is_empty() && inherited.len() == header_texts.len() {
        header_texts = inherited.to_vec();
    }
    // 标签列的表头：只算整格都落在标签列里的表头格，跨进值列的是值列的头
    let label_header: String = {
        let mut parts: Vec<String> = Vec::new();
        for h in &headers {
            for c in h
                .cells
                .iter()
                .filter(|c| !c.text.is_empty() && c.col + c.span <= label_end)
            {
                if parts.last() != Some(&c.text) {
                    parts.push(c.text.clone());
                }
            }
        }
        parts.join(" ")
    };

    let mut lines: Vec<String> = Vec::new();
    if !captions.is_empty() {
        let mut caption = captions.join(" · ");
        if !caption.ends_with(':') && !caption.ends_with('：') {
            caption.push(':');
        }
        lines.push(caption);
        lines.push(String::new());
    }
    // 最左一列本身就是数字（年份列的表）：没有标签列，行就是它的值
    let has_label_col = label_end > 0;
    let ncols = usize::from(has_label_col) + value_cols.len().max(1);
    let header_cells: Vec<String> = has_label_col
        .then(|| escape(&label_header))
        .into_iter()
        .chain(header_texts.iter().map(|h| escape(h)))
        .chain(std::iter::repeat(String::new()))
        .take(ncols)
        .collect();
    lines.push(format!("| {} |", header_cells.join(" | ")));
    lines.push(format!("|{}", " --- |".repeat(ncols)));
    for (sections, r) in &data {
        let mut cells: Vec<String> = Vec::new();
        if has_label_col {
            let own = own_label(r);
            let label = match (sections.is_empty(), own.is_empty()) {
                (true, _) => own,
                (false, true) => sections.clone(),
                (false, false) => format!("{sections}{PATH_SEP}{own}"),
            };
            cells.push(escape(&label));
        }
        cells.extend(value_cols.iter().map(|g| escape(&at(r, g))));
        while cells.len() < ncols {
            cells.push(String::new());
        }
        lines.push(format!("| {} |", cells.join(" | ")));
    }
    Some((lines.join("\n"), header_texts))
}

/// 跨列的格会让同一个值落在好几根网格列上，"$" 又常常单独占一格：相邻两列在每一行
/// 上要么由同一格覆盖、要么至少一边是空的，就是同一根值列，并成一组。
fn collapse_spans(cols: Vec<usize>, headers: &[Row], data: &[(String, Row)]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    for k in cols {
        let same_as_prev = out.last().and_then(|g| g.last().copied()).is_some_and(|p| {
            headers.iter().chain(data.iter().map(|(_, r)| r)).all(|r| {
                match (r.covering(p), r.covering(k)) {
                    (Some(a), Some(b)) => a.col == b.col,
                    _ => true,
                }
            })
        });
        match out.last_mut() {
            Some(g) if same_as_prev => g.push(k),
            _ => out.push(vec![k]),
        }
    }
    out
}

/// 一行切成格：每格的文字，以及它在这一行里的字节偏移。行首行尾的空段不算格。
/// **带着偏移**，因为表头里两列同名是常事（两个「Amount」），光按文字回头找会指错格
fn cells(row: &str) -> Vec<(&str, usize)> {
    let lead = row.len() - row.trim_start().len();
    let trimmed = row.trim();
    let (inner, open) = match trimmed.strip_prefix('|') {
        Some(rest) => (rest, 1),
        None => (trimmed, 0),
    };
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let mut at = lead + open;
    let mut out = Vec::new();
    for piece in inner.split('|') {
        let pad = piece.len() - piece.trim_start().len();
        out.push((piece.trim(), at + pad));
        at += piece.len() + 1;
    }
    out
}

/// 比较一格与一个值：空白、千分位逗号、货币符号不算数，别的一个字都不能差。
/// **不做包含**：一格写着 `$1,234`，值是 `234` 的那一条不该认领它
fn same_number(cell: &str, value: &str) -> bool {
    let strip = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace() && !is_currency(*c) && *c != ',')
            .collect::<String>()
    };
    let (a, b) = (strip(cell), strip(value));
    !a.is_empty() && a == b
}

/// 这个值落在这一行的哪一列，以及那一列的表头是什么（#729：财务表格单元的期间列）。
///
/// 一张财务表的列头写着期间（「Three Months Ended July 26, 2026」），值写在行里。**人是
/// 按列读表的，抽取却是按行读的**：一行五个数进了图，五条陈述除了值一模一样，没有任何
/// 东西说得出哪个数属于哪个期间。这里把那一列的表头找回来，交给时间解析去判它是不是一个
/// 期间——代码不认「Months Ended」这类字眼，只认位置。
///
/// 它宁可什么都不给，也不给错的：
///
/// * 这一行在这一块里出现不止一次——说不清是哪张表的哪一行；
/// * 这一行里没有恰好一格等于这个值——两格相等就分不出是哪一列；
/// * 这一行上面不是 `| --- |` 分隔线，或分隔线上面不是表头行；
/// * 表头那一格是空的，或一个数字都没有（「Amount」不是期间，而任何期间都会写出年份、
///   日期或季度的数字）。
///
/// 返回表头那一格的文字和它在这一块里的字节偏移——一条提及要说得出自己在原文的哪个位置。
pub fn column_header<'a>(chunk: &'a str, row: &str, value: &str) -> Option<(&'a str, usize)> {
    let row = row.trim();
    if !row.starts_with('|') {
        return None;
    }
    let at = chunk.find(row)?;
    if chunk[at + row.len()..].contains(row) {
        return None;
    }
    let mut hit = cells(row)
        .into_iter()
        .enumerate()
        .filter(|(_, (c, _))| same_number(c, value));
    let (col, _) = hit.next()?;
    if hit.next().is_some() {
        return None;
    }
    // 紧挨着这一行往上：一条分隔线，再上去一行是表头。位置用 rfind 从原文里量，
    // 不靠换行符占几个字节
    let before = &chunk[..at];
    let mut lines = before.lines().rev();
    let sep = lines.next()?;
    if !sep.trim_start().starts_with("| ---") {
        return None;
    }
    let head = lines.next()?;
    if !head.trim_start().starts_with('|') {
        return None;
    }
    let sep_at = before.rfind(sep)?;
    let head_at = before[..sep_at].rfind(head)?;
    let (cell, in_line) = *cells(head).get(col)?;
    if cell.is_empty() || !cell.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let from = head_at + in_line;
    Some((&chunk[from..from + cell.len()], from))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #729：一行五个数，每个数的期间在它那一列的表头上
    const COSTS: &str = "(A) Acquisition-related and other costs are comprised of amortization.\n\n\
|  | Three Months Ended July 26, 2026 | Three Months Ended April 26, 2026 | Six Months Ended July 26, 2026 |\n\
| --- | --- | --- | --- |\n\
| Cost of revenue | $46 | $47 | $93 |\n\
| Sales, general and administrative | $6 | $5 | $11 |\n";

    #[test]
    fn a_cell_takes_the_period_of_its_own_column() {
        let row = "| Cost of revenue | $46 | $47 | $93 |";
        let (head, at) = column_header(COSTS, row, "$46").expect("46 是第一列的值");
        assert_eq!(head, "Three Months Ended July 26, 2026");
        assert_eq!(&COSTS[at..at + head.len()], head, "偏移指回原文那一格");
        assert_eq!(
            column_header(COSTS, row, "$47").unwrap().0,
            "Three Months Ended April 26, 2026",
            "同一行的下一个数换了一列，期间也跟着换"
        );
        assert_eq!(
            column_header(COSTS, row, "93").unwrap().0,
            "Six Months Ended July 26, 2026",
            "货币符号与千分位不算数"
        );
    }

    #[test]
    fn a_header_that_names_no_time_gives_nothing() {
        let t = "| Item | Amount |\n| --- | --- |\n| Cost of revenue | $46 |\n";
        assert_eq!(column_header(t, "| Cost of revenue | $46 |", "$46"), None);
    }

    #[test]
    fn an_ambiguous_cell_gives_nothing() {
        // 同一行两格相同：说不出是哪一列
        let t = "|  | Q1 2026 | Q2 2026 |\n| --- | --- | --- |\n| Cost | $46 | $46 |\n";
        assert_eq!(column_header(t, "| Cost | $46 | $46 |", "$46"), None);
        // 同一块里两张表有一模一样的行：说不出是哪张表
        let twice = format!("{COSTS}\n{COSTS}");
        assert_eq!(
            column_header(&twice, "| Cost of revenue | $46 | $47 | $93 |", "$46"),
            None
        );
    }

    #[test]
    fn a_row_with_no_table_above_it_gives_nothing() {
        // 没有分隔线就没有表头
        let t = "| Cost of revenue | $46 |\n";
        assert_eq!(column_header(t, "| Cost of revenue | $46 |", "$46"), None);
        // 引文不是一行表格
        assert_eq!(
            column_header(COSTS, "Acquisition-related and other costs", "$46"),
            None
        );
        // 一格写着 $1,234，值是 234 的不该认领它
        let t2 = "|  | Q1 2026 |\n| --- | --- |\n| Cost | $1,234 |\n";
        assert_eq!(column_header(t2, "| Cost | $1,234 |", "234"), None);
        assert_eq!(
            column_header(t2, "| Cost | $1,234 |", "1234").unwrap().0,
            "Q1 2026"
        );
    }

    fn render(html: &str) -> String {
        let (_, tables) = lift_tables(html);
        tables.join("\n=====\n")
    }

    #[test]
    fn a_filing_table_gets_its_real_headers_and_its_section_path() {
        let html = r#"<table>
          <tr><td style="width:0.1%"></td><td style="width:40%"></td><td style="width:0.1%"></td><td style="width:1%"></td><td style="width:20%"></td><td style="width:1%"></td><td style="width:1%"></td><td style="width:20%"></td><td style="width:0.1%"></td><td colspan="3" style="display:none"></td></tr>
          <tr><td colspan="9" style="text-align:center">NVIDIA CORPORATION</td><td colspan="3" style="display:none"></td></tr>
          <tr><td colspan="9" style="text-align:center">CONDENSED CONSOLIDATED BALANCE SHEETS</td></tr>
          <tr><td colspan="9" style="text-align:center">(In millions)</td></tr>
          <tr><td colspan="3"></td><td colspan="3" style="text-align:center">July 26,</td><td colspan="3" style="text-align:center">January 25,</td></tr>
          <tr><td colspan="3"></td><td colspan="3" style="text-align:center">2026</td><td colspan="3" style="text-align:center">2026</td></tr>
          <tr><td colspan="3" style="padding:2pt 1pt 2pt 6pt">Current assets:</td><td colspan="3"></td><td colspan="3"></td></tr>
          <tr><td colspan="3" style="padding:2pt 1pt 2pt 12pt">Accounts receivable, net</td><td>$</td><td>63,059</td><td></td><td>$</td><td>38,466</td><td></td></tr>
          <tr><td colspan="3" style="padding:2pt 1pt 2pt 12pt">Inventories</td><td></td><td>31,575</td><td></td><td></td><td>21,403</td><td></td></tr>
          <tr><td colspan="3" style="padding:2pt 1pt 2pt 6pt">Total current assets</td><td></td><td>197,412</td><td></td><td></td><td>125,605</td><td></td></tr>
        </table>"#;
        let md = render(html);
        assert!(
            md.starts_with(
                "NVIDIA CORPORATION · CONDENSED CONSOLIDATED BALANCE SHEETS · (In millions):\n"
            ),
            "标题行提成表前的说明句: {md}"
        );
        assert!(
            md.contains("|  | July 26, 2026 | January 25, 2026 |"),
            "两行列头拼成一行: {md}"
        );
        assert!(
            md.contains("| Current assets › Accounts receivable, net | $63,059 | $38,466 |"),
            "小节折进标签、美元符并回数字: {md}"
        );
        assert!(
            md.contains("| Current assets › Inventories | 31,575 | 21,403 |"),
            "{md}"
        );
        assert!(
            md.contains("| Total current assets | 197,412 | 125,605 |"),
            "合计行回到小节外: {md}"
        );
        assert!(!md.contains("Current assets:"), "小节行自己不再出现: {md}");
    }

    #[test]
    fn a_lone_label_before_everything_is_the_caption_and_rows_keep_their_values() {
        let html = "<table><tr><td colspan=\"2\">a. Tench Coxe</td></tr>\
                    <tr><td>Number of shares For</td><td>15,411,252,412</td></tr>\
                    <tr><td>Number of shares Against</td><td>1,399,727</td></tr></table>";
        let md = render(html);
        assert!(md.starts_with("a. Tench Coxe:\n"), "{md}");
        assert!(
            md.contains("| Number of shares For | 15,411,252,412 |"),
            "{md}"
        );
        assert!(
            md.contains("| Number of shares Against | 1,399,727 |"),
            "{md}"
        );
    }

    #[test]
    fn a_first_row_of_words_is_the_header_when_the_table_has_none() {
        let html = "<table><tr><td>Revenue</td><td>Q2 FY27</td><td>Q1 FY27</td></tr>\
                    <tr><td>total</td><td>$96,221</td><td>$81,615</td></tr></table>";
        let md = render(html);
        assert!(md.contains("| Revenue | Q2 FY27 | Q1 FY27 |"), "{md}");
        assert!(md.contains("| total | $96,221 | $81,615 |"), "{md}");
    }

    #[test]
    fn hidden_cells_and_empty_columns_do_not_survive() {
        let html = "<table><tr><td>Margin</td><td style=\"display:none\">ghost</td><td></td><td>75.0</td><td></td></tr></table>";
        let md = render(html);
        assert!(!md.contains("ghost"), "{md}");
        assert!(md.contains("| Margin | 75.0 |"), "{md}");
    }

    #[test]
    fn a_closing_paren_in_its_own_cell_rejoins_the_number() {
        let html =
            "<table><tr><td>Other</td><td>(5,497</td><td>)</td><td>387</td><td></td></tr></table>";
        let md = render(html);
        assert!(md.contains("| Other | (5,497) | 387 |"), "{md}");
    }

    #[test]
    fn a_dash_that_means_nothing_stays_in_its_own_cell() {
        let html =
            "<table><tr><td>Other</td><td>112</td><td>—</td><td>31</td><td>—</td></tr></table>";
        let md = render(html);
        assert!(md.contains("| Other | 112 | — | 31 | — |"), "{md}");
    }

    #[test]
    fn a_headerless_table_after_a_page_break_takes_the_headers_before_it() {
        let html = "<table>\
            <tr><td></td><td>Q2 FY27</td><td>Q1 FY27</td></tr>\
            <tr><td>Revenue</td><td>96,221</td><td>81,615</td></tr></table>\
            <hr>\
            <table><tr><td colspan=\"3\">Cash flows from financing activities:</td></tr>\
            <tr><td>Dividends paid</td><td>(6,047)</td><td>(244)</td></tr></table>";
        let (_, tables) = lift_tables(html);
        assert_eq!(tables.len(), 2, "{tables:?}");
        assert!(
            tables[1].starts_with("Cash flows from financing activities:\n"),
            "{}",
            tables[1]
        );
        assert!(
            tables[1].contains("|  | Q2 FY27 | Q1 FY27 |"),
            "续表接着用前一张的列头: {}",
            tables[1]
        );
        assert!(
            tables[1].contains("| Dividends paid | (6,047) | (244) |"),
            "{}",
            tables[1]
        );
    }

    #[test]
    fn nested_tables_are_left_to_the_markdown_converter() {
        let html =
            "<table><tr><td><table><tr><td>inner</td><td>1</td></tr></table></td></tr></table>";
        let (out, tables) = lift_tables(html);
        assert!(tables.is_empty(), "{tables:?}");
        assert_eq!(out, html);
    }

    #[test]
    fn placeholders_come_back_as_tables() {
        let md = "before\n\nUTOPIATABLE0\n\nafter";
        let table = "cap:\n\n|  | a |\n| --- | --- |\n| x | 1 |".to_string();
        let out = restore_tables(md, &[table]);
        assert!(
            out.contains("before\n\n\ncap:\n\n|  | a |\n| --- | --- |\n| x | 1 |\n\n\nafter"),
            "{out}"
        );
    }
}
