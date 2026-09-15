//! 审核台其余几档交给 agent 时的提示词与回复解析（0043）。
//!
//! 与重复对的裁决（`build_adjudication_messages`）同一个形状：一次一批带编号的项，每项
//! 附上人在这个库里做过的同类决定，回一个 JSON。模型只给出路与一句理由；它给的日期、
//! 引文由服务端去核对在不在证据里——结构检查在服务端，语义在这里。

use serde::Deserialize;
use utopia_llm::ChatMessage;

/// 一条证据，给模型看的样子
#[derive(Debug, Clone)]
pub struct EvidenceLine {
    pub document: String,
    /// 文档自己的日期，YYYY-MM-DD
    pub dated: Option<String>,
    pub quote: Option<String>,
}

/// 一条事实，给模型看的样子
#[derive(Debug, Clone)]
pub struct FactCard {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// 起止，已经写成 YYYY[-MM[-DD]] / unknown / 空
    pub from: Option<String>,
    pub to: Option<String>,
    pub confidence: f32,
    pub evidence: Vec<EvidenceLine>,
}

/// 事实那两档里的一项
#[derive(Debug, Clone)]
pub struct FactQuestion {
    pub fact: FactCard,
    /// 证据全在文档的旧版本上；`current` 是文档现在提到主语的那几段
    pub stale: bool,
    pub current: Vec<String>,
    pub precedents: Vec<String>,
}

/// 冲突那一档里的一项
#[derive(Debug, Clone)]
pub struct ConflictQuestion {
    /// no_time | simultaneous | low_confidence
    pub reason: String,
    pub old: FactCard,
    pub new: FactCard,
    /// 同一持有者、同一关系上的其他值
    pub neighbours: Vec<FactCard>,
    pub precedents: Vec<String>,
}

/// 模型对一项的回答
#[derive(Debug, Clone, Deserialize)]
pub struct QueueVerdict {
    pub i: usize,
    pub action: String,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub why: Option<String>,
    /// 冲突：close_old 闭合在哪天、retime_new 改成从哪天起，YYYY[-MM[-DD]]
    #[serde(default)]
    pub date: Option<String>,
    /// 事实：证据过期时，文档现在那段里说出这件事的原话
    #[serde(default)]
    pub quote: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Reply {
    #[serde(default)]
    verdicts: Vec<QueueVerdict>,
}

pub fn parse_verdicts(raw: &str) -> anyhow::Result<Vec<QueueVerdict>> {
    let json = crate::json_block(raw)?;
    let reply: Reply = serde_json::from_str(&json)
        .map_err(|e| anyhow::anyhow!("Failed to parse queue verdicts: {e}"))?;
    Ok(reply.verdicts)
}

const PRECEDENTS_NOTE: &str = "Some items carry precedents: decisions people made in this same \
knowledge base on items of this kind. Treat them as how the owners of this base want such cases \
judged, and follow one whose ground holds here; a precedent never overrides what the evidence says.";

fn card(f: &FactCard) -> String {
    let when = match (&f.from, &f.to) {
        (None, None) => String::new(),
        (from, to) => format!(
            " [{} → {}]",
            from.as_deref().unwrap_or("?"),
            to.as_deref().unwrap_or("now")
        ),
    };
    let evidence = if f.evidence.is_empty() {
        "    (no evidence)".to_string()
    } else {
        f.evidence
            .iter()
            .map(|e| {
                format!(
                    "    - {}{}: \"{}\"",
                    e.document,
                    e.dated
                        .as_deref()
                        .map(|d| format!(" (dated {d})"))
                        .unwrap_or_default(),
                    e.quote.as_deref().unwrap_or("(no quote)")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "{} · {} · {}{when} (confidence {:.2})\n  evidence:\n{evidence}",
        f.subject, f.predicate, f.object, f.confidence
    )
}

fn precedent_lines(p: &[String]) -> String {
    if p.is_empty() {
        String::new()
    } else {
        format!(
            "  precedents (decided by people in this base):\n{}\n",
            p.iter()
                .map(|l| format!("    - {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

/// 低置信与证据过期的事实：证据说没说这件事
pub fn fact_messages(items: &[FactQuestion]) -> Vec<ChatMessage> {
    let system = format!(
        "You review facts a knowledge graph extracted from documents. For each numbered fact, \
         decide from its evidence whether the fact is what the text says.\n\
         \n\
         Actions:\n\
         - \"confirm\": the evidence states this fact — the same subject, relation, value and \
           dates. A value written in a table, a list or an amendment's new column is stated. \
           A fact is judged over its dates: text that ends a value on a date (deleted, \
           terminated, replaced) states that the value held until then.\n\
         - \"reject\": the evidence does not say it, says something else, or attaches it to \
           another subject.\n\
         - \"unsure\": the evidence genuinely points both ways; say what a person should check.\n\
         \n\
         A fact marked STALE has evidence only in an older version of its document. Judge it \
         against the CURRENT text shown with it: confirm only when the current text still states \
         it, and then put in \"quote\" the exact words of the current text that state it; reject \
         when the current text says otherwise or no longer says it.\n\
         \n\
         {PRECEDENTS_NOTE}\n\
         \n\
         Output exactly one JSON object and nothing else:\n\
         {{\"verdicts\":[{{\"i\":0,\"action\":\"confirm|reject|unsure\",\"confidence\":0.9,\
         \"why\":\"one sentence\",\"quote\":\"only for a stale fact you confirm\"}}]}}\n\
         \n\
         Rules:\n\
         1. One verdict per fact, using the fact's number as \"i\".\n\
         2. confidence in 0~1 is how sure you are of the action.\n\
         3. \"why\" is one short sentence naming what in the evidence decided it."
    );
    let mut user = String::new();
    for (i, q) in items.iter().enumerate() {
        let current = if q.stale {
            let text = if q.current.is_empty() {
                "    (the current version no longer mentions the subject)".to_string()
            } else {
                q.current
                    .iter()
                    .map(|t| format!("    \"\"\"{t}\"\"\""))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!("  STALE. current text:\n{text}\n")
        } else {
            String::new()
        };
        user.push_str(&format!(
            "Fact {i}: {}\n{current}{}\n",
            card(&q.fact),
            precedent_lines(&q.precedents)
        ));
    }
    vec![
        ChatMessage {
            role: "system".into(),
            content: system,
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}

/// 时态冲突：两个值在同一刻都成立、引擎排不开
pub fn conflict_messages(items: &[ConflictQuestion]) -> Vec<ChatMessage> {
    let system = format!(
        "You resolve conflicts in a knowledge graph's timelines. Each numbered conflict is two \
         values of a relation that holds one value at a time, both holding at once, which the \
         engine could not order by itself. The reason says why: \"simultaneous\" (both start \
         on the same date), \"no_time\" (one has no date at all), \"low_confidence\" (the later \
         value was extracted with too little confidence to take over).\n\
         \n\
         Actions:\n\
         - \"close_old\": the new value took over from the old one. Give \"date\" when the old \
           value ended on a day other than the new value's start, taken from the evidence \
           (a document's date or a date its quote states).\n\
         - \"retime_new\": the new value is right but its start date is wrong; give \"date\", the \
           day it took effect according to its evidence (a document's date or a date its quote \
           states). The old value will then end there.\n\
         - \"keep_both\": both hold at once and neither replaces the other.\n\
         - \"reject_new\": the new fact misreads its evidence.\n\
         - \"unsure\": the evidence genuinely points both ways; say what a person should check.\n\
         \n\
         {PRECEDENTS_NOTE}\n\
         \n\
         Output exactly one JSON object and nothing else:\n\
         {{\"verdicts\":[{{\"i\":0,\"action\":\"close_old|retime_new|keep_both|reject_new|unsure\",\
         \"confidence\":0.9,\"why\":\"one sentence\",\"date\":\"YYYY-MM-DD when the action needs one\"}}]}}\n\
         \n\
         Rules:\n\
         1. One verdict per conflict, using its number as \"i\".\n\
         2. confidence in 0~1 is how sure you are of the action.\n\
         3. A date must come from the evidence shown; never infer one."
    );
    let mut user = String::new();
    for (i, q) in items.iter().enumerate() {
        let neighbours = if q.neighbours.is_empty() {
            String::new()
        } else {
            format!(
                "  other values on this relation:\n{}\n",
                q.neighbours
                    .iter()
                    .map(|n| format!("    - {}", card(n).replace('\n', "\n      ")))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        user.push_str(&format!(
            "Conflict {i} ({}):\n  OLD: {}\n  NEW: {}\n{neighbours}{}\n",
            q.reason,
            card(&q.old).replace('\n', "\n  "),
            card(&q.new).replace('\n', "\n  "),
            precedent_lines(&q.precedents)
        ));
    }
    vec![
        ChatMessage {
            role: "system".into(),
            content: system,
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(value: &str, from: Option<&str>, confidence: f32) -> FactCard {
        FactCard {
            subject: "HQ Lease".into(),
            predicate: "option deadline".into(),
            object: value.into(),
            from: from.map(str::to_string),
            to: None,
            confidence,
            evidence: vec![EvidenceLine {
                document: "sixth-amendment.html".into(),
                dated: Some("2020-03-17".into()),
                quote: Some("(Phase 2 Exercise Deadline) | April 14, 2020".into()),
            }],
        }
    }

    #[test]
    fn a_fact_question_shows_its_evidence_and_the_current_text_when_stale() {
        let msgs = fact_messages(&[
            FactQuestion {
                fact: fact("2020-04-14", Some("2020-03-17"), 0.7),
                stale: false,
                current: vec![],
                precedents: vec!["fact.confirm: HQ Lease option deadline 2020-03-17".into()],
            },
            FactQuestion {
                fact: fact("2020-05-26", None, 0.9),
                stale: true,
                current: vec!["The Phase 2 Exercise Deadline is May 26, 2020.".into()],
                precedents: vec![],
            },
        ]);
        let user = &msgs[1].content;
        assert!(user.contains(
            "Fact 0: HQ Lease · option deadline · 2020-04-14 [2020-03-17 → now] (confidence 0.70)"
        ));
        assert!(user.contains("sixth-amendment.html (dated 2020-03-17): \"(Phase 2 Exercise Deadline) | April 14, 2020\""));
        assert!(user.contains("fact.confirm: HQ Lease option deadline 2020-03-17"));
        assert!(user.contains("Fact 1:"));
        assert!(user.contains("STALE. current text:"));
        assert!(msgs[0].content.contains("\"confirm\""));
    }

    #[test]
    fn a_conflict_question_shows_both_sides_and_their_neighbours() {
        let msgs = conflict_messages(&[ConflictQuestion {
            reason: "low_confidence".into(),
            old: fact("2020-03-17", Some("2020-02-18"), 0.9),
            new: fact("2020-04-14", Some("2020-03-17"), 0.7),
            neighbours: vec![fact("2020-05-26", Some("2020-04-14"), 0.9)],
            precedents: vec![],
        }]);
        let user = &msgs[1].content;
        assert!(user.contains("Conflict 0 (low_confidence):"));
        assert!(user.contains("OLD: HQ Lease · option deadline · 2020-03-17"));
        assert!(user.contains("NEW: HQ Lease · option deadline · 2020-04-14"));
        assert!(user.contains("other values on this relation:"));
        assert!(msgs[0].content.contains("\"retime_new\""));
    }

    #[test]
    fn verdicts_parse_with_their_optional_fields() {
        let raw = "```json\n{\"verdicts\":[{\"i\":0,\"action\":\"close_old\",\"confidence\":0.9,\"why\":\"x\",\"date\":\"2020-08-13\"},{\"i\":1,\"action\":\"unsure\"}]}\n```";
        let v = parse_verdicts(raw).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].date.as_deref(), Some("2020-08-13"));
        assert_eq!(v[1].confidence, None);
        assert_eq!(v[1].quote, None);
    }
}
