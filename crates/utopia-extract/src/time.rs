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
        } else if self.month.is_some() {
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

/// 文档自己的时间语境（决定 3）：从开头读出来、存在文档上、每块都带着
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentDating {
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
(\"2019\") is {\"y\": 2019} with granularity \"year\". \"to\" is added only for an interval the \
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
    let month = part(v, "m", 1..=12)?;
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

    fn ymd(y: i32, m: u32, d: u32) -> DateParts {
        DateParts {
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
                    year: 2024,
                    month: Some(3),
                    day: None,
                    hour: None,
                    minute: None,
                    second: None
                },
                to: Some(DateParts {
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
