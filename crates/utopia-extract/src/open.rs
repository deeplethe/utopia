//! 开放抽取（0044 第 1 刀，#729）：模型读一块，用**文档自己的话**写下它陈述了什么。
//!
//! 与 lib.rs 里带本体的契约不同，这条路的提示词里**没有类型清单、没有关系清单、
//! 没有文档日期**：关系短语照原文写（"acquired"、"修建"），时间照原文的字抄
//! （"去年冬天"），不算日期、不归一。本体是之后套上去的视图，不是抽取时的筛子。
//! 提示词里唯一逐块变化的是正文、开头与已知实体，所以系统消息是一个常量——
//! 前缀缓存能命中的就是它。
//!
//! 回复是紧凑 JSON——数组、短键、引文只列一次按下标引用——因为一块的输出动辄
//! 上百条，冗长的键名与每条重复一遍的引文烧的是 max_tokens，截断一次丢的是整块
//! 的后半。
//!
//! ```json
//! {"q": ["verbatim sentence", "..."],
//!  "e": [[0, "name or description", "kind word", 1]],
//!  "s": [[0, "relation phrase as written", 1, null, {"to": "k2", "amount": "$2 million"}, [0], 0]],
//!  "t": [[0, "verbatim time words", 0]],
//!  "n": [[0, "another name", 0]]}
//! ```
//!
//! - `q`：这一段里陈述了事情的句子，逐字抄，每句一次。其余数组按下标引用它。
//! - `e`：`[id, 名字或描述, 种类词, named]`。named = 1 是原文点了名的东西（人、组织、
//!   产品、地点、文件、法律、事件），0 是只描述了的东西（"the northern wing"、"farmland"）。
//!   名字不带引用语（"the Harbor Treaty signed on May 3, 1998" 叫 "Harbor Treaty"），
//!   也不带数字（"about 40 hectares of farmland" 是 "farmland" 加一条 `area` = "about 40 hectares"）。
//! - `s`：`[主语 ref, 短语, 宾语 ref 或 null, 字面值或 null, 限定词对象或 null, 时间 id 数组或 null, 引文下标]`。
//!   宾语与字面值恰有一个（解析层不裁，两个都给调用方看）；ref 是整数（本次回复的 `e`）
//!   或已知句柄字符串（"k1"）。一句话里超过两方参与、或者链接带着数额、头衔、条件、比较时，
//!   主要的一对进主宾，其余进限定词，键是原文里说明其角色的一两个词。
//! - `t`：`[id, 说明何时成立/发生/终止的原话, 引文下标]`。从不计算或归一日期；一条陈述可以
//!   指向几个时间（起、止各一个）。
//! - `n`：`[实体 ref, 这一段用的另一个名字, 引文下标]`。只收原文真写了的名字。
//!
//! 解析是逐项宽容的：一条坏记录计入 [`OpenExtraction::skipped`]，不毁掉整块；调用方
//! **必须**把这个数报出去——不报就是一次静默丢弃（#108 那类错）。

use std::collections::HashSet;

use serde_json::Value;
use utopia_llm::ChatMessage;

use crate::{
    close_brackets, json_block, json_text, opening_block, KnownEntity, KNOWN_BUDGET_CHARS,
};

/// A reference to a thing: `Local(n)` = entry n of this reply's `e` list; `Known(handle)` = a thing
/// listed in the prompt as already known from earlier chunks ("k1", "k2", …).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ref {
    Local(i64),
    Known(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum QualifierValue {
    Entity(Ref),
    Text(String),
}

#[derive(Debug, Clone)]
pub struct OpenEntity {
    pub id: i64,
    pub name: String,
    /// 原文说它是个什么，两三个词。不校验、不入本体
    pub kind: String,
    /// 原文点了名（true）还是只描述了（false）
    pub named: bool,
}

#[derive(Debug, Clone)]
pub struct OpenStatement {
    pub subject: Ref,
    /// the relation phrase as written, lowercase short phrase
    pub phrase: String,
    /// entity object, or
    pub object: Option<Ref>,
    /// literal value as written (figure with units, title, status word, date words)
    pub value: Option<String>,
    /// keyed by the document's role word ("to", "amount", "title", "compared to"); the order is
    /// serde_json's map order (alphabetical by key) and carries no meaning
    pub qualifiers: Vec<(String, QualifierValue)>,
    /// ids into [`OpenExtraction::times`]; the caller resolves them, an id no `t` entry carries
    /// resolves to nothing
    pub times: Vec<i64>,
    /// index into [`OpenExtraction::quotes`]; None when missing or out of range
    pub quote: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct OpenTime {
    pub id: i64,
    pub text: String,
    pub quote: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct OpenName {
    pub entity: Ref,
    pub name: String,
    pub quote: Option<usize>,
}

#[derive(Debug, Default)]
pub struct OpenExtraction {
    pub quotes: Vec<String>,
    pub entities: Vec<OpenEntity>,
    pub statements: Vec<OpenStatement>,
    pub times: Vec<OpenTime>,
    pub names: Vec<OpenName>,
    /// items skipped because they were malformed (counted per array item; must be reported by the caller)
    pub skipped: usize,
    /// the reply was truncated and repaired to the last complete object
    pub truncated: bool,
}

/// 系统消息。规则 1–8 是原型上测过的措辞，改写成紧凑数组；例子是中性的，不出自任何
/// 测量语料。**没有**本体、关系清单、文档日期——这条路的全部意义就在这三个「没有」
const OPEN_SYSTEM: &str = "\
You read one passage of a document and write down what it states, in the passage's own words. \
Output one JSON object and nothing else, shaped like this:\n\
\n\
{\"q\": [\"verbatim sentence\", \"...\"],\n\
 \"e\": [[0, \"name or description\", \"kind word\", 1]],\n\
 \"s\": [[0, \"relation phrase as written\", 1, null, {\"to\": \"k2\", \"amount\": \"$2 million\"}, [0], 0]],\n\
 \"t\": [[0, \"verbatim time words\", 0]],\n\
 \"n\": [[0, \"another name\", 0]]}\n\
\n\
1. \"q\" lists the sentences of the passage that state something, copied verbatim, each once. \
The last element of every \"s\", \"t\" and \"n\" entry is the index in \"q\" of the sentence \
that states it.\n\
2. \"e\" lists the things the passage talks about, one entry each: [id, name, kind, named]. \
id is an integer, unique within this reply. A thing the passage names (people, organizations, \
products, places, documents, laws, events) has named = 1 and is named as the passage writes \
it, without citation words: \"the Harbor Treaty signed on May 3, 1998\" is named \"Harbor \
Treaty\". A thing the passage only describes has named = 0 and is named by the words that say \
what it is (\"the northern wing\", \"farmland\", \"beekeepers\"). kind is what the thing is, in \
two or three words, as the passage says it. A name never carries a figure: \"about 40 hectares \
of farmland\" is the thing \"farmland\" with a statement \"area\" = \"about 40 hectares\". List \
each thing once.\n\
3. \"s\" lists the statements, one entry each: [subject, phrase, object, value, qualifiers, \
times, quote]. subject and object are refs: an id from \"e\", or the handle of a thing already \
recorded (\"k1\"). Exactly one of object and value is set; the other is null.\n\
   A statement with an object links two things. phrase is how the passage says the link, as a \
short lowercase phrase (\"acquired\", \"was designed by\", \"houses\", \"is the captain of\", \
\"修建\"); do not translate it into any vocabulary of your own.\n\
   A statement with a value is what the passage says about one thing by itself: a figure with \
its units, a percentage, an amount, a count, a title, a status. phrase is how the passage says \
what is measured or stated (\"area\", \"was completed\", \"joined\", \"占地面积\"); value is the \
literal as written. Wording that names or describes something is not a value: that something \
goes in \"e\" and is linked by a statement with an object. \"1,200 beekeepers joined\" is the \
thing \"beekeepers\" with \"joined\" = \"1,200\". Write every figure the passage states. In a \
table, a cell is a statement about its row's thing whose phrase is the column heading; leave \
out cells whose column heading is not in the passage.\n\
   A list is one statement per member, and so is a subject or object that joins several things \
(\"the city and the county funded the bridge\" is two statements).\n\
4. Nothing in a sentence is dropped. When more than two things take part, or the link carries \
a figure, a title, a condition or a comparison, put the main pair in subject and object and the \
rest in qualifiers: an object keyed by a word or two for each part's role, whose values are a \
ref or the words as written. \"the city council awarded the paving contract to Brightway \
Builders for $2 million\" gives council —awarded→ paving contract with {\"to\": the ref of \
Brightway Builders, \"amount\": \"$2 million\"}; \"Jane Doe is interim headmaster of Hillside \
School\" gives Jane Doe —is headmaster of→ Hillside School with {\"title\": \"interim \
headmaster\"}. A statement with a value takes qualifiers the same way: \"the plant cut water \
use by 12% compared to 2019\" gives the plant \"cut water use\" = \"12%\" with {\"compared \
to\": \"2019\"}. qualifiers is null when there are none.\n\
5. When a description has another thing folded into it, also write the statement that unfolds \
it: \"hospitals accredited by the Joint Commission\" also gives Joint Commission —accredited→ \
hospitals.\n\
6. \"t\" lists the time mentions, one entry each: [id, words, quote]. words are the words that \
say when something holds, happened or stopped, exactly as written (\"March 4, 2011\", \
\"去年冬天\", \"by the end of next season\"). Never compute, convert or normalise a date, and \
never write one the passage does not. A statement points at its time mentions by id in times \
— one for when it began or happened, another for when it ended; times is null when the \
passage gives none.\n\
7. \"n\" lists other names, one entry each: [ref, name, quote] — a short form, a former name, \
a spelling in another script that this passage uses for a thing in \"e\" or a thing already \
recorded. Only names actually written in the passage; never a pronoun or a description.\n\
8. State nothing the passage does not state, and state each thing once: with a value or with \
an object, not both. The Document line, the opening of the document and the list of things \
already recorded only say where the passage comes from; write nothing about them. If the \
passage states nothing, output {\"q\":[],\"e\":[],\"s\":[],\"t\":[],\"n\":[]}.\n\
\n\
Example. The passage \"The city council awarded the paving contract to Brightway Builders for \
$2 million on March 4, 2011.\" gives:\n\
{\"q\": [\"The city council awarded the paving contract to Brightway Builders for $2 million on March 4, 2011.\"],\n\
 \"e\": [[0, \"city council\", \"council\", 0], [1, \"paving contract\", \"contract\", 0], [2, \"Brightway Builders\", \"builders\", 1]],\n\
 \"s\": [[0, \"awarded\", 1, null, {\"to\": 2, \"amount\": \"$2 million\"}, [0], 0]],\n\
 \"t\": [[0, \"March 4, 2011\", 0]],\n\
 \"n\": []}";

/// 构造开放抽取的两条消息：常量系统消息 + `Document:` / 开头 / 已知实体 / `Passage:`。
///
/// 没有文档日期、没有类型与关系清单、没有属性——这些都不进提示词。开头与已知实体
/// 只说明这一段从哪来（模型被告知不从它们里面抽东西）。
pub fn build_open_messages(
    filename: &str,
    known: &[KnownEntity],
    opening: Option<&str>,
    chunk_text: &str,
) -> Vec<ChatMessage> {
    // 已知实体紧挨着正文：服从性靠位置，理由见 lib.rs 里 known_block 的注释
    let user = format!(
        "Document: {filename}\n{}{}\nPassage:\n{chunk_text}",
        opening_block(opening),
        known_block(known)
    );
    vec![
        ChatMessage {
            role: "system".into(),
            content: OPEN_SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}

/// 「本文档已经认下的实体」的开放版：`k1: <name> (<type_key>)`，指令按紧凑数组的
/// 说法写。lib.rs 那版的指令讲的是 subject_ref/object_ref 与「给它同样的类型」，
/// 都是带本体那条路的字段，这里没有。预算与截断规则同那版
fn known_block(known: &[KnownEntity]) -> String {
    if known.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    let mut used = 0usize;
    for entity in known {
        used += entity.handle.chars().count()
            + entity.type_key.chars().count()
            + entity.name.chars().count()
            + 6;
        if used > KNOWN_BUDGET_CHARS {
            break;
        }
        let kind = entity.type_key.trim();
        if kind.is_empty() {
            lines.push(format!("  {}: {}", entity.handle, entity.name));
        } else {
            lines.push(format!("  {}: {} ({kind})", entity.handle, entity.name));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\nAlready recorded from earlier parts of this same document:\n{}\n\
         \n\
         When the passage refers to one of these, use its handle (\"k1\") as the ref and do not \
         list it again in \"e\"; a shortened form the passage uses for it goes in \"n\". If it \
         is a different thing, list it as the passage names it; do not force it onto this \
         list.\n",
        lines.join("\n")
    )
}

/// 截断的回复从第一个 `{` 取到**结尾**，交给修补自己退到最后一个完整条目。
///
/// `json_block` 取的是第一个 `{` 到最后一个 `}`，对紧凑回复这一刀切错两回：`}` 只在
/// 限定词对象与结尾出现，截断的回复要么一个 `}` 都没有（它报「没有 JSON」，修补
/// 根本没机会跑），要么最后一个 `}` 是半路某条陈述的限定词（它把后面完整的条目
/// 一并切掉）。围栏与思考过程的切法与它共用
fn json_tail(raw: &str) -> Option<&str> {
    let text = json_text(raw);
    text.find('{').map(|s| &text[s..])
}

/// 紧凑格式的截断修补：退到**最后一个完整的数组或对象**的结尾再把括号补齐。
///
/// lib.rs 的 `repair_truncated` 只认 `}` 作为回退点——那边每条记录都是对象，以 `}`
/// 结尾。这里每条记录是数组，限定词全是 null 的回复里可能一个 `}` 都没有，只认 `}`
/// 就是整块作废。所以回退点是最后一个 `]` 或 `}`，谁靠后用谁；括号是否在字符串里
/// 由 `close_brackets` 判断
fn repair_truncated_compact(json: &str) -> Option<String> {
    let mut cut = json.len();
    for _ in 0..64 {
        let idx = json[..cut].rfind([']', '}'])?;
        if let Some(closed) = close_brackets(&json[..=idx]) {
            if serde_json::from_str::<Value>(&closed).is_ok() {
                return Some(closed);
            }
        }
        cut = idx;
    }
    None
}

/// 引文下标那一格放了既不是 null 也不是整数的东西
struct Malformed;

/// 顶层某个键下的数组；缺了或不是数组就当空——那不是一条坏记录，是整段没这一类
fn items<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// 整数 → 本回复的 `e`；"k" 加数字 → 提示词给的已知句柄；别的都不是 ref
fn parse_ref(v: &Value) -> Option<Ref> {
    if let Some(n) = v.as_i64() {
        return Some(Ref::Local(n));
    }
    let s = v.as_str()?.trim();
    let digits = s.strip_prefix('k')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Ref::Known(s.to_string()))
}

/// 第 i 格的非空字符串（去掉首尾空白）
fn text_at(arr: &[Value], i: usize) -> Option<&str> {
    arr.get(i)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 第 i 格的引文下标。缺格、null、越界、指向空引文都是 None——条目本身照收；
/// 放了别的东西才算坏
fn quote_at(arr: &[Value], i: usize, quotes: &[String]) -> Result<Option<usize>, Malformed> {
    match arr.get(i) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_u64().ok_or(Malformed)?;
            Ok(usize::try_from(n)
                .ok()
                .filter(|&n| quotes.get(n).is_some_and(|q| !q.is_empty())))
        }
    }
}

/// 字面值照原样：字符串去首尾空白；模型偶尔把数字写成 JSON 数字，照它的写法转回文本
fn literal(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => {
            let s = s.trim();
            (!s.is_empty()).then(|| s.to_string())
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 限定词的值：整数或 "k…" 是实体引用，别的字符串是原文的话；其余类型悄悄丢
fn qualifier_value(v: &Value) -> Option<QualifierValue> {
    if let Some(r) = parse_ref(v) {
        return Some(QualifierValue::Entity(r));
    }
    let s = v.as_str()?.trim();
    (!s.is_empty()).then(|| QualifierValue::Text(s.to_string()))
}

/// `[id, name, kind, named]`：id 与 name 缺一不可；kind 缺了是空串；named 缺了当点了名
fn parse_entity(arr: &[Value]) -> Option<OpenEntity> {
    let id = arr.first()?.as_i64()?;
    let name = text_at(arr, 1)?.to_string();
    let kind = arr
        .get(2)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let named = match arr.get(3) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        _ => true,
    };
    Some(OpenEntity {
        id,
        name,
        kind,
        named,
    })
}

/// `[subject, phrase, object, value, qualifiers, times, quote]`。七格都得在：短了的多半是
/// 截断修补切出来的半条，引文都没有，收下也没法核对。宾语格里放了不是 ref 的东西整条
/// 不要；宾语与字面值同时有则两个都留给调用方
fn parse_statement(arr: &[Value], quotes: &[String]) -> Option<OpenStatement> {
    if arr.len() < 7 {
        return None;
    }
    let subject = parse_ref(&arr[0])?;
    let phrase = text_at(arr, 1)?.to_string();
    let object = match &arr[2] {
        Value::Null => None,
        v => Some(parse_ref(v)?),
    };
    let value = literal(&arr[3]);
    let qualifiers = arr[4]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    let k = k.trim();
                    (!k.is_empty()).then(|| Some((k.to_string(), qualifier_value(v)?)))?
                })
                .collect()
        })
        .unwrap_or_default();
    let times = arr[5]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    let quote = quote_at(arr, 6, quotes).ok()?;
    Some(OpenStatement {
        subject,
        phrase,
        object,
        value,
        qualifiers,
        times,
        quote,
    })
}

/// `[id, words, quote]`
fn parse_time(arr: &[Value], quotes: &[String]) -> Option<OpenTime> {
    let id = arr.first()?.as_i64()?;
    let text = text_at(arr, 1)?.to_string();
    let quote = quote_at(arr, 2, quotes).ok()?;
    Some(OpenTime { id, text, quote })
}

/// `[ref, name, quote]`
fn parse_name(arr: &[Value], quotes: &[String]) -> Option<OpenName> {
    let entity = parse_ref(arr.first()?)?;
    let name = text_at(arr, 1)?.to_string();
    let quote = quote_at(arr, 2, quotes).ok()?;
    Some(OpenName {
        entity,
        name,
        quote,
    })
}

/// **一条坏记录不该毁掉一整块**（同 lib.rs 的 `parse_response`）。
///
/// 先取 JSON 块解成 `Value`（截断就先退到最后一个完整条目补齐括号，并标 `truncated`），
/// 再逐项宽容地解：形状不对的条目计入 `skipped`，好的照收。引文下标越界不算坏，
/// 那条陈述留下、引文为 None。`e` 里重复的 id 留第一个，其余计入 `skipped`
pub fn parse_open_response(raw: &str) -> anyhow::Result<OpenExtraction> {
    // 先按常规取块（第一个 `{` 到最后一个 `}`）：解得开就是完整回复，结尾之后
    // 哪怕跟着废话也不算截断。解不开才从第一个 `{` 取到结尾去修
    let block = json_block(raw)
        .and_then(|b| serde_json::from_str::<Value>(&b).map_err(anyhow::Error::from));
    let (value, truncated) = match block {
        Ok(v) => (v, false),
        Err(e) => {
            let fixed = json_tail(raw)
                .and_then(repair_truncated_compact)
                // 补不回来才是真解析失败：连一个完整条目都没有
                .ok_or_else(|| anyhow::anyhow!("Failed to parse open extraction JSON: {e}"))?;
            let v = serde_json::from_str::<Value>(&fixed)
                .map_err(|e| anyhow::anyhow!("Failed to parse open extraction JSON: {e}"))?;
            (v, true)
        }
    };

    let mut out = OpenExtraction {
        truncated,
        ..Default::default()
    };
    // 引文按下标引用，所以坏的一格不能删——留个空串占位，指向它的下标解成 None
    for q in items(&value, "q") {
        match q.as_str() {
            Some(s) => out.quotes.push(s.trim().to_string()),
            None => {
                out.quotes.push(String::new());
                out.skipped += 1;
            }
        }
    }
    let mut seen = HashSet::new();
    for item in items(&value, "e") {
        match item.as_array().and_then(|a| parse_entity(a)) {
            Some(e) if seen.insert(e.id) => out.entities.push(e),
            _ => out.skipped += 1,
        }
    }
    for item in items(&value, "s") {
        match item
            .as_array()
            .and_then(|a| parse_statement(a, &out.quotes))
        {
            Some(s) => out.statements.push(s),
            None => out.skipped += 1,
        }
    }
    for item in items(&value, "t") {
        match item.as_array().and_then(|a| parse_time(a, &out.quotes)) {
            Some(t) => out.times.push(t),
            None => out.skipped += 1,
        }
    }
    for item in items(&value, "n") {
        match item.as_array().and_then(|a| parse_name(a, &out.quotes)) {
            Some(n) => out.names.push(n),
            None => out.skipped += 1,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份完整的紧凑回复：本地与已知两种 ref、两种限定词、时间、别名
    const FULL: &str = r#"{"q": ["The city council awarded the paving contract to Brightway Builders for $2 million on March 4, 2011.",
        "Nebula (formerly Starlight Labs) leased the northern wing from Harbor Estates for ten years until the end of 2019."],
     "e": [[0, "city council", "council", 0], [1, "paving contract", "contract", 0],
           [2, "Brightway Builders", "builders", 1], [3, "northern wing", "wing", 0]],
     "s": [[0, "awarded", 1, null, {"to": 2, "amount": "$2 million"}, [0], 0],
           ["k1", "leased", 3, null, {"from": "k2", "term": "ten years"}, [1], 1],
           [1, "worth", null, "$2 million", null, null, 0]],
     "t": [[0, "March 4, 2011", 0], [1, "until the end of 2019", 1]],
     "n": [["k1", "Starlight Labs", 1]]}"#;

    /// serde_json 的对象按键排序（没开 preserve_order），限定词的顺序不承载意义
    fn by_key(mut q: Vec<(String, QualifierValue)>) -> Vec<(String, QualifierValue)> {
        q.sort_by(|a, b| a.0.cmp(&b.0));
        q
    }

    #[test]
    fn a_full_reply_parses_into_the_structs() {
        let x = parse_open_response(FULL).unwrap();
        assert!(!x.truncated);
        assert_eq!(x.skipped, 0);
        assert_eq!(x.quotes.len(), 2);
        assert!(x.quotes[0].starts_with("The city council"));

        assert_eq!(x.entities.len(), 4);
        assert_eq!(x.entities[0].id, 0);
        assert_eq!(x.entities[0].name, "city council");
        assert_eq!(x.entities[0].kind, "council");
        assert!(!x.entities[0].named);
        assert_eq!(x.entities[2].name, "Brightway Builders");
        assert!(x.entities[2].named);

        assert_eq!(x.statements.len(), 3);
        let s = &x.statements[0];
        assert_eq!(s.subject, Ref::Local(0));
        assert_eq!(s.phrase, "awarded");
        assert_eq!(s.object, Some(Ref::Local(1)));
        assert_eq!(s.value, None);
        assert_eq!(
            by_key(s.qualifiers.clone()),
            by_key(vec![
                ("to".to_string(), QualifierValue::Entity(Ref::Local(2))),
                (
                    "amount".to_string(),
                    QualifierValue::Text("$2 million".into())
                ),
            ])
        );
        assert_eq!(s.times, vec![0]);
        assert_eq!(s.quote, Some(0));

        let s = &x.statements[1];
        assert_eq!(s.subject, Ref::Known("k1".into()));
        assert_eq!(s.object, Some(Ref::Local(3)));
        assert_eq!(
            by_key(s.qualifiers.clone()),
            by_key(vec![
                (
                    "from".to_string(),
                    QualifierValue::Entity(Ref::Known("k2".into()))
                ),
                ("term".to_string(), QualifierValue::Text("ten years".into())),
            ])
        );
        assert_eq!(s.times, vec![1]);
        assert_eq!(s.quote, Some(1));

        let s = &x.statements[2];
        assert_eq!(s.object, None);
        assert_eq!(s.value.as_deref(), Some("$2 million"));
        assert!(s.qualifiers.is_empty());
        assert!(s.times.is_empty());

        assert_eq!(x.times.len(), 2);
        assert_eq!(x.times[1].id, 1);
        assert_eq!(x.times[1].text, "until the end of 2019");
        assert_eq!(x.times[1].quote, Some(1));

        assert_eq!(x.names.len(), 1);
        assert_eq!(x.names[0].entity, Ref::Known("k1".into()));
        assert_eq!(x.names[0].name, "Starlight Labs");
        assert_eq!(x.names[0].quote, Some(1));
    }

    /// 截在 `n` 数组中间：`t` 之前的全留下，`truncated` 标出来
    #[test]
    fn a_cut_off_reply_keeps_what_was_complete() {
        let cut = FULL.find("\"n\": [[\"k1\", \"Starl").unwrap() + "\"n\": [[\"k1\", \"Starl".len();
        let x = parse_open_response(&FULL[..cut]).unwrap();
        assert!(x.truncated, "截断要标出来");
        assert_eq!(x.statements.len(), 3);
        assert_eq!(x.times.len(), 2);
        assert!(x.names.is_empty());
        assert_eq!(x.skipped, 0);
    }

    /// 限定词全是 null 的回复里一个 `}` 都没有：只认 `}` 的修补会把整块作废，
    /// 这里退到最后一个完整的 `]`
    #[test]
    fn a_cut_off_reply_without_any_closing_brace_is_still_repaired() {
        let raw = r#"{"q": ["A is b.", "A was c."], "e": [[0, "A", "thing", 1]],
            "s": [[0, "is", null, "b", null, null, 0], [0, "was", null, "c", nu"#;
        let x = parse_open_response(raw).unwrap();
        assert!(x.truncated);
        assert_eq!(x.entities.len(), 1);
        assert_eq!(x.statements.len(), 1);
        assert_eq!(x.statements[0].value.as_deref(), Some("b"));
    }

    /// 连一个完整条目都没有时仍然报失败——容错不是把空结果说成成功
    #[test]
    fn a_reply_with_nothing_complete_still_fails() {
        assert!(parse_open_response(r#"{"q": ["a", "b"#).is_err());
    }

    /// 引文下标越界或指向坏格子：陈述留下，引文为 None
    #[test]
    fn a_bad_quote_index_becomes_none_and_the_statement_stays() {
        let raw = r#"{"q": ["A is b.", 42], "e": [[0, "A", "thing", 1]],
            "s": [[0, "is", null, "b", null, null, 7], [0, "is", null, "b", null, null, 1],
                  [0, "is", null, "b", null, null, null], [0, "is", null, "b", null, null, 0]],
            "t": [[0, "in 2019", 9]], "n": [[0, "Ay", 9]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.statements.len(), 4);
        assert_eq!(x.statements[0].quote, None, "越界");
        assert_eq!(x.statements[1].quote, None, "指向不是字符串的那格");
        assert_eq!(x.statements[2].quote, None, "null");
        assert_eq!(x.statements[3].quote, Some(0));
        assert_eq!(x.times[0].quote, None);
        assert_eq!(x.names[0].quote, None);
        assert_eq!(x.skipped, 1, "只有 q 里那个 42 算坏");
    }

    /// 坏条目逐个计数：缺名字的实体、重复的 id、不是 ref 的主语、少一格的陈述、
    /// 引文格放了字符串、没文字的时间、没名字的别名
    #[test]
    fn malformed_items_are_counted_not_fatal() {
        let raw = r#"{"q": ["A is b."],
            "e": [[0, "A", "thing", 1], [5], [0, "A again", "thing", 1], "not an array", [1, "  "]],
            "s": [[0, "is", null, "b", null, null, 0],
                  ["x1", "is", null, "b", null, null, 0],
                  [0, "is", null, "b", null, null],
                  [0, "is", null, "b", null, null, "0"],
                  [0, "", null, "b", null, null, 0],
                  [0, "is", "nobody", null, null, null, 0]],
            "t": [[0, "in 2019", 0], ["a", "in 2019"], [1]],
            "n": [[0, "Ay", 0], [0], [{}, "Ay", 0]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.entities.len(), 1);
        assert_eq!(x.statements.len(), 1);
        assert_eq!(x.times.len(), 1);
        assert_eq!(x.names.len(), 1);
        assert_eq!(x.skipped, 4 + 5 + 2 + 2);
        assert!(!x.truncated);
    }

    /// 宾语与字面值同时有：照收，两个都给调用方看，这里不裁
    #[test]
    fn a_statement_with_both_object_and_value_is_passed_through() {
        let raw = r#"{"q": ["A is b."], "e": [[0, "A", "thing", 1], [1, "B", "thing", 1]],
            "s": [[0, "is", 1, "b", null, null, 0]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.statements.len(), 1);
        assert_eq!(x.statements[0].object, Some(Ref::Local(1)));
        assert_eq!(x.statements[0].value.as_deref(), Some("b"));
        assert_eq!(x.skipped, 0);
    }

    /// named 认 1/0 与 true/false，缺了当点了名；kind 缺了是空串
    #[test]
    fn named_takes_numbers_and_booleans_and_defaults_to_true() {
        let raw = r#"{"e": [[0, "a", "k", 1], [1, "b", "k", 0], [2, "c", "k", true],
                            [3, "d", "k", false], [4, "e"]]}"#;
        let x = parse_open_response(raw).unwrap();
        let named: Vec<bool> = x.entities.iter().map(|e| e.named).collect();
        assert_eq!(named, vec![true, false, true, false, true]);
        assert_eq!(x.entities[4].kind, "");
        assert_eq!(x.skipped, 0);
    }

    /// 限定词：整数与 "k…" 是实体，别的字符串是原文，其余类型悄悄丢；
    /// 字面值写成 JSON 数字也照它的写法收
    #[test]
    fn qualifiers_and_literals_take_their_shapes() {
        let raw = r#"{"q": ["x"], "e": [[0, "a", "k", 1]],
            "s": [[0, "cut water use", null, 12, {"compared to": "2019", "at": "k3", "by": 0, "ratio": 1.5, "flag": true, "": "x"}, null, 0]]}"#;
        let x = parse_open_response(raw).unwrap();
        let s = &x.statements[0];
        assert_eq!(s.value.as_deref(), Some("12"));
        assert_eq!(
            by_key(s.qualifiers.clone()),
            by_key(vec![
                (
                    "compared to".to_string(),
                    QualifierValue::Text("2019".into())
                ),
                (
                    "at".to_string(),
                    QualifierValue::Entity(Ref::Known("k3".into()))
                ),
                ("by".to_string(), QualifierValue::Entity(Ref::Local(0))),
            ])
        );
    }

    /// 围栏与前后废话照旧容忍（走的是同一个 json_block）
    #[test]
    fn a_fenced_reply_parses() {
        let raw = "Here you go:\n```json\n{\"q\": [], \"e\": [[0, \"a\", \"k\", 1]], \"s\": [], \"t\": [], \"n\": []}\n```";
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.entities.len(), 1);
    }

    fn known(handle: &str, name: &str, type_key: &str) -> KnownEntity {
        KnownEntity {
            handle: handle.into(),
            type_key: type_key.into(),
            name: name.into(),
        }
    }

    /// 提示词里没有文档日期、没有类型与关系清单；有文件名、开头、已知句柄与正文，
    /// 并且按这个顺序
    #[test]
    fn the_prompt_carries_no_ontology_and_no_document_date() {
        let msgs = build_open_messages(
            "annual-report.txt",
            &[
                known("k1", "Nebula Technologies Inc.", "organization"),
                known("k2", "Harbor Estates", ""),
            ],
            Some("Nebula Technologies Inc. Annual Report 2019"),
            "Nebula leased the northern wing.",
        );
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        let system = &msgs[0].content;
        let user = &msgs[1].content;
        for s in [system, user] {
            assert!(!s.contains("Document date"), "{s}");
            assert!(!s.contains("Resolve relative time"), "{s}");
            assert!(!s.contains("Entity types"), "{s}");
            assert!(!s.contains("Relation types"), "{s}");
            assert!(!s.contains("Attributes ("), "{s}");
        }
        assert!(system.contains("Never compute, convert or normalise a date"));
        assert!(system.contains("do not translate it into any vocabulary of your own"));
        assert!(system.contains("\"s\": [[0, \"relation phrase as written\", 1, null"));

        let doc = user.find("Document: annual-report.txt").unwrap();
        let opening = user.find("Opening of this document").unwrap();
        assert!(user.contains("Annual Report 2019"));
        let recorded = user.find("Already recorded from earlier parts").unwrap();
        assert!(user.contains("  k1: Nebula Technologies Inc. (organization)\n"));
        assert!(user.contains("  k2: Harbor Estates\n"), "空类型不留括号");
        let passage = user
            .find("Passage:\nNebula leased the northern wing.")
            .unwrap();
        assert!(doc < opening && opening < recorded && recorded < passage);
        assert!(user.ends_with("Nebula leased the northern wing."));
    }

    /// 第一块：没有开头也没有已知实体，两段都不出现
    #[test]
    fn the_first_chunk_carries_neither_opening_nor_known_block() {
        let msgs = build_open_messages("a.txt", &[], None, "text");
        let user = &msgs[1].content;
        assert_eq!(user, "Document: a.txt\n\nPassage:\ntext");
        assert!(!user.contains("Opening of this document"));
        assert!(!user.contains("Already recorded"));
    }
}
