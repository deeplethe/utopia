//! 开放抽取（0044 第 1 刀，#729）：模型读一块，用**文档自己的话**写下它陈述了什么。
//!
//! 与 lib.rs 里带本体的契约不同，这条路的提示词里**没有类型清单、没有关系清单、
//! 没有文档日期**：关系短语照原文写（"acquired"、"修建"），时间照原文的字抄
//! （"去年冬天"），不算日期、不归一。本体是之后套上去的视图，不是抽取时的筛子。
//! 提示词里唯一逐块变化的是正文、开头与已知实体，所以系统消息是一个常量——
//! 前缀缓存能命中的就是它。
//!
//! 回复是紧凑 JSON——数组、短键——因为一块的输出动辄上百条，冗长的键名烧的是
//! max_tokens，截断一次丢的是整块的后半。但**陈述指东西用名字，引文每条自己抄一遍**，
//! 不编号：编号版在一块密集的财报上测过，deepseek-v4-flash 把 id 串了位（"NVIDIA has
//! reached AI"、"tokens are tokens"，原文说的是 "AI has reached its inflection point"），
//! 同一块的两次回复编号还不一致；原型的名字 + 逐条引文版 333 条里 0 条编造。
//! 多花的 token 买的是不串位。
//!
//! ```json
//! {"e": [["Harbor Bridge", "bridge", 1], ["farmland", "land", 0]],
//!  "s": [["city council of Westbrook", "awarded", "paving contract", null, {"to": "Brightway Builders", "amount": "$2 million"}, "March 4, 2011", null, "The city council of Westbrook awarded the paving contract for the Harbor Bridge to Brightway Builders for $2 million on March 4, 2011."]],
//!  "n": [["Brightway Builders", "Brightway", "Brightway Builders, known locally as Brightway, is based in Port Ellen."]]}
//! ```
//!
//! - `e`：`[名字或描述, 种类词, named]`。陈述提到的每样东西都在这里列一次，用的正是
//!   陈述里写的那个名字。named = 1 是原文点了名的东西（人、组织、产品、地点、文件、
//!   法律、事件），0 是只描述了的东西（"the northern wing"、"farmland"）。名字不带引用语
//!   （"the Harbor Treaty signed on May 3, 1998" 叫 "Harbor Treaty"），也不带数字
//!   （"about 40 hectares of farmland" 是 "farmland" 加一条 `area` = "about 40 hectares"）。
//!   没有 id。
//! - `s`：`[主语名, 短语, 宾语名或 null, 字面值或 null, 限定词对象或 null, 何时或 null,
//!   何时终止或 null, 引文]`。主语与宾语是 `e` 里（或提示词已知清单里）的名字，拼写一模
//!   一样。宾语与字面值恰有一个（解析层不裁，两个都给调用方看）。一句话里超过两方参与、
//!   或者链接带着数额、头衔、条件、比较时，主要的一对进主宾，其余进限定词，键是原文里
//!   说明其角色的一两个词，值是列出的东西的名字或原文的话。何时/终止照原文的字抄，
//!   从不计算。引文是陈述它的那一句，逐字抄。
//! - `n`：`[e 里列的名字, 这一段用的另一个名字, 引文]`。只收原文真写了的名字。
//!
//! 主语、宾语的名字对不对得上 `e`（或已知清单）**这里不查**，交给服务端——那边手里有
//! 已知实体的句柄与库里的名字，这里只有字。
//!
//! 解析是逐项宽容的：一条坏记录计入 [`OpenExtraction::skipped`]，不毁掉整块；调用方
//! **必须**把这个数报出去——不报就是一次静默丢弃（#108 那类错）。

use std::collections::HashSet;

use serde_json::Value;
use utopia_llm::ChatMessage;

use crate::{
    close_brackets, json_block, json_text, opening_block, KnownEntity, KNOWN_BUDGET_CHARS,
};

#[derive(Debug, Clone)]
pub struct OpenEntity {
    /// the name the statements use for it, exactly as listed
    pub name: String,
    /// 原文说它是个什么，两三个词。不校验、不入本体
    pub kind: String,
    /// 原文点了名（true）还是只描述了（false）
    pub named: bool,
}

#[derive(Debug, Clone)]
pub struct OpenStatement {
    /// the name of a thing, as listed in `e` or in the known-things list; not resolved here
    pub subject: String,
    /// the relation phrase as written, lowercase short phrase
    pub phrase: String,
    /// entity object by name, or
    pub object: Option<String>,
    /// literal value as written (figure with units, title, status word, date words)
    pub value: Option<String>,
    /// keyed by the document's role word ("to", "amount", "title", "compared to"); the value is
    /// text, which may be the name of a listed thing. The order is serde_json's map order
    /// (alphabetical by key) and carries no meaning
    pub qualifiers: Vec<(String, String)>,
    /// the words that say when it holds, happened or began, exactly as written
    pub when: Option<String>,
    /// the words that say when it stopped, exactly as written
    pub ended: Option<String>,
    /// the sentence of the passage that states it, verbatim
    pub quote: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenName {
    /// the thing's name as listed in `e` or in the known-things list
    pub entity: String,
    pub name: String,
    pub quote: Option<String>,
}

#[derive(Debug, Default)]
pub struct OpenExtraction {
    pub entities: Vec<OpenEntity>,
    pub statements: Vec<OpenStatement>,
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
{\"e\": [[\"name or description\", \"kind word\", 1]],\n\
 \"s\": [[\"subject name\", \"relation phrase as written\", \"object name\", null, {\"to\": \"name of a listed thing\", \"amount\": \"$2 million\"}, \"when it holds or happened, as written\", \"when it ended, as written\", \"the sentence that states it, verbatim\"]],\n\
 \"n\": [[\"name as listed in e\", \"another name\", \"the sentence that uses it, verbatim\"]]}\n\
\n\
1. \"e\" lists the things the passage talks about, one entry each: [name, kind, named]. Every \
thing a statement refers to is listed here once, under exactly the name the statements use. A \
thing the passage names (people, organizations, products, places, documents, laws, events) has \
named = 1 and is named as the passage writes it, without citation words: \"the Harbor Treaty \
signed on May 3, 1998\" is named \"Harbor Treaty\". A thing the passage only describes has \
named = 0 and is named by the words that say what it is (\"the northern wing\", \"farmland\", \
\"beekeepers\"). kind is what the thing is, in two or three words, as the passage says it. A \
name never carries a figure: \"about 40 hectares of farmland\" is the thing \"farmland\" with a \
statement \"area\" = \"about 40 hectares\". List each thing once.\n\
2. \"s\" lists the statements, one entry each: [subject, phrase, object, value, qualifiers, \
when, ended, quote]. subject and object are names, written exactly as listed in \"e\" or in \
the list of things already recorded, with the same spelling every time. Exactly one of object \
and value is set; the other is null.\n\
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
3. Nothing in a sentence is dropped. When more than two things take part, or the link carries \
a figure, a title, a condition or a comparison, put the main pair in subject and object and the \
rest in qualifiers: an object keyed by a word or two for each part's role, whose values are the \
name of a listed thing or the words as written. \"the city council awarded the paving contract \
to Brightway Builders for $2 million\" gives city council —awarded→ paving contract with \
{\"to\": \"Brightway Builders\", \"amount\": \"$2 million\"}; \"Jane Doe is interim headmaster \
of Hillside School\" gives Jane Doe —is headmaster of→ Hillside School with {\"title\": \
\"interim headmaster\"}. A statement with a value takes qualifiers the same way: \"the plant \
cut water use by 12% compared to 2019\" gives the plant \"cut water use\" = \"12%\" with \
{\"compared to\": \"2019\"}. qualifiers is null when there are none.\n\
4. When a description has another thing folded into it, also write the statement that unfolds \
it: \"hospitals accredited by the Joint Commission\" also gives Joint Commission —accredited→ \
hospitals. Do not unfold a description that only names what it belongs to or is about: \"the \
northern wing of the palace\" and \"the use of the coating\" give no statement, the palace and \
the coating are part of the description.\n\
5. when is the words that say when the statement holds, happened or began, and ended is the \
words that say when it stopped, each copied exactly as written (\"March 4, 2011\", \"去年冬天\", \
\"by the end of next season\"). Never compute, convert or normalise a date, and never write one \
the passage does not. Each is null when the passage gives none.\n\
6. quote is the sentence of the passage that states it, copied verbatim: one sentence, and when \
two sentences are needed, the one that carries the link. Every \"s\" and \"n\" entry carries its \
own quote.\n\
7. \"n\" lists other names, one entry each: [name as listed, other name, quote] — a short form, \
a former name, a spelling in another script that this passage uses for a thing in \"e\" or a \
thing already recorded. Only names actually written in the passage; never a pronoun or a \
description.\n\
8. State nothing the passage does not state, and state each thing once: with a value or with \
an object, not both. The Document line, the opening of the document and the list of things \
already recorded only say where the passage comes from; write nothing about them. If the \
passage states nothing, output {\"e\":[],\"s\":[],\"n\":[]}.\n\
\n\
Example. The passage \"The city council awarded the paving contract to Brightway Builders for \
$2 million on March 4, 2011.\" gives:\n\
{\"e\": [[\"city council\", \"council\", 0], [\"paving contract\", \"contract\", 0], [\"Brightway Builders\", \"builders\", 1]],\n\
 \"s\": [[\"city council\", \"awarded\", \"paving contract\", null, {\"to\": \"Brightway Builders\", \"amount\": \"$2 million\"}, \"March 4, 2011\", null, \"The city council awarded the paving contract to Brightway Builders for $2 million on March 4, 2011.\"]],\n\
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

/// 「本文档已经认下的实体」的开放版：`- <name> (<kind>)`，句柄不给模型看——这条路
/// 陈述里写的是名字，模型照这里的拼写写，对回哪个实体由服务端拿名字去查。lib.rs
/// 那版的指令讲的是 subject_ref/object_ref 与「给它同样的类型」，都是带本体那条路的
/// 字段，这里没有。预算与截断规则同那版（句柄不显示，所以不计入）
fn known_block(known: &[KnownEntity]) -> String {
    if known.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    let mut used = 0usize;
    for entity in known {
        used += entity.type_key.chars().count() + entity.name.chars().count() + 6;
        if used > KNOWN_BUDGET_CHARS {
            break;
        }
        let kind = entity.type_key.trim();
        if kind.is_empty() {
            lines.push(format!("- {}", entity.name));
        } else {
            lines.push(format!("- {} ({kind})", entity.name));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\nAlready recorded from earlier parts of this same document:\n{}\n\
         \n\
         When the passage refers to one of these, write its name exactly as it is listed here \
         and do not list it again in \"e\"; a shortened form the passage uses for it goes in \
         \"n\". If it is a different thing, list it as the passage names it; do not force it \
         onto this list.\n",
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
pub(crate) fn repair_truncated_compact(json: &str) -> Option<String> {
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

/// 顶层某个键下的数组；缺了或不是数组就当空——那不是一条坏记录，是整段没这一类
fn items<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// 第 i 格的非空字符串（去掉首尾空白）——缺一不可的格子用它
fn text_at(arr: &[Value], i: usize) -> Option<&str> {
    arr.get(i)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 可选的文字格：字符串去首尾空白；null、空串与放错的类型都是 None——那一格放错了
/// 东西不算整条坏，坏的只是那一格
fn opt_text(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 字面值照原样：字符串去首尾空白；模型偶尔把数字写成 JSON 数字，照它的写法转回文本。
/// 限定词的值走同一条规则
fn literal(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        v => opt_text(v),
    }
}

/// 限定词：对象里值为字符串或数字的那些；键去首尾空白，空键与别的类型悄悄丢；
/// 不是对象当作没有限定词
fn qualifiers(v: &Value) -> Vec<(String, String)> {
    v.as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    let k = k.trim();
                    (!k.is_empty()).then(|| Some((k.to_string(), literal(v)?)))?
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 去重键：首尾空白去掉、内部空白折成一个、小写。"Harbor  Bridge" 与 "harbor bridge"
/// 是同一条
fn name_key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// `[name, kind, named]`：name 缺不得；kind 缺了是空串；named 缺了当点了名
fn parse_entity(arr: &[Value]) -> Option<OpenEntity> {
    let name = text_at(arr, 0)?.to_string();
    let kind = arr
        .get(1)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let named = match arr.get(2) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        _ => true,
    };
    Some(OpenEntity { name, kind, named })
}

/// `[subject, phrase, object, value, qualifiers, when, ended, quote]`。八格都得在：短了的
/// 多半是截断修补切出来的半条，引文都没有，收下也没法核对。主语与短语缺不得；其余格
/// 放错了类型只是那一格为 None。宾语与字面值同时有则两个都留给调用方
fn parse_statement(arr: &[Value]) -> Option<OpenStatement> {
    if arr.len() < 8 {
        return None;
    }
    let subject = text_at(arr, 0)?.to_string();
    let phrase = text_at(arr, 1)?.to_string();
    Some(OpenStatement {
        subject,
        phrase,
        object: opt_text(&arr[2]),
        value: literal(&arr[3]),
        qualifiers: qualifiers(&arr[4]),
        when: opt_text(&arr[5]),
        ended: opt_text(&arr[6]),
        quote: opt_text(&arr[7]),
    })
}

/// `[entity name, other name, quote]`
fn parse_name(arr: &[Value]) -> Option<OpenName> {
    let entity = text_at(arr, 0)?.to_string();
    let name = text_at(arr, 1)?.to_string();
    let quote = arr.get(2).and_then(opt_text);
    Some(OpenName {
        entity,
        name,
        quote,
    })
}

/// **一条坏记录不该毁掉一整块**（同 lib.rs 的 `parse_response`）。
///
/// 先取 JSON 块解成 `Value`（截断就先退到最后一个完整条目补齐括号，并标 `truncated`），
/// 再逐项宽容地解：形状不对的条目计入 `skipped`，好的照收。`e` 里重名（去空白、
/// 不分大小写）留第一个，其余计入 `skipped`。主语、宾语的名字不在这里对回 `e`
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
    let mut seen = HashSet::new();
    for item in items(&value, "e") {
        match item.as_array().and_then(|a| parse_entity(a)) {
            Some(e) if seen.insert(name_key(&e.name)) => out.entities.push(e),
            _ => out.skipped += 1,
        }
    }
    for item in items(&value, "s") {
        match item.as_array().and_then(|a| parse_statement(a)) {
            Some(s) => out.statements.push(s),
            None => out.skipped += 1,
        }
    }
    for item in items(&value, "n") {
        match item.as_array().and_then(|a| parse_name(a)) {
            Some(n) => out.names.push(n),
            None => out.skipped += 1,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AWARD: &str =
        "The city council awarded the paving contract to Brightway Builders for $2 million on March 4, 2011.";
    const LEASE: &str = "Nebula (formerly Starlight Labs) leased the northern wing from Harbor Estates from 2010 until the end of 2019.";

    /// 一份完整的紧凑回复：本块列出的与已知清单里的名字、两种限定词、起止时间、别名
    const FULL: &str = r#"{"e": [["city council", "council", 0], ["paving contract", "contract", 0],
           ["Brightway Builders", "builders", 1], ["northern wing", "wing", 0]],
     "s": [["city council", "awarded", "paving contract", null, {"to": "Brightway Builders", "amount": "$2 million"}, "March 4, 2011", null,
            "The city council awarded the paving contract to Brightway Builders for $2 million on March 4, 2011."],
           ["Nebula Technologies Inc.", "leased", "northern wing", null, {"from": "Harbor Estates", "term": "ten years"}, "from 2010", "until the end of 2019",
            "Nebula (formerly Starlight Labs) leased the northern wing from Harbor Estates from 2010 until the end of 2019."],
           ["paving contract", "worth", null, "$2 million", null, null, null,
            "The city council awarded the paving contract to Brightway Builders for $2 million on March 4, 2011."]],
     "n": [["Nebula Technologies Inc.", "Starlight Labs", "Nebula (formerly Starlight Labs) leased the northern wing from Harbor Estates from 2010 until the end of 2019."]]}"#;

    /// serde_json 的对象按键排序（没开 preserve_order），限定词的顺序不承载意义
    fn by_key(mut q: Vec<(String, String)>) -> Vec<(String, String)> {
        q.sort_by(|a, b| a.0.cmp(&b.0));
        q
    }

    fn pairs(q: &[(&str, &str)]) -> Vec<(String, String)> {
        by_key(
            q.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn a_full_reply_parses_into_the_structs() {
        let x = parse_open_response(FULL).unwrap();
        assert!(!x.truncated);
        assert_eq!(x.skipped, 0);

        assert_eq!(x.entities.len(), 4);
        assert_eq!(x.entities[0].name, "city council");
        assert_eq!(x.entities[0].kind, "council");
        assert!(!x.entities[0].named);
        assert_eq!(x.entities[2].name, "Brightway Builders");
        assert!(x.entities[2].named);

        assert_eq!(x.statements.len(), 3);
        let s = &x.statements[0];
        assert_eq!(s.subject, "city council");
        assert_eq!(s.phrase, "awarded");
        assert_eq!(s.object.as_deref(), Some("paving contract"));
        assert_eq!(s.value, None);
        assert_eq!(
            by_key(s.qualifiers.clone()),
            pairs(&[("to", "Brightway Builders"), ("amount", "$2 million")])
        );
        assert_eq!(s.when.as_deref(), Some("March 4, 2011"));
        assert_eq!(s.ended, None);
        assert_eq!(s.quote.as_deref(), Some(AWARD));

        let s = &x.statements[1];
        assert_eq!(
            s.subject, "Nebula Technologies Inc.",
            "已知清单里的名字照字面收"
        );
        assert_eq!(s.object.as_deref(), Some("northern wing"));
        assert_eq!(
            by_key(s.qualifiers.clone()),
            pairs(&[("from", "Harbor Estates"), ("term", "ten years")])
        );
        assert_eq!(s.when.as_deref(), Some("from 2010"));
        assert_eq!(s.ended.as_deref(), Some("until the end of 2019"));
        assert_eq!(s.quote.as_deref(), Some(LEASE));

        let s = &x.statements[2];
        assert_eq!(s.object, None);
        assert_eq!(s.value.as_deref(), Some("$2 million"));
        assert!(s.qualifiers.is_empty());
        assert_eq!(s.when, None);
        assert_eq!(s.ended, None);
        assert_eq!(s.quote.as_deref(), Some(AWARD));

        assert_eq!(x.names.len(), 1);
        assert_eq!(x.names[0].entity, "Nebula Technologies Inc.");
        assert_eq!(x.names[0].name, "Starlight Labs");
        assert_eq!(x.names[0].quote.as_deref(), Some(LEASE));
    }

    /// 截在 `n` 数组中间：`s` 之前的全留下，`truncated` 标出来
    #[test]
    fn a_cut_off_reply_keeps_what_was_complete() {
        let marker = "\"n\": [[\"Nebula Technologies Inc.\", \"Starl";
        let cut = FULL.find(marker).unwrap() + marker.len();
        let x = parse_open_response(&FULL[..cut]).unwrap();
        assert!(x.truncated, "截断要标出来");
        assert_eq!(x.entities.len(), 4);
        assert_eq!(x.statements.len(), 3);
        assert_eq!(x.statements[2].quote.as_deref(), Some(AWARD));
        assert!(x.names.is_empty());
        assert_eq!(x.skipped, 0);
    }

    /// 截在一条陈述的限定词之后：修补退到那个 `}`，留下的半条不够八格，计入 skipped
    /// 而不是收成一条没引文的陈述
    #[test]
    fn a_half_statement_left_by_the_repair_is_counted() {
        let raw = r#"{"e": [["A", "thing", 1]],
            "s": [["A", "is", null, "b", null, null, null, "A is b."], ["A", "was", null, "c", {"at": "home"}, "in 20"#;
        let x = parse_open_response(raw).unwrap();
        assert!(x.truncated);
        assert_eq!(x.statements.len(), 1);
        assert_eq!(x.skipped, 1);
    }

    /// 限定词全是 null 的回复里一个 `}` 都没有：只认 `}` 的修补会把整块作废，
    /// 这里退到最后一个完整的 `]`
    #[test]
    fn a_cut_off_reply_without_any_closing_brace_is_still_repaired() {
        let raw = r#"{"e": [["A", "thing", 1]],
            "s": [["A", "is", null, "b", null, null, null, "A is b."], ["A", "was", null, "c", nu"#;
        let x = parse_open_response(raw).unwrap();
        assert!(x.truncated);
        assert_eq!(x.entities.len(), 1);
        assert_eq!(x.statements.len(), 1);
        assert_eq!(x.statements[0].value.as_deref(), Some("b"));
        assert_eq!(x.skipped, 0);
    }

    /// 连一个完整条目都没有时仍然报失败——容错不是把空结果说成成功
    #[test]
    fn a_reply_with_nothing_complete_still_fails() {
        assert!(parse_open_response(r#"{"e": [["a", "b"#).is_err());
    }

    /// 七格的陈述是坏条目：跳过并计数，八格的照收
    #[test]
    fn a_statement_with_seven_slots_is_skipped_and_counted() {
        let raw = r#"{"e": [["A", "thing", 1]],
            "s": [["A", "is", null, "b", null, null, null],
                  ["A", "is", null, "b", null, null, null, "A is b."]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.statements.len(), 1);
        assert_eq!(x.statements[0].quote.as_deref(), Some("A is b."));
        assert_eq!(x.skipped, 1);
        assert!(!x.truncated);
    }

    /// 字面值写成 JSON 数字也照它的写法收成文本
    #[test]
    fn a_number_in_the_value_slot_becomes_text() {
        let raw = r#"{"e": [["a", "k", 1]],
            "s": [["a", "cut water use", null, 12, null, null, null, "q"],
                  ["a", "ratio", null, 1.5, null, null, null, "q"]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.statements[0].value.as_deref(), Some("12"));
        assert_eq!(x.statements[1].value.as_deref(), Some("1.5"));
        assert_eq!(x.skipped, 0);
    }

    /// `e` 里重名（去首尾空白、内部空白折一、不分大小写）留第一个，其余计入 skipped
    #[test]
    fn a_duplicate_entity_name_keeps_the_first_and_is_counted() {
        let raw = r#"{"e": [["Harbor Bridge", "bridge", 1], [" harbor   bridge ", "span", 0],
                            ["HARBOR BRIDGE", "bridge", 1], ["Harbor Bridge Authority", "agency", 1]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.entities.len(), 2);
        assert_eq!(x.entities[0].name, "Harbor Bridge");
        assert_eq!(x.entities[0].kind, "bridge");
        assert!(x.entities[0].named);
        assert_eq!(x.entities[1].name, "Harbor Bridge Authority");
        assert_eq!(x.skipped, 2);
    }

    /// 坏条目逐个计数：没名字的实体、不是数组的实体、没主语的陈述、空短语的陈述、
    /// 少一格的陈述、不是数组的陈述、没别名的 n、实体格放了对象的 n、实体名为空的 n
    #[test]
    fn malformed_items_are_counted_not_fatal() {
        let raw = r#"{"e": [["A", "thing", 1], [""], [5, "x"], "not an array", []],
            "s": [["A", "is", null, "b", null, null, null, "A is b."],
                  [null, "is", null, "b", null, null, null, "A is b."],
                  ["A", "  ", null, "b", null, null, null, "A is b."],
                  ["A", "is", null, "b", null, null, null],
                  {"subject": "A"}],
            "n": [["A", "Ay", "A is b."], ["A"], [{}, "Ay", "q"], ["", "Ay", "q"]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.entities.len(), 1);
        assert_eq!(x.statements.len(), 1);
        assert_eq!(x.names.len(), 1);
        assert_eq!(x.skipped, 4 + 4 + 3);
        assert!(!x.truncated);
    }

    /// 可选格放错了类型只是那一格为 None，整条照收：宾语放数字、时间放数组、终止放
    /// 对象、引文放数字、字面值放布尔、限定词放字符串
    #[test]
    fn a_wrong_type_in_an_optional_slot_is_null_not_malformed() {
        let raw = r#"{"e": [["A", "thing", 1]],
            "s": [["A", "is", 3, true, "not an object", [2019], {"y": 2020}, 7]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.statements.len(), 1);
        let s = &x.statements[0];
        assert_eq!(s.object, None);
        assert_eq!(s.value, None);
        assert!(s.qualifiers.is_empty());
        assert_eq!(s.when, None);
        assert_eq!(s.ended, None);
        assert_eq!(s.quote, None);
        assert_eq!(x.skipped, 0);
    }

    /// 限定词：字符串是名字或原文，数字照写法转文本，其余类型与空键悄悄丢
    #[test]
    fn qualifiers_take_strings_and_numbers_and_drop_the_rest() {
        let raw = r#"{"e": [["a", "k", 1]],
            "s": [["a", "cut water use", null, "12%", {"compared to": "2019", "at": "Harbor Estates", "by": 3, "ratio": 1.5, "flag": true, "who": null, "list": ["x"], "": "x", "blank": "  "}, null, null, "q"]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(
            by_key(x.statements[0].qualifiers.clone()),
            pairs(&[
                ("compared to", "2019"),
                ("at", "Harbor Estates"),
                ("by", "3"),
                ("ratio", "1.5"),
            ])
        );
        assert_eq!(x.skipped, 0);
    }

    /// 每个字符串都去首尾空白：名字、种类、主语、短语、宾语、时间、引文、限定词的键与值
    #[test]
    fn every_string_is_trimmed() {
        let raw = r#"{"e": [["  A  ", " thing ", 1]],
            "s": [[" A ", " is ", " B ", null, {" at ": " home "}, " in 2019 ", " until 2020 ", " A is B. "]],
            "n": [[" A ", " Ay ", " A is B. "]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.entities[0].name, "A");
        assert_eq!(x.entities[0].kind, "thing");
        let s = &x.statements[0];
        assert_eq!(s.subject, "A");
        assert_eq!(s.phrase, "is");
        assert_eq!(s.object.as_deref(), Some("B"));
        assert_eq!(s.qualifiers, vec![("at".to_string(), "home".to_string())]);
        assert_eq!(s.when.as_deref(), Some("in 2019"));
        assert_eq!(s.ended.as_deref(), Some("until 2020"));
        assert_eq!(s.quote.as_deref(), Some("A is B."));
        assert_eq!(x.names[0].entity, "A");
        assert_eq!(x.names[0].name, "Ay");
        assert_eq!(x.names[0].quote.as_deref(), Some("A is B."));
    }

    /// 宾语与字面值同时有：照收，两个都给调用方看，这里不裁
    #[test]
    fn a_statement_with_both_object_and_value_is_passed_through() {
        let raw = r#"{"e": [["A", "thing", 1], ["B", "thing", 1]],
            "s": [["A", "is", "B", "b", null, null, null, "A is b."]]}"#;
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.statements.len(), 1);
        assert_eq!(x.statements[0].object.as_deref(), Some("B"));
        assert_eq!(x.statements[0].value.as_deref(), Some("b"));
        assert_eq!(x.skipped, 0);
    }

    /// named 认 1/0 与 true/false，缺了当点了名；kind 缺了是空串
    #[test]
    fn named_takes_numbers_and_booleans_and_defaults_to_true() {
        let raw = r#"{"e": [["a", "k", 1], ["b", "k", 0], ["c", "k", true],
                            ["d", "k", false], ["e"]]}"#;
        let x = parse_open_response(raw).unwrap();
        let named: Vec<bool> = x.entities.iter().map(|e| e.named).collect();
        assert_eq!(named, vec![true, false, true, false, true]);
        assert_eq!(x.entities[4].kind, "");
        assert_eq!(x.skipped, 0);
    }

    /// 围栏与前后废话照旧容忍（走的是同一个 json_block）
    #[test]
    fn a_fenced_reply_parses() {
        let raw = "Here you go:\n```json\n{\"e\": [[\"a\", \"k\", 1]], \"s\": [], \"n\": []}\n```";
        let x = parse_open_response(raw).unwrap();
        assert_eq!(x.entities.len(), 1);
        assert!(!x.truncated);
    }

    fn known(handle: &str, name: &str, type_key: &str) -> KnownEntity {
        KnownEntity {
            handle: handle.into(),
            type_key: type_key.into(),
            name: name.into(),
        }
    }

    /// 提示词里没有文档日期、没有类型与关系清单、没有编号与句柄；有文件名、开头、
    /// 已知名字与正文，并且按这个顺序
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
            assert!(!s.contains("\"q\""), "没有引文表: {s}");
            assert!(!s.contains("\"t\""), "没有时间表: {s}");
            assert!(!s.contains("k1"), "句柄不给模型看: {s}");
        }
        assert!(system.contains("Never compute, convert or normalise a date"));
        assert!(system.contains("do not translate it into any vocabulary of your own"));
        assert!(system.contains("written exactly as listed in \"e\""));
        assert!(system.contains("copied verbatim"));
        assert!(system.contains("write nothing about them"));
        assert!(system.contains(
            "\"s\": [[\"city council\", \"awarded\", \"paving contract\", null, {\"to\": \"Brightway Builders\""
        ));

        let doc = user.find("Document: annual-report.txt").unwrap();
        let opening = user.find("Opening of this document").unwrap();
        assert!(user.contains("Annual Report 2019"));
        let recorded = user.find("Already recorded from earlier parts").unwrap();
        assert!(user.contains("\n- Nebula Technologies Inc. (organization)\n"));
        assert!(user.contains("\n- Harbor Estates\n"), "空类型不留括号");
        assert!(user.contains("write its name exactly as it is listed here"));
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
