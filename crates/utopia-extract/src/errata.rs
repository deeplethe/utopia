//! 勘误 agent 的提示词与回复解析（0044 决定 7）：一份文档、本体的属性、从它读出的事实，
//! 结构报了的带着理由；模型对每一条说 keep / retract / revise，全部答完才许 add。
//! 撤、改、加都得引文档的原话——引文在不在文档里由调用方验，这里只认形状。

use utopia_llm::ChatMessage;

/// 送去看的一条事实
#[derive(Debug, Clone)]
pub struct ErrataFact<'a> {
    pub id: i64,
    pub subject: &'a str,
    pub subject_class: Option<&'a str>,
    pub property: &'a str,
    pub object: &'a str,
    pub object_class: Option<&'a str>,
    /// 结构报的理由：domain / range / name_absent / no_date；空 = 抽样
    pub flag: Option<&'a str>,
    pub quote: Option<&'a str>,
}

/// 本体里的一条属性，给模型当词汇表
#[derive(Debug, Clone)]
pub struct ErrataProperty<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub description: &'a str,
    /// relation（宾语是东西）| attribute（宾语是值）
    pub kind: &'a str,
    pub domains: Vec<&'a str>,
    pub ranges: Vec<&'a str>,
    pub datatype: Option<&'a str>,
}

pub const ERRATA_SYSTEM: &str = "You review facts that were read from ONE document into a knowledge graph. \
You get the document, the ontology's properties, and a numbered list of facts. Some facts carry a FLAG from a structural check: \
domain = the subject is not a kind of thing the property allows; range = the object is not; \
name_absent = a name in the fact does not occur in the document; no_date = a date property holds something that is not a date.\n\
For EVERY fact answer one of:\n\
- keep: the document states it and the property fits.\n\
- retract: the document does not state it, or no property in the ontology fits what it states.\n\
- revise: the document states something close; give the property key and/or the object the document supports (a name that occurs in the document, or a value).\n\
Only after you have answered every fact may you add facts the document states plainly, the ontology can hold, and the list lacks. \
Do not add a fact whose subject or object is not named in the document.\n\
Every retract, revise and add must quote the document's own words: one contiguous span, verbatim. Never quote words that are not in the document. \
Prefer keeping a fact over retracting it when the document supports it; removing a correct fact costs more than leaving a doubtful one.\n\
Answer with JSON only, no prose:\n\
{\"a\":[[id,\"keep\"],[id,\"retract\",reason,quote],[id,\"revise\",{\"property\":key|null,\"object\":text|null},reason,quote],\
[null,\"add\",{\"subject\":name,\"property\":key,\"object\":name_or_value},reason,quote]]}";

fn property_line(p: &ErrataProperty<'_>) -> String {
    let mut line = format!("- {} ({}): {}", p.key, p.label, p.description.trim());
    match p.kind {
        "attribute" => {
            line.push_str(&format!(
                " [value{}]",
                p.datatype.map(|d| format!(": {d}")).unwrap_or_default()
            ));
        }
        _ => line.push_str(" [thing]"),
    }
    if !p.domains.is_empty() {
        line.push_str(&format!(" subject: {}", p.domains.join("|")));
    }
    if !p.ranges.is_empty() {
        line.push_str(&format!(" object: {}", p.ranges.join("|")));
    }
    line
}

fn fact_line(f: &ErrataFact<'_>) -> String {
    let class = |c: Option<&str>| c.map(|c| format!(" ({c})")).unwrap_or_default();
    let mut line = format!(
        "{}: {}{} —{}→ {}{}",
        f.id,
        f.subject,
        class(f.subject_class),
        f.property,
        f.object,
        class(f.object_class)
    );
    if let Some(flag) = f.flag {
        line.push_str(&format!(" FLAG {flag}"));
    }
    if let Some(q) = f.quote.map(str::trim).filter(|q| !q.is_empty()) {
        line.push_str(&format!(" · read from: \"{q}\""));
    }
    line
}

pub fn build_errata_messages(
    document: &str,
    properties: &[ErrataProperty<'_>],
    facts: &[ErrataFact<'_>],
) -> Vec<ChatMessage> {
    let props = properties
        .iter()
        .map(property_line)
        .collect::<Vec<_>>()
        .join("\n");
    let list = facts.iter().map(fact_line).collect::<Vec<_>>().join("\n");
    vec![
        ChatMessage {
            role: "system".into(),
            content: ERRATA_SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user".into(),
            content: format!("DOCUMENT:\n{document}\n\nPROPERTIES:\n{props}\n\nFACTS:\n{list}"),
        },
    ]
}

/// 模型对一条事实的说法
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    Retract,
    Revise {
        property: Option<String>,
        object: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactVerdict {
    pub id: i64,
    pub verdict: Verdict,
    pub reason: String,
    pub quote: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Addition {
    pub subject: String,
    pub property: String,
    pub object: String,
    pub reason: String,
    pub quote: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ErrataResponse {
    pub verdicts: Vec<FactVerdict>,
    pub additions: Vec<Addition>,
    /// 形状不对的行：不认识的 id、重复的 id、撤改加没带引文、改了等于没改
    pub malformed: usize,
    /// 没把给它的事实答完就加的：加的全部不算（「先查完给它的，再加」是契约）
    pub additions_refused: usize,
}

fn text_of(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|x| match x {
        serde_json::Value::String(s) => Some(s.trim().to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
    .filter(|s| !s.is_empty())
}

/// 解析勘误的回复。给出的事实一条一票，多的、错的计数不算；加的只在全部答完时算
pub fn parse_errata_response(
    raw: &str,
    facts: &[ErrataFact<'_>],
) -> Result<ErrataResponse, String> {
    let v: serde_json::Value =
        serde_json::from_str(extract_json(raw)).map_err(|e| e.to_string())?;
    let rows = v
        .get("a")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "no \"a\" array".to_string())?;
    let mut out = ErrataResponse::default();
    let mut seen = std::collections::HashSet::new();
    let mut additions = Vec::new();
    for row in rows {
        let Some(arr) = row.as_array() else {
            out.malformed += 1;
            continue;
        };
        let action = arr.get(1).and_then(|x| x.as_str()).unwrap_or("");
        if action == "add" {
            let parsed = (|| {
                let spec = arr.get(2)?.as_object()?;
                Some(Addition {
                    subject: text_of(spec.get("subject"))?,
                    property: text_of(spec.get("property"))?,
                    object: text_of(spec.get("object"))?,
                    reason: text_of(arr.get(3)).unwrap_or_default(),
                    quote: text_of(arr.get(4))?,
                })
            })();
            match parsed {
                Some(a) => additions.push(a),
                None => out.malformed += 1,
            }
            continue;
        }
        let parsed = (|| {
            let id = arr.first()?.as_i64()?;
            facts.iter().find(|f| f.id == id)?;
            if !seen.insert(id) {
                return None;
            }
            match action {
                "keep" => Some(FactVerdict {
                    id,
                    verdict: Verdict::Keep,
                    reason: text_of(arr.get(2)).unwrap_or_default(),
                    quote: None,
                }),
                "retract" => Some(FactVerdict {
                    id,
                    verdict: Verdict::Retract,
                    reason: text_of(arr.get(2)).unwrap_or_default(),
                    quote: Some(text_of(arr.get(3))?),
                }),
                "revise" => {
                    let spec = arr.get(2)?.as_object()?;
                    let property = text_of(spec.get("property"));
                    let object = text_of(spec.get("object"));
                    if property.is_none() && object.is_none() {
                        return None;
                    }
                    Some(FactVerdict {
                        id,
                        verdict: Verdict::Revise { property, object },
                        reason: text_of(arr.get(3)).unwrap_or_default(),
                        quote: Some(text_of(arr.get(4))?),
                    })
                }
                _ => None,
            }
        })();
        match parsed {
            Some(v) => out.verdicts.push(v),
            None => out.malformed += 1,
        }
    }
    if facts.iter().all(|f| seen.contains(&f.id)) {
        out.additions = additions;
    } else {
        out.additions_refused = additions.len();
    }
    Ok(out)
}

fn extract_json(raw: &str) -> &str {
    match (raw.find('{'), raw.rfind('}')) {
        (Some(a), Some(b)) if b > a => &raw[a..=b],
        _ => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Vec<ErrataFact<'static>> {
        vec![
            ErrataFact {
                id: 0,
                subject: "Acme",
                subject_class: Some("organization"),
                property: "based_in",
                object: "Paris",
                object_class: Some("place"),
                flag: Some("name_absent"),
                quote: Some("Acme is based in London"),
            },
            ErrataFact {
                id: 1,
                subject: "Acme",
                subject_class: Some("organization"),
                property: "based_in",
                object: "London",
                object_class: Some("place"),
                flag: None,
                quote: None,
            },
        ]
    }

    #[test]
    fn the_prompt_carries_the_document_the_flags_and_the_properties() {
        let props = vec![ErrataProperty {
            key: "based_in",
            label: "based in",
            description: "where an organization is based",
            kind: "relation",
            domains: vec!["organization"],
            ranges: vec!["place"],
            datatype: None,
        }];
        let m = build_errata_messages("Acme is based in London.", &props, &facts());
        assert_eq!(m.len(), 2);
        assert!(m[0].content.contains("JSON only"));
        let u = &m[1].content;
        assert!(u.contains("DOCUMENT:\nAcme is based in London."));
        assert!(u.contains("- based_in (based in): where an organization is based [thing] subject: organization object: place"));
        assert!(u.contains("0: Acme (organization) —based_in→ Paris (place) FLAG name_absent · read from: \"Acme is based in London\""));
        assert!(
            u.contains("1: Acme (organization) —based_in→ London (place)\n")
                || u.ends_with("1: Acme (organization) —based_in→ London (place)")
        );
    }

    #[test]
    fn every_fact_gets_one_verdict_and_bad_rows_are_counted_not_believed() {
        let raw = r#"Sure: {"a":[[0,"retract","not in the document","Acme is based in London"],[1,"keep"],[0,"keep"],[7,"keep"],[1,"revise",{"property":null,"object":null},"x","q"],["nope"]]}"#;
        let r = parse_errata_response(raw, &facts()).unwrap();
        assert_eq!(r.verdicts.len(), 2);
        assert_eq!(r.verdicts[0].verdict, Verdict::Retract);
        assert_eq!(
            r.verdicts[0].quote.as_deref(),
            Some("Acme is based in London")
        );
        assert_eq!(r.verdicts[1].verdict, Verdict::Keep);
        // 重复的 0、不认识的 7、改了等于没改的、不是数组的：四条坏行
        assert_eq!(r.malformed, 4);
    }

    #[test]
    fn a_retraction_without_a_quote_is_malformed() {
        let raw = r#"{"a":[[0,"retract","no quote"],[1,"keep"]]}"#;
        let r = parse_errata_response(raw, &facts()).unwrap();
        assert_eq!(r.verdicts.len(), 1);
        assert_eq!(r.malformed, 1);
    }

    #[test]
    fn additions_count_only_after_every_given_fact_is_answered() {
        let add = r#"[null,"add",{"subject":"Acme","property":"ceo","object":"Jane Roe"},"stated","Jane Roe runs Acme"]"#;
        let partial = format!(r#"{{"a":[[0,"keep"],{add}]}}"#);
        let r = parse_errata_response(&partial, &facts()).unwrap();
        assert!(r.additions.is_empty());
        assert_eq!(r.additions_refused, 1);
        let full = format!(
            r#"{{"a":[[0,"keep"],[1,"keep"],{add},[null,"add",{{"subject":"Acme"}},"half","q"]]}}"#
        );
        let r = parse_errata_response(&full, &facts()).unwrap();
        assert_eq!(r.additions.len(), 1);
        assert_eq!(r.additions[0].object, "Jane Roe");
        assert_eq!(r.additions[0].quote, "Jane Roe runs Acme");
        assert_eq!(r.malformed, 1);
    }

    #[test]
    fn no_array_is_an_error_not_an_empty_answer() {
        assert!(parse_errata_response("{}", &facts()).is_err());
        assert!(parse_errata_response("not json", &facts()).is_err());
    }
}
