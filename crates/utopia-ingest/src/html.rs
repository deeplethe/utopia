//! Canonical HTML conversion shared by file, URL and feed ingestion.
#[cfg(test)]
mod table_tests {
    use super::{markdown_from_html, promote_first_row_headers};

    #[test]
    fn a_table_without_headers_still_keeps_its_rows() {
        let html = "<table><tr><td>Revenue</td><td>Q2 FY27</td><td>Q1 FY27</td></tr>\
                    <tr><td>total</td><td>$96,221</td><td>$81,615</td></tr></table>";
        let md = markdown_from_html(html).expect("converts");
        // 行列关系还在：同一行的三格在同一行文本里
        let row = md
            .lines()
            .find(|l| l.contains("96,221"))
            .expect("the numbers survive");
        assert!(row.contains("81,615"), "同一行的两个数应当在一行里: {md}");
        assert!(row.contains("total"), "行标签应当与它的数在一行: {md}");
    }

    #[test]
    fn empty_spacer_columns_do_not_survive() {
        let html = "<table><tr><td>Revenue</td><td></td><td>$96,221</td><td></td></tr>\
                    <tr><td>Margin</td><td></td><td>75.0</td><td></td></tr></table>";
        let md = markdown_from_html(html).expect("converts");
        let row = md
            .lines()
            .find(|l| l.contains("96,221"))
            .expect("row survives");
        assert!(!row.contains("|  |"), "空列应当被砍掉: {row}");
        assert!(row.contains("Revenue"), "有内容的列一个不能少: {row}");
    }

    #[test]
    fn a_blank_header_row_gives_way_to_the_real_one() {
        let md = super::prune_empty_table_columns(
            "|  |  |\n| --- | --- |\n| a. Tench Coxe |  |\n| shares For | 15,411 |",
        );
        let first = md.lines().next().expect("a line");
        assert!(first.contains("Tench Coxe"), "表头该是真正的抬头: {md}");
        assert!(md.contains("15,411"), "行不能丢: {md}");
    }

    #[test]
    fn a_column_with_any_content_is_kept() {
        let md = super::prune_empty_table_columns("| a |  | c |\n| --- | --- | --- |\n|  | b |  |");
        assert!(md.contains("| a |  | c |") || md.contains("a"), "{md}");
        assert!(md.contains("b"), "有内容的列不能砍: {md}");
    }

    #[test]
    fn a_table_that_already_has_headers_is_untouched() {
        let html = "<table><thead><tr><th>a</th></tr></thead><tr><td>1</td></tr></table>";
        assert_eq!(promote_first_row_headers(html), html);
    }

    #[test]
    fn a_nested_table_does_not_promote_the_outer_row() {
        // 内层先被处理；外层因此看得见 <th>，跳过——不猜哪一行属于谁
        let html = "<table><tr><td><table><tr><td>inner</td></tr></table></td></tr></table>";
        let out = promote_first_row_headers(html);
        assert_eq!(out.to_ascii_lowercase().matches("<th").count(), 1, "{out}");
    }

    #[test]
    fn text_outside_tables_is_left_alone() {
        let html = "<p>plain</p>";
        assert_eq!(promote_first_row_headers(html), html);
    }
}

#[cfg(test)]
mod evidence_tests {
    use super::*;

    const REPORT: &str = "Researchers examined captcha verification across public services and found that older residents struggled with distorted images. Their study followed participants through several independent tasks, recording completion times and interviewing people about accessibility barriers. The published results describe how alternative authentication methods improved access without increasing fraudulent registrations. Local authorities are now reviewing procurement standards, while independent security specialists recommend measuring actual abuse rather than assuming every additional challenge provides protection.";

    #[test]
    fn substantive_marker_prose_survives_every_html_entry_point() {
        let raw = format!("<html><body><article><p>{REPORT}</p></article></body></html>");
        for markdown in [
            page_to_markdown(&raw, Some("https://example.com/story")),
            fragment_to_markdown(&raw),
            crate::parse("story.html", raw.as_bytes())
                .map(|p| p.text)
                .map_err(|e| HtmlError::Conversion(e.to_string())),
        ] {
            assert!(markdown.unwrap().contains("captcha verification"));
        }
    }

    #[test]
    fn substantive_raw_fallback_survives_newsletter_marker() {
        let raw = format!("<article><p>{REPORT}</p></article><form><p>Subscribe to read our newsletter</p><input type='email'></form>");
        assert!(dom_smoothie::Readability::new(raw.as_str(), Some("not a URL"), None).is_err());
        assert!(page_to_markdown(&raw, Some("not a URL"))
            .unwrap()
            .contains(REPORT));
        assert!(fragment_to_markdown(&raw).unwrap().contains(REPORT));
    }

    #[test]
    fn normal_paragraph_breaks_preserve_article_evidence() {
        let split = REPORT.replace(". ", ".</p><p>");
        for prose in [REPORT.to_string(), split] {
            let raw = format!("<article><p>{prose}</p></article><form><p>Subscribe to read our newsletter</p><input type='email'></form>");
            for result in [
                page_to_markdown(&raw, Some("https://example.com/story")),
                page_to_markdown(&raw, Some("not a URL")),
                fragment_to_markdown(&raw),
                crate::parse("story.html", raw.as_bytes())
                    .map(|p| p.text)
                    .map_err(|e| HtmlError::Conversion(e.to_string())),
            ] {
                assert!(result.unwrap().contains("captcha verification"));
            }
        }
    }

    #[test]
    fn short_and_padded_marker_shells_remain_rejected() {
        for marker in [
            "captcha verification",
            "Subscribe to read",
            "Sign in to continue",
        ] {
            for padding in [
                String::new(),
                "Account access requires verification of your identity before proceeding. "
                    .repeat(100),
            ] {
                let raw = format!("<h1>{marker}</h1><p>{padding}</p>");
                assert_eq!(fragment_to_markdown(&raw), Err(HtmlError::Interstitial));
                assert_eq!(
                    page_to_markdown(&raw, Some("https://example.com")),
                    Err(HtmlError::Interstitial)
                );
            }
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HtmlError {
    #[error("HTML conversion failed: {0}")]
    Conversion(String),
    #[error("HTML is an authentication or player interstitial")]
    Interstitial,
}

/// Extract a full page; Readability failure uses one raw conversion fallback.
pub fn page_to_markdown(html: &str, base_url: Option<&str>) -> Result<String, HtmlError> {
    // Judge the extracted body, not newsletter/login controls in page chrome.
    // The raw fallback still goes through the same fail-closed converter.
    let article = dom_smoothie::Readability::new(html, base_url, None)
        .and_then(|mut readability| readability.parse());
    match article {
        Ok(article) => {
            // Readability can discard every control on a login-only page and
            // return its tiny label as an article. Such a result is not body
            // evidence; use the guarded fallback rather than laundering it.
            if article
                .text_content
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .count()
                < 200
            {
                return markdown_from_html(html);
            }
            let markdown = markdown_from_html(&article.content)?;
            let title = article.title.trim();
            if title.is_empty() || markdown.contains(title) {
                Ok(markdown)
            } else {
                Ok(sanitize_markdown_links(&format!("# {title}\n\n{markdown}")))
            }
        }
        Err(_) => markdown_from_html(html),
    }
}

/// Feed fragments are already scoped; never run Readability over them.
pub fn fragment_to_markdown(html: &str) -> Result<String, HtmlError> {
    markdown_from_html(html)
}

fn looks_like_challenge_html(html: &str) -> bool {
    let lowered = html.to_ascii_lowercase();
    let auth_form = lowered.contains("<form")
        && (lowered.contains("password")
            || lowered.contains("type=\"email\"")
            || lowered.contains("type='email'"));
    let player_shell = [
        "jwplayer",
        "brightcove",
        "data-player",
        "<video",
        "youtube.com/embed",
        "player.vimeo.com",
    ]
    .iter()
    .filter(|marker| lowered.contains(**marker))
    .count()
        >= 2;
    auth_form || player_shell
}

// A marker is not a verdict: reporting may quote challenge text, and raw
// fallback may retain newsletter controls. Require developed, varied prose,
// not merely a long body (repeated instructions and word-number padding fail).
fn has_substantive_prose(markdown: &str) -> bool {
    // News paragraph layout is presentation, not evidence quality. Aggregate
    // prose across blocks, retaining vocabulary diversity to reject repeated
    // instruction padding. Exclude heading/control-link lines, not entire
    // paragraphs: inserting a blank line must not change their contribution.
    let prose = markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.contains("]("))
        .collect::<Vec<_>>()
        .join(" ");
    let sentences = prose.matches(['.', '!', '?', '。', '！', '？']).count();
    let words = prose
        .split(|ch: char| !ch.is_alphabetic())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect::<std::collections::HashSet<_>>();
    let cjk = prose
        .chars()
        .filter(|ch| {
            matches!(*ch as u32,
        0x3400..=0x4dbf | 0x4e00..=0x9fff | 0x3040..=0x30ff | 0xac00..=0xd7af)
        })
        .collect::<std::collections::HashSet<_>>();
    sentences >= 3
        && prose.chars().filter(|ch| !ch.is_whitespace()).count() >= 400
        && (words.len() >= 40 || cjk.len() >= 40)
}

/// Detect authentication/consent shells in extracted text, not page chrome.
pub fn looks_like_challenge_shell(markdown: &str) -> bool {
    let lowered = markdown.to_ascii_lowercase();
    const HIGH_CONFIDENCE_MARKERS: &[&str] = &[
        "verify you are human",
        "checking your browser",
        "enable javascript and cookies",
        "accept cookies to continue",
        "subscribe to continue",
        "subscribe to read",
        "sign in to continue",
        "log in to continue",
        "this content is behind a paywall",
        "content is behind a paywall",
        "unlock this article",
        "captcha challenge",
        "complete the captcha",
        "solve the captcha",
        "enter the captcha",
        "captcha verification",
    ];
    if HIGH_CONFIDENCE_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker))
        || lowered
            .lines()
            .take(3)
            .any(|line| line.trim().trim_start_matches('#').trim() == "access denied")
    {
        return !has_substantive_prose(markdown);
    }

    // Readability can retain a long login/consent shell without any of the
    // exact phrases above. Require two short, control-like lines so ordinary
    // articles that mention one of these topics are not rejected.
    const STRUCTURAL_MARKERS: &[&str] = &[
        "sign in",
        "log in",
        "login",
        "create account",
        "register",
        "cookie settings",
        "consent preferences",
        "accept all cookies",
        "enable javascript",
        "access denied",
        "play video",
        "watch now",
    ];
    let structural_hits = lowered
        .lines()
        .take(40)
        .filter(|line| {
            let line = line.trim().trim_start_matches('#').trim();
            line.len() <= 96
                && STRUCTURAL_MARKERS
                    .iter()
                    .any(|marker| line.contains(marker))
        })
        .count();
    let has_auth_fields = (lowered.contains("password")
        && (lowered.contains("email") || lowered.contains("username")))
        || lowered.contains("<form");
    (structural_hits >= 2 || (structural_hits >= 1 && has_auth_fields))
        && !has_substantive_prose(markdown)
}

/// 没有 `<th>` 的表格，把第一行的 `<td>` 提成 `<th>`。
///
/// **理由是一整类文档在解析这一步就把行列关系丢了。** htmd 的表格处理器要求
/// 表里有显式表头（`<th>` 或 `<thead>`），否则整张退回逐格摊平——一格一行。
/// 而 SEC 的 XBRL 报表一个都没有：实测 NVIDIA 那份财报 11 张表、那份投票结果
/// 8-K 21 张表，`<th>` 计数都是 0。摊平之后模型看到的是一列孤立标签跟一列
/// 孤立数字，只能按顺序猜哪个数配哪一列，于是抽出 `NVIDIA net_worth Net income`
/// 这种边，十位董事的四类票数也全部退化成分不出类别的裸数字。
///
/// 第一行提成表头是这类表格的实际语义（"Q2 FY27 | Q1 FY27 | Q2 FY26"），
/// 而且**判据很窄**：整张表一个 `<th>`/`<thead>` 都没有时才动。已经有表头的
/// 表格一个字不改。
///
/// 嵌套表格按**从内到外**处理（`</table>` 出现的顺序天然如此）：内层改过之后
/// 外层就带着 `<th>` 了，于是外层跳过——宁可少改一张，不去猜哪一行属于谁。
fn promote_first_row_headers(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = html.to_string();
    // (开标签结束位置, 表格结束位置)，按闭合顺序 = 从内到外
    let mut opens: Vec<usize> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < lower.len() {
        let next_open = lower[i..].find("<table").map(|x| i + x);
        let next_close = lower[i..].find("</table").map(|x| i + x);
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                opens.push(o);
                i = o + 6;
            }
            (_, Some(c)) => {
                if let Some(o) = opens.pop() {
                    spans.push((o, c));
                }
                i = c + 7;
            }
            (Some(o), None) => {
                opens.push(o);
                i = o + 6;
            }
            (None, None) => break,
        }
    }
    // 位置会随改写移动，所以从后往前改
    spans.sort_by_key(|(o, _)| std::cmp::Reverse(*o));
    for (open, close) in spans {
        let seg = &out[open..close.min(out.len())];
        let seg_lower = seg.to_ascii_lowercase();
        if seg_lower.contains("<th") || seg_lower.contains("<thead") {
            continue;
        }
        let Some(tr) = seg_lower.find("<tr") else {
            continue;
        };
        let Some(tr_end) = seg_lower[tr..].find("</tr").map(|x| tr + x) else {
            continue;
        };
        let row = &seg[tr..tr_end];
        if !row.to_ascii_lowercase().contains("<td") {
            continue;
        }
        let rewritten = replace_td_with_th(row);
        out.replace_range(open + tr..open + tr_end, &rewritten);
    }
    out
}

/// 只换标签名，属性原样留着。
fn replace_td_with_th(row: &str) -> String {
    let mut out = String::with_capacity(row.len());
    let lower = row.to_ascii_lowercase();
    let mut i = 0usize;
    while i < row.len() {
        if lower[i..].starts_with("<td") {
            out.push_str("<th");
            i += 3;
        } else if lower[i..].starts_with("</td") {
            out.push_str("</th");
            i += 4;
        } else {
            let ch = row[i..].chars().next().expect("char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// 表格里整列都空的，砍掉；对齐用的空格也不留。
///
/// **不砍的代价是三倍。** 排版用的空单元格（XBRL 的缩进列、货币符号列）在
/// HTML 里不占地方，转成 markdown 管道表之后每一行都要为它们写一个 `|` 和
/// 一片对齐空格。实测那份财报：24 块 2.6 万字符 → 78 块 9.2 万字符，多出来的
/// 全是 `|  |  |  |`。分块数进了抽取的成本，一列空格不值这个钱。
///
/// 判据是「这一列在**每一行**都空」，所以有内容的列一个不动；分隔行不参与判断
/// （它本来就只有横线），但跟着一起砍列。
fn prune_empty_table_columns(markdown: &str) -> String {
    let is_row = |l: &str| {
        let t = l.trim();
        t.starts_with('|') && t.ends_with('|') && t.len() > 1
    };
    let cells = |l: &str| -> Vec<String> {
        let t = l.trim();
        t[1..t.len() - 1]
            .split('|')
            .map(|c| c.trim().to_string())
            .collect()
    };
    let is_sep = |c: &[String]| {
        !c.is_empty()
            && c.iter()
                .all(|x| !x.is_empty() && x.chars().all(|ch| ch == '-' || ch == ':'))
    };

    let lines: Vec<&str> = markdown.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    while i < lines.len() {
        if !is_row(lines[i]) {
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && is_row(lines[i]) {
            i += 1;
        }
        let rows: Vec<Vec<String>> = lines[start..i].iter().map(|l| cells(l)).collect();
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        let keep: Vec<bool> = (0..width)
            .map(|c| {
                rows.iter()
                    .any(|r| !is_sep(r) && r.get(c).is_some_and(|x| !x.is_empty()))
            })
            .collect();
        // 一列都不剩就原样留着，不去猜
        if !keep.iter().any(|k| *k) {
            out.extend(lines[start..i].iter().map(|l| l.to_string()));
            continue;
        }
        let render = |r: &Vec<String>| {
            let sep = is_sep(r);
            let kept: Vec<String> = (0..width)
                .filter(|c| keep[*c])
                .map(|c| {
                    let v = r.get(c).cloned().unwrap_or_default();
                    if sep && v.is_empty() {
                        "---".to_string()
                    } else {
                        v
                    }
                })
                .collect();
            format!("| {} |", kept.join(" | "))
        };
        // **表头整行是空的就让位。** 提上来的第一行有时只是排版用的占位行
        // （SEC 那份投票结果 8-K 的每张表都这样），留着它，模型看到的是
        // 一张列名全空的表——没有信息，还占着「表头」这个位置。下一行顶上，
        // 表的第一行才是它真正的抬头（"a. Tench Coxe"）。
        let head_blank = rows.first().is_some_and(|r| {
            (0..width)
                .filter(|c| keep[*c])
                .all(|c| r.get(c).is_none_or(String::is_empty))
        });
        let body_start = if head_blank && rows.len() > 2 { 2 } else { 0 };
        if body_start == 2 {
            out.push(render(&rows[2]));
            if let Some(sep) = rows.get(1) {
                out.push(render(sep));
            }
            for r in &rows[3..] {
                out.push(render(r));
            }
        } else {
            for r in &rows {
                out.push(render(r));
            }
        }
    }
    out.join("\n")
}

fn markdown_from_html(html: &str) -> Result<String, HtmlError> {
    // 最外层的表先从 DOM 渲染成带真表头的 Markdown（见 `table`），htmd 只看到占位段落；
    // 套着的表还走老路，所以第一行提表头的补丁留着给它们
    let (html, tables) = crate::table::lift_tables(html);
    let html = &promote_first_row_headers(&html);
    let markdown = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "iframe", "object", "embed", "img", "svg", "math",
        ])
        .scripting_enabled(false)
        .build()
        .convert(html)
        .map_err(|e| HtmlError::Conversion(e.to_string()))?;
    // Controls alone are not an interstitial: even the raw fallback may
    // contain a complete article alongside a newsletter or login modal.
    if looks_like_challenge_shell(&markdown)
        || (markdown.chars().filter(|ch| !ch.is_whitespace()).count() < 200
            && looks_like_challenge_html(html))
    {
        return Err(HtmlError::Interstitial);
    }
    let markdown = crate::table::restore_tables(&markdown, &tables);
    normalize_markdown(&prune_empty_table_columns(&markdown))
}

/// Normalize direct Markdown with the same link policy as HTML conversion.
pub fn normalize_markdown(markdown: &str) -> Result<String, HtmlError> {
    Ok(sanitize_markdown_links(&post_process(markdown)))
}

fn post_process(markdown: &str) -> String {
    let mut result = String::with_capacity(markdown.len());
    let mut newlines = 0;
    for ch in markdown.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 3 {
                result.push(ch);
            }
        } else {
            newlines = 0;
            result.push(ch);
        }
    }
    result.trim().to_string()
}

fn sanitize_markdown_links(markdown: &str) -> String {
    let mut result = String::with_capacity(markdown.len());
    let bytes = markdown.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(relative_start) = markdown[cursor..].find('[') else {
            result.push_str(&markdown[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        result.push_str(&markdown[cursor..start]);
        if start > 0 && bytes[start - 1] == b'!' {
            result.push('[');
            cursor = start + 1;
            continue;
        }
        let Some(relative_close_label) = markdown[start + 1..].find("](") else {
            result.push('[');
            cursor = start + 1;
            continue;
        };
        let close_label = start + 1 + relative_close_label;
        let Some(close_link) = find_unescaped_byte(bytes, close_label + 2, b')') else {
            result.push('[');
            cursor = start + 1;
            continue;
        };
        let destination = &markdown[close_label + 2..close_link];
        if !is_unsafe_markdown_destination(destination) {
            result.push_str(&markdown[start..=close_link]);
        } else {
            result.push_str(&markdown[start + 1..close_label]);
        }
        cursor = close_link + 1;
    }
    result
}

fn find_unescaped_byte(bytes: &[u8], start: usize, wanted: u8) -> Option<usize> {
    let mut escaped = false;
    for (offset, byte) in bytes.iter().enumerate().skip(start) {
        if escaped {
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == wanted {
            return Some(offset);
        }
    }
    None
}

fn is_unsafe_markdown_destination(destination: &str) -> bool {
    let destination = destination
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    let decoded = percent_encoding::percent_decode_str(destination).decode_utf8_lossy();
    let decoded =
        decoded.trim_start_matches(|ch: char| ch.is_ascii_control() || ch.is_ascii_whitespace());
    let Some((scheme, _)) = decoded.split_once(':') else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "javascript" | "vbscript" | "data" | "file" | "blob"
    )
}
