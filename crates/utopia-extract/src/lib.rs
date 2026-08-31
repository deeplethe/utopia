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
    /// 逐项解析时被跳过的条目数。**必须报给调用方**——不报就是一次静默丢弃，
    /// 与 #108「部分抽取报告成完成」同一类错
    #[serde(skip)]
    pub skipped_entities: usize,
    #[serde(skip)]
    pub skipped_facts: usize,
    /// 模型的输出被截断，这里是修补后解析的
    #[serde(skip)]
    pub truncated: bool,
}

#[derive(Debug, Deserialize)]
pub struct ExtractedEntity {
    pub name: String,
    #[serde(rename = "type")]
    pub type_key: String,
    /// 模型自己的说法：它认为这最具体是个什么。**不校验、不入本体**。
    ///
    /// 存在的理由是清单里总有个"差不多"的：本体有 product，模型觉得够用就选了，
    /// 心里那个"向量数据库软件"就此丢失。实测 17 个实体的 proposed_type
    /// 全是空的，正是这个原因——而事后消解最需要的恰是这个名字：
    /// 短名字对短标签，比拿一段中文散文去匹配 "A software application." 近得多。
    #[serde(default)]
    pub specific_type: Option<String>,
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

/// 提示词里的一条关系。
///
/// 比类多一样东西：**类型签名**。它是给模型的签名，不是闸门——在模型落笔那一刻
/// 减少"Alice works_at 西雅图"，而不是等错了再拦。事后校验面对的是既成事实
///（丢掉可惜、留着是脏数据），签名是在写出来之前掰正。本体写错时模型看到原文
/// 说了别的仍可覆盖；硬闸门会系统性丢数据，`part_of` 烧我们的正是那种方式。
pub struct PromptRelation {
    pub key: String,
    pub label: String,
    pub description: String,
    /// 形如 `person|organization → vendor`，`*` 表示那一侧不限。空串 = 两侧都不限。
    /// **一律用 key**：模型要输出的就是 key，中文库里 person 的 label 是"人物"，
    /// 写进签名等于教它输出一个不存在的类型（docs/decisions/0004）
    pub signature: String,
}

/// 构造抽取提示词。`types` 为 (key, label, description) 三元组；
/// description 非空时按行列出——本体里的语义指引直接决定抽取质量。
/// `attributes` 为调用方预排版的属性行（"person.salary (number, CNY): 月薪"）；
/// 为空时提示词一字不变——没定义属性的库零成本。
pub fn build_messages(
    types: &[(String, String, String)],
    relations: &[PromptRelation],
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
    // 关系行：有签名时括号里放签名，没有才退回 label。
    // `- works_at (person → organization): 一个人受雇于某个组织。`
    let rel_list = relations
        .iter()
        .map(|r| {
            let d = r.description.trim();
            let paren = if !r.signature.is_empty() {
                r.signature.clone()
            } else if d.is_empty() {
                r.label.clone()
            } else {
                String::new()
            };
            match (paren.is_empty(), d.is_empty()) {
                (false, false) => format!("- {} ({paren}): {d}", r.key),
                (false, true) => format!("- {} ({paren})", r.key),
                (true, false) => format!("- {}: {d}", r.key),
                (true, true) => format!("- {}", r.key),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    // 记号只在真有签名时解释一次；没有签名的库，提示词一字不变。
    // 说明用英文——提示词的**指令语言**是英文，只有 description 跟语料走
    //
    // **签名管两件事，而它们的可覆盖性不同。** 第一版把两件事混成了一句
    // "It is a hint, not a rule — when the text says otherwise, write what the
    // text says"，于是模型连参数顺序也一并按原文的说法写：
    //
    //     Elon Musk (person) --employee--> Microsoft
    //
    // 而 schema.org 声明的是 employee (organization → person)。实测一次跑里
    // 130 条可校验的事实有 102 条这样反着落库——**恰恰是本体包最主要的卖点失效**，
    // 选 schema.org 的理由就是「方向是声明的不是描述的」。
    //
    // 两件事分开说：
    //
    // - **哪些类型能参与**：提示不是闸门。本体可能写错，原文说西雅图就写西雅图。
    //   0001 的判断在这里不变——硬闸门会系统性丢数据，part_of 烧我们的正是那样。
    // - **参数顺序**：由签名定。顺序不是关于世界的断言，是这个 key 的编码约定；
    //   原文从来没有「说了别的方向」，它只说两个实体之间存在某种关系。
    //   反着说时该交换主宾，而不是反过来用这个关系。
    let sig_note = if relations.iter().any(|r| !r.signature.is_empty()) {
        ". A parenthesis after the key is the type signature, subject then object; \
         \"|\" means or, \"*\" means unconstrained. Which kinds of things may take part \
         is a hint, not a rule — when the text says otherwise, write what the text says. \
         The order is not a hint: the signature fixes which side is the subject. If the \
         text puts them the other way round, swap subject and object so that the subject \
         matches the left side — do not reverse the relation. For example, given \
         \"employee (organization → person)\" and a text saying \"X is an employee of Y\", \
         write Y as the subject and X as the object"
    } else {
        ""
    };
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
         Relation types (prefer these keys){sig_note}:\n{rel_list}\n\
         {attr_section}\
         \n\
         Output format:\n\
         {{\"entities\":[{{\"name\":\"entity name\",\"type\":\"type key\",\"specific_type\":\"what you would call it\"}}],\n\
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
         3. Dates must be \"YYYY\", \"YYYY-MM\", \"YYYY-MM-DD\", or null — never invent dates.\n\
         3a. valid_to takes a third value: \"unknown\". Use it when the text says the relation \
            has ended but does not say when — \"former CEO of X\", \"stepped down\", \"left the \
            company\", \"no longer available\", \"until recently\". Use null only for something \
            still going on. These are not interchangeable: null asserts it still holds, and \
            writing null for a relation the text says is over makes us claim the opposite of \
            the source.\n\
         4. {time_ctx}\n\
         5. quote must be a contiguous excerpt from the source text; every fact needs one.\n\
         6. confidence in 0~1: 0.9 explicitly stated, 0.7 inferred, 0.5 uncertain.\n\
         7. If nothing can be extracted, output {{\"entities\":[],\"facts\":[]}}.\n\
         7a. Every entity you list must take part in at least one fact. If the text says \
            nothing relatable about a thing, leave it out of entities entirely — a name with \
            no fact attached tells the reader nothing. Before you finish, check each entity \
            against your facts: an entity that appears in none of them means you either \
            missed a relation the text states about it, or should not have listed it.\n\
         8. If no listed relation fits, do not force the nearest one — write the predicate the \
            text itself uses, in snake_case (e.g. \"available_on\", \"runs_on\"). A relation \
            named after the text is worth more than a listed one that says something false.\n\
         9. The same holds for entity types: if none of the listed types fits, write the type \
            the text implies, in snake_case (e.g. \"model\", \"technology\"). Do not fall back \
            to a broad listed type such as \"thing\" or \"creative_work\" merely because \
            nothing specific matched — that hides the gap instead of reporting it.\n\
         10. specific_type is required on every entity and is never checked against the list. \
            Name the most specific kind the thing is, in the words you would use for it. Write \
            it even when \"type\" already fits, and make it narrower than \"type\" wherever the \
            text supports it — type \"product\", specific_type \"vector database software\". \
            Repeat the listed type only when the text genuinely says nothing more precise.\
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

/// 把 head 后面缺的括号补上。字符串字面量里的括号不算——`"a[b"` 不是一个开括号。
///
/// 返回 None = 结构本身就不对（比如括号已经多了），不是"没写完"。
fn close_brackets(head: &str) -> Option<String> {
    let mut stack: Vec<char> = Vec::new();
    let (mut in_str, mut esc) = (false, false);
    for c in head.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '[' | '{' => stack.push(c),
            // 不写成两条带守卫的分支：那样 stack.pop() 的副作用藏在守卫里，
            // 碰巧是对的，但读的人不会预期守卫会改状态
            ']' | '}' => {
                let want = if c == ']' { '[' } else { '{' };
                if stack.pop() != Some(want) {
                    return None;
                }
            }
            _ => {}
        }
    }
    if in_str {
        return None; // 断在字符串中间，这一截不可用
    }
    let mut out = String::from(head);
    for c in stack.iter().rev() {
        out.push(if *c == '[' { ']' } else { '}' });
    }
    Some(out)
}

/// 输出被截断时，退到**最后一个完整对象**的结尾再把括号补齐。
///
/// 模型写到一半没了（撞上 max_tokens）时，前面那些对象是完整且正确的。
/// 整块作废等于把已经抽对的十几条事实一起扔掉——实测 246 次调用里 4 次是这种。
fn repair_truncated(json: &str) -> Option<String> {
    let mut cut = json.len();
    for _ in 0..64 {
        let idx = json[..cut].rfind('}')?;
        if let Some(closed) = close_brackets(&json[..=idx]) {
            if serde_json::from_str::<serde_json::Value>(&closed).is_ok() {
                return Some(closed);
            }
        }
        cut = idx;
    }
    None
}

/// **一条坏记录不该毁掉一整块。**
///
/// 从前这里是 `serde_json::from_str::<Extraction>`——全有或全无。一个缺 `predicate`
/// 的对象、或者一次输出截断，整块的实体和事实一起作废，而一块里常有二十条好事实。
/// 实测 246 次调用里 5 次这样丢掉（2%），并且会让整个 `extract_document` 任务失败、
/// 走重试，三次之后文档标记失败。
///
/// 现在：先解成 `Value`（截断就先补齐括号），再逐项 `from_value`，好的收下、
/// 坏的计数。**计数必须往外传**——静默跳过就是另一种"报告成完成"。
pub fn parse_response(raw: &str) -> anyhow::Result<Extraction> {
    let json_str = json_block(raw)?;
    let (value, truncated) = match serde_json::from_str::<serde_json::Value>(&json_str) {
        Ok(v) => (v, false),
        Err(e) => match repair_truncated(&json_str) {
            Some(fixed) => (
                serde_json::from_str::<serde_json::Value>(&fixed)
                    .map_err(|e| anyhow::anyhow!("Failed to parse extraction JSON: {e}"))?,
                true,
            ),
            // 补不回来才是真解析失败：连一个完整对象都没有
            None => anyhow::bail!("Failed to parse extraction JSON: {e}"),
        },
    };

    fn take<T: serde::de::DeserializeOwned>(
        value: &serde_json::Value,
        key: &str,
    ) -> (Vec<T>, usize) {
        let Some(arr) = value.get(key).and_then(|v| v.as_array()) else {
            return (Vec::new(), 0);
        };
        let mut out = Vec::with_capacity(arr.len());
        let mut skipped = 0;
        for item in arr {
            match serde_json::from_value::<T>(item.clone()) {
                Ok(v) => out.push(v),
                Err(_) => skipped += 1,
            }
        }
        (out, skipped)
    }

    let (entities, skipped_entities) = take::<ExtractedEntity>(&value, "entities");
    let (facts, skipped_facts) = take::<ExtractedFact>(&value, "facts");
    Ok(Extraction {
        entities,
        facts,
        skipped_entities,
        skipped_facts,
        truncated,
    })
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
        Abbreviation removes a qualifier from the FRONT. It never adds a noun or a \
        prepositional phrase at the end, so those are different entities however much text \
        they share: \"the operator library for the Canghai Platform\" is not the Canghai \
        Platform, \"Qiming X7 programme\" is not the Qiming X7, and \"沧海平台项目\" is not \
        \"沧海平台\" — a project, a programme, a team or a component is its own record.\n\
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

    fn rel(key: &str, description: &str, signature: &str) -> PromptRelation {
        PromptRelation {
            key: key.into(),
            label: key.replace('_', " "),
            description: description.into(),
            signature: signature.into(),
        }
    }

    /// 签名进括号，而且**一律是 key**：中文库的 label 是"人物"，
    /// 写进提示词等于教模型输出一个不存在的类型。
    #[test]
    fn a_signature_takes_the_parenthesis_and_uses_keys() {
        let rels = vec![rel("works_at", "受雇于某个组织。", "person → organization")];
        let msgs = build_messages(&[], &rels, &[], None, "a.txt", &[], "text");
        assert!(msgs[0]
            .content
            .contains("- works_at (person → organization): 受雇于某个组织。"));
    }

    /// 多值用 `|`，空的一侧用 `*` —— 都是 key 层面的记号，不是类型名
    #[test]
    fn several_classes_join_with_a_pipe_and_an_empty_side_is_a_star() {
        let rels = vec![rel("buys_from", "", "employee|team → *")];
        let msgs = build_messages(&[], &rels, &[], None, "a.txt", &[], "text");
        assert!(msgs[0].content.contains("- buys_from (employee|team → *)"));
    }

    /// **没有签名的库，提示词一字不变**：记号说明也不出现。
    /// 大多数库不会声明 domain/range，不该为此付每块的 token
    #[test]
    fn a_base_without_signatures_pays_nothing() {
        let rels = vec![rel("works_at", "受雇于某个组织。", "")];
        let msgs = build_messages(&[], &rels, &[], None, "a.txt", &[], "text");
        assert!(msgs[0].content.contains("- works_at: 受雇于某个组织。"));
        assert!(!msgs[0].content.contains("type signature"));
        assert!(!msgs[0].content.contains('→'));
    }

    /// 签名是提示不是闸门。这句话必须在提示词里 —— 少了它，
    /// 模型会把签名当硬规则，本体写错时就系统性丢数据（part_of 那种方式）
    #[test]
    fn the_prompt_says_the_signature_is_a_hint() {
        let rels = vec![rel("works_at", "d", "person → organization")];
        let msgs = build_messages(&[], &rels, &[], None, "a.txt", &[], "text");
        assert!(msgs[0].content.contains("hint, not a rule"));
    }

    /// **但顺序不是提示。**
    ///
    /// 两句话必须同时在场，少哪一句都退回一种老毛病：少了「提示不是闸门」，
    /// 本体写错时系统性丢数据（part_of 那种方式）；少了「顺序由签名定」，
    /// 模型按英语直觉写 `Musk --employee--> Microsoft`，而 schema.org 声明的是
    /// `employee (organization → person)`——实测一次跑里 130 条可校验的事实
    /// 有 102 条这样反着落库。
    #[test]
    fn the_prompt_says_the_order_is_not_a_hint() {
        let rels = vec![rel("employee", "d", "organization → person")];
        let msgs = build_messages(&[], &rels, &[], None, "a.txt", &[], "text");
        let c = &msgs[0].content;
        assert!(c.contains("hint, not a rule"), "类型那句丢了");
        assert!(c.contains("The order is not a hint"), "顺序那句丢了");
        assert!(
            c.contains("swap subject and object"),
            "只说了顺序重要，没说反着写时该怎么办"
        );
        assert!(
            c.contains("do not reverse the relation"),
            "少了这句，模型可能去找一个反向关系而不是交换主宾"
        );
    }

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

    /// **一条坏记录不该毁掉一整块。**
    ///
    /// 形态取自真实日志：`missing field \`predicate\``。模型偶尔会漏写这个字段
    /// （`related_to` 退场后它没有万能选项可挑），从前 serde 会让整块作废，
    /// 而这一块里另外两条事实是好的。
    #[test]
    fn one_malformed_fact_does_not_take_the_whole_chunk() {
        let raw = r#"{
          "entities": [{"name": "OpenAI", "type": "organization"}],
          "facts": [
            {"subject": "OpenAI", "predicate": "produces", "object": "GPT-4"},
            {"subject": "OpenAI", "object": "ChatGPT"},
            {"subject": "Sam Altman", "predicate": "leads", "object": "OpenAI"}
          ]
        }"#;
        let x = parse_response(raw).unwrap();
        assert_eq!(x.facts.len(), 2, "好的两条该留下");
        assert_eq!(x.skipped_facts, 1, "跳过的那条要报出来，不能静默");
        assert_eq!(x.entities.len(), 1);
        assert!(!x.truncated);
    }

    /// **输出被截断时，已经完整的那些要救回来。**
    ///
    /// 撞上 max_tokens 时模型写到一半就没了（真实日志：`EOF while parsing a list`）。
    /// 前面的对象是完整且正确的，整块作废等于把抽对的十几条一起扔掉。
    #[test]
    fn a_cut_off_reply_keeps_what_was_complete() {
        let raw = r#"{
          "entities": [{"name": "Anthropic", "type": "organization"}],
          "facts": [
            {"subject": "Anthropic", "predicate": "produces", "object": "Claude"},
            {"subject": "Dario Amodei", "predicate": "leads", "object": "Anthropic"},
            {"subject": "Anthropic", "predicate": "loca"#;
        let x = parse_response(raw).unwrap();
        assert!(x.truncated, "截断要标出来");
        assert_eq!(x.facts.len(), 2, "断点之前的两条是完整的");
        assert_eq!(x.entities.len(), 1);
    }

    /// 括号出现在字符串里不算结构——`"a[b"` 不是一个开括号。
    #[test]
    fn brackets_inside_strings_are_not_structure() {
        let raw =
            r#"{"entities": [], "facts": [{"subject": "a[b{c", "predicate": "p", "object": "o"}]}"#;
        let x = parse_response(raw).unwrap();
        assert_eq!(x.facts.len(), 1);
        assert!(!x.truncated, "结构完整，不该判成截断");
    }

    /// 连一个完整对象都没有时，仍然要报失败——**容错不是把空结果说成成功**。
    #[test]
    fn a_reply_with_nothing_complete_still_fails() {
        assert!(parse_response(r#"{"facts": [{"subject": "a"#).is_err());
    }

    #[test]
    fn parse_response_with_fence() {
        let raw = "好的，结果如下：\n```json\n{\"entities\":[{\"name\":\"张三\",\"type\":\"person\"}],\"facts\":[]}\n```";
        let e = parse_response(raw).unwrap();
        assert_eq!(e.entities.len(), 1);
        assert_eq!(e.entities[0].type_key, "person");
    }

    /// specific_type 在骨架里、也在规则里，且两处都说"永远要填"。
    ///
    /// 只写进骨架是不够的：**规则与骨架冲突时骨架赢**（语言那条就栽过一次）。
    /// 这里两边一致，所以要一起钉住。
    #[test]
    fn every_entity_is_asked_for_its_own_words() {
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &[], "text");
        let sys = &msgs[0].content;
        assert!(sys.contains("\"specific_type\":\"what you would call it\""));
        assert!(sys.contains("required on every entity"));
        // 关键的一句：不校验。校验它就等于又造了一个词表
        assert!(sys.contains("never checked against the list"));
        // 与 type 的关系必须说清楚，否则模型会把粗类抄一遍
        assert!(sys.contains("narrower than"));
    }
}
