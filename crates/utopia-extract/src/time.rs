//! 时间提法的解读（0045 决定 2、3、8）：**模型读，代码算**。
//!
//! 两次调用，都只返回结构化的字段，模型从不写一个算出来的日期：
//!
//! 1. **给文档定日期**（[`build_dating_messages`] / [`parse_dating_response`]）：读文件开头，
//!    抄下文档自己的日期（电头、申报日、公报的报告年份）、它命名的期间及其边界、财年
//!    的截止日。上传时间永远不进来——那是记录时间轴（决定 3）。
//! 2. **解读提法**（[`build_interpretation_messages`] / [`parse_interpretation_response`]）：
//!    每条提法给形状、引用、粒度。绝对值按原文的字抄成数字部件；相对的说锚点（另一条
//!    提法、文档本身、文档命名的期间）与偏移（数、单位、方向）；期间报名字。日历算术、
//!    财年到区间、按粒度截断、形状蕴含的区间，全在服务端算。
//!
//! 代码里**没有时间词**（决定 8）：不认月份名、不认「去年」「上年末」，只解 JSON、验
//! 部件范围、拼提示词。提示词是常量英文；例子是中性的（镇议会、桥、面包房），不出自
//! 任何测量语料。
//!
//! 部件的键是短的：`y` 年、`m` 月、`d` 日、`h` 时、`min` 分、`s` 秒；只写原文说到的
//! 部件（「2024 年」就是 `{"y": 2024}`）。细的部件必须带着粗的（有日必有月），这是
//! 0024 的精度阶梯。
//!
//! 解析逐项宽容：坏的条目计数，不毁整批；调用方**必须**把计数报出去（#108 那类错）。
//! 截断的回复退到最后一个完整条目补括号，与 `open.rs` 同一套修补。

use std::collections::HashSet;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utopia_llm::ChatMessage;

use crate::open::repair_truncated_compact;
use crate::{json_block, json_text};

/// 原文说到的日期部件；只有原文写了的才是 `Some`。存成 JSONB 时用全名键
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateParts {
    pub year: i32,
    /// 写明的年里的第几季度（「2025年第三季度」「Q3 2025」），1–4；有它就没有月。
    /// 季度到月份是日历算术，归代码（0045 决定 2）：模型只抄「第三」这个数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarter: Option<u32>,
    pub month: Option<u32>,
    pub day: Option<u32>,
    pub hour: Option<u32>,
    pub minute: Option<u32>,
    pub second: Option<u32>,
}

impl DateParts {
    /// 部件够到的那一级：有日就是日、只有年就是年。服务端拿它对照模型报的粒度——
    /// 两者不合的提法进审核（记录「不做」的最后一条）
    pub fn granularity(&self) -> Granularity {
        if self.second.is_some() {
            Granularity::Second
        } else if self.minute.is_some() {
            Granularity::Minute
        } else if self.hour.is_some() {
            Granularity::Hour
        } else if self.day.is_some() {
            Granularity::Day
        } else if self.month.is_some() || self.quarter.is_some() {
            Granularity::Month
        } else {
            Granularity::Year
        }
    }
}

/// 只排版、不计算：`2011`、`2011-03`、`2011-03-04`、`2011-03-04 14:30`
impl fmt::Display for DateParts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.year)?;
        if let Some(q) = self.quarter {
            write!(f, "-Q{q}")?;
        }
        if let Some(m) = self.month {
            write!(f, "-{m:02}")?;
        }
        if let Some(d) = self.day {
            write!(f, "-{d:02}")?;
        }
        if let Some(h) = self.hour {
            write!(f, " {h:02}")?;
        }
        if let Some(mi) = self.minute {
            write!(f, ":{mi:02}")?;
        }
        if let Some(s) = self.second {
            write!(f, ":{s:02}")?;
        }
        Ok(())
    }
}

/// 文档命名的期间及其边界（「fiscal 2019」到 9 月 30 日；「2024年」作为报告年）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedPeriod {
    pub name: String,
    pub from: DateParts,
    pub to: DateParts,
}

/// 文档说到的一个日期，连它的名字和它管到哪（0064 决定 1）。抽取每一块时报上来，
/// 服务端核对字在块里、算出它所在的标题路径，存在文档的时间语境里。
///
/// 一篇文档不止一个日期，哪个都不比别的大：提报日期、数据统计截止、生效日期各管各的事。
/// 相对的时间词从哪天起算，看它所在的那一节里哪个 `now` 在管（[`now_in_force`]）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeEntry {
    /// `now`：文本说话的那一刻（电头、报告的提报日、会议的日期）；
    /// `date`：文本给了名字的别的日期（截止、生效、期限）；`period`：命名的期间
    pub kind: String,
    /// the label as written ("提报日期", "Data as of"); empty for a bare dateline
    pub name: String,
    /// the date words as written
    pub words: String,
    pub from: DateParts,
    pub to: Option<DateParts>,
    /// the headings the entry was stated under, outermost first; it governs what sits under them
    #[serde(default)]
    pub scope: Vec<String>,
    /// the chunk that stated it and where in it (set by the server)
    #[serde(default)]
    pub chunk: Option<String>,
    #[serde(default)]
    pub char_start: Option<i32>,
}

/// 块的正文里，这个字符位置之前还在管着的标题，由外到内。
///
/// 判据是结构的：行首连着的 `#` 是标题的级，同级或更深的旧标题被新标题顶掉。分块器让
/// 每一块以它所在的标题开头，所以一块自己的正文就说得出它在哪一节——不靠块与块的先后，
/// 块并行抽的时候照样成立（#588）
pub fn headings_at(text: &str, char_pos: usize) -> Vec<String> {
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut seen = 0usize;
    for line in text.split_inclusive('\n') {
        if seen > char_pos {
            break;
        }
        let trimmed = line.trim();
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level) {
            let title = trimmed[level..].trim();
            if !title.is_empty() && trimmed[level..].starts_with(char::is_whitespace) {
                stack.retain(|(l, _)| *l < level);
                stack.push((level, title.to_string()));
            }
        }
        seen += line.chars().count();
    }
    stack.into_iter().map(|(_, t)| t).collect()
}

/// 这个位置上管事的那个 `now`：所在的标题路径以它的范围开头的里面，范围最深的那个；
/// 同一层有几个时取先说的。没有就是 `None`——相对的时间词锚不到，等着（0045 决定 4）
pub fn now_in_force<'a>(entries: &'a [TimeEntry], path: &[String]) -> Option<&'a TimeEntry> {
    let mut best: Option<&TimeEntry> = None;
    for e in entries.iter().filter(|e| e.kind == "now") {
        if e.scope.len() <= path.len()
            && e.scope.iter().zip(path).all(|(a, b)| a == b)
            && best.is_none_or(|b| e.scope.len() > b.scope.len())
        {
            best = Some(e);
        }
    }
    best
}

/// 抽取回复里的一条 `t`：`[kind, name, words, from, to]`。部件是照字抄的数字，出界的、
/// 缺字的都是坏条目
pub fn time_entry(item: &Value) -> Option<TimeEntry> {
    let a = item.as_array()?;
    let text = |i: usize| a.get(i).and_then(Value::as_str).map(str::trim);
    let kind = text(0)?.to_lowercase();
    if !matches!(kind.as_str(), "now" | "date" | "period") {
        return None;
    }
    let words = text(2).filter(|w| !w.is_empty())?;
    let from = parts(a.get(3)?)?;
    let to = match a.get(4) {
        None | Some(Value::Null) => None,
        Some(v) => Some(parts(v)?),
    };
    Some(TimeEntry {
        kind,
        name: text(1).unwrap_or("").to_string(),
        words: words.to_string(),
        from,
        to,
        scope: Vec::new(),
        chunk: None,
        char_start: None,
    })
}

/// 文档自己的时间语境（决定 3）：从开头读出来、存在文档上、每块都带着
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentDating {
    /// 抽取时各块报上来的日期（0064）：有它们就不再另问一次开头
    pub entries: Vec<TimeEntry>,
    /// the document's own date as the text states it (a dateline, a filing date, a bulletin's
    /// reporting year); None when the text gives none
    pub date: Option<DateParts>,
    /// the words that state it, verbatim (the server checks they occur in the opening)
    pub date_words: Option<String>,
    /// periods the document names with bounds it states ("fiscal 2027" with its end date;
    /// "2024年" as a reporting year)
    pub periods: Vec<NamedPeriod>,
    /// (month, day) on which the document's fiscal year ends, when stated
    pub fiscal_year_end: Option<(u32, u32)>,
    /// items the reply wrote but that were malformed (a date with month 13, a period without
    /// bounds); must be reported by the caller
    pub skipped: usize,
}

/// 提法对它所定的陈述做了什么
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Point,
    Since,
    Until,
    Interval,
    AsOf,
    Duration,
    EndedUnknown,
}

/// 原文的字够到阶梯的哪一级（0024），与引用分开记
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Year,
    Month,
    /// 阶梯上没有「周」（0024）：说到周的提法（「上周」）落在日这一档。模型照字报 week，
    /// 从前整条解释因此被当成坏条目丢掉，「上周」就永远没有日期
    #[serde(alias = "week")]
    Day,
    Hour,
    Minute,
    Second,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Year,
    Quarter,
    Month,
    Week,
    Day,
    Hour,
    Minute,
    Second,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Before,
    After,
}

/// 原文写的偏移：「two years later」= 2 年 after；「this quarter」= 0 quarter
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offset {
    pub count: i64,
    pub unit: Unit,
    pub direction: Direction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Anchor {
    Mention { id: i64 },
    Document,
    Period { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reference {
    /// the words state the value; `to` only for an interval written out ("from March to May 2024")
    Absolute {
        from: DateParts,
        to: Option<DateParts>,
    },
    /// relative to an anchor, with an offset when the words give one ("two years later"); no
    /// offset = the anchor itself ("today")
    Anchored {
        anchor: Anchor,
        offset: Option<Offset>,
    },
    /// a named period from the context or one whose bounds the text states here
    Period {
        name: String,
        from: Option<DateParts>,
        to: Option<DateParts>,
    },
    None,
}

/// 一条提法的解读；存成 JSONB，服务端从它算区间
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interpretation {
    pub id: i64,
    pub shape: Shape,
    pub reference: Reference,
    pub granularity: Granularity,
}

/// 送去解读的一条提法：编号、原文的字、所在的句子（模型靠句子挑锚点）
#[derive(Debug, Clone, Copy)]
pub struct MentionInput<'a> {
    pub id: i64,
    pub text: &'a str,
    pub sentence: &'a str,
}

/// 进提示词的时间语境：[`DocumentDating`] 的借用视图
#[derive(Debug, Clone, Copy)]
pub struct TimeContext<'a> {
    pub date: Option<&'a DateParts>,
    pub date_words: Option<&'a str>,
    pub periods: &'a [NamedPeriod],
    pub fiscal_year_end: Option<(u32, u32)>,
}

/// 定日期的系统消息。只抄开头说了的；文档自己的日期，从不是今天；部件是抄写的数字
const DATING_SYSTEM: &str = "\
You read the opening of a document and write down what it states about the document's own \
time: its date, the periods it names with their bounds, and the day its fiscal year ends. You \
transcribe what the words state; you never compute a date. Output one JSON object and nothing \
else, shaped like this:\n\
\n\
{\"date\": {\"y\": 2011, \"m\": 3, \"d\": 4}, \"date_words\": \"March 4, 2011\", \"periods\": [[\"name as written\", {\"y\": 2010, \"m\": 10, \"d\": 1}, {\"y\": 2011, \"m\": 9, \"d\": 30}]], \"fiscal_year_end\": [9, 30]}\n\
\n\
1. date is the document's own date: the day it was written, issued, filed, published or signed \
(a dateline, a filing date, a date under the title), or, for a report or bulletin that covers a \
period, the period it reports on. It is never today's date, and never a date the opening \
mentions for something else — an event it reports, an agreement it refers to, a deadline it \
sets. null when the opening states none.\n\
2. A date is written as parts, each a number transcribed from the words: y the year, m the \
month (a month name becomes its number), d the day, and h, min, s for a clock time when one is \
written. Write only the parts the words state: a bulletin for the year 2024 has {\"y\": 2024}; \
a report for March 2024 has {\"y\": 2024, \"m\": 3}; a dateline \"4 March 2011\" has \
{\"y\": 2011, \"m\": 3, \"d\": 4}.\n\
3. date_words is the words that state the date, copied verbatim from the opening; null when \
date is null.\n\
4. periods lists the periods the opening names and bounds, one entry each, [name, from, to]: \
a fiscal year or quarter, a reporting period, a term. name is the period as the opening writes \
it (\"fiscal 2019\", \"the second quarter\", \"2024年\"); from and to are its first and last \
day, month or year as parts, at the precision the words give. A reporting year \"2024\" runs \
from {\"y\": 2024} to {\"y\": 2024}. A fiscal year or quarter the opening states by its end \
date (\"the year ended September 30, 2019\") runs from the day after the previous one ended to \
that end date. Leave out a period whose bounds the opening does not fix.\n\
5. fiscal_year_end is [month, day] on which the document's fiscal year ends, when the opening \
states it or a stated fiscal year end makes it plain; null otherwise.\n\
6. Never compute a date from today, never resolve a relative expression, never fill in a part \
the words do not state. When the opening states nothing about the document's date or periods, \
output {\"date\": null, \"date_words\": null, \"periods\": [], \"fiscal_year_end\": null}.\n\
\n\
Example. The opening \"Westbrook Town Council — Minutes of the meeting held on March 4, 2011. \
Present: the mayor and six councillors.\" gives {\"date\": {\"y\": 2011, \"m\": 3, \"d\": 4}, \
\"date_words\": \"March 4, 2011\", \"periods\": [], \"fiscal_year_end\": null}. The opening \
\"Harbor Bakery — Annual report for fiscal 2019, the year ended September 30, 2019. Issued \
November 12, 2019.\" gives {\"date\": {\"y\": 2019, \"m\": 11, \"d\": 12}, \"date_words\": \
\"November 12, 2019\", \"periods\": [[\"fiscal 2019\", {\"y\": 2018, \"m\": 10, \"d\": 1}, \
{\"y\": 2019, \"m\": 9, \"d\": 30}]], \"fiscal_year_end\": [9, 30]}";

/// 解读提法的系统消息。形状各一行；引用是字面说的，不是它蕴含的；从不计算
const INTERPRETATION_SYSTEM: &str = "\
You interpret the time expressions of a document. Each mention is given with its id, its words \
exactly as written and the sentence it occurs in; before them, the document's own time context: \
its date and the words that state it, the periods it names with their bounds, and the day its \
fiscal year ends. For each mention you write what its words state — a shape, a reference and a \
granularity — and never a date you computed: the reader computes dates from what you write. \
Output one JSON object and nothing else, shaped like this:\n\
\n\
{\"m\": [[id, \"shape\", reference, \"granularity\"]]}\n\
\n\
1. shape is what the time does to the statement it dates, one of:\n\
   \"point\" — the statement happened or holds at that time;\n\
   \"since\" — it holds from that time on;\n\
   \"until\" — it holds up to that time;\n\
   \"interval\" — it holds between two bounds;\n\
   \"as_of\" — the state observed at that time;\n\
   \"duration\" — a length of time with no position;\n\
   \"ended_unknown\" — the words say it ended but not when.\n\
2. reference is what the words say, not what they imply, in one of four forms:\n\
   {\"kind\": \"absolute\", \"from\": {\"y\": 2011, \"m\": 3, \"d\": 4}} — the words state the \
value. Parts are transcribed digits: y the year, m the month (a month name becomes its number), \
d the day, and h, min, s for a clock time; write only the parts the words state. A bare year \
(\"2019\") is {\"y\": 2019} with granularity \"year\". A quarter of a stated year is written \
with q, the number of the quarter as the words give it, and no month: \"Q3 2025\" and \"2025年\
第三季度\" are {\"y\": 2025, \"q\": 3} with granularity \"month\". \"to\" is added only for an interval the \
words write out with both bounds (\"from March to May 2024\": from {\"y\": 2024, \"m\": 3}, to \
{\"y\": 2024, \"m\": 5}).\n\
   {\"kind\": \"anchored\", \"anchor\": {\"kind\": \"document\"}, \"offset\": {\"count\": 1, \
\"unit\": \"year\", \"direction\": \"before\"}} — the words point at another time. The anchor is \
the document itself, {\"kind\": \"document\"}, for \"today\", \"now\", \"this quarter\", \"last \
year\", \"the prior year\"; the mention the sentence counts from, {\"kind\": \"mention\", \
\"id\": 3}, for \"two years later\", \"three months earlier\", \"the following day\" when the \
sentence names that time, and the document when it names none; a period from the context, \
{\"kind\": \"period\", \"name\": \"fiscal 2019\"}, for \"the end of fiscal 2019\". offset is the \
count, unit and direction the words state: \"two years later\" is count 2, unit \"year\", \
direction \"after\"; \"this quarter\" and \"this year\" are count 0 with that unit; \"today\" \
and \"now\" have no offset. unit is one of year, quarter, month, week, day, hour, minute, \
second; direction is before or after.\n\
   {\"kind\": \"period\", \"name\": \"Q2 fiscal 2019\"} — the words name a period. One listed in \
the context is named exactly as listed. One whose bounds the words state here carries them: a \
heading \"Three months ended June 30, 2019\" is {\"kind\": \"period\", \"name\": \"Three months \
ended June 30, 2019\", \"to\": {\"y\": 2019, \"m\": 6, \"d\": 30}}, with \"from\" only when the \
words state it. Words that name a period only relatively (the whole year, the end of the year, \
the start of the month, this quarter) are not a period: they are anchored to the document with \
count 0 and that unit; the end of it takes shape \"as_of\" or \"until\", the start \"since\", the \
whole \"interval\". A count inside something the sentence names (a week of a trial, a month of a \
programme) anchors to that thing's dated mention when the sentence gives one, otherwise it is \
{\"kind\": \"none\"}: it is not counted from the document.\n\
   {\"kind\": \"none\"} — the words give nothing to anchor to: a duration (\"ten years\"), an \
ending without a date (\"formerly\", \"no longer\"), a vague time (\"recently\").\n\
3. granularity is the rung the words reach: \"year\", \"month\", \"day\", \"hour\", \"minute\" \
or \"second\". \"March 2024\" is month; \"two years later\" is year; \"today\" is day; a quarter \
or a season is month.\n\
4. Never compute: do not resolve \"last year\" to a year, do not add an offset to a date, do not \
turn a period into dates, do not use today's date. Write one item per mention, with the id from \
the list, and no item for an id not listed.\n\
\n\
Example. With the document dated March 4, 2011 and the mentions [id 1 \"March 4, 2011\" in \"The \
council met on March 4, 2011.\", id 2 \"since 2005\" in \"Harbor Bakery has supplied the school \
since 2005.\", id 3 \"2019\" and id 4 \"two years later\" in \"The bridge opened in 2019; two \
years later the eastern span was widened.\", id 5 \"last year\" in \"Last year the bakery opened \
a second shop.\"]:\n\
{\"m\": [[1, \"point\", {\"kind\": \"absolute\", \"from\": {\"y\": 2011, \"m\": 3, \"d\": 4}}, \"day\"],\n\
 [2, \"since\", {\"kind\": \"absolute\", \"from\": {\"y\": 2005}}, \"year\"],\n\
 [3, \"point\", {\"kind\": \"absolute\", \"from\": {\"y\": 2019}}, \"year\"],\n\
 [4, \"point\", {\"kind\": \"anchored\", \"anchor\": {\"kind\": \"mention\", \"id\": 3}, \"offset\": {\"count\": 2, \"unit\": \"year\", \"direction\": \"after\"}}, \"year\"],\n\
 [5, \"point\", {\"kind\": \"anchored\", \"anchor\": {\"kind\": \"document\"}, \"offset\": {\"count\": 1, \"unit\": \"year\", \"direction\": \"before\"}}, \"year\"]]}";

/// 定日期的两条消息：常量系统消息 + 文件名与开头。开头不在这里截——它就是这次
/// 调用的正文，预算由调用方定
pub fn build_dating_messages(filename: &str, opening: &str) -> Vec<ChatMessage> {
    let user = format!("Document: {filename}\n\nOpening:\n\"\"\"\n{opening}\n\"\"\"");
    vec![
        ChatMessage {
            role: "system".into(),
            content: DATING_SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}

/// 解读提法的两条消息：常量系统消息 + 语境块 + 提法清单（编号、原文的字、句子）
pub fn build_interpretation_messages(
    context: &TimeContext<'_>,
    mentions: &[MentionInput<'_>],
) -> Vec<ChatMessage> {
    let mut user = String::from("Document time context:\n");
    match (context.date, context.date_words) {
        (Some(date), Some(words)) => {
            user.push_str(&format!(
                "- date: {date}, stated by the words \"{words}\"\n"
            ));
        }
        (Some(date), None) => user.push_str(&format!("- date: {date}\n")),
        (None, _) => user.push_str(
            "- date: not stated; still anchor to the document where the words point at it\n",
        ),
    }
    if context.periods.is_empty() {
        user.push_str("- periods: none named\n");
    } else {
        let listed: Vec<String> = context
            .periods
            .iter()
            .map(|p| format!("\"{}\" from {} to {}", p.name, p.from, p.to))
            .collect();
        user.push_str(&format!("- periods: {}\n", listed.join("; ")));
    }
    match context.fiscal_year_end {
        Some((m, d)) => user.push_str(&format!("- fiscal year end: month {m}, day {d}\n")),
        None => user.push_str("- fiscal year end: not stated\n"),
    }
    user.push_str("\nMentions:\n");
    for m in mentions {
        user.push_str(&format!(
            "- id {}, words \"{}\", in the sentence: \"{}\"\n",
            m.id, m.text, m.sentence
        ));
    }
    vec![
        ChatMessage {
            role: "system".into(),
            content: INTERPRETATION_SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}

/// 先按常规取块解；解不开再从第一个 `{` 取到结尾，退到最后一个完整条目补括号
/// （同 `open.rs`：紧凑回复的最后一个 `}` 不是可靠的结尾）
fn reply_value(raw: &str, what: &str) -> anyhow::Result<Value> {
    let block = json_block(raw)
        .and_then(|b| serde_json::from_str::<Value>(&b).map_err(anyhow::Error::from));
    match block {
        Ok(v) => Ok(v),
        Err(e) => {
            let text = json_text(raw);
            let fixed = text
                .find('{')
                .and_then(|s| repair_truncated_compact(&text[s..]))
                .ok_or_else(|| anyhow::anyhow!("Failed to parse {what} JSON: {e}"))?;
            serde_json::from_str::<Value>(&fixed)
                .map_err(|e| anyhow::anyhow!("Failed to parse {what} JSON: {e}"))
        }
    }
}

/// 一个整数：JSON 数字，或模型偶尔写成字符串的数字
fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// 可选的整数部件，带范围：缺或 null 是 `Some(None)`；有但出界或不是数是 `None`（坏）
fn part(obj: &Value, key: &str, range: std::ops::RangeInclusive<i64>) -> Option<Option<u32>> {
    match obj.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(v) => {
            let n = int(v)?;
            range.contains(&n).then_some(Some(n as u32))
        }
    }
}

/// 日期部件：`y` 必有；细的部件必须带着粗的（有日必有月），月 1–12、日 1–31、
/// 时 0–23、分秒 0–59；出界就是坏条目
fn parts(v: &Value) -> Option<DateParts> {
    if !v.is_object() {
        return None;
    }
    let year = i32::try_from(int(v.get("y")?)?).ok()?;
    let quarter = part(v, "q", 1..=4)?;
    let month = part(v, "m", 1..=12)?;
    // 季度和月不同时写：写了季度就是说到季度为止
    if quarter.is_some() && month.is_some() {
        return None;
    }
    let day = part(v, "d", 1..=31)?;
    let hour = part(v, "h", 0..=23)?;
    let minute = part(v, "min", 0..=59)?;
    let second = part(v, "s", 0..=59)?;
    let ladder = [
        month.is_some(),
        day.is_some(),
        hour.is_some(),
        minute.is_some(),
        second.is_some(),
    ];
    if ladder.windows(2).any(|w| w[1] && !w[0]) {
        return None;
    }
    Some(DateParts {
        year,
        quarter,
        month,
        day,
        hour,
        minute,
        second,
    })
}

/// 可选的部件：缺或 null 是 `Some(None)`；有但坏是 `None`
fn opt_parts(v: Option<&Value>) -> Option<Option<DateParts>> {
    match v {
        None | Some(Value::Null) => Some(None),
        Some(v) => parts(v).map(Some),
    }
}

/// 非空文字，去首尾空白
fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 枚举词：借 serde 的 snake_case 名认，大小写、空格与连字符都宽容（"As of" → as_of）
fn word<T: DeserializeOwned>(v: &Value) -> Option<T> {
    let s = v
        .as_str()?
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-'], "_");
    serde_json::from_value(Value::String(s)).ok()
}

/// 期间条目 `[name, from, to]`
fn named_period(v: &Value) -> Option<NamedPeriod> {
    let arr = v.as_array()?;
    Some(NamedPeriod {
        name: text(arr.first()?)?,
        from: parts(arr.get(1)?)?,
        to: parts(arr.get(2)?)?,
    })
}

/// 解定日期的回复。日期、财年截止日坏了就留空并计数；期间逐条计数
pub fn parse_dating_response(raw: &str) -> anyhow::Result<DocumentDating> {
    let value = reply_value(raw, "document dating")?;
    let mut out = DocumentDating::default();
    match value.get("date") {
        None | Some(Value::Null) => {}
        Some(v) => match parts(v) {
            Some(d) => out.date = Some(d),
            None => out.skipped += 1,
        },
    }
    out.date_words = value.get("date_words").and_then(text);
    if let Some(items) = value.get("periods").and_then(Value::as_array) {
        for item in items {
            match named_period(item) {
                Some(p) => out.periods.push(p),
                None => out.skipped += 1,
            }
        }
    }
    match value.get("fiscal_year_end") {
        None | Some(Value::Null) => {}
        Some(v) => {
            let pair = v.as_array().and_then(|a| {
                let m = int(a.first()?)?;
                let d = int(a.get(1)?)?;
                ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((m as u32, d as u32))
            });
            match pair {
                Some(p) => out.fiscal_year_end = Some(p),
                None => out.skipped += 1,
            }
        }
    }
    Ok(out)
}

/// 锚点：`{"kind": "mention", "id": N}` / `{"kind": "document"}` / `{"kind": "period", "name"}`；
/// 光秃秃的 "document" 也认——只有它不带别的字段
fn anchor(v: &Value) -> Option<Anchor> {
    let kind = match v {
        Value::String(s) => s.clone(),
        v => v.get("kind")?.as_str()?.to_string(),
    };
    let kind = kind.trim().to_ascii_lowercase();
    match kind.as_str() {
        "mention" => Some(Anchor::Mention {
            id: int(v.get("id")?)?,
        }),
        "document" => Some(Anchor::Document),
        "period" => Some(Anchor::Period {
            name: text(v.get("name")?)?,
        }),
        _ => None,
    }
}

/// 偏移：数不为负（方向另说），单位与方向是枚举词
fn offset(v: &Value) -> Option<Offset> {
    let count = int(v.get("count")?)?;
    if count < 0 {
        return None;
    }
    Some(Offset {
        count,
        unit: word(v.get("unit")?)?,
        direction: word(v.get("direction")?)?,
    })
}

fn opt_offset(v: Option<&Value>) -> Option<Option<Offset>> {
    match v {
        None | Some(Value::Null) => Some(None),
        Some(v) => offset(v).map(Some),
    }
}

/// 引用：按 `kind` 分四种；JSON null 当 none
fn reference(v: &Value) -> Option<Reference> {
    if v.is_null() {
        return Some(Reference::None);
    }
    let kind = v.get("kind")?.as_str()?.trim().to_ascii_lowercase();
    match kind.as_str() {
        "absolute" => Some(Reference::Absolute {
            from: parts(v.get("from")?)?,
            to: opt_parts(v.get("to"))?,
        }),
        "anchored" => Some(Reference::Anchored {
            anchor: anchor(v.get("anchor")?)?,
            offset: opt_offset(v.get("offset"))?,
        }),
        "period" => Some(Reference::Period {
            name: text(v.get("name")?)?,
            from: opt_parts(v.get("from"))?,
            to: opt_parts(v.get("to"))?,
        }),
        "none" => Some(Reference::None),
        _ => None,
    }
}

/// 一条 `[id, shape, reference, granularity]`
fn interpretation(v: &Value) -> Option<Interpretation> {
    let arr = v.as_array()?;
    Some(Interpretation {
        id: int(arr.first()?)?,
        shape: word(arr.get(1)?)?,
        reference: reference(arr.get(2)?)?,
        granularity: word(arr.get(3)?)?,
    })
}

/// 解解读的回复：返回解开的解读与坏条目数。编号不在 `ids` 里的、同一编号第二次出现的
/// 都算坏；截断的回复修补后少掉的条目不算坏——调用方拿 `ids` 对一下就知道谁没回来
pub fn parse_interpretation_response(
    raw: &str,
    ids: &[i64],
) -> anyhow::Result<(Vec<Interpretation>, usize)> {
    let value = reply_value(raw, "time interpretation")?;
    let known: HashSet<i64> = ids.iter().copied().collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut skipped = 0;
    if let Some(items) = value.get("m").and_then(Value::as_array) {
        for item in items {
            match interpretation(item) {
                Some(i) if known.contains(&i.id) && seen.insert(i.id) => out.push(i),
                _ => skipped += 1,
            }
        }
    }
    Ok((out, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headings_in_force_are_read_from_the_chunk_itself() {
        let text = "# 周报汇编\n\n## 第35周周报\n\n提报日期：2026年8月28日\n\n### 风险\n无。\n\n## 第36周周报\n\n上周开售。\n";
        let at = |needle: &str| {
            text.chars().count() - text[text.find(needle).unwrap()..].chars().count()
        };
        assert_eq!(
            headings_at(text, at("提报日期")),
            vec!["周报汇编", "第35周周报"]
        );
        assert_eq!(
            headings_at(text, at("无。")),
            vec!["周报汇编", "第35周周报", "风险"]
        );
        // 同级的新标题顶掉旧的，连同它下面更深的
        assert_eq!(
            headings_at(text, at("上周开售")),
            vec!["周报汇编", "第36周周报"]
        );
        // `#` 后面没有空白的不是标题（`#588`、`#标签`）
        assert_eq!(
            headings_at("#588 是一个编号\n正文", 12),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_now_in_force_is_the_one_of_the_innermost_section_that_holds_the_position() {
        let entry = |words: &str, scope: &[&str], kind: &str| TimeEntry {
            kind: kind.into(),
            name: "提报日期".into(),
            words: words.into(),
            from: DateParts {
                quarter: None,
                year: 2026,
                month: Some(9),
                day: Some(4),
                hour: None,
                minute: None,
                second: None,
            },
            to: None,
            scope: scope.iter().map(|s| s.to_string()).collect(),
            chunk: None,
            char_start: None,
        };
        let entries = vec![
            entry("2026年3月28日", &["汇编"], "now"),
            entry("2026年8月28日", &["汇编", "第35周"], "now"),
            entry("2026年9月4日", &["汇编", "第36周"], "now"),
            entry("2026年8月31日", &["汇编", "第36周"], "date"),
        ];
        let path = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            now_in_force(&entries, &path(&["汇编", "第36周"]))
                .unwrap()
                .words,
            "2026年9月4日"
        );
        assert_eq!(
            now_in_force(&entries, &path(&["汇编", "第35周", "风险"]))
                .unwrap()
                .words,
            "2026年8月28日"
        );
        // 别的节里的不管这里；退到整篇的那个
        assert_eq!(
            now_in_force(&entries, &path(&["汇编", "附录"]))
                .unwrap()
                .words,
            "2026年3月28日"
        );
        assert!(now_in_force(&entries[1..], &path(&["别的文档"])).is_none());
    }

    #[test]
    fn a_quarter_of_a_stated_year_is_transcribed_not_computed() {
        let q = parts(&serde_json::json!({"y": 2025, "q": 3})).unwrap();
        assert_eq!((q.year, q.quarter, q.month), (2025, Some(3), None));
        assert_eq!(q.granularity(), Granularity::Month);
        assert_eq!(q.to_string(), "2025-Q3");
        // 季度和月不同时写；第五季度不存在
        assert!(parts(&serde_json::json!({"y": 2025, "q": 3, "m": 7})).is_none());
        assert!(parts(&serde_json::json!({"y": 2025, "q": 5})).is_none());
        // 老数据里没有这一格：读回来是 None，写出去不带这一格
        let old: DateParts = serde_json::from_value(serde_json::json!({
            "year": 2024, "month": 3, "day": null, "hour": null, "minute": null, "second": null
        }))
        .unwrap();
        assert_eq!(old.quarter, None);
        assert!(serde_json::to_value(&old).unwrap().get("quarter").is_none());
    }

    #[test]
    fn a_context_entry_is_read_with_its_parts_and_a_bad_one_is_refused() {
        let ok = serde_json::json!(["now", "提报日期", "2026年9月4日", {"y": 2026, "m": 9, "d": 4}, null]);
        let e = time_entry(&ok).unwrap();
        assert_eq!(
            (e.kind.as_str(), e.name.as_str(), e.words.as_str()),
            ("now", "提报日期", "2026年9月4日")
        );
        assert_eq!(e.from.to_string(), "2026-09-04");
        let period = serde_json::json!(["period", "报告期", "2025年1月1日至2025年12月31日", {"y": 2025, "m": 1, "d": 1}, {"y": 2025, "m": 12, "d": 31}]);
        assert_eq!(
            time_entry(&period).unwrap().to.unwrap().to_string(),
            "2025-12-31"
        );
        for bad in [
            serde_json::json!(["today", "x", "2026", {"y": 2026}, null]),
            serde_json::json!(["now", "x", "", {"y": 2026}, null]),
            serde_json::json!(["now", "x", "13月", {"y": 2026, "m": 13}, null]),
            serde_json::json!(["now", "x", "2026"]),
        ] {
            assert!(time_entry(&bad).is_none(), "{bad}");
        }
    }

    fn ymd(y: i32, m: u32, d: u32) -> DateParts {
        DateParts {
            quarter: None,
            year: y,
            month: Some(m),
            day: Some(d),
            hour: None,
            minute: None,
            second: None,
        }
    }

    fn year(y: i32) -> DateParts {
        DateParts {
            quarter: None,
            year: y,
            month: None,
            day: None,
            hour: None,
            minute: None,
            second: None,
        }
    }

    /// 一份完整的定日期回复：带部件的日期、原话、两个期间、财年截止日
    const DATING: &str = r#"{"date": {"y": 2019, "m": 11, "d": 12}, "date_words": "November 12, 2019",
        "periods": [["fiscal 2019", {"y": 2018, "m": 10, "d": 1}, {"y": 2019, "m": 9, "d": 30}],
                    ["2024年", {"y": 2024}, {"y": 2024}]],
        "fiscal_year_end": [9, 30]}"#;

    #[test]
    fn a_dating_reply_parses_into_the_context() {
        let d = parse_dating_response(DATING).unwrap();
        assert_eq!(d.skipped, 0);
        assert_eq!(d.date, Some(ymd(2019, 11, 12)));
        assert_eq!(d.date_words.as_deref(), Some("November 12, 2019"));
        assert_eq!(d.periods.len(), 2);
        assert_eq!(d.periods[0].name, "fiscal 2019");
        assert_eq!(d.periods[0].from, ymd(2018, 10, 1));
        assert_eq!(d.periods[0].to, ymd(2019, 9, 30));
        assert_eq!(d.periods[1].name, "2024年");
        assert_eq!(d.periods[1].from, year(2024));
        assert_eq!(d.periods[1].to, year(2024));
        assert_eq!(d.fiscal_year_end, Some((9, 30)));
    }

    #[test]
    fn a_dating_reply_with_no_date_leaves_everything_empty() {
        let d = parse_dating_response(
            r#"{"date": null, "date_words": null, "periods": [], "fiscal_year_end": null}"#,
        )
        .unwrap();
        assert_eq!(d.date, None);
        assert_eq!(d.date_words, None);
        assert!(d.periods.is_empty());
        assert_eq!(d.fiscal_year_end, None);
        assert_eq!(d.skipped, 0);
    }

    /// 坏的日期、坏的期间、坏的财年截止日各计一次；好的期间照收
    #[test]
    fn a_dating_reply_counts_its_malformed_items() {
        let d = parse_dating_response(
            r#"{"date": {"y": 2019, "m": 13}, "date_words": "Undecimber 2019",
                "periods": [["fiscal 2019", {"y": 2018, "m": 10, "d": 1}, {"y": 2019, "m": 9, "d": 30}],
                            ["broken", {"y": 2018}],
                            ["day without month", {"y": 2018, "d": 5}, {"y": 2019}]],
                "fiscal_year_end": [13, 1]}"#,
        )
        .unwrap();
        assert_eq!(d.date, None);
        assert_eq!(d.periods.len(), 1);
        assert_eq!(d.fiscal_year_end, None);
        assert_eq!(d.skipped, 4);
    }

    #[test]
    fn a_clock_time_parses_and_prints_on_the_ladder() {
        let d = parse_dating_response(
            r#"{"date": {"y": 2011, "m": 3, "d": 4, "h": 14, "min": 30, "s": 5}, "date_words": "14:30:05 on March 4, 2011"}"#,
        )
        .unwrap();
        let date = d.date.unwrap();
        assert_eq!(date.granularity(), Granularity::Second);
        assert_eq!(date.to_string(), "2011-03-04 14:30:05");
        assert_eq!(year(2024).to_string(), "2024");
        assert_eq!(year(2024).granularity(), Granularity::Year);
        assert_eq!(ymd(2019, 9, 30).granularity(), Granularity::Day);
    }

    /// 四种引用、七种形状都在这一份回复里
    const INTERPRETED: &str = r#"{"m": [
        [1, "point", {"kind": "absolute", "from": {"y": 2011, "m": 3, "d": 4}}, "day"],
        [2, "since", {"kind": "anchored", "anchor": {"kind": "document"}, "offset": {"count": 2, "unit": "year", "direction": "before"}}, "year"],
        [3, "interval", {"kind": "period", "name": "fiscal 2019"}, "day"],
        [4, "until", {"kind": "none"}, "day"],
        [5, "as_of", {"kind": "anchored", "anchor": {"kind": "document"}}, "day"],
        [6, "duration", {"kind": "none"}, "year"],
        [7, "ended_unknown", null, "day"],
        [8, "point", {"kind": "anchored", "anchor": {"kind": "mention", "id": 1}, "offset": {"count": 3, "unit": "month", "direction": "after"}}, "month"],
        [9, "interval", {"kind": "absolute", "from": {"y": 2024, "m": 3}, "to": {"y": 2024, "m": 5}}, "month"],
        [10, "interval", {"kind": "period", "name": "Three months ended June 30, 2019", "to": {"y": 2019, "m": 6, "d": 30}}, "day"],
        [11, "point", {"kind": "anchored", "anchor": {"kind": "period", "name": "fiscal 2019"}, "offset": {"count": 0, "unit": "quarter", "direction": "after"}}, "month"]
    ]}"#;

    #[test]
    fn an_interpretation_reply_parses_every_reference_kind_and_shape() {
        let ids: Vec<i64> = (1..=11).collect();
        let (items, skipped) = parse_interpretation_response(INTERPRETED, &ids).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(items.len(), 11);

        assert_eq!(
            items[0],
            Interpretation {
                id: 1,
                shape: Shape::Point,
                reference: Reference::Absolute {
                    from: ymd(2011, 3, 4),
                    to: None
                },
                granularity: Granularity::Day,
            }
        );
        assert_eq!(
            items[1].reference,
            Reference::Anchored {
                anchor: Anchor::Document,
                offset: Some(Offset {
                    count: 2,
                    unit: Unit::Year,
                    direction: Direction::Before
                }),
            }
        );
        assert_eq!(items[1].shape, Shape::Since);
        assert_eq!(items[1].granularity, Granularity::Year);
        assert_eq!(
            items[2].reference,
            Reference::Period {
                name: "fiscal 2019".into(),
                from: None,
                to: None
            }
        );
        assert_eq!(items[2].shape, Shape::Interval);
        assert_eq!(items[3].reference, Reference::None);
        assert_eq!(items[3].shape, Shape::Until);
        assert_eq!(
            items[4].reference,
            Reference::Anchored {
                anchor: Anchor::Document,
                offset: None
            }
        );
        assert_eq!(items[4].shape, Shape::AsOf);
        assert_eq!(items[5].shape, Shape::Duration);
        assert_eq!(items[6].shape, Shape::EndedUnknown);
        assert_eq!(
            items[6].reference,
            Reference::None,
            "JSON null reads as none"
        );
        assert_eq!(
            items[7].reference,
            Reference::Anchored {
                anchor: Anchor::Mention { id: 1 },
                offset: Some(Offset {
                    count: 3,
                    unit: Unit::Month,
                    direction: Direction::After
                }),
            }
        );
        assert_eq!(
            items[8].reference,
            Reference::Absolute {
                from: DateParts {
                    quarter: None,
                    year: 2024,
                    month: Some(3),
                    day: None,
                    hour: None,
                    minute: None,
                    second: None
                },
                to: Some(DateParts {
                    quarter: None,
                    year: 2024,
                    month: Some(5),
                    day: None,
                    hour: None,
                    minute: None,
                    second: None
                }),
            }
        );
        assert_eq!(
            items[9].reference,
            Reference::Period {
                name: "Three months ended June 30, 2019".into(),
                from: None,
                to: Some(ymd(2019, 6, 30)),
            }
        );
        assert_eq!(
            items[10].reference,
            Reference::Anchored {
                anchor: Anchor::Period {
                    name: "fiscal 2019".into()
                },
                offset: Some(Offset {
                    count: 0,
                    unit: Unit::Quarter,
                    direction: Direction::After
                }),
            }
        );
    }

    /// 坏形状、13 月、不在清单里的编号、重复的编号、坏单位各计一次；好的照收
    #[test]
    fn malformed_interpretation_items_are_counted_not_fatal() {
        let raw = r#"{"m": [
            [1, "point", {"kind": "absolute", "from": {"y": 2011, "m": 3, "d": 4}}, "day"],
            [2, "sometime", {"kind": "none"}, "day"],
            [3, "point", {"kind": "absolute", "from": {"y": 2011, "m": 13}}, "month"],
            [99, "point", {"kind": "none"}, "day"],
            [1, "since", {"kind": "none"}, "day"],
            [4, "point", {"kind": "anchored", "anchor": {"kind": "document"}, "offset": {"count": 2, "unit": "fortnight", "direction": "after"}}, "day"],
            [5, "point", {"kind": "elsewhere"}, "day"],
            [6, "point", {"kind": "none"}, "fortnight"],
            [7, "AS OF", {"kind": "anchored", "anchor": "document"}, "Day"]
        ]}"#;
        let (items, skipped) = parse_interpretation_response(raw, &[1, 2, 3, 4, 5, 6, 7]).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, 1);
        assert_eq!(items[1].id, 7, "case and spacing of the words are forgiven");
        assert_eq!(items[1].shape, Shape::AsOf);
        assert_eq!(items[1].granularity, Granularity::Day);
        assert_eq!(
            items[1].reference,
            Reference::Anchored {
                anchor: Anchor::Document,
                offset: None
            }
        );
        assert_eq!(skipped, 7);
    }

    /// 截在第三条中间：前两条留下，没有报错
    #[test]
    fn a_cut_off_interpretation_reply_keeps_the_complete_items() {
        let marker = r#"[3, "interval", {"kind": "period", "name": "fis"#;
        let cut = INTERPRETED.find(marker).unwrap() + marker.len();
        let ids: Vec<i64> = (1..=11).collect();
        let (items, skipped) = parse_interpretation_response(&INTERPRETED[..cut], &ids).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].id, 2);
    }

    #[test]
    fn a_cut_off_dating_reply_keeps_the_complete_periods() {
        let marker = r#"["2024年", {"y": 20"#;
        let cut = DATING.find(marker).unwrap() + marker.len();
        let d = parse_dating_response(&DATING[..cut]).unwrap();
        assert_eq!(d.date, Some(ymd(2019, 11, 12)));
        assert_eq!(d.periods.len(), 1);
        assert_eq!(d.periods[0].name, "fiscal 2019");
    }

    #[test]
    fn a_reply_without_json_is_an_error() {
        assert!(parse_dating_response("I could not find a date.").is_err());
        assert!(parse_interpretation_response("no", &[1]).is_err());
    }

    /// 语料里的字样：提示词里一个都不许有（例子是镇议会、桥、面包房）
    const CORPUS_MARKS: &[&str] = &[
        "NVIDIA",
        "Food and Drug",
        "FDA",
        "January 25, 2027",
        "January 26, 2026",
        "July 26, 2026",
        "fiscal 2027",
        "上年末",
        "统计公报",
        "10-Q",
        "10-K",
    ];

    #[test]
    fn the_dating_prompt_carries_the_document_and_the_rules() {
        let msgs = build_dating_messages(
            "minutes.pdf",
            "Westbrook Town Council — Minutes of the meeting held on March 4, 2011.",
        );
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert!(msgs[0].content.contains("Never compute"));
        assert!(msgs[0].content.contains("never today's date"));
        assert!(msgs[0].content.contains("fiscal_year_end"));
        for mark in CORPUS_MARKS {
            assert!(!msgs[0].content.contains(mark), "prompt carries {mark}");
        }
        assert_eq!(msgs[1].role, "user");
        assert!(msgs[1].content.contains("Document: minutes.pdf"));
        assert!(msgs[1].content.contains("held on March 4, 2011"));
    }

    #[test]
    fn the_interpretation_prompt_carries_the_context_and_every_mention() {
        let date = ymd(2019, 11, 12);
        let periods = vec![
            NamedPeriod {
                name: "fiscal 2019".into(),
                from: ymd(2018, 10, 1),
                to: ymd(2019, 9, 30),
            },
            NamedPeriod {
                name: "the fourth quarter".into(),
                from: ymd(2019, 7, 1),
                to: ymd(2019, 9, 30),
            },
        ];
        let context = TimeContext {
            date: Some(&date),
            date_words: Some("November 12, 2019"),
            periods: &periods,
            fiscal_year_end: Some((9, 30)),
        };
        let mentions = [
            MentionInput {
                id: 7,
                text: "two years later",
                sentence:
                    "The bridge opened in 2019; two years later the eastern span was widened.",
            },
            MentionInput {
                id: 8,
                text: "this quarter",
                sentence: "Sales this quarter rose at the bakery.",
            },
        ];
        let msgs = build_interpretation_messages(&context, &mentions);
        assert_eq!(msgs.len(), 2);
        let system = &msgs[0].content;
        assert!(system.contains("Never compute"));
        for shape in [
            "\"point\"",
            "\"since\"",
            "\"until\"",
            "\"interval\"",
            "\"as_of\"",
            "\"duration\"",
            "\"ended_unknown\"",
        ] {
            assert!(system.contains(shape), "shape {shape} is not explained");
        }
        for mark in CORPUS_MARKS {
            assert!(!system.contains(mark), "prompt carries {mark}");
        }
        let user = &msgs[1].content;
        assert!(user.contains("2019-11-12"));
        assert!(user.contains("\"November 12, 2019\""));
        assert!(user.contains("\"fiscal 2019\" from 2018-10-01 to 2019-09-30"));
        assert!(user.contains("\"the fourth quarter\""));
        assert!(user.contains("fiscal year end: month 9, day 30"));
        assert!(user.contains("id 7, words \"two years later\""));
        assert!(user.contains(mentions[0].sentence));
        assert!(user.contains("id 8, words \"this quarter\""));
        assert!(user.contains(mentions[1].sentence));
    }

    #[test]
    fn an_undated_document_says_so_in_the_context() {
        let context = TimeContext {
            date: None,
            date_words: None,
            periods: &[],
            fiscal_year_end: None,
        };
        let msgs = build_interpretation_messages(&context, &[]);
        let user = &msgs[1].content;
        assert!(user.contains("date: not stated"));
        assert!(user.contains("periods: none named"));
        assert!(user.contains("fiscal year end: not stated"));
    }

    /// 存成 JSONB 再读回来，一字不差
    #[test]
    fn interpretations_and_datings_round_trip_through_serde() {
        let ids: Vec<i64> = (1..=11).collect();
        let (items, _) = parse_interpretation_response(INTERPRETED, &ids).unwrap();
        for item in &items {
            let json = serde_json::to_string(item).unwrap();
            let back: Interpretation = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, item);
        }
        let json = serde_json::to_value(&items[1]).unwrap();
        assert_eq!(json["shape"], "since");
        assert_eq!(json["reference"]["kind"], "anchored");
        assert_eq!(json["reference"]["anchor"]["kind"], "document");
        assert_eq!(json["reference"]["offset"]["unit"], "year");
        assert_eq!(json["granularity"], "year");

        let dating = parse_dating_response(DATING).unwrap();
        let json = serde_json::to_string(&dating).unwrap();
        let back: DocumentDating = serde_json::from_str(&json).unwrap();
        assert_eq!(back.date, dating.date);
        assert_eq!(back.date_words, dating.date_words);
        assert_eq!(back.periods, dating.periods);
        assert_eq!(back.fiscal_year_end, dating.fiscal_year_end);
        assert_eq!(back.skipped, dating.skipped);
        let stored = serde_json::to_value(&dating).unwrap();
        assert_eq!(stored["date"]["year"], 2019, "stored parts use full names");
        assert_eq!(stored["fiscal_year_end"], serde_json::json!([9, 30]));

        let old: DocumentDating = serde_json::from_str("{}").unwrap();
        assert_eq!(old.date, None, "a row stored without fields reads as empty");
    }
}
