//! 蕴含规则的两次模型调用（0044 决定 3 第五片）。
//!
//! 一是**提规则**：给对齐器刚判过的签名（或类别词），问「这种形状的陈述除了它绑到的属性，
//! 还蕴含哪条属性的事实，宾语怎么来」。答案是 (属性, 读数或 null)；null 属性 = 什么也不蕴含。
//! 二是**读数**：给 distinct 的字，问「按这种读法它指什么」，答一个名字或一个值或 null。
//! 读数按字缓存，一个字一辈子只问一次；提示词里只有字和读法，没有文档。
//!
//! 回复都是紧凑 JSON，解析同 phrase_align：坏的一条计数，不毁掉整批；没答到的 id 不出现。

use crate::phrase_align::{candidate_line, PropertyCandidate};
use utopia_llm::ChatMessage;

/// 一条待提规则的形状：签名或类别词，带例句与候选属性
#[derive(Debug, Clone)]
pub struct RuleItem<'a> {
    pub id: i64,
    /// phrase | kind_word
    pub trigger: &'a str,
    pub phrase: &'a str,
    pub subject_class: Option<&'a str>,
    pub object_class: Option<&'a str>,
    pub object_is_value: bool,
    /// 签名已绑到的属性键（类别词没有）；提的规则不能又是它
    pub bound_to: Option<&'a str>,
    pub examples: &'a [String],
    pub candidates: Vec<PropertyCandidate<'a>>,
}

/// 模型对一条形状的回答：Some((属性键, 读数)) 或 None（什么也不蕴含）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleChoice {
    pub id: i64,
    pub implies: Option<(String, Option<String>)>,
}

const RULE_SYSTEM: &str = "\
You find facts a careful reader draws from a statement without the text stating them. \
Each numbered item is one shape: a relation phrase between a subject class and an object class \
(or a value), or a kind word the documents use for a thing, with example statements. The item \
may already be bound to a property; that binding is not the question. The question is whether \
this shape implies a fact of ANOTHER candidate property about the subject, and how the object \
of that fact is obtained:\n\
- null: the object is the statement's own object (the shape implies a second property about the same pair);\n\
- a reading: the object is read from the words of the statement's object (for a kind word, from the kind word itself). \
Readings available, by name:\n\
{READINGS}\n\
Examples of what is implied: a kind word \"British film\" implies country of origin = United Kingdom \
(reading country_of_nationality); \"located in the Piedmont region of Virginia\" implies country = \
United States (reading country_of_place); \"released in the summer of 1952\" implies publication year \
(reading year_of_phrase). Answer null when nothing beyond the binding is implied, when the implication \
would only sometimes hold, or when no candidate property fits.\n\
Answer with one JSON object and nothing else: {\"i\": [[id, \"property_key\" | null, \"reading\" | null]]}. \
Every item id appears exactly once.";

fn readings_text(readings: &[(&str, &str)]) -> String {
    readings
        .iter()
        .map(|(k, d)| format!("- {k}: {d}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn build_rule_messages(items: &[RuleItem<'_>], readings: &[(&str, &str)]) -> Vec<ChatMessage> {
    // 候选表一批只写一遍，各项只列键（与 `phrase_align::build_phrase_messages` 同一条理由：
    // 每项带整张表时一次请求五万多 token）
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
        let shape = if item.trigger == "kind_word" {
            format!("kind word \"{}\"", item.phrase)
        } else {
            let object = if item.object_is_value {
                "value".to_string()
            } else {
                item.object_class.unwrap_or("?").to_string()
            };
            format!(
                "phrase \"{}\" · subject class: {} · object: {}",
                item.phrase,
                item.subject_class.unwrap_or("?"),
                object
            )
        };
        let mut examples = String::new();
        for ex in item.examples {
            examples.push_str(&format!("\n  · {ex}"));
        }
        if examples.is_empty() {
            examples.push_str(" (none)");
        }
        let bound = item
            .bound_to
            .map(|b| format!(" · already bound to: {b}"))
            .unwrap_or_default();
        let candidates = if item.candidates.is_empty() {
            " (none)".to_string()
        } else {
            let keys: Vec<&str> = item.candidates.iter().map(|c| c.key.trim()).collect();
            format!(" {}", keys.join(", "))
        };
        user.push_str(&format!(
            "Item {}: {shape}{bound}\nExamples:{examples}\nCandidates:{candidates}\n\n",
            item.id
        ));
    }
    vec![
        ChatMessage {
            role: "system".into(),
            content: RULE_SYSTEM.replace("{READINGS}", &readings_text(readings)),
        },
        ChatMessage {
            role: "user".into(),
            content: user.trim_end().to_string(),
        },
    ]
}

/// 解析提规则的回复：`(裁决, 坏项数)`。同一个 id 只收第一次；属性键不在候选里、读数不在
/// 清单里、属性就是已绑到的那条，都算坏
pub fn parse_rule_response(
    raw: &str,
    items: &[RuleItem<'_>],
    readings: &[(&str, &str)],
) -> Result<(Vec<RuleChoice>, usize), String> {
    let v: serde_json::Value =
        serde_json::from_str(extract_json(raw)).map_err(|e| e.to_string())?;
    let rows = v
        .get("i")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "no \"i\" array".to_string())?;
    let mut out = Vec::new();
    let mut malformed = 0usize;
    // 坏行不占 id：只有收下的答案才算答过，后面同 id 的完好答案还能收；收过再来的才是重复
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let parsed = (|| {
            let arr = row.as_array()?;
            let id = arr.first()?.as_i64()?;
            let item = items.iter().find(|i| i.id == id)?;
            if seen.contains(&id) {
                return None;
            }
            let key = arr.get(1).and_then(|x| x.as_str());
            let reading = arr.get(2).and_then(|x| x.as_str());
            let implies = match key {
                None => None,
                Some(k) => {
                    if !item.candidates.iter().any(|c| c.key == k) || item.bound_to == Some(k) {
                        return None;
                    }
                    if let Some(r) = reading {
                        if !readings.iter().any(|(name, _)| *name == r) {
                            return None;
                        }
                    }
                    // 类别词自己没有宾语：没有读数就没有宾语，这条答案没意义
                    if item.trigger == "kind_word" && reading.is_none() {
                        return None;
                    }
                    Some((k.to_string(), reading.map(str::to_string)))
                }
            };
            Some(RuleChoice { id, implies })
        })();
        match parsed {
            Some(choice) => {
                seen.insert(choice.id);
                out.push(choice);
            }
            None => malformed += 1,
        }
    }
    Ok((out, malformed))
}

/// 一条待读的字
#[derive(Debug, Clone)]
pub struct ReadingItem<'a> {
    pub id: i64,
    pub reading: &'a str,
    pub phrase: &'a str,
}

/// 读数的答案：一个名字（库里的一样东西）、一个值，或读不出来
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingAnswer {
    pub id: i64,
    pub name: Option<String>,
    pub value: Option<String>,
}

const READING_SYSTEM: &str = "\
You read a short phrase in a stated way and answer what it names. Each numbered item gives the \
reading and the phrase. Readings:\n\
{READINGS}\n\
Answer the canonical English name of the thing (a country's common name, e.g. \"United Kingdom\", \
\"United States\"), or for year_of_phrase the four-digit year as a string, or null when the phrase \
does not determine an answer (an ambiguous demonym, a place you cannot place, no year in the words). \
Do not guess. Answer with one JSON object and nothing else: {\"r\": [[id, \"answer\" | null]]}. \
Every item id appears exactly once.";

pub fn build_reading_messages(
    items: &[ReadingItem<'_>],
    readings: &[(&str, &str)],
) -> Vec<ChatMessage> {
    let user = items
        .iter()
        .map(|i| format!("Item {}: {} · \"{}\"", i.id, i.reading, i.phrase))
        .collect::<Vec<_>>()
        .join("\n");
    vec![
        ChatMessage {
            role: "system".into(),
            content: READING_SYSTEM.replace("{READINGS}", &readings_text(readings)),
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}

/// 解析读数的回复。年份读数的答案落成值，其余落成名字；空串、非四位数的年份算坏
pub fn parse_reading_response(
    raw: &str,
    items: &[ReadingItem<'_>],
) -> Result<(Vec<ReadingAnswer>, usize), String> {
    let v: serde_json::Value =
        serde_json::from_str(extract_json(raw)).map_err(|e| e.to_string())?;
    let rows = v
        .get("r")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "no \"r\" array".to_string())?;
    let mut out = Vec::new();
    let mut malformed = 0usize;
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let parsed = (|| {
            let arr = row.as_array()?;
            let id = arr.first()?.as_i64()?;
            let item = items.iter().find(|i| i.id == id)?;
            if seen.contains(&id) {
                return None;
            }
            let answer = arr.get(1).and_then(|x| match x {
                serde_json::Value::String(s) => Some(s.trim().to_string()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            });
            match answer {
                None => Some(ReadingAnswer {
                    id,
                    name: None,
                    value: None,
                }),
                Some(a) if a.is_empty() => None,
                Some(a) if item.reading == "year_of_phrase" => {
                    if a.len() == 4 && a.chars().all(|c| c.is_ascii_digit()) {
                        Some(ReadingAnswer {
                            id,
                            name: None,
                            value: Some(a),
                        })
                    } else {
                        None
                    }
                }
                Some(a) => Some(ReadingAnswer {
                    id,
                    name: Some(a),
                    value: None,
                }),
            }
        })();
        match parsed {
            Some(answer) => {
                seen.insert(answer.id);
                out.push(answer);
            }
            None => malformed += 1,
        }
    }
    Ok((out, malformed))
}

/// 回复里可能裹着 ```json 围栏或前后的话：取第一个 { 到最后一个 }
fn extract_json(raw: &str) -> &str {
    match (raw.find('{'), raw.rfind('}')) {
        (Some(a), Some(b)) if b > a => &raw[a..=b],
        _ => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READINGS: &[(&str, &str)] = &[("country_of_nationality", "…"), ("year_of_phrase", "…")];

    fn cand(key: &'static str) -> PropertyCandidate<'static> {
        PropertyCandidate {
            key,
            label: key,
            description: "",
            kind: "relation",
            domains: vec![],
            ranges: vec![],
            via: vec![],
        }
    }
    fn item(id: i64, trigger: &'static str, bound: Option<&'static str>) -> RuleItem<'static> {
        RuleItem {
            id,
            trigger,
            phrase: "british film",
            subject_class: Some("film"),
            object_class: None,
            object_is_value: false,
            bound_to: bound,
            examples: &[],
            candidates: vec![cand("country_of_origin"), cand("genre")],
        }
    }

    #[test]
    fn a_rule_answer_names_a_candidate_and_a_known_reading() {
        let items = vec![item(0, "kind_word", None), item(1, "phrase", Some("genre"))];
        let (choices, bad) = parse_rule_response(
            "```json\n{\"i\":[[0,\"country_of_origin\",\"country_of_nationality\"],[1,null,null]]}\n```",
            &items,
            READINGS,
        )
        .unwrap();
        assert_eq!(bad, 0);
        assert_eq!(
            choices[0].implies,
            Some((
                "country_of_origin".into(),
                Some("country_of_nationality".into())
            ))
        );
        assert_eq!(choices[1].implies, None);
    }

    #[test]
    fn bad_rule_answers_are_counted_not_believed() {
        let items = vec![item(0, "kind_word", None), item(1, "phrase", Some("genre"))];
        let (choices, bad) = parse_rule_response(
            // 未知属性；类别词没有读数；已绑到的属性；未知读数；重复 id
            "{\"i\":[[0,\"director\",null],[0,\"country_of_origin\",null],[1,\"genre\",null],[1,\"country_of_origin\",\"made_up\"],[1,null,null],[1,null,null]]}",
            &items,
            READINGS,
        )
        .unwrap();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, 1);
        assert_eq!(bad, 5);
    }

    #[test]
    fn readings_land_as_names_or_four_digit_years() {
        let items = vec![
            ReadingItem {
                id: 0,
                reading: "country_of_nationality",
                phrase: "british",
            },
            ReadingItem {
                id: 1,
                reading: "year_of_phrase",
                phrase: "the summer of 1952",
            },
            ReadingItem {
                id: 2,
                reading: "year_of_phrase",
                phrase: "last year",
            },
            ReadingItem {
                id: 3,
                reading: "country_of_nationality",
                phrase: "iberian",
            },
        ];
        let (answers, bad) = parse_reading_response(
            "{\"r\":[[0,\"United Kingdom\"],[1,1952],[2,\"recently\"],[3,null]]}",
            &items,
        )
        .unwrap();
        assert_eq!(bad, 1, "a non-year for year_of_phrase is malformed");
        assert_eq!(answers[0].name.as_deref(), Some("United Kingdom"));
        assert_eq!(answers[1].value.as_deref(), Some("1952"));
        assert_eq!(
            (answers[2].name.as_deref(), answers[2].value.as_deref()),
            (None, None)
        );
    }

    #[test]
    fn the_prompts_carry_the_readings_and_the_shape() {
        let items = vec![item(0, "phrase", Some("genre"))];
        let m = build_rule_messages(&items, READINGS);
        assert!(m[0].content.contains("- country_of_nationality"));
        assert!(m[1]
            .content
            .contains("phrase \"british film\" · subject class: film · object: ?"));
        assert!(m[1].content.contains("already bound to: genre"));
        let r = build_reading_messages(
            &[ReadingItem {
                id: 7,
                reading: "year_of_phrase",
                phrase: "in 1952",
            }],
            READINGS,
        );
        assert!(r[1]
            .content
            .contains("Item 7: year_of_phrase · \"in 1952\""));
    }
}
