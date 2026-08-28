//! utopia-extract: LLM 抽取（实体/关系/时间归一化）。
//! 提示词注入本体类型与文档元时间；输出严格 JSON；证据引句强制（无引句降置信度）。

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::Deserialize;
use utopia_llm::ChatMessage;

#[derive(Debug, Deserialize)]
pub struct Extraction {
    #[serde(default)]
    pub entities: Vec<ExtractedEntity>,
    #[serde(default)]
    pub facts: Vec<ExtractedFact>,
}

#[derive(Debug, Deserialize)]
pub struct ExtractedEntity {
    pub name: String,
    #[serde(rename = "type")]
    pub type_key: String,
}

#[derive(Debug, Deserialize)]
pub struct ExtractedFact {
    pub subject: String,
    pub predicate: String,
    /// 关系事实的宾语实体名；属性事实为空
    #[serde(default)]
    pub object: Option<String>,
    /// 属性事实的字面值（谓词是 attribute 时）
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_to: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub quote: Option<String>,
}

/// 构造抽取提示词。`types`/`relations` 为 (key, label, description) 三元组；
/// description 非空时按行列出——本体里的语义指引直接决定抽取质量。
/// `attributes` 为调用方预排版的属性行（"person.salary (number, CNY): 月薪"）；
/// 为空时提示词一字不变——没定义属性的库零成本。
pub fn build_messages(
    types: &[(String, String, String)],
    relations: &[(String, String, String)],
    attributes: &[String],
    doc_time: Option<&str>,
    filename: &str,
    // 本文档前面几块已经认下的 (类型 key, 实体名)，按首次出现排序。
    // 第一块为空——那时还没有"前面"
    known: &[(String, String)],
    chunk_text: &str,
) -> Vec<ChatMessage> {
    // **有描述时不送 label**。label 是给人看的显示名，而且它跟界面无关、
    // 跟这个库的语料语言走——中文库里 person 的 label 是"人物"。
    // `- person (人物): 有名有姓的具体的人…` 里那个"人物"相对 key 近乎零信息量，
    // 却让提示词在语料语言与标识符之间来回跳。描述为空时才拿它兜底：
    // 光一个 key 太单薄。见 docs/decisions/0004
    let fmt_list = |items: &[(String, String, String)]| {
        items
            .iter()
            .map(|(k, l, d)| {
                let d = d.trim();
                if d.is_empty() {
                    format!("- {k} ({l})")
                } else {
                    format!("- {k}: {d}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let type_list = fmt_list(types);
    let rel_list = fmt_list(relations);
    let time_ctx = doc_time
        .map(|t| {
            format!(
                "Document date: {t}. Resolve relative time expressions (e.g. \"last year\", \
                 \"this March\") to absolute dates using it as the reference."
            )
        })
        .unwrap_or_else(|| {
            "Document date unknown — only output dates explicitly written in the text.".into()
        });

    // 属性段按需注入：清单 + 输出说明 + 取值规则。没定义属性时完全不出现
    let attr_section = if attributes.is_empty() {
        String::new()
    } else {
        format!(
            "\nAttributes (literal-valued fields, listed as class.attribute_key; as \"predicate\" \
             use the attribute_key alone — e.g. \"salary\", not \"person.salary\" — with a \
             \"value\" instead of \"object\"):\n{}\n",
            attributes.join("\n")
        )
    };
    let attr_rules = if attributes.is_empty() {
        String::new()
    } else {
        "\n10. Attribute facts carry \"value\" (no \"object\"): number = plain number without \
         thousands separators or unit symbols; date = \"YYYY[-MM[-DD]]\"; bool = true/false; \
         text = a short string. Only attach an attribute to a subject of its listed class. \
         valid_from = when this value took effect, if the text says so."
            .to_string()
    };
    let system = format!(
        "You are a knowledge-graph extraction engine. Extract entities and factual relations \
         from the given text. Output exactly one JSON object and nothing else.\n\
         \n\
         Entity types (prefer these keys):\n{type_list}\n\
         \n\
         Relation types (prefer these keys):\n{rel_list}\n\
         {attr_section}\
         \n\
         Output format:\n\
         {{\"entities\":[{{\"name\":\"entity name\",\"type\":\"type key\"}}],\n\
          \"facts\":[{{\"subject\":\"subject entity name\",\"predicate\":\"relation key\",\"object\":\"object entity name\",\n\
                     \"valid_from\":\"2023-01\",\"valid_to\":null,\"confidence\":0.9,\"quote\":\"verbatim supporting quote\"}}]}}\n\
         \n\
         Rules:\n\
         1. Use the canonical full name as written in the text, in the text's original language; \
            list each entity once. Text introduces a full name and then shortens it — \
            \"星云科技上海研究院\" becomes \"上海研究院\", \"Nebula Technologies Inc.\" becomes \
            \"Nebula\" — and both forms mean one entity, listed once under the fuller form. \
            Two names are two entities only when the text is talking about two things.\n\
         2. Every fact's subject/object must appear in entities.\n\
         3. Dates must be \"YYYY\", \"YYYY-MM\", \"YYYY-MM-DD\", or null. If a relation is still \
            ongoing, valid_to is null. If the text states no date, use null — never invent dates.\n\
         4. {time_ctx}\n\
         5. quote must be a contiguous excerpt from the source text; every fact needs one.\n\
         6. confidence in 0~1: 0.9 explicitly stated, 0.7 inferred, 0.5 uncertain.\n\
         7. If nothing can be extracted, output {{\"entities\":[],\"facts\":[]}}.\n\
         8. If no listed relation fits, do not force the nearest one — write the predicate the \
            text itself uses, in snake_case (e.g. \"available_on\", \"runs_on\"). A relation \
            named after the text is worth more than a listed one that says something false.\n\
         9. The same holds for entity types: if none of the listed types fits, write the type \
            the text implies, in snake_case (e.g. \"model\", \"technology\"). Do not fall back \
            to a broad listed type such as \"concept\" merely because nothing specific matched \
            — that hides the gap instead of reporting it.\
         {attr_rules}"
    );

    // 已知实体紧挨着正文：服从性靠位置，理由见 known_block 的注释
    let user = format!(
        "Source file: \"{filename}\"\n{}\nText:\n{chunk_text}",
        known_block(known)
    );

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

/// 已在本文档中出现过的实体，放进提示词的字符预算。
///
/// 超出就截断（保留先出现的）。中文商业文本先出全称、主角先出场，所以
/// **首次出现顺序天然偏向那些后面会被简称的名字**。
const KNOWN_BUDGET_CHARS: usize = 1200;

/// 把「本文档已经认下的实体」排版成提示词里的一段。空则返回空串。
///
/// **为什么在正文之前、指令贴着清单**：抽象规则打不过挨着它的具体块——本体建议
/// 那次，语言要求就输给了紧随其后的英文 JSON 骨架，挪到骨架之后并点名它才生效。
/// 服从性靠位置，所以指令挨着它管的数据放，两者一起挨着正文。
///
/// **顺带一条与放哪条消息无关的规矩：逐块变化的内容一律放最后。** 前缀缓存匹配的是
/// token 前缀，而消息按 system→user 拼接，所以「system 末尾」与「user 开头」几乎等价；
/// 真正会打碎缓存的是把它塞在**中间**（本体之后、规则之前），那会把规则挤出前缀。
/// 缓存本身不归我们管——供应商开不开、报不报都由它，本部署实测 `cached=0`——
/// 我们只负责别把它弄碎。自部署 vLLM 默认开着自动前缀缓存，那省的是算力不是钱。
fn known_block(known: &[(String, String)]) -> String {
    if known.is_empty() {
        return String::new();
    }
    // 按类型分组：更紧凑，而且顺带压住跨块类型漂移
    //（同一个"沧海"在一块里是 product、另一块里是 project）
    let mut by_type: Vec<(&str, Vec<&str>)> = Vec::new();
    let mut used = 0usize;
    for (type_key, name) in known {
        used += name.chars().count() + 2;
        if used > KNOWN_BUDGET_CHARS {
            break;
        }
        match by_type.iter_mut().find(|(k, _)| *k == type_key.as_str()) {
            Some((_, names)) => names.push(name),
            None => by_type.push((type_key, vec![name])),
        }
    }
    if by_type.is_empty() {
        return String::new();
    }
    let lines = by_type
        .iter()
        .map(|(k, names)| format!("  {k}: {}", names.join(", ")))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\nAlready recorded from earlier parts of this same document:\n{lines}\n\
         \n\
         If something in the text below refers to one of these, write that exact string as \
         the name, and give it that same type — documents abbreviate after first mention \
         (\"星云科技上海研究院\" later becomes \"上海研究院\"), and the shortened form must \
         not become a second entity. If it is a different thing, name it as the text does; \
         do not force it onto this list.\n"
    )
}

/// 从 LLM 回复中稳健地取出 JSON 块（容忍代码围栏与前后废话）。
pub fn json_block(raw: &str) -> anyhow::Result<String> {
    let text = raw.trim();
    let cleaned = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .map(|s| s.trim_end_matches("```"))
        .unwrap_or(text);
    let start = cleaned.find('{');
    let end = cleaned.rfind('}');
    match (start, end) {
        (Some(s), Some(e)) if e > s => Ok(cleaned[s..=e].to_string()),
        _ => anyhow::bail!("No JSON found in LLM reply"),
    }
}

pub fn parse_response(raw: &str) -> anyhow::Result<Extraction> {
    let json_str = json_block(raw)?;
    let extraction: Extraction = serde_json::from_str(&json_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse extraction JSON: {e}"))?;
    Ok(extraction)
}

// ---------------------------------------------------------------------------
// 实体消解裁决（攒批：一次调用裁多对，LLM 只处理 embedding 分不出的灰区）
// ---------------------------------------------------------------------------

/// 待裁决的一侧：名字 + 类型 + 事实摘要行。
pub struct AdjudicationSide {
    pub name: String,
    pub type_label: String,
    pub facts: Vec<String>,
}

pub struct AdjudicationPair {
    pub left: AdjudicationSide,
    pub right: AdjudicationSide,
}

#[derive(Debug, Deserialize)]
pub struct AdjudicationVerdict {
    pub i: usize,
    pub verdict: String,
    #[serde(default)]
    pub confidence: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct AdjudicationReply {
    #[serde(default)]
    verdicts: Vec<AdjudicationVerdict>,
}

/// 构造攒批裁决提示词。保守偏置：证据不足答 unsure（宁分勿合，合并要证据）。
pub fn build_adjudication_messages(pairs: &[AdjudicationPair]) -> Vec<ChatMessage> {
    let system = "You are an entity-resolution adjudicator for a knowledge graph. \
        For each numbered pair, decide whether the two records refer to the SAME real-world \
        entity or are namesakes (different entities that share a name).\n\
        \n\
        Judge by the facts attached to each record: employer/affiliation, role, time ranges, \
        and connected entities. Identical names alone are NEVER sufficient evidence of sameness. \
        Contradictory affiliations in overlapping time periods indicate different entities \
        (but people do change jobs — non-overlapping periods can belong to one person).\n\
        \n\
        One name containing the other is a different case, and the rule above does not apply \
        to it: \"星云科技上海研究院\" against \"上海研究院\", \"Nebula Technologies Inc.\" \
        against \"Nebula\". Documents drop the qualifier after first mention, so the shorter \
        form is usually the longer one abbreviated — treat the containment as evidence FOR \
        sameness and let the facts settle it. Shared people, parent or location confirm one \
        entity; a different parent or conflicting leadership means the shorter name belongs \
        to something else.\n\
        \n\
        Output exactly one JSON object and nothing else:\n\
        {\"verdicts\":[{\"i\":0,\"verdict\":\"same|different|unsure\",\"confidence\":0.9}]}\n\
        \n\
        Rules:\n\
        1. One verdict per pair, using the pair's number as \"i\".\n\
        2. confidence in 0~1.\n\
        3. Be conservative: if the evidence is insufficient to decide, answer \"unsure\" — \
           a wrong merge is far more damaging than leaving two records separate."
        .to_string();

    let mut user = String::new();
    for (i, p) in pairs.iter().enumerate() {
        let fmt = |s: &AdjudicationSide| {
            let facts = if s.facts.is_empty() {
                "  (no recorded facts)".to_string()
            } else {
                s.facts
                    .iter()
                    .map(|f| format!("  - {f}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!("\"{}\" ({})\n{}", s.name, s.type_label, facts)
        };
        user.push_str(&format!(
            "Pair {i}:\nRecord A: {}\nRecord B: {}\n\n",
            fmt(&p.left),
            fmt(&p.right)
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

pub fn parse_adjudication(raw: &str) -> anyhow::Result<Vec<AdjudicationVerdict>> {
    let json_str = json_block(raw)?;
    let reply: AdjudicationReply = serde_json::from_str(&json_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse adjudication JSON: {e}"))?;
    Ok(reply.verdicts)
}

/// 属性值按 datatype 归一。失败返回 None——宁缺勿脏，调用方跳过并记日志。
/// number 容忍千分位/空格；date 要求 YYYY[-MM[-DD]] 且保留原精度；bool 宽容 yes/no。
pub fn normalize_attr_value(datatype: &str, raw: &serde_json::Value) -> Option<serde_json::Value> {
    match datatype {
        "number" => match raw {
            serde_json::Value::Number(n) => Some(serde_json::Value::Number(n.clone())),
            serde_json::Value::String(s) => {
                let cleaned: String = s
                    .chars()
                    .filter(|c| !matches!(c, ',' | ' ' | '_'))
                    .collect();
                cleaned
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .and_then(serde_json::Number::from_f64)
                    .map(serde_json::Value::Number)
            }
            _ => None,
        },
        "date" => {
            let s = raw.as_str()?.trim();
            parse_time(s).map(|_| serde_json::Value::String(s.to_string()))
        }
        "bool" => match raw {
            serde_json::Value::Bool(b) => Some(serde_json::Value::Bool(*b)),
            serde_json::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "是" => Some(serde_json::Value::Bool(true)),
                "false" | "no" | "否" => Some(serde_json::Value::Bool(false)),
                _ => None,
            },
            _ => None,
        },
        _ => {
            let s = match raw {
                serde_json::Value::String(s) => s.trim().to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                _ => return None,
            };
            (!s.is_empty()).then(|| serde_json::Value::String(s.chars().take(500).collect()))
        }
    }
}

/// 解析时间字符串 → (UTC 时间, 精度)。支持 YYYY / YYYY-MM / YYYY-MM-DD。
pub fn parse_time(s: &str) -> Option<(DateTime<Utc>, &'static str)> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("null") {
        return None;
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some((Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?), "day"));
    }
    if let Ok(d) = NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d") {
        return Some((Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?), "month"));
    }
    if s.len() == 4 {
        if let Ok(year) = s.parse::<i32>() {
            let d = NaiveDate::from_ymd_opt(year, 1, 1)?;
            return Some((Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?), "year"));
        }
    }
    None
}

#[cfg(test)]
mod prompt_shape_tests {
    use super::*;

    /// 已知实体必须落在 **user** 消息里、紧挨着正文。
    ///
    /// 理由是服从性不是缓存：抽象规则打不过挨着它的具体块。清单放进 system 的
    /// 规则区，就会隔着输出格式、十条规则、文件名，离它要管的正文最远。
    #[test]
    fn known_entities_stay_out_of_the_system_message() {
        // 用一个规则 1 的例子里没有的名字：规则 1 也提"星云科技上海研究院"，
        // 拿它断言等于测不出清单到底在哪条消息里
        let known = vec![(
            "organization".to_string(),
            "华瑞集团智能制造研究院".to_string(),
        )];
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &known, "text");
        assert_eq!(msgs[0].role, "system");
        assert!(!msgs[0].content.contains("Already recorded"));
        assert!(!msgs[0].content.contains("华瑞集团智能制造研究院"));
        assert!(msgs[1]
            .content
            .contains("organization: 华瑞集团智能制造研究院"));
    }

    /// 第一块没有"前面"，那一段应当完全不出现——成本为零，而不是一段空标题
    #[test]
    fn the_first_chunk_carries_no_block() {
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &[], "text");
        assert!(!msgs[1].content.contains("Already recorded"));
    }

    /// 反向护栏必须在：给了参照物就会有人硬套（`concept` 那次的教训）
    #[test]
    fn the_block_tells_the_model_not_to_force_a_match() {
        let known = vec![("person".to_string(), "陈立".to_string())];
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &known, "text");
        assert!(msgs[1].content.contains("do not force it onto this list"));
    }

    /// 有描述就不送 label——中文库的 label 是中文，混进提示词只会让
    /// 标识符与语料语言来回跳，而它相对 key 近乎零信息量。
    #[test]
    fn described_types_drop_the_label() {
        let types = vec![
            (
                "person".into(),
                "人物".into(),
                "有名有姓的具体的人。".into(),
            ),
            ("event".into(), "事件".into(), String::new()),
        ];
        let msgs = build_messages(&types, &[], &[], None, "a.txt", &[], "text");
        let prompt = format!("{:?}", msgs);
        assert!(prompt.contains("- person: 有名有姓的具体的人。"));
        assert!(!prompt.contains("person (人物)"));
        // 描述为空时 label 仍是唯一的额外线索，留着
        assert!(prompt.contains("- event (事件)"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_precisions() {
        assert_eq!(parse_time("2024").unwrap().1, "year");
        assert_eq!(parse_time("2024-07").unwrap().1, "month");
        assert_eq!(parse_time("2024-07-15").unwrap().1, "day");
        assert!(parse_time("null").is_none());
        assert!(parse_time("").is_none());
        assert!(parse_time("下个月").is_none());
    }

    #[test]
    fn parse_adjudication_reply() {
        let raw = "```json\n{\"verdicts\":[{\"i\":0,\"verdict\":\"same\",\"confidence\":0.92},{\"i\":1,\"verdict\":\"unsure\"}]}\n```";
        let v = parse_adjudication(raw).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].verdict, "same");
        assert_eq!(v[1].confidence, None);
    }

    #[test]
    fn normalize_attr_values() {
        use serde_json::json;
        assert_eq!(
            normalize_attr_value("number", &json!("35,000")),
            Some(json!(35000.0))
        );
        assert_eq!(normalize_attr_value("number", &json!(42)), Some(json!(42)));
        assert_eq!(normalize_attr_value("number", &json!("about ten")), None);
        assert_eq!(
            normalize_attr_value("date", &json!("2024-07")),
            Some(json!("2024-07"))
        );
        assert_eq!(normalize_attr_value("date", &json!("下个月")), None);
        assert_eq!(
            normalize_attr_value("bool", &json!("yes")),
            Some(json!(true))
        );
        assert_eq!(
            normalize_attr_value("text", &json!(" CTO ")),
            Some(json!("CTO"))
        );
        assert_eq!(normalize_attr_value("text", &json!([1])), None);
    }

    #[test]
    fn parse_response_with_fence() {
        let raw = "好的，结果如下：\n```json\n{\"entities\":[{\"name\":\"张三\",\"type\":\"person\"}],\"facts\":[]}\n```";
        let e = parse_response(raw).unwrap();
        assert_eq!(e.entities.len(), 1);
        assert_eq!(e.entities[0].type_key, "person");
    }
}
