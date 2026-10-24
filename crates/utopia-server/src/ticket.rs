//! 一张工单一篇文档，三个工单来源共用这一份排版（0013 决定 7）。
//!
//! 每个来源把自己的数据装进 [`Ticket`]、自己去取；这里只管把它写成文档。措辞（GitHub 写
//! "Opened by"、Jira 写 "Reported by"）由来源成句后放进来，这里不改一个字：文档的字节就是
//! 它的身份，改一个字就是每张工单下次同步都多一个版本。`ticket_golden_tests` 钉着这一点

use chrono::{DateTime, Utc};

pub struct Ticket {
    /// `# ` 后面那一行，如 `#12 Fix it`、`KAFKA-1 Summary`
    pub heading: String,
    /// 抬头下的句子，已经成句、按来源自己的顺序
    pub sentences: Vec<String>,
    /// `## Description` 下的正文；空白的不写这一节
    pub description: Option<String>,
    /// `(时刻, 文字)`，写成 `- <时刻> — <文字>`，按时刻稳定排序
    pub history: Vec<(DateTime<Utc>, String)>,
    /// 空正文的评论不写，但只要有评论就有 `## Comments` 这一节，与从前一样
    pub comments: Vec<Comment>,
}

pub struct Comment {
    pub author: String,
    /// 没有时刻的写成 `?`
    pub at: Option<DateTime<Utc>>,
    pub body: String,
}

/// 工单上的一个时刻，写进正文的样子：到秒、带 `Z`（#691，0024 第 3 节）
pub fn stamp(t: DateTime<Utc>) -> String {
    crate::time_text::world(t, Some("second"))
}

pub fn render(t: &Ticket) -> String {
    let mut out = format!("# {}\n\n", t.heading);
    for s in &t.sentences {
        out.push_str(s);
        out.push('\n');
    }
    if let Some(d) = t
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        out.push_str("\n## Description\n\n");
        out.push_str(d);
        out.push('\n');
    }
    // 端点的顺序不是契约，排序自己做——排错了的后果是历史倒着讲。稳定排序：同一刻的几条
    // 保持来源给的先后
    if !t.history.is_empty() {
        let mut history: Vec<&(DateTime<Utc>, String)> = t.history.iter().collect();
        history.sort_by_key(|(at, _)| *at);
        out.push_str("\n## History\n\n");
        for (at, text) in history {
            out.push_str(&format!("- {} — {}\n", stamp(*at), text));
        }
    }
    if !t.comments.is_empty() {
        out.push_str("\n## Comments\n\n");
        for c in &t.comments {
            let body = c.body.trim();
            if body.is_empty() {
                continue;
            }
            let at = c.at.map(stamp).unwrap_or_else(|| "?".into());
            out.push_str(&format!("### {} on {}\n\n{}\n\n", c.author, at, body));
        }
    }
    out
}
