//! 给没有能力问题的库提问题（0061 决定 1）：从开放图谱说得最多的东西——陈述最多的形状、
//! 最多的类别词、连接最多的实体——出发，让模型写出这个库该答得上来的问题。提出来的是
//! `proposed`，人接受、改或拒；代理不替人决定库是干什么的。

use std::collections::HashSet;

use serde_json::Value;
use utopia_llm::ChatMessage;

use crate::align::parse_value;
use crate::ontology_agent::Glossary;

/// 一条说得多的形状；`property` 是对齐绑上的属性键（绑上了才有）
#[derive(Debug, Clone)]
pub struct TopSignature<'a> {
    pub phrase: &'a str,
    pub subject_class: Option<&'a str>,
    pub object_class: Option<&'a str>,
    pub object_is_value: bool,
    pub statement_count: i64,
    pub property: Option<&'a str>,
    pub example: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct TopKindWord<'a> {
    pub kind_word: &'a str,
    pub count: i64,
}

#[derive(Debug, Clone)]
pub struct TopEntity<'a> {
    pub name: &'a str,
    pub class: Option<&'a str>,
    pub degree: i64,
}

/// 一条提出来的问题：原话，加上答它要走的类与属性（都是现有的键；模型编的键丢掉）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedQuestion {
    pub question: String,
    pub classes: Vec<String>,
    pub properties: Vec<String>,
}

const SYSTEM: &str = "You write competency questions for a knowledge base built from documents. \
A competency question is a question a person would actually ask this base and expect it to answer \
from its graph; the ontology (its classes and properties) is judged by whether such questions can be \
answered. You are given what the documents say most: the most frequent statement shapes (a phrase \
between a subject class and an object class or a value, with the property it binds to if any), the \
most frequent kind words, and the most connected entities, plus the current ontology.\n\
Rules:\n\
- Write questions in the language of the documents, as a person would phrase them, specific enough \
to have a checkable answer (name the kind of thing asked for; a concrete entity from the list may \
be used when it makes the question natural).\n\
- Prefer questions that need the graph to answer: several statements, or a join across two \
properties. Do not ask what a single sentence answers.\n\
- Each question lists the classes and properties an answer must traverse, as keys from the \
ontology; leave the lists empty when the ontology has no element for it yet (that gap is the point).\n\
- No duplicates, no yes/no questions, at most the number asked for.\n\
Answer with one JSON object and nothing else:\n\
{\"q\":[{\"question\":\"...\",\"classes\":[\"class_key\"],\"properties\":[\"property_key\"]}]}";

pub fn build_question_messages(
    signatures: &[TopSignature<'_>],
    kind_words: &[TopKindWord<'_>],
    entities: &[TopEntity<'_>],
    glossary: &Glossary<'_>,
    existing: &[&str],
    want: usize,
) -> Vec<ChatMessage> {
    let mut user = String::new();
    user.push_str("Ontology classes:\n");
    if glossary.classes.is_empty() {
        user.push_str("  (none)\n");
    }
    for (key, label, description) in &glossary.classes {
        user.push_str(&format!("- {key} · {label}"));
        if !description.is_empty() {
            user.push_str(&format!(" · {description}"));
        }
        user.push('\n');
    }
    user.push_str("Ontology properties:\n");
    if glossary.properties.is_empty() {
        user.push_str("  (none)\n");
    }
    for (key, label, kind, domains, ranges, description) in &glossary.properties {
        user.push_str(&format!("- {key} · {label} · {kind}"));
        if !domains.is_empty() || !ranges.is_empty() {
            user.push_str(&format!(" · {} → {}", domains.join("|"), ranges.join("|")));
        }
        if !description.is_empty() {
            user.push_str(&format!(" · {description}"));
        }
        user.push('\n');
    }
    user.push_str("\nMost frequent statement shapes:\n");
    for s in signatures {
        user.push_str(&format!(
            "- {} — \"{}\" → {} · {} statements",
            s.subject_class.unwrap_or("?"),
            s.phrase,
            if s.object_is_value {
                "value"
            } else {
                s.object_class.unwrap_or("?")
            },
            s.statement_count
        ));
        match s.property {
            Some(p) => user.push_str(&format!(" · bound to {p}")),
            None => user.push_str(" · not bound to any property"),
        }
        if let Some(e) = s.example {
            user.push_str(&format!(" · e.g. {e}"));
        }
        user.push('\n');
    }
    user.push_str("\nMost frequent kind words:\n");
    for k in kind_words {
        user.push_str(&format!("- {} · {} entities\n", k.kind_word, k.count));
    }
    user.push_str("\nMost connected entities:\n");
    for e in entities {
        user.push_str(&format!(
            "- {} ({}) · {} statements\n",
            e.name,
            e.class.unwrap_or("?"),
            e.degree
        ));
    }
    if !existing.is_empty() {
        user.push_str("\nQuestions the base already has (do not repeat them):\n");
        for q in existing {
            user.push_str(&format!("- {q}\n"));
        }
    }
    user.push_str(&format!("\nWrite up to {want} competency questions."));
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

/// 读回复。`class_keys` / `property_keys` 是本体现有的键：答案里不存在的键丢掉（不算坏项——
/// 模型常把它想要的属性写成键，那正是缺口）。返回 (问题, 坏项数)
pub fn parse_question_response(
    raw: &str,
    class_keys: &HashSet<&str>,
    property_keys: &HashSet<&str>,
) -> anyhow::Result<(Vec<ProposedQuestion>, usize)> {
    let v = parse_value(raw)?;
    let items: Vec<&Value> = match v.get("q") {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(_) => anyhow::bail!("`q` is not a list"),
    };
    let mut out: Vec<ProposedQuestion> = Vec::new();
    let mut malformed = 0usize;
    let keys = |v: Option<&Value>, allowed: &HashSet<&str>| -> Vec<String> {
        v.and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|k| allowed.contains(k))
            .map(str::to_string)
            .collect()
    };
    for it in items {
        let Some(obj) = it.as_object() else {
            malformed += 1;
            continue;
        };
        let question = obj
            .get("question")
            .and_then(Value::as_str)
            .map(|q| q.trim().trim_end_matches('\n').to_string())
            .unwrap_or_default();
        if question.chars().count() < 8 {
            malformed += 1;
            continue;
        }
        if out
            .iter()
            .any(|q| q.question.eq_ignore_ascii_case(&question))
        {
            continue;
        }
        out.push(ProposedQuestion {
            question,
            classes: keys(obj.get("classes"), class_keys),
            properties: keys(obj.get("properties"), property_keys),
        });
    }
    Ok((out, malformed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_lists_shapes_kind_words_entities_and_existing_questions() {
        let glossary = Glossary {
            classes: vec![("person", "Person", "")],
            properties: vec![(
                "p20",
                "placeOfDeath",
                "relation",
                vec!["person"],
                vec!["location"],
                "",
            )],
        };
        let sigs = [TopSignature {
            phrase: "died in",
            subject_class: Some("person"),
            object_class: Some("location"),
            object_is_value: false,
            statement_count: 7,
            property: Some("p20"),
            example: Some("Orlov died in Moscow"),
        }];
        let words = [TopKindWord {
            kind_word: "district",
            count: 12,
        }];
        let ents = [TopEntity {
            name: "Pedro II",
            class: Some("person"),
            degree: 9,
        }];
        let m = build_question_messages(&sigs, &words, &ents, &glossary, &["Who died where?"], 10);
        let user = &m[1].content;
        assert!(user.contains("\"died in\" → location · 7 statements · bound to p20"));
        assert!(user.contains("district · 12 entities"));
        assert!(user.contains("Pedro II (person) · 9 statements"));
        assert!(user.contains("do not repeat them"));
        assert!(user.contains("Who died where?"));
        assert!(user.contains("up to 10"));
    }

    #[test]
    fn keys_the_ontology_lacks_are_dropped_and_short_or_duplicate_questions_are_not_kept() {
        let classes: HashSet<&str> = ["person"].into();
        let props: HashSet<&str> = ["p20"].into();
        let raw = r#"{"q":[
          {"question":"Where did each person die?","classes":["person","ghost"],"properties":["p20","died_in"]},
          {"question":"where did each PERSON die?","classes":[],"properties":[]},
          {"question":"Why?","classes":[],"properties":[]},
          "not an object"
        ]}"#;
        let (qs, malformed) = parse_question_response(raw, &classes, &props).unwrap();
        assert_eq!(
            qs,
            vec![ProposedQuestion {
                question: "Where did each person die?".into(),
                classes: vec!["person".into()],
                properties: vec!["p20".into()],
            }]
        );
        assert_eq!(malformed, 2);
    }
}
