//! 关系短语按签名绑到属性（0044 决定 3 的第二片）。签名 = 短语 × 主语的类 × 宾语的类
//! （宾语是字面值时记「值」）。一个库里 distinct 的签名比陈述少得多，每条只判一次，绑定按
//! 库缓存、按签名复用。
//!
//! 对齐读来源（决定 3）：每条签名带着它下面的几条陈述和各自的引文进提示词，模型判的是
//! 「**每一条**这个签名下的陈述都在陈述这个属性吗」，还要说方向：forward 是陈述的主语
//! 就是属性的主语，reverse 是反过来（「owns」绑到 subsidiary_of）。属性可以比短语宽
//! （「opened a plant in」是 located_in），不能比短语窄、不能只是沾边（「announced the
//! acquisition of」不是 acquired）；只报告、只评价的短语（said、is expected to）答 null；
//! 值不是属性量的东西也答 null（股数不是营收）。候选属性的定义、定义域、值域照库里的
//! 写法给，答案里的键照抄。
//!
//! 回复是紧凑 JSON：`{"b": [[id, "key" | null, "forward" | "reverse" | null]]}`。解析同
//! 类别词那边：坏的一条计数、不毁掉整批；键不在候选里、id 不在批里、绑了却没方向、
//! 同一个 id 的第二次都算坏；没答到的 id 是「再问」，不是 null。

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use utopia_llm::ChatMessage;

use crate::align::{item_id, parse_value};

/// 一个候选属性：键照抄进答案；其余是模型判断的依据，照库里的语言给
#[derive(Debug, Clone)]
pub struct PropertyCandidate<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub description: &'a str,
    /// relation（两样东西之间）或 attribute（宾语是值）
    pub kind: &'a str,
    /// 定义域、值域的类键；空表示没声明
    pub domains: Vec<&'a str>,
    pub ranges: Vec<&'a str>,
    /// 这条候选是经继承命中的：声明在祖先上，签名的类是它的子类。把依据写给模型看，
    /// 不然它对着一条 domain 是 legal_entity 的属性和一个 organization 的主语会答 null
    /// （#807：只在代码里放宽候选是不够的，模型得看见继承的依据）
    pub via: Vec<String>,
}

/// 一条待绑定的签名：短语、两端的类、例句与引文、候选属性
#[derive(Debug, Clone)]
pub struct PhraseItem<'a> {
    pub id: i64,
    /// the normalised phrase ("acquired")
    pub phrase: &'a str,
    /// the subject's class key; None when its kind word is bound to no class yet
    pub subject_class: Option<&'a str>,
    /// the object's class key; None when unbound, or when the object is a value
    pub object_class: Option<&'a str>,
    pub object_is_value: bool,
    pub statement_count: i64,
    /// rendered statements ("Brightway Builders —acquired→ Harbor Estates") with their quotes
    pub examples: &'a [String],
    pub quotes: &'a [String],
    pub candidates: Vec<PropertyCandidate<'a>>,
}

/// 模型对一条签名的裁决：Some = (候选的键, 方向)；None = 不绑
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhraseChoice {
    pub id: i64,
    pub property: Option<(String, Direction)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Reverse,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Forward => "forward",
            Direction::Reverse => "reverse",
        }
    }
}

/// 系统消息。例子是中性的，不出自任何测量语料；规则 1 的「每一条」是整条路的判据
const PHRASE_SYSTEM: &str = "\
You bind the relation phrases documents use to the properties of a knowledge base's ontology. \
Each numbered item is one signature: a phrase as the documents wrote it, the class of the thing \
it is said of (its subject) and the class of what it points at (its object), or \"value\" when \
the object is a figure, a title or a status; a few statements with that signature, each with the \
sentence it was taken from; and the candidate properties, each with its key, its label, its \
definition, its kind (a relation between two things, or an attribute whose object is a value), \
its domain and its range. A class written as \"?\" means the documents' kind word for that side \
is bound to no class yet. A candidate marked \"fits by inheritance\" declares its domain or range on \
an ancestor of the item's class; that is a fit, not a mismatch.\n\
For each item, answer with the key of the one property that every statement of this signature \
states by that property's definition, and the direction: \"forward\" when the statement's \
subject is the property's subject, \"reverse\" when the statement's object is; or null.\n\
\n\
Output exactly one JSON object and nothing else, one triple per item:\n\
{\"b\": [[12, \"headquartered_in\", \"forward\"], [13, null, null]]}\n\
\n\
1. Choose a property only when each of the statements under this signature states that \
property, by the definition as given. The property may be broader than the phrase: \"opened a \
plant in\" states located_in; \"is the chief executive of\" states officer_of. It is never \
narrower and never merely related: \"announced the acquisition of\" is not acquired when the \
sentence says it was announced, not completed; \"revenue grew 18%\" is a change from the prior \
period, not revenue.\n\
2. Judge by the definition and by the sentences, not by the label. Labels, definitions and \
phrases may be in any language.\n\
3. Answer null when no candidate fits; when the sentences show the phrase meaning different \
things under this signature; when the phrase only reports, introduces or evaluates (\"said\", \
\"announced\", \"is expected to\") unless a candidate is about that; or when a value is not \
what the attribute measures (a share count is not revenue, a date is not an amount).\n\
4. The direction follows the definition: for \"X —is a subsidiary of→ Y\" subsidiary_of is \
forward; for \"X —owns→ Y\", if subsidiary_of is the only fitting candidate, it is reverse.\n\
5. Never invent a key, never answer with a label, never choose for an item a key that is not \
among its candidates. One triple per item, every item answered.";

fn candidate_line(c: &PropertyCandidate<'_>) -> String {
    let mut line = format!("- {} · {} · {}", c.key, c.label, c.kind);
    if !c.domains.is_empty() {
        line.push_str(&format!(" · domain: {}", c.domains.join(", ")));
    }
    if !c.ranges.is_empty() {
        line.push_str(&format!(" · range: {}", c.ranges.join(", ")));
    }
    if !c.via.is_empty() {
        line.push_str(&format!(" · fits by inheritance: {}", c.via.join("; ")));
    }
    line.push_str(&format!(" · {}", c.description));
    line
}

/// 构造两条消息：常量系统消息 + 逐项的用户消息。每项：id、短语、两端的类、例句与引文、候选。
pub fn build_phrase_messages(items: &[PhraseItem<'_>]) -> Vec<ChatMessage> {
    let mut user = String::new();
    for item in items {
        let object = if item.object_is_value {
            "value".to_string()
        } else {
            item.object_class.unwrap_or("?").to_string()
        };
        let mut examples = String::new();
        for (i, ex) in item.examples.iter().enumerate() {
            let quote = item.quotes.get(i).map(String::as_str).unwrap_or("");
            examples.push_str(&format!("\n  · {ex}\n    \"{}\"", quote.trim()));
        }
        if examples.is_empty() {
            examples.push_str(" (none)");
        }
        let candidates = if item.candidates.is_empty() {
            " (none)".to_string()
        } else {
            let lines: Vec<String> = item.candidates.iter().map(candidate_line).collect();
            format!("\n{}", lines.join("\n"))
        };
        user.push_str(&format!(
            "Item {}: phrase \"{}\" · subject class: {} · object: {} · {} statements\nStatements:{examples}\nCandidates:{candidates}\n\n",
            item.id,
            item.phrase,
            item.subject_class.unwrap_or("?"),
            object,
            item.statement_count,
        ));
    }
    vec![
        ChatMessage {
            role: "system".into(),
            content: PHRASE_SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user".into(),
            content: user.trim_end().to_string(),
        },
    ]
}

/// 解析回复：`(裁决, 坏项数)`。同一个 id 只收第一次；没答到的 id 不出现。
pub fn parse_phrase_response(
    raw: &str,
    items: &[PhraseItem<'_>],
) -> anyhow::Result<(Vec<PhraseChoice>, usize)> {
    let value = parse_value(raw)?;
    let by_id: HashMap<i64, &PhraseItem<'_>> = items.iter().map(|i| (i.id, i)).collect();
    let triples = value
        .get("b")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut choices = Vec::new();
    let mut malformed = 0usize;
    let mut seen = HashSet::new();
    for t in triples {
        match parse_triple(t, &by_id) {
            Some(choice) if seen.insert(choice.id) => choices.push(choice),
            _ => malformed += 1,
        }
    }
    Ok((choices, malformed))
}

/// `[id, key | null, direction | null]`：绑了就得有方向，方向不认识算坏
fn parse_triple(v: &Value, by_id: &HashMap<i64, &PhraseItem<'_>>) -> Option<PhraseChoice> {
    let arr = v.as_array()?;
    if arr.len() < 2 {
        return None;
    }
    let id = item_id(&arr[0])?;
    let item = by_id.get(&id)?;
    let property = match &arr[1] {
        Value::Null => None,
        Value::String(written) => {
            let key = candidate_key(item, written)?;
            let direction = match arr.get(2).and_then(Value::as_str).map(str::trim) {
                Some("forward") | Some("Forward") => Direction::Forward,
                Some("reverse") | Some("Reverse") => Direction::Reverse,
                _ => return None,
            };
            Some((key, direction))
        }
        _ => return None,
    };
    Some(PhraseChoice { id, property })
}

/// 模型写的键对回这一项的候选：先原样，再不分大小写（只在唯一命中时）
fn candidate_key(item: &PhraseItem<'_>, written: &str) -> Option<String> {
    let written = written.trim();
    if written.is_empty() {
        return None;
    }
    let keys = || item.candidates.iter().map(|c| c.key.trim());
    if let Some(exact) = keys().find(|k| *k == written) {
        return Some(exact.to_string());
    }
    let lower = written.to_lowercase();
    let mut hits = keys().filter(|k| k.to_lowercase() == lower);
    let first = hits.next()?;
    hits.next().is_none().then(|| first.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn candidates<'a>() -> Vec<PropertyCandidate<'a>> {
        vec![
            PropertyCandidate {
                key: "headquartered_in",
                label: "headquartered in",
                description: "The organization's principal office is at the place.",
                kind: "relation",
                domains: vec!["organization"],
                ranges: vec!["place"],
                via: Vec::new(),
            },
            PropertyCandidate {
                key: "subsidiary_of",
                label: "subsidiary of",
                description: "The organization is owned or controlled by the other organization.",
                kind: "relation",
                domains: vec!["organization"],
                ranges: vec!["organization"],
                via: Vec::new(),
            },
            PropertyCandidate {
                key: "revenue",
                label: "revenue",
                description: "Total income from sales for a period, as an amount of money.",
                kind: "attribute",
                domains: vec!["organization"],
                ranges: vec![],
                via: Vec::new(),
            },
        ]
    }

    #[test]
    fn the_prompt_lists_the_signature_its_sentences_and_the_candidates() {
        let examples = strings(&["Harbor Bakery —is based in→ Port Ellen"]);
        let quotes = strings(&["Harbor Bakery is based in Port Ellen."]);
        let items = vec![PhraseItem {
            id: 3,
            phrase: "is based in",
            subject_class: Some("organization"),
            object_class: None,
            object_is_value: false,
            statement_count: 4,
            examples: &examples,
            quotes: &quotes,
            candidates: candidates(),
        }];
        let msgs = build_phrase_messages(&items);
        let user = &msgs[1].content;
        assert!(user.contains("Item 3: phrase \"is based in\" · subject class: organization · object: ? · 4 statements"), "{user}");
        assert!(user.contains("· Harbor Bakery —is based in→ Port Ellen\n    \"Harbor Bakery is based in Port Ellen.\""), "{user}");
        assert!(user.contains("- headquartered_in · headquartered in · relation · domain: organization · range: place · The organization's"), "{user}");
        assert!(
            user.contains("- revenue · revenue · attribute · domain: organization · Total income"),
            "{user}"
        );
        assert!(msgs[0]
            .content
            .contains("every statement of this signature"));
    }

    #[test]
    fn a_value_signature_says_value_for_its_object() {
        let examples = strings(&["Harbor Bakery —revenue→ $2 million"]);
        let quotes = strings(&["Harbor Bakery's revenue was $2 million."]);
        let items = vec![PhraseItem {
            id: 0,
            phrase: "revenue",
            subject_class: Some("organization"),
            object_class: None,
            object_is_value: true,
            statement_count: 1,
            examples: &examples,
            quotes: &quotes,
            candidates: candidates(),
        }];
        let user = &build_phrase_messages(&items)[1].content;
        assert!(user.contains("· object: value ·"), "{user}");
    }

    /// 绑上带方向；null 不绑；键照候选抄回；绑了没方向、键不在候选里、id 不在批里都算坏
    #[test]
    fn triples_parse_and_bad_ones_are_counted() {
        let examples = strings(&[]);
        let quotes = strings(&[]);
        let mk = |id: i64, phrase: &'static str| PhraseItem {
            id,
            phrase,
            subject_class: Some("organization"),
            object_class: Some("organization"),
            object_is_value: false,
            statement_count: 1,
            examples: &examples,
            quotes: &quotes,
            candidates: candidates(),
        };
        let items = vec![
            mk(0, "owns"),
            mk(1, "said"),
            mk(2, "acquired"),
            mk(3, "employs"),
            mk(4, "x"),
        ];
        let raw = r#"{"b": [[0, "Subsidiary_Of", "reverse"], [1, null, null], [2, "acquired", "forward"], [3, "subsidiary_of"], ["4", "revenue", "forward"], [9, null, null], [0, null, null]]}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(
            choices,
            vec![
                PhraseChoice {
                    id: 0,
                    property: Some(("subsidiary_of".into(), Direction::Reverse))
                },
                PhraseChoice {
                    id: 1,
                    property: None
                },
                PhraseChoice {
                    id: 4,
                    property: Some(("revenue".into(), Direction::Forward))
                },
            ]
        );
        // 2：键不在候选里；3：绑了没方向；9：id 不在批里；0 的第二次
        assert_eq!(malformed, 4);
    }

    #[test]
    fn a_truncated_reply_keeps_the_complete_triples() {
        let examples = strings(&[]);
        let quotes = strings(&[]);
        let items = vec![
            PhraseItem {
                id: 0,
                phrase: "a",
                subject_class: None,
                object_class: None,
                object_is_value: false,
                statement_count: 1,
                examples: &examples,
                quotes: &quotes,
                candidates: candidates(),
            },
            PhraseItem {
                id: 1,
                phrase: "b",
                subject_class: None,
                object_class: None,
                object_is_value: false,
                statement_count: 1,
                examples: &examples,
                quotes: &quotes,
                candidates: candidates(),
            },
        ];
        let raw = r#"{"b": [[0, "revenue", "forward"], [1, "subsid"#;
        let (choices, _) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, 0);
    }
}
