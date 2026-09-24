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
//!
//! **形状也宽容**（同类别词那边的教训）：模型（实测 DeepSeek-V3.2）并不总照样例写。它会
//! 把整段答成按 id 作键的对象（`{"0": ["headquartered_in", "forward"], "1": null}`），
//! 会把一条写成 `{"id": 0, "key": ..., "direction": ...}`，会把 `b` 写成对象、不要外层
//! 对象只给数组、或在值里先抄一遍 id。这些说的都是同一件事，读法只有一种；温度为零时
//! 同一段提示词回来的形状还是同一个，读不出就是每一轮都读不出——从前这种回复解出来是
//! 「零条、零坏」，调用方当成「有一票没答到」静静跳过，没有日志也不再问。读不出的键与
//! id 算坏项，不当缺席。

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
    /// 结构上也对得上、但没进短名单的键：模型在批里的属性表里看见了它、选了它，照样算票——
    /// 短名单是省 token 的手段，不是限制
    pub also_allowed: Vec<&'a str>,
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
sentence it was taken from; and the keys of its candidate properties. The properties themselves \
are listed once under \"Properties\", each with its key, its label, its kind (a relation between two \
things, or an attribute whose object is a value), its domain, its range and its definition. A class \
written as \"?\" means the documents' kind word for that side is bound to no class yet. A candidate \
key marked \"fits by inheritance\" declares its domain or range on an ancestor of the item's class; \
that is a fit, not a mismatch.\n\
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

pub fn candidate_line(c: &PropertyCandidate<'_>) -> String {
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
/// 候选属性表在一批里只写一遍（Properties），每项只列它的候选键；从前每项都带整张表
/// （最多 60 条、各带定义），12 项一批就是三万多 token，一轮跑下来对齐占了八成的用量
/// （bench README，2026-09-24）。「按继承对上」是项与候选之间的事，写在项的键后面
pub fn build_phrase_messages(items: &[PhraseItem<'_>]) -> Vec<ChatMessage> {
    let mut glossary: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in items.iter().flat_map(|i| i.candidates.iter()) {
        if seen.insert(c.key.trim()) {
            glossary.push(candidate_line(&PropertyCandidate {
                via: Vec::new(),
                ..c.clone()
            }));
        }
    }
    let mut user = String::new();
    if !glossary.is_empty() {
        user.push_str(&format!("Properties:\n{}\n\n", glossary.join("\n")));
    }
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
            let keys: Vec<String> = item
                .candidates
                .iter()
                .map(|c| {
                    if c.via.is_empty() {
                        c.key.to_string()
                    } else {
                        format!("{} (fits by inheritance: {})", c.key, c.via.join("; "))
                    }
                })
                .collect();
            format!(" {}", keys.join(", "))
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
    let mut choices = Vec::new();
    let mut malformed = 0usize;
    let mut seen = HashSet::new();
    for (id, key, direction) in answers(&value) {
        match parse_triple(&id, &key, &direction, &by_id) {
            Some(choice) if seen.insert(choice.id) => choices.push(choice),
            _ => malformed += 1,
        }
    }
    Ok((choices, malformed))
}

/// 回复里的每一条答案：（id，键，方向）三个原始值，形状还没验。
///
/// 认这几种写法，说的都是「这个 id 选了这个键、这个方向」：`{"b": [[id, key, dir]]}`
/// （样例）、`{"b": {"id": [key, dir]}}`、没有 `b` 的顶层对象 `{"id": [key, dir]}`、
/// 顶层数组 `[[id, key, dir]]`；一条也可以写成 `{"id": .., "key": .., "direction": ..}`，
/// 按 id 作键时值也可以是 `{"key": .., "direction": ..}`、`null`（不绑）或先抄一遍 id 的
/// `[id, key, dir]`。`b` 在就只看 `b`，顶层别的键是模型的旁白，不是答案。缺了 `b` 又
/// 不是对象或数组的，一条都没有
fn answers(value: &Value) -> Vec<(Value, Value, Value)> {
    let listed = value.get("b").unwrap_or(value);
    match listed {
        Value::Array(entries) => entries
            .iter()
            .map(|entry| match entry {
                Value::Array(list) if list.len() >= 2 => {
                    let (key, direction) = key_and_direction(&list[1..]);
                    (list[0].clone(), key, direction)
                }
                Value::Object(map) => {
                    let (key, direction) = fields(map);
                    (
                        map.get("id").cloned().unwrap_or(Value::Null),
                        key,
                        direction,
                    )
                }
                other => (other.clone(), Value::Null, Value::Null),
            })
            .collect(),
        Value::Object(map) => map
            .iter()
            .map(|(id, v)| {
                let (key, direction) = match v {
                    Value::Array(list) => {
                        // 值里先抄一遍 id 再给键：`{"0": [0, "headquartered_in", "forward"]}`
                        let copied = list
                            .first()
                            .and_then(item_id)
                            .is_some_and(|first| id.trim().parse::<i64>().ok() == Some(first));
                        let list = if copied { &list[1..] } else { &list[..] };
                        key_and_direction(list)
                    }
                    Value::Object(inner) => fields(inner),
                    other => (other.clone(), Value::Null),
                };
                (Value::String(id.clone()), key, direction)
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `[key, dir]` 的两格；只有一格就没有方向（null 的答案常只写一格）。空数组是「什么
/// 都没写」，读成 null 键会把没答到说成不绑——留成对象，调用方算坏
fn key_and_direction(list: &[Value]) -> (Value, Value) {
    match list {
        [] => (Value::Array(Vec::new()), Value::Null),
        [key] => (key.clone(), Value::Null),
        [key, direction, ..] => (key.clone(), direction.clone()),
    }
}

/// 一条写成对象时的两个字段：键叫 key / property / p，方向叫 direction / dir / d
fn fields(map: &serde_json::Map<String, Value>) -> (Value, Value) {
    let pick = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| map.get(*n))
            .cloned()
            .unwrap_or(Value::Null)
    };
    (
        pick(&["key", "property", "p"]),
        pick(&["direction", "dir", "d"]),
    )
}

/// 一条答案：id 得是这批里的；键是 null（不绑）或候选里的一个，绑了就得有方向，方向
/// 不认识算坏。键包在数组里的（`["headquartered_in", "forward"]` 塞在第二格）照样读，
/// 方向从数组里取
fn parse_triple(
    id: &Value,
    key: &Value,
    direction: &Value,
    by_id: &HashMap<i64, &PhraseItem<'_>>,
) -> Option<PhraseChoice> {
    let id = item_id(id)?;
    let item = by_id.get(&id)?;
    let (key, direction) = match key {
        Value::Array(list) if !list.is_empty() => {
            let (k, d) = key_and_direction(list);
            let d = if d.is_null() { direction.clone() } else { d };
            (k, d)
        }
        other => (other.clone(), direction.clone()),
    };
    let property = match &key {
        Value::Null => None,
        Value::String(written) => {
            let key = candidate_key(item, written)?;
            let direction = match direction
                .as_str()
                .map(|d| d.trim().to_lowercase())
                .as_deref()
            {
                Some("forward") => Direction::Forward,
                Some("reverse") => Direction::Reverse,
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
    let keys = || {
        item.candidates
            .iter()
            .map(|c| c.key.trim())
            .chain(item.also_allowed.iter().map(|k| k.trim()))
    };
    if let Some(exact) = keys().find(|k| *k == written) {
        return Some(exact.to_string());
    }
    let lower = written.to_lowercase();
    let mut hits = keys().filter(|k| k.to_lowercase() == lower);
    if let Some(first) = hits.next() {
        return hits.next().is_none().then(|| first.to_string());
    }
    // 键是 `p569` 这种代号时模型十有八九答标签（"dateOfBirth"）：第一次真跑里一半的签名
    // 因此判成坏票（bench README，2026-09-24）。标签唯一对上就认它的键；对上两个不认
    let folded = fold(written);
    let mut by_label = item
        .candidates
        .iter()
        .filter(|c| fold(c.label) == folded)
        .map(|c| c.key.trim());
    let first = by_label.next()?;
    by_label.next().is_none().then(|| first.to_string())
}

/// 标签比较用的折叠：大小写、空格、下划线、连字符都不算
pub fn fold(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
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
            also_allowed: vec![],
        }];
        let msgs = build_phrase_messages(&items);
        let user = &msgs[1].content;
        assert!(user.contains("Item 3: phrase \"is based in\" · subject class: organization · object: ? · 4 statements"), "{user}");
        assert!(user.contains("· Harbor Bakery —is based in→ Port Ellen\n    \"Harbor Bakery is based in Port Ellen.\""), "{user}");
        assert!(user.contains("- headquartered_in · headquartered in · relation · domain: organization · range: place · The organization's"), "{user}");
        assert!(
            user.starts_with("Properties:\n- "),
            "the glossary comes first, once: {user}"
        );
        assert!(
            user.contains("Candidates: headquartered_in"),
            "items list keys only: {user}"
        );
        assert_eq!(
            user.matches("- headquartered_in ·").count(),
            1,
            "each property is described once: {user}"
        );
        assert!(
            user.contains("- revenue · revenue · attribute · domain: organization · Total income"),
            "{user}"
        );
        assert!(msgs[0]
            .content
            .contains("every statement of this signature"));
    }

    #[test]
    fn a_key_the_shortlist_hid_still_counts_when_it_fits_structurally() {
        let examples = strings(&[]);
        let quotes = strings(&[]);
        let items = vec![PhraseItem {
            id: 0,
            phrase: "is based in",
            subject_class: Some("organization"),
            object_class: None,
            object_is_value: false,
            statement_count: 1,
            examples: &examples,
            quotes: &quotes,
            candidates: candidates(),
            also_allowed: vec!["located_in"],
        }];
        let (choices, malformed) =
            parse_phrase_response(r#"{"b":[[0,"located_in","forward"]]}"#, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices[0].property.as_ref().map(|(k, _)| k.as_str()),
            Some("located_in")
        );
        let (_, malformed) =
            parse_phrase_response(r#"{"b":[[0,"made_up","forward"]]}"#, &items).unwrap();
        assert_eq!(malformed, 1, "a key that fits nowhere is still malformed");
    }

    #[test]
    fn a_label_answer_maps_to_its_key_when_unique() {
        let examples = strings(&[]);
        let quotes = strings(&[]);
        let items = vec![PhraseItem {
            id: 0,
            phrase: "is based in",
            subject_class: Some("organization"),
            object_class: None,
            object_is_value: false,
            statement_count: 1,
            examples: &examples,
            quotes: &quotes,
            candidates: candidates(),
            also_allowed: vec![],
        }];
        let (choices, malformed) =
            parse_phrase_response(r#"{"b":[[0,"Headquartered In","forward"]]}"#, &items).unwrap();
        assert_eq!(malformed, 0, "a label, folded, names the key");
        assert_eq!(
            choices[0].property.as_ref().map(|(k, _)| k.as_str()),
            Some("headquartered_in")
        );
        assert_eq!(fold("Date of Birth"), "dateofbirth");
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
            also_allowed: vec![],
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
            also_allowed: vec![],
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
                also_allowed: vec![],
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
                also_allowed: vec![],
            },
        ];
        let raw = r#"{"b": [[0, "revenue", "forward"], [1, "subsid"#;
        let (choices, _) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, 0);
    }

    /// 三条签名：owns（应反向绑 subsidiary_of）、said（应 null）、revenue（值）
    fn three_items<'a>(examples: &'a [String], quotes: &'a [String]) -> Vec<PhraseItem<'a>> {
        let mk = |id: i64, phrase: &'static str, value: bool| PhraseItem {
            id,
            phrase,
            subject_class: Some("organization"),
            object_class: (!value).then_some("organization"),
            object_is_value: value,
            statement_count: 1,
            examples,
            quotes,
            candidates: candidates(),
            also_allowed: vec![],
        };
        vec![
            mk(0, "owns", false),
            mk(1, "said", false),
            mk(2, "revenue", true),
        ]
    }

    fn bound(id: i64, key: &str, direction: Direction) -> PhraseChoice {
        PhraseChoice {
            id,
            property: Some((key.to_string(), direction)),
        }
    }

    fn unbound(id: i64) -> PhraseChoice {
        PhraseChoice { id, property: None }
    }

    /// 类别词那边实测的写法搬到短语上：整段是按 id 作键的对象，没有 `b`，值是
    /// `[key, dir]`。解出来必须和样例形状一模一样
    #[test]
    fn an_id_keyed_object_with_pairs_is_read_as_the_triple_list() {
        let (examples, quotes) = (strings(&[]), strings(&[]));
        let items = three_items(&examples, &quotes);
        let raw = r#"{ "0": ["subsidiary_of", "reverse"], "1": [null, null], "2": ["revenue", "forward"] }"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![
                bound(0, "subsidiary_of", Direction::Reverse),
                unbound(1),
                bound(2, "revenue", Direction::Forward),
            ]
        );
        // 不绑的写成裸 null 或只有一格；绑了却只有一格是没方向，算坏
        let raw = r#"{"0": ["subsidiary_of"], "1": null, "2": [null]}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 1, "bound without a direction");
        assert_eq!(choices, vec![unbound(1), unbound(2)]);
        // 值里先抄一遍 id 再给键与方向
        let raw = r#"{"0": [0, "subsidiary_of", "reverse"], "1": [1, null, null]}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![bound(0, "subsidiary_of", Direction::Reverse), unbound(1)]
        );
        // 空数组不是一个答案
        let raw = r#"{"0": [], "1": [null]}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 1);
        assert_eq!(choices, vec![unbound(1)]);
    }

    /// 按 id 作键、值是对象：`{"key": .., "direction": ..}`，字段名的几种别名都认
    #[test]
    fn an_id_keyed_object_with_object_values_parses() {
        let (examples, quotes) = (strings(&[]), strings(&[]));
        let items = three_items(&examples, &quotes);
        let raw = r#"{"0": {"key": "subsidiary_of", "direction": "Reverse"}, "1": {"key": null, "direction": null}, "2": {"property": "revenue", "dir": "FORWARD"}}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![
                bound(0, "subsidiary_of", Direction::Reverse),
                unbound(1),
                bound(2, "revenue", Direction::Forward),
            ]
        );
    }

    /// `b` 下面是对象而不是数组：一样读；`b` 在就只看 `b`，顶层别的键是旁白
    #[test]
    fn b_as_an_object_parses_and_other_top_level_keys_are_ignored() {
        let (examples, quotes) = (strings(&[]), strings(&[]));
        let items = three_items(&examples, &quotes);
        let raw = r#"{"note": "done", "b": {"0": ["subsidiary_of", "reverse"], "1": null}}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![bound(0, "subsidiary_of", Direction::Reverse), unbound(1)]
        );
        let raw = r#"{"b": {"0": {"key": "subsidiary_of", "direction": "reverse"}}}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(choices, vec![bound(0, "subsidiary_of", Direction::Reverse)]);
    }

    /// 数组里的一条写成对象、或整段不要外层对象只给数组：照读
    #[test]
    fn object_triples_and_a_bare_array_parse() {
        let (examples, quotes) = (strings(&[]), strings(&[]));
        let items = three_items(&examples, &quotes);
        let raw = r#"{"b": [{"id": 0, "key": "subsidiary_of", "direction": "reverse"}, {"id": "1", "key": null, "direction": null}]}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![bound(0, "subsidiary_of", Direction::Reverse), unbound(1)]
        );
        let raw = concat!(
            "```json\n",
            r#"[[0, "subsidiary_of", "reverse"], [2, "revenue", "forward"]]"#,
            "\n```"
        );
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![
                bound(0, "subsidiary_of", Direction::Reverse),
                bound(2, "revenue", Direction::Forward)
            ]
        );
        // 样例形状里第二格又包了一层：`[id, [key, dir]]`
        let raw = r#"{"b": [[0, ["subsidiary_of", "reverse"]], [1, [null]]]}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![bound(0, "subsidiary_of", Direction::Reverse), unbound(1)]
        );
    }

    /// `b` 是空的：一项都没答，不是坏项。没有 `b`、键又不是 id 的：那是读不出的答案，
    /// 算坏项——从前这种回复算「一项都没答」，调用方静静跳过，什么痕迹都不留
    #[test]
    fn an_empty_b_answers_nothing_but_an_unreadable_reply_is_malformed() {
        let (examples, quotes) = (strings(&[]), strings(&[]));
        let items = three_items(&examples, &quotes);
        for raw in [r#"{"b": []}"#, r#"{"b": {}}"#] {
            let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
            assert!(choices.is_empty(), "{raw}");
            assert_eq!(malformed, 0, "{raw}");
        }
        let raw = r#"{"answer": "subsidiary_of", "direction": "reverse"}"#;
        let (choices, malformed) = parse_phrase_response(raw, &items).unwrap();
        assert!(choices.is_empty());
        assert_eq!(malformed, 2);
        // `b` 是标量：一条都没有
        let (choices, malformed) = parse_phrase_response(r#"{"b": 1}"#, &items).unwrap();
        assert!(choices.is_empty());
        assert_eq!(malformed, 0);
    }
}
