//! 本体代理的一次提案调用（0061 决定 2）。
//!
//! 输入是开放图谱里本体接不住的形状——对齐判成「无」或「判不定」的签名（短语 × 两端的类，
//! 带例句与引文）和绑不到类的类别词（带例名）——外加现在的本体（类与属性的定义、定义域、值域）
//! 和库的能力问题。问的是：**加上哪几个类、哪几条属性，这些形状就接得住、这些问题就答得了**。
//! 每条提案带一份写给对齐器看的定义、它会绑上的形状、它服务的问题；本体里已经有的属性接得住
//! 的形状写在 `existing` 里，不另提。代理只提，不写本体（0012：本体是人签的契约）。
//!
//! 回复是紧凑 JSON，解析同 phrase_align：坏的一条计数、不毁掉整批；键不合法、形状 id 不在批里
//! 都算坏。

use crate::align::parse_value;
use serde_json::Value;
use std::collections::HashSet;
use utopia_llm::ChatMessage;

/// 一条对齐接不住的签名
#[derive(Debug, Clone)]
pub struct OpenSignature<'a> {
    pub id: i64,
    pub phrase: &'a str,
    pub subject_class: Option<&'a str>,
    pub object_class: Option<&'a str>,
    pub object_is_value: bool,
    pub statement_count: i64,
    pub examples: &'a [String],
    pub quotes: &'a [String],
}

/// 一个绑不到类的类别词
#[derive(Debug, Clone)]
pub struct OpenKindWord<'a> {
    pub id: i64,
    pub kind_word: &'a str,
    pub count: i64,
    pub examples: &'a [String],
    pub phrases: &'a [String],
}

/// 本体现状：类与属性，照库里的语言给
#[derive(Debug, Clone, Default)]
pub struct Glossary<'a> {
    /// (key, label, description)
    pub classes: Vec<(&'a str, &'a str, &'a str)>,
    /// (key, label, kind, domains, ranges, description)
    pub properties: Vec<GlossaryProperty<'a>>,
}

/// 词表里的一条属性：(key, label, kind, domains, ranges, description)
pub type GlossaryProperty<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Vec<&'a str>,
    Vec<&'a str>,
    &'a str,
);

/// 一条能力问题
#[derive(Debug, Clone)]
pub struct QuestionItem<'a> {
    pub id: i64,
    pub text: &'a str,
}

/// 代理提的一条元素
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// class | property
    pub kind: String,
    pub key: String,
    pub label: String,
    pub definition: String,
    /// 属性：宾语是值（attribute）还是实体（relation）
    pub value: bool,
    /// 值属性的数据类型：number | date | text | bool
    pub datatype: Option<String>,
    /// 属性的定义域 / 值域，或类的父类：类键，可以是本体里的，也可以是这一批里刚提的
    pub domains: Vec<String>,
    pub ranges: Vec<String>,
    pub parents: Vec<String>,
    /// 它会绑上的签名 id 与类别词 id（本批内的）
    pub signatures: Vec<i64>,
    pub kind_words: Vec<i64>,
    /// 它服务的问题 id
    pub questions: Vec<i64>,
}

/// 「本体里已经有」：现有属性或类的键，与它接得住的形状
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Existing {
    pub key: String,
    pub signatures: Vec<i64>,
    pub kind_words: Vec<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProposalReply {
    pub proposals: Vec<Proposal>,
    pub existing: Vec<Existing>,
    /// 读不出的项数
    pub malformed: usize,
}

const SYSTEM: &str = "\
You are the ontology agent of a knowledge base. The base extracts statements from documents as \
they are worded; an aligner then binds each statement shape (a relation phrase between a subject \
class and an object class, or a kind word the documents use for a thing) to a property or class of \
the ontology. The shapes below are the ones the ontology cannot hold: no property or class fits \
them. The competency questions are what the base exists to answer.\n\
Propose the smallest set of new classes and properties that would let the base hold these shapes \
AND serve the questions. Rules:\n\
- A proposal binds shapes: list the shape ids it would bind (s for signatures, k for kind words). \
Do not propose an element that binds nothing.\n\
- Prefer serving a question. An element that serves no question must bind at least two statements' \
worth of shapes, or be a class that several kind words name.\n\
- Write the definition for the aligner, in the language of the ontology: one or two sentences saying \
what the property asserts, what its subject and object are, and what it does NOT cover.\n\
- A property whose object is a value (a number, a date, text) is an attribute: value=true with a \
datatype (number | date | text | bool) and no ranges. Otherwise value=false with domains and ranges \
as class keys, existing or proposed in this same answer.\n\
- Keys are snake_case ASCII. Reuse an existing key only under existing, never as a proposal.\n\
- If an existing property or class already holds a shape, list it under existing with the shape ids \
instead of proposing.\n\
Answer with one JSON object and nothing else:\n\
{\"p\":[{\"kind\":\"property\",\"key\":\"...\",\"label\":\"...\",\"definition\":\"...\",\"value\":false,\
\"datatype\":null,\"domains\":[\"class_key\"],\"ranges\":[\"class_key\"],\"s\":[ids],\"k\":[ids],\"q\":[ids]},\
{\"kind\":\"class\",\"key\":\"...\",\"label\":\"...\",\"definition\":\"...\",\"parents\":[\"class_key\"],\
\"s\":[],\"k\":[ids],\"q\":[ids]}],\"existing\":[{\"key\":\"existing_key\",\"s\":[ids],\"k\":[ids]}]}";

pub fn build_proposal_messages(
    signatures: &[OpenSignature<'_>],
    kind_words: &[OpenKindWord<'_>],
    glossary: &Glossary<'_>,
    questions: &[QuestionItem<'_>],
) -> Vec<ChatMessage> {
    let mut user = String::new();
    user.push_str("Ontology classes:\n");
    if glossary.classes.is_empty() {
        user.push_str("  (none)\n");
    }
    for (key, label, description) in &glossary.classes {
        user.push_str(&format!("- {key} · {label}"));
        if !description.trim().is_empty() {
            user.push_str(&format!(" · {}", description.trim()));
        }
        user.push('\n');
    }
    user.push_str("\nOntology properties:\n");
    if glossary.properties.is_empty() {
        user.push_str("  (none)\n");
    }
    for (key, label, kind, domains, ranges, description) in &glossary.properties {
        user.push_str(&format!("- {key} · {label} · {kind}"));
        if !domains.is_empty() {
            user.push_str(&format!(" · domain: {}", domains.join(", ")));
        }
        if !ranges.is_empty() {
            user.push_str(&format!(" · range: {}", ranges.join(", ")));
        }
        if !description.trim().is_empty() {
            user.push_str(&format!(" · {}", description.trim()));
        }
        user.push('\n');
    }
    user.push_str("\nCompetency questions:\n");
    if questions.is_empty() {
        user.push_str(
            "  (none written yet: propose for the shapes that bind the most statements)\n",
        );
    }
    for q in questions {
        user.push_str(&format!("q{}: {}\n", q.id, q.text.trim()));
    }
    user.push_str("\nShapes the ontology cannot hold:\n");
    for s in signatures {
        let subject = s.subject_class.unwrap_or("(untyped)");
        let object = if s.object_is_value {
            "(a value)".to_string()
        } else {
            s.object_class.unwrap_or("(untyped)").to_string()
        };
        user.push_str(&format!(
            "s{}: phrase \"{}\" · subject class: {} · object: {} · {} statements\n",
            s.id, s.phrase, subject, object, s.statement_count
        ));
        for (i, ex) in s.examples.iter().enumerate() {
            let quote = s.quotes.get(i).map(String::as_str).unwrap_or("").trim();
            if quote.is_empty() {
                user.push_str(&format!("  · {ex}\n"));
            } else {
                user.push_str(&format!("  · {ex}\n    \"{quote}\"\n"));
            }
        }
    }
    for k in kind_words {
        user.push_str(&format!(
            "k{}: kind word \"{}\" · {} things",
            k.id, k.kind_word, k.count
        ));
        if !k.examples.is_empty() {
            user.push_str(&format!(" · e.g. {}", k.examples.join(", ")));
        }
        if !k.phrases.is_empty() {
            user.push_str(&format!(
                " · they are subjects of: {}",
                k.phrases.join(", ")
            ));
        }
        user.push('\n');
    }
    vec![
        ChatMessage {
            role: "system".into(),
            content: SYSTEM.into(),
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}

fn ids(v: Option<&Value>, prefix: char, known: &HashSet<i64>) -> Result<Vec<i64>, ()> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let Some(arr) = v.as_array() else {
        return Err(());
    };
    let mut out = Vec::new();
    for x in arr {
        let id = match x {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.trim().trim_start_matches(prefix).parse::<i64>().ok(),
            _ => None,
        };
        match id {
            Some(id) if known.contains(&id) => out.push(id),
            _ => return Err(()),
        }
    }
    Ok(out)
}

fn strings(v: Option<&Value>) -> Result<Vec<String>, ()> {
    let Some(v) = v else { return Ok(Vec::new()) };
    match v {
        Value::Null => Ok(Vec::new()),
        Value::String(s) => Ok(vec![s.trim().to_string()]),
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_str().map(|s| s.trim().to_string()).ok_or(()))
            .collect(),
        _ => Err(()),
    }
}

fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && k.chars().next().is_some_and(|c| c.is_ascii_lowercase())
}

/// 读回复。`signature_ids` / `kind_word_ids` / `question_ids` 是这一批里合法的 id：答案里指向别处的算坏项
pub fn parse_proposal_response(
    raw: &str,
    signature_ids: &HashSet<i64>,
    kind_word_ids: &HashSet<i64>,
    question_ids: &HashSet<i64>,
) -> anyhow::Result<ProposalReply> {
    let v = parse_value(raw)?;
    let mut reply = ProposalReply::default();
    let items: Vec<&Value> = match v.get("p") {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(Value::Object(m)) => m.values().collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(_) => {
            reply.malformed += 1;
            Vec::new()
        }
    };
    let mut seen: HashSet<String> = HashSet::new();
    for it in items {
        let Some(obj) = it.as_object() else {
            reply.malformed += 1;
            continue;
        };
        let kind = obj
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("property")
            .trim()
            .to_lowercase();
        let key = obj
            .get("key")
            .and_then(Value::as_str)
            .map(|k| k.trim().to_lowercase())
            .unwrap_or_default();
        if !matches!(kind.as_str(), "class" | "property")
            || !valid_key(&key)
            || !seen.insert(key.clone())
        {
            reply.malformed += 1;
            continue;
        }
        let label = obj
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&key)
            .to_string();
        let definition = obj
            .get("definition")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        let value = obj.get("value").and_then(Value::as_bool).unwrap_or(false);
        let datatype = obj
            .get("datatype")
            .and_then(Value::as_str)
            .map(|d| d.trim().to_lowercase())
            .filter(|d| matches!(d.as_str(), "number" | "date" | "text" | "bool"));
        let (Ok(domains), Ok(ranges), Ok(parents)) = (
            strings(obj.get("domains")),
            strings(obj.get("ranges")),
            strings(obj.get("parents")),
        ) else {
            reply.malformed += 1;
            continue;
        };
        let (Ok(signatures), Ok(kind_words), Ok(questions)) = (
            ids(obj.get("s"), 's', signature_ids),
            ids(obj.get("k"), 'k', kind_word_ids),
            ids(obj.get("q"), 'q', question_ids),
        ) else {
            reply.malformed += 1;
            continue;
        };
        // 什么形状都不绑的提案不是从图谱里长出来的：不收
        if signatures.is_empty() && kind_words.is_empty() {
            reply.malformed += 1;
            continue;
        }
        reply.proposals.push(Proposal {
            kind,
            key,
            label,
            definition,
            value,
            datatype: if value { datatype } else { None },
            domains: domains.into_iter().filter(|d| !d.is_empty()).collect(),
            ranges: if value {
                Vec::new()
            } else {
                ranges.into_iter().filter(|r| !r.is_empty()).collect()
            },
            parents: parents.into_iter().filter(|p| !p.is_empty()).collect(),
            signatures,
            kind_words,
            questions,
        });
    }
    if let Some(Value::Array(ex)) = v.get("existing") {
        for it in ex {
            let Some(obj) = it.as_object() else {
                reply.malformed += 1;
                continue;
            };
            let key = obj
                .get("key")
                .and_then(Value::as_str)
                .map(|k| k.trim().to_string())
                .unwrap_or_default();
            let (Ok(signatures), Ok(kind_words)) = (
                ids(obj.get("s"), 's', signature_ids),
                ids(obj.get("k"), 'k', kind_word_ids),
            ) else {
                reply.malformed += 1;
                continue;
            };
            if key.is_empty() || (signatures.is_empty() && kind_words.is_empty()) {
                reply.malformed += 1;
                continue;
            }
            reply.existing.push(Existing {
                key,
                signatures,
                kind_words,
            });
        }
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids_of(xs: &[i64]) -> HashSet<i64> {
        xs.iter().copied().collect()
    }

    #[test]
    fn a_well_formed_reply_yields_proposals_and_existing() {
        let raw = r#"{"p":[{"kind":"property","key":"supplies","label":"supplies","definition":"The subject organisation delivers goods to the object organisation. Not for one-off sales.","value":false,"datatype":null,"domains":["organization"],"ranges":["organization"],"s":[0,2],"k":[],"q":[1]},{"kind":"class","key":"supplier","label":"Supplier","definition":"An organisation that supplies others.","parents":["organization"],"s":[],"k":[0],"q":[]}],"existing":[{"key":"located_in","s":[1],"k":[]}]}"#;
        let r = parse_proposal_response(raw, &ids_of(&[0, 1, 2]), &ids_of(&[0]), &ids_of(&[1]))
            .unwrap();
        assert_eq!(r.malformed, 0);
        assert_eq!(r.proposals.len(), 2);
        assert_eq!(r.proposals[0].key, "supplies");
        assert_eq!(r.proposals[0].signatures, vec![0, 2]);
        assert_eq!(r.proposals[0].questions, vec![1]);
        assert_eq!(r.proposals[0].domains, vec!["organization"]);
        assert_eq!(r.proposals[1].kind, "class");
        assert_eq!(r.proposals[1].parents, vec!["organization"]);
        assert_eq!(
            r.existing,
            vec![Existing {
                key: "located_in".into(),
                signatures: vec![1],
                kind_words: vec![]
            }]
        );
    }

    #[test]
    fn a_proposal_that_binds_nothing_or_points_outside_the_batch_is_malformed() {
        let raw = r#"{"p":[{"kind":"property","key":"orphan","label":"o","definition":"","s":[],"k":[],"q":[]},{"kind":"property","key":"far","label":"f","definition":"","s":[9],"k":[],"q":[]},{"kind":"property","key":"Bad Key","label":"b","definition":"","s":[0],"k":[],"q":[]},{"kind":"property","key":"ok","label":"ok","definition":"d","value":true,"datatype":"number","s":["s0"],"k":[],"q":["q1"]}]}"#;
        let r = parse_proposal_response(raw, &ids_of(&[0]), &ids_of(&[]), &ids_of(&[1])).unwrap();
        assert_eq!(r.malformed, 3);
        assert_eq!(r.proposals.len(), 1);
        assert_eq!(r.proposals[0].key, "ok");
        assert!(r.proposals[0].value);
        assert_eq!(r.proposals[0].datatype.as_deref(), Some("number"));
        assert_eq!(
            r.proposals[0].signatures,
            vec![0],
            "an id written as s0 still counts"
        );
        assert_eq!(r.proposals[0].questions, vec![1]);
    }

    #[test]
    fn the_prompt_names_every_input_once() {
        let examples = vec!["W-1 —supplies→ F-2".to_string()];
        let quotes = vec!["W-1 supplies F-2 with parts.".to_string()];
        let sigs = vec![OpenSignature {
            id: 0,
            phrase: "supplies",
            subject_class: Some("organization"),
            object_class: Some("organization"),
            object_is_value: false,
            statement_count: 7,
            examples: &examples,
            quotes: &quotes,
        }];
        let names = vec!["Acme Supply".to_string()];
        let phrases = vec!["supplies".to_string()];
        let kws = vec![OpenKindWord {
            id: 0,
            kind_word: "supplier",
            count: 3,
            examples: &names,
            phrases: &phrases,
        }];
        let glossary = Glossary {
            classes: vec![("organization", "Organization", "a company or institution")],
            properties: vec![(
                "located_in",
                "located in",
                "relation",
                vec!["organization"],
                vec!["location"],
                "where a thing is",
            )],
        };
        let qs = vec![QuestionItem {
            id: 1,
            text: "Who supplies whom?",
        }];
        let m = build_proposal_messages(&sigs, &kws, &glossary, &qs);
        assert_eq!(m.len(), 2);
        let user = &m[1].content;
        for needle in [
            "s0: phrase \"supplies\"",
            "k0: kind word \"supplier\"",
            "q1: Who supplies whom?",
            "- located_in · located in · relation · domain: organization · range: location",
            "\"W-1 supplies F-2 with parts.\"",
        ] {
            assert!(user.contains(needle), "missing {needle:?} in\n{user}");
        }
    }
}
