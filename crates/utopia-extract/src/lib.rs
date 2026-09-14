//! utopia-extract: LLM 抽取（实体/关系/时间归一化）。
//! 提示词注入本体类型与文档元时间；输出严格 JSON；证据引句强制（无引句降置信度）。

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::Deserialize;
use utopia_llm::ChatMessage;

pub mod governor;

pub mod normalize;
pub use normalize::{drop_quotes_from_opening, normalize_facts, Normalization};

#[derive(Debug, Deserialize)]
pub struct Extraction {
    #[serde(default)]
    pub entities: Vec<ExtractedEntity>,
    #[serde(default)]
    pub facts: Vec<ExtractedFact>,
    /// 实体在这段文字里的**别的名字**（0041 决定 2）：简称、曾用名、另一种文字的写法。
    /// 模型报，服务端只核对名字与引文确实在原文里——认不认「简称」「又名」这些词是模型的事
    #[serde(default)]
    pub names: Vec<ExtractedName>,
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
    /// Identifier scoped to this extraction response. It preserves mention identity while
    /// facts are bound; it is not a persistent entity id or a resolution verdict.
    #[serde(default)]
    pub local_id: Option<String>,
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

/// 一个实体的一个别的名字。`ref` 是这次回复里的 local_id，或者提示词给的 k 句柄
#[derive(Debug, Deserialize)]
pub struct ExtractedName {
    #[serde(rename = "ref")]
    pub entity_ref: String,
    pub name: String,
    /// 名字出现在里面的那段原文，逐字抄
    #[serde(default)]
    pub quote: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExtractedFact {
    pub subject: String,
    /// Response-local entity handle. When present, callers must bind through it rather than
    /// guessing from the surface name.
    #[serde(default)]
    pub subject_ref: Option<String>,
    pub predicate: String,
    /// 关系事实的宾语实体名；属性事实为空
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub object_ref: Option<String>,
    /// 属性事实的字面值（谓词是 attribute 时）
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    /// **边上的属性**（0037）：`{"amount": "$5 billion", "stake": "20%"}`。
    /// 只对关系事实有意义，key 必须是清单里这条关系声明过的；值照原文写，换算在服务端
    #[serde(default)]
    pub qualifiers: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_to: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub quote: Option<String>,
    /// 引文里逐字点名主语的那几个字（#582）。模型抄，不判断；落库时机器核对它
    /// 是不是 `subject` 那个名字——"Former OpenAI personnel" 不是 OpenAI
    #[serde(default)]
    pub subject_span: Option<String>,
    /// 同上，宾语那一侧
    #[serde(default)]
    pub object_span: Option<String>,
    /// 日期属性的值只相对一件事给出（「触发日后 45 天」），没有日历上的日期（#681 §4）。
    /// 模型判断、模型标；服务端不认这类说法的词，只看这个标记决定收不收
    #[serde(default)]
    pub relative: bool,
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
    /// 时间语义（`relation_types.temporal`）：`state` / `event` / `eternal`（0031）。
    /// 只有 event 与 eternal 会在清单里带标记——状态是默认，写出来只多花 token
    pub temporal: String,
    /// 这条关系的边能带的属性，已排好版：`amount: number $`（0037）。空 = 不带
    pub qualifiers: Vec<String>,
}

/// Response-scoped reference to a persistent entity; database UUIDs must never enter prompts.
pub struct KnownEntity {
    pub handle: String,
    pub type_key: String,
    pub name: String,
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
    // 本文档前面几块已经认下的实体，按首次出现排序。handle 只对这次回复有效。
    // 第一块为空——那时还没有"前面"
    known: &[KnownEntity],
    chunk_text: &str,
) -> Vec<ChatMessage> {
    build_messages_with_opening(
        types, relations, attributes, doc_time, filename, known, None, chunk_text,
    )
}

/// 文件开头进提示词的字符预算。一份补充协议的标题、生效日、当事方和「修订的是哪份
/// 协议」通常在头一千字符里；新闻稿的电头与导语也是
pub const OPENING_BUDGET_CHARS: usize = 1500;

/// 同 [`build_messages`]，另带**这份文件的开头**（第一块的原文），给第二块往后用。
///
/// **一块是孤立抽取的，它看不见自己属于什么。** 补充协议把截止日写在第三块的表格里，
/// 那一块只说「Article 13 的日期延至……」：改的是哪份租约、从哪天起改，都写在第一块。
/// 模型拿不到，就只能把「Phase 2 Exercise Deadline」本身当主语（服务端按主语未声明丢掉），
/// 或者抽出一个没有起点的日期（时态引擎没法据此关闭旧值）——Blackbaud 总部租约链
/// 上五次改期丢了两次，抽到的三次一次都没关上旧值。开头只作背景，不从里面抽事实：
/// 它自己那一块会抽，重复抽只会多出重复的事实
#[allow(clippy::too_many_arguments)]
pub fn build_messages_with_opening(
    types: &[(String, String, String)],
    relations: &[PromptRelation],
    attributes: &[String],
    doc_time: Option<&str>,
    filename: &str,
    known: &[KnownEntity],
    opening: Option<&str>,
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
            // 事件与恒常带方括号标记；状态是默认，不标（0031）
            let mark = temporal_mark(&r.temporal)
                .map(|m| format!(" [{m}]"))
                .unwrap_or_default();
            // 边上能带的属性跟在标记后面：`{amount: number $, stake: number %}`
            let mark = if r.qualifiers.is_empty() {
                mark
            } else {
                format!("{mark} {{{}}}", r.qualifiers.join(", "))
            };
            match (paren.is_empty(), d.is_empty()) {
                (false, false) => format!("- {} ({paren}){mark}: {d}", r.key),
                (false, true) => format!("- {} ({paren}){mark}", r.key),
                (true, false) => format!("- {}{mark}: {d}", r.key),
                (true, true) => format!("- {}{mark}", r.key),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    // 标记也只在真有事件或恒常关系时解释一次；全是状态的库，提示词一字不变。
    // 说的是**写什么**而不是「它是什么」：事件的那一刻进 valid_from、valid_to 留空
    // ——不然模型照状态的样子填一个起点，账本就把一次收购读成从那天起一直持续
    let temporal_note = if relations
        .iter()
        .any(|r| temporal_mark(&r.temporal).is_some())
    {
        "\n         3b. A relation marked [event] happens at one moment: put the date it happened in \
            valid_from and leave valid_to null — it has no span and does not end. A relation \
            marked [eternal] holds regardless of time: leave both dates null."
    } else {
        ""
    };
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
        "\n11. Attribute facts carry \"value\" (no \"object\"): number = the figure **as the text writes it, magnitude and currency included** \n         (\"86亿元\", \"$5 billion\", \"4,300 人\") — never reduce it to a bare number, the server converts; date = \"YYYY[-MM[-DD]]\" (a zoned clock time only when the text gives one) — a date the text gives only relative to an event \
         (\"45 days after the Trigger Date\", \"within 30 days of closing\") has no calendar date to convert: write it as the text writes it and add \"relative\": true; bool = true/false; \
         text = a short string. Only attach an attribute to a subject of its listed class. \
         valid_from = when this value took effect, if the text or the opening of the document says so. \
         A document that changes a value set earlier — amends, extends or replaces it — makes the new value \
         hold from the date the change takes effect, which is the document's own effective date unless the text gives another."
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
         {{\"entities\":[{{\"local_id\":\"e1\",\"name\":\"entity name\",\"type\":\"type key\",\"specific_type\":\"what you would call it\"}}],\n\
          \"facts\":[{{\"subject\":\"subject entity name\",\"subject_ref\":\"e1\",\"subject_span\":\"the words in quote that name the subject\",\"predicate\":\"relation key\",\"object\":\"object entity name\",\"object_ref\":\"e2\",\"object_span\":\"the words in quote that name the object\",\n\
                     \"valid_from\":\"2023-01\",\"valid_to\":null,\"confidence\":0.9,\"quote\":\"verbatim supporting quote\"}}],\n\
          \"names\":[{{\"ref\":\"e1\",\"name\":\"another name the text uses for it\",\"quote\":\"verbatim text containing that name\"}}]}}\n\
         \n\
         Rules:\n\
         1. Give every newly listed entity a local_id unique within this response (e1, e2, ...). \
            A local_id may define at most one entity. Reuse the same local_id when this response \
            mentions the same entity again, including abbreviations; never allocate a new handle \
            merely because a mention repeats. Different handles mean the mentions should be \
            tracked separately for attribution, not that their permanent identity is proven. \
            When the text clearly describes two different entities with the same surface name, \
            list both under different local_ids. A shared name alone neither proves sameness nor \
            requires a split.\n\
         1a. Use the canonical full name as written in the text, in the text's original language; \
            list each entity once. Text introduces a full name and then shortens it — \
            \"星云科技上海研究院\" becomes \"上海研究院\", \"Nebula Technologies Inc.\" becomes \
            \"Nebula\" — and both forms mean one entity, listed once under the fuller form. \
            Two names are two entities only when the text is talking about two things. \
            A name identifies the thing; it is not a description of its history. When the text \
            names something and then describes what happened to it, the name ends where the \
            description begins.\n\
         1b. Every other name the text gives an entity goes into \"names\", once per name: the \
            shortened form it introduces or uses (\"上海研究院\" for \"星云科技上海研究院\"), a \
            former name, the name in another language. \"ref\" is the entity's local_id or its \
            known handle, and \"quote\" is a verbatim excerpt that contains the name. Only \
            names belong there — never a pronoun or a description (\"该公司\", \"the company\", \
            \"former employees\") — and never the name already written in entities. A name \
            must name the entity itself, not something that belongs to it: \"星云科技研发团队\" \
            names a team, not 星云科技.\n\
         2. Every fact keeps its name fields and uses subject_ref; relation facts also use \
            object_ref. Each ref must be either a local_id defined exactly once in \
            entities or a known handle supplied with this text. An entity referenced by a known \
            handle must not be copied into entities.\n\
         3. Dates must be \"YYYY\", \"YYYY-MM\", \"YYYY-MM-DD\", or null — never invent dates. \
            A clock time is allowed only together with its zone, as \"YYYY-MM-DDTHH:MM[:SS]Z\" \
            or with a \"+HH:MM\" offset, and only when the text or the document states that \
            zone; a time of day without a zone stays a plain date — never guess a zone.\n\
         3a. valid_to takes a third value: \"unknown\". Use it when the text says the relation \
            has ended but does not say when — \"former CEO of X\", \"stepped down\", \"left the \
            company\", \"no longer available\", \"until recently\". Use null only for something \
            still going on. These are not interchangeable: null asserts it still holds, and \
            writing null for a relation the text says is over makes us claim the opposite of \
            the source.\n\
         3c. A period is when a fact holds, never what it is about. A quarter, a half, a \
            fiscal or calendar year, a month, \"the three months ended July 26, 2026\" — \
            none of these is an entity and none is an object. Put the period's dates in \
            valid_from and valid_to (a fiscal period resolves to the dates the document \
            states for it) and write the figure as the fact's \"value\" — the figure alone, as it stands in the \
            quote, with nothing appended. A column of a table headed by a period is a column \
            of values that hold in that period.\n\
         {temporal_note}\n\
         4. {time_ctx}\n\
         5. quote must be a contiguous excerpt from the Text block; never quote the opening of the document. Every fact needs one.\n\
         6. confidence in 0~1: 0.9 explicitly stated, 0.7 inferred, 0.5 uncertain. A value \
            the text writes out is stated whatever the layout — a sentence, a list, a table \
            cell, a schedule, the new column of an amendment that replaces an earlier term. \
            Inferred means the text does not write the value and you worked it out.\n\
         7. If nothing can be extracted, output {{\"entities\":[],\"facts\":[]}}.\n\
         8. If no listed relation fits, do not force the nearest one — write the predicate the \
            text itself uses, in snake_case (e.g. \"available_on\", \"runs_on\"). A relation \
            named after the text is worth more than a listed one that says something false.\n\
         8a. The same holds for a literal the text states outright — an amount, a share count, \
            a percentage, a capacity, a date, a job title, a ticker. Write it as a fact with \
            \"value\" and no \"object\": {{\"subject\":\"NVIDIA\",\"subject_ref\":\"e1\",\
            \"predicate\":\"purchase_price\",\"value\":\"$11.9 billion\",\"confidence\":0.9,\
            \"quote\":\"...\"}}. Name the predicate after the text when no listed attribute \
            fits — \"purchase_price\", \"job_title\", \"generation_capacity\", \"record_date\". \
            Attach it to the entity the text attaches it to, and keep the literal as written, \
            units and all — except a date, which is always written in the format of rule 3 \
            (\"June 23, 2020\" is \"2020-06-23\"). A deadline or a period stated \
            relative to an event, with no calendar date, is not a date: keep it as written \
            and mark it \"relative\" as rule 11 says. \
            **A stated figure left out is the loss that costs most**: the reader \
            came for those numbers, and no later step can recover one that was never written \
            down.\n\
         8b. A listed relation followed by {{…}} can carry those **qualifiers on the edge**:             when the same sentence gives both the other entity and a figure for it — an             amount, a stake, a price, a share count — write the relation with its \"object\"             and put the figure in \"qualifiers\" keyed exactly as listed, **as written in the text, currency and all** (\"€30 million\", \"15亿元人民币\", never a bare number) — except a date, which takes the format of rule 3:             {{\"subject\":\"Vega Capital\",\"predicate\":\"invested_in\",\"object\":\"Northwind\",            \"qualifiers\":{{\"amount\":\"$5 billion\"}},…}}. Never invent a key that is not             listed for that relation, and never drop the figure to keep the edge — a             relation without its amount is half the sentence. A relation you name after the text (rule 8) carries its figure the same way — keyed by the listed attribute that fits it, or by the plainest word for it (\"amount\", \"stake\", \"price\") when none does.
         8c. A **listed** relation also takes \"value\" when what the text gives is a \
            string rather than another entity — a job title, a designation, a ticker, a \
            model number. Never invent an entity for a string. And when the text introduces \
            someone by their role — \"X, founder and CEO of Y\", \"Z, co-CEO of W\", \
            \"Y's vice president of research\", \"the president of OpenAI\", \
            \"chief executive of Quora\", \"OpenAI's chief technology officer of \
            applications\" — write both facts: the tie to the organization, and \
            the role itself as a value on the person. The tie alone says they \
            are connected; the role is what the sentence was actually telling \
            you. The possessive (\"Y's <role>\", \"<role> of Y\", \"<role> at Y\"), the past \
            tense (\"was Y's <role>\", \"former <role> of Y\"), and the implied form \
            (\"appointed … as OpenAI's CTO of applications\") all carry the same \
            shape — the role is the value, the organization is the other \
            entity. Past tense and \"former\" give the tie valid_to: \"unknown\".\n\
         8d. A list of named parties is a list of facts — one per name. \"partners \
            including A, B, C and D\" is four facts, not one; \"advisors A and B\" is two. \
            Do not collapse an enumeration into a summary or into its first member. \
            The same applies to the entities: each named party is its own entity.\n\
         8e. subject_span and object_span are the exact words in quote that name each side. \
            Copy them; never paraphrase. When the words that do the thing are a description \
            rather than a name — \"former X employees\", \"companies using X\" — the span \
            is that description, whatever you wrote in subject.\n\
         8f. An obligation, a deadline or a right belongs to the agreement, law or decision \
            that imposes it, even when it concerns another agreement or thing. A lease that \
            sets the last day to sign a second lease gives that deadline to the first lease; \
            the second lease is only what the deadline is about.\n\
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
        "Source file: \"{filename}\"\n{}{}\nText:\n{chunk_text}",
        opening_block(opening),
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

/// 文件开头排版成提示词里的一段。开头为空（或只有空白）时返回空串；
/// 「这一块就是开头本身」由调用方判断，那时它传 `None`。
///
/// 按字符截：不会截断一个字符，但会截在词中间——英文的最后一个词可能只剩半个
fn opening_block(opening: Option<&str>) -> String {
    let Some(text) = opening.map(str::trim).filter(|t| !t.is_empty()) else {
        return String::new();
    };
    let cut: String = text.chars().take(OPENING_BUDGET_CHARS).collect();
    let more = if cut.chars().count() < text.chars().count() {
        " …"
    } else {
        ""
    };
    format!(
        "\nOpening of this document, for context only (do not extract facts from it; they are \
         extracted from that part separately). Use it to know what the text below belongs to — \
         which agreement, company or event it concerns, who the parties are, and the date it \
         takes effect — so that facts in the text below attach to the right entity and carry \
         the right dates:\n\"\"\"\n{cut}{more}\n\"\"\"\n"
    )
}

/// 一段描述的第一句（句子边界按 UAX #29）。按块检索出的清单只带这一句：schema.org 的
/// 描述后半截多是用法说明与示例，一块铺上百行时它们占了清单的八成
pub fn first_sentence(text: &str) -> &str {
    use unicode_segmentation::UnicodeSegmentation;
    text.trim().unicode_sentences().next().map_or("", str::trim)
}

/// 清单里给关系带的标记：事件 `[event]`、恒常 `[eternal]`；状态不标。
/// 认不出的值当状态——数据库的 CHECK 只放这三个进来，这里不再报错
fn temporal_mark(temporal: &str) -> Option<&'static str> {
    match temporal {
        "event" => Some("event"),
        "eternal" => Some("eternal"),
        _ => None,
    }
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
fn known_block(known: &[KnownEntity]) -> String {
    if known.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    let mut used = 0usize;
    for entity in known {
        used += entity.handle.chars().count()
            + entity.type_key.chars().count()
            + entity.name.chars().count()
            + 6;
        if used > KNOWN_BUDGET_CHARS {
            break;
        }
        lines.push(format!(
            "  {} [{}]: {}",
            entity.handle, entity.type_key, entity.name
        ));
    }
    if lines.is_empty() {
        return String::new();
    }
    let lines = lines.join("\n");
    format!(
        "\nAlready recorded from earlier parts of this same document:\n{lines}\n\
         \n\
         If something in the text below refers to one of these, use its k-handle in the fact's \
         subject_ref/object_ref, write that exact string as the name, and give it that same \
         type — documents abbreviate after first mention \
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

    let (mut entities, mut skipped_entities) = take::<ExtractedEntity>(&value, "entities");
    let (facts, skipped_facts) = take::<ExtractedFact>(&value, "facts");
    // 名字条目坏了不算实体或事实被跳过：丢一个名字只是少一座桥，不丢断言
    let (names, _) = take::<ExtractedName>(&value, "names");

    // A handle identifies exactly one entity definition within one response. Reject every
    // definition participating in a duplicate (including identical duplicates): keeping the
    // first or last would make fact attribution depend on array order. Empty handles are
    // malformed too; legacy output is represented by an absent field, not an empty id.
    let mut handle_counts = std::collections::HashMap::<String, usize>::new();
    for entity in &entities {
        if let Some(handle) = entity.local_id.as_deref() {
            *handle_counts.entry(handle.trim().to_string()).or_default() += 1;
        }
    }
    entities.retain(|entity| match entity.local_id.as_deref() {
        None => true,
        Some(handle) => {
            let handle = handle.trim();
            let valid = !handle.is_empty() && handle_counts.get(handle) == Some(&1);
            if !valid {
                skipped_entities += 1;
            }
            valid
        }
    });
    Ok(Extraction {
        entities,
        facts,
        names,
        skipped_entities,
        skipped_facts,
        truncated,
    })
}

// ---------------------------------------------------------------------------
// 实体消解裁决（攒批：一次调用裁多对，LLM 只处理 embedding 分不出的灰区）
// ---------------------------------------------------------------------------

/// 待裁决的一侧：名字 + 类型 + 事实摘要行。
#[derive(Debug, Clone)]
pub struct AdjudicationSide {
    pub name: String,
    pub type_label: String,
    pub facts: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AdjudicationPair {
    pub left: AdjudicationSide,
    pub right: AdjudicationSide,
    /// 这个库里的人对这一对、这个名字、这种类型对做过什么（0025）。空就不提
    pub precedents: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct AdjudicationVerdict {
    pub i: usize,
    pub verdict: String,
    #[serde(default)]
    pub confidence: Option<f32>,
    /// 一句理由；带先例的裁决要说依据了哪条
    #[serde(default)]
    pub why: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AdjudicationReply {
    #[serde(default)]
    verdicts: Vec<AdjudicationVerdict>,
}

/// 构造攒批裁决提示词。保守偏置：证据不足答 unsure（宁分勿合，合并要证据）。
/// 身份规则（0025 第一轮迭代，2026-09-07）：攒批与工具循环两个提示词共用。
///
/// 从 ai-timeline 真值上量出来的四种错：版本并进系列、「X 的 Y」并进 X、列表并进成员、
/// 子公司并进母公司；以及同名被「类型标签不同」「合作伙伴不同」拆开。每一条都写成
/// 一句规则加例子，例子取自那份语料。
pub const IDENTITY_RULES: &str = "\
A record is one specific thing in the world: a person, an organization, a legal entity, a \
product, one version of a product, a document, an event, a place, or a concept. Two records \
are the SAME only when they denote exactly the same thing.\n\
\n\
Names:\n\
- The same proper name, compatible kinds, and no real contradiction: the same thing. Type \
  labels were assigned by an extractor and are noisy: Organization, Corporation, \
  ResearchOrganization, NGO and Store can all be one company; SoftwareApplication, \
  CreativeWork, Intangible, Product, Service, Offer and ComputerLanguage can all be one \
  product; Place and State can be one state. Only incompatible kinds count as a difference: \
  a person against a company, a place against a product, an event against an organization. \
  For identical proper names answer \"different\" only when you can name the contradiction \
  in one sentence; a type label, a missing fact, fewer facts, or different partners, \
  products, roles or events is not one. Never treat the absence of facts as a difference.\n\
- A surname or a first name alone against a full name that contains it, in the same \
  documents, is the same person unless another person with that name appears: \"Pachocki\" \
  is \"Jakub Pachocki\", \"Kwon\" is \"Jason Kwon\", \"Nadella\" is \"Satya Nadella\". A person \
  who moved between two organizations is still one person.\n\
- A parenthetical acronym, an expanded acronym or a fuller product designation is the same \
  thing: \"reinforcement learning (RL)\" is \"reinforcement learning\", \"US Federal Trade \
  Commission (FTC)\" is \"Federal Trade Commission\", \"MI450\" is the \"AMD Instinct MI450\".\n\
- A name that is the other name with a qualifier removed from the FRONT is usually the same \
  thing abbreviated: \"Google DeepMind\" and \"DeepMind\", \"Adam D'Angelo\" and \"D'Angelo\", \
  \"Meta Platforms\" and \"Meta\", \"Altimeter Capital\" and \"Altimeter\", \"The New York \
  Times\" and \"New York Times\", \"Apple Inc.\" and \"Apple\". Documents drop the qualifier \
  after first mention.\n\
- A name that is the other name with something ADDED AT THE END is a different, more \
  specific thing: a version or edition (\"Claude 4 Opus\" is not \"Claude\", \"AlphaFold2\" is \
  not \"AlphaFold\", \"Genie 2\" is not \"Genie\", \"GPT-4.5\" is not \"GPT-4\", \"2024 \
  International Mathematical Olympiad\" is not \"International Mathematical Olympiad\"), a \
  variant or tier (\"Gemini Robotics-ER\" is not \"Gemini Robotics\", \"Claude 3 Haiku\" is \
  not \"Haiku\"), a division, subsidiary or legal entity (\"DeepMind Health\" is not \
  \"DeepMind\", \"OpenAI Ireland Ltd\" is not \"OpenAI\", \"Microsoft AI\" is not \
  \"Microsoft\"), a project, programme, team, app or component. Never merge a version into \
  its family or a part into its whole.\n\
- A document, agreement or filing is cited in many ways and stays one thing through its \
  amendments: \"the Lease\", \"Lease Agreement\", \"Lease Agreement dated May 16, 2016\" and \
  \"Lease Agreement dated May 16, 2016, as amended\" are one agreement when their parties and \
  subject do not contradict each other. When it was signed and that it was amended describe \
  the agreement; they do not make a second one. Each amendment is a document of its own, and \
  an agreement for other premises, another phase or other parties (\"Phase 2 Lease\") is a \
  different agreement.\n\
- A phrase that merely contains a name is not that name: \"Sam Altman's efforts\", \
  \"psychological abuse from Sam Altman\", \"share sale led by Thrive Capital\", \"leaked \
  letter from the National Data Guardian\", \"ChatGPT played a role in the campaign\", \"a \
  consistent pattern of lying\". These describe something about the thing; they are not the \
  thing.\n\
- A list of names (\"MuZero, AlphaStar, AlphaGeometry\") is not any of its members.\n\
- A common noun or generic phrase (\"employees\", \"users\", \"lawsuit\", \"investors\", \
  \"event\", \"safety\") is not a proper name. Two such records are the same only when their \
  facts show they are one specific instance; usually they are different.\n\
- When the facts of either record describe ownership, control, a subsidiary, a holding or \
  a parent relation between the two names, or show one of them as one legal entity among \
  several in a group (\"OpenAI, Inc. controls the for-profit company\", \"OpenAI GP LLC \
  controls OpenAI LP\"), they are two entities even if one name is the other plus a \
  corporate suffix. A group and its legal entities are different records.\n\
\n\
Facts:\n\
- Different facts are not contradictory facts. One company has many partnerships, investors, \
  lawsuits and contracts; a person changes jobs; one product is praised in one document and \
  criticised in another. A contradiction is two facts that cannot both hold of one thing at \
  once: two different founders, two headquarters at the same time, two different birth dates, \
  or affiliations that overlap in time and exclude each other. Only a contradiction is \
  evidence of difference.\n\
- A record with no facts contributes nothing; the name and the other record decide.";

pub fn build_adjudication_messages(pairs: &[AdjudicationPair]) -> Vec<ChatMessage> {
    let system = format!(
        "You are an entity-resolution adjudicator for a knowledge graph. For each numbered \
         pair, decide whether the two records refer to the SAME real-world thing or are two \
         things that share a name.\n\
         \n\
         {IDENTITY_RULES}\n\
         \n\
         Some pairs carry precedents: decisions people made in this same knowledge base on \
         these names, or on pairs of the same two types. Treat them as how the owners of this \
         base want such cases judged. Follow a precedent on the same pair unless the facts of \
         this pair clearly differ from it; when precedents disagree with each other, answer \
         \"unsure\". A precedent never overrides a contradiction in the facts. Some precedents quote what the person wrote when deciding: weigh that stated ground, not only the outcome; a decision made for a reason that does not hold here is not a precedent for this pair.\n\
         \n\
         Output exactly one JSON object and nothing else:\n\
         {{\"verdicts\":[{{\"i\":0,\"verdict\":\"same|different|unsure\",\"confidence\":0.9,\
         \"why\":\"one sentence\"}}]}}\n\
         \n\
         Rules:\n\
         1. One verdict per pair, using the pair's number as \"i\".\n\
         2. confidence in 0~1.\n\
         3. \"why\" is one short sentence; when a precedent decided it, say which.\n\
         4. A wrong merge is far more damaging than leaving two records separate: answer \
            \"same\" only when the rules above make it so; when a rule says two things, say \
            \"different\" with confidence, not \"unsure\"; \"unsure\" is for evidence that \
            genuinely points both ways."
    );

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
        let precedents = if p.precedents.is_empty() {
            String::new()
        } else {
            let lines = p
                .precedents
                .iter()
                .map(|l| format!("  - {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("Precedents (decided by people in this base):\n{lines}\n")
        };
        user.push_str(&format!(
            "Pair {i}:\nRecord A: {}\nRecord B: {}\n{precedents}\n",
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

/// 一个**整体就是一个量**的字符串 → (数值, 单位)。
///
/// 判据从严：可选货币符号 + 数字 + 可选量级词 + 可选百分号，此外**一个词都不许有**。
/// 尾巴上还挂着实词的，含义就不再只是那个数：
///
/// ```text
/// "$5 billion"                        → (5e9, Some("$"))
/// "52%"                               → (52.0, Some("%"))
/// "3.5 million"                       → (3.5e6, None)
/// "35,000"                            → (35000.0, None)
/// "900 million weekly active users"   → None   后面还有实词
/// "2025 Atlantic hurricane season"    → None   那是一场赛事，不是 2025
/// "8GW data center"                   → None
/// "3M"                                → None   那是一家公司
/// ```
///
/// **量级词只认全写**。单字母后缀（`3M`、`5k`、`2B`）看着省事，代价是把 3M、
/// K2、B1 这些名字读成数字——一个真实体被读成量值，事实的形状就错了，
/// 而错的那一头是不可逆的：节点没建，名字也没留下。
///
/// **单位照抄符号，不猜币种。** `$` 可能是美元、加元、澳元，`¥` 可能是日元或
/// 人民币。猜出来的 "USD" 是一条没人负责的断言，而原文写的 `$` 是事实。
pub fn parse_quantity(s: &str) -> Option<(f64, Option<String>)> {
    scan_quantity(s, true)
}

/// 开头是一个量、后面还挂着词的 → 那个量。`"1,250 people"` → (1250, "people")。
///
/// **这是给已经知道要什么的地方用的**，与 `parse_quantity` 的严不是一回事。
/// `parse_quantity` 要判「这串字是不是一个东西」，判错就把一个真实体吃掉，
/// 所以尾巴上有实词一律不认。而这里的调用方手上已经有一条声明了
/// `datatype = number` 的属性——问的不再是「是不是数」，是「那个数是多少」，
/// 判错的代价只是一个值不对，量级差着好几档。
pub fn parse_leading_quantity(s: &str) -> Option<(f64, Option<String>)> {
    scan_quantity(s, false)
}

/// 货币：符号、ISO 码、中英文单词，统一成符号。**只认这张表**，认不出的不猜。
pub fn currency_unit(tok: &str) -> Option<&'static str> {
    Some(
        match tok.trim_matches(|c: char| c == ',' || c == '.' || c == ';') {
            "$" | "USD" | "usd" | "US$" | "dollar" | "dollars" | "美元" => "$",
            "€" | "EUR" | "eur" | "euro" | "euros" | "欧元" => "€",
            "£" | "GBP" | "gbp" | "pound" | "pounds" | "英镑" => "£",
            "¥" | "JPY" | "jpy" | "yen" | "日元" => "¥",
            "CNY" | "cny" | "RMB" | "rmb" | "yuan" | "人民币" | "元" | "元人民币" | "人民币元" => {
                "¥"
            }
            "HKD" | "hkd" | "HK$" | "港元" | "港币" => "HK$",
            "₩" | "KRW" | "won" | "韩元" => "₩",
            "₹" | "INR" | "rupee" | "rupees" | "卢比" => "₹",
            _ => return None,
        },
    )
}

/// 量级词：英文全写，中文千/万/亿。**不认单字母**（`3M` 是一家公司）。
fn magnitude(tok: &str) -> Option<f64> {
    Some(match tok {
        "thousand" | "千" => 1e3,
        "万" => 1e4,
        "million" | "百万" => 1e6,
        "千万" => 1e7,
        "亿" => 1e8,
        "billion" | "十亿" => 1e9,
        "trillion" | "万亿" => 1e12,
        _ => return None,
    })
}

/// 把 `2亿美元`、`15亿元人民币`、`€30 million`、`30 million euros`、`USD 30m`（不认 m）
/// 这类写法拆成 [前缀货币] 数字 [量级] [后缀货币/单位] [其余]。
/// `strict` = 整体必须就是一个量：其余部分非空就不认。
fn scan_quantity(s: &str, strict: bool) -> Option<(f64, Option<String>)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (body, percent) = match s.strip_suffix('%') {
        Some(b) => (b.trim_end(), true),
        None => (s, false),
    };
    // 1. 前缀货币：符号紧贴，或 ISO 码/单词后跟空格
    let mut rest = body;
    let mut currency: Option<&'static str> = None;
    if let Some(c) = rest.chars().next() {
        if let Some(u) = currency_unit(&c.to_string()) {
            currency = Some(u);
            rest = rest[c.len_utf8()..].trim_start();
        }
    }
    if currency.is_none() {
        if let Some((head, tail)) = rest.split_once(char::is_whitespace) {
            if let Some(u) = currency_unit(head) {
                currency = Some(u);
                rest = tail.trim_start();
            }
        }
    }
    // 2. 数字：前导的 [-+0-9.,_]
    let num_end = rest
        .char_indices()
        .find(|(_, c)| !matches!(c, '0'..='9' | '.' | ',' | '_' | '-' | '+'))
        .map(|(i, _)| i)
        .unwrap_or(rest.len());
    let (num, after) = rest.split_at(num_end);
    let cleaned: String = num.chars().filter(|c| !matches!(c, ',' | '_')).collect();
    let mut n: f64 = cleaned.parse().ok()?;
    // 3. 数字后面：紧贴或空格隔开的量级词、货币词，逐个吃；吃不动的就是「其余」
    let mut tail = after.trim_start();
    let mut unit: Option<String> = None;
    let mut ate_magnitude = false;
    loop {
        if tail.is_empty() {
            break;
        }
        // 取下一个记号：中文按字（量级/货币词最长两三个字），其它按空白分词
        let (tok, next) = next_token(tail);
        if !ate_magnitude {
            if let Some(m) = magnitude(tok) {
                n *= m;
                ate_magnitude = true;
                tail = next.trim_start();
                continue;
            }
        }
        if unit.is_none() && currency.is_none() {
            if let Some(u) = currency_unit(tok) {
                unit = Some(u.to_string());
                tail = next.trim_start();
                continue;
            }
        }
        break;
    }
    if percent && (currency.is_some() || unit.is_some()) {
        return None;
    }
    if !n.is_finite() {
        return None;
    }
    // 9.2 × 1e8 在二进制浮点里是 919999999.9999999；乘过量级词的数本来就是整数，收回去
    if ate_magnitude && (n - n.round()).abs() < 1e-6 * n.abs().max(1.0) {
        n = n.round();
    }
    let unit = if percent {
        Some("%".to_string())
    } else {
        currency.map(str::to_string).or(unit)
    };
    if strict {
        return tail.is_empty().then_some((n, unit));
    }
    // 宽松：其余部分的第一个词当单位（`1,250 people` → people），没有货币时才用
    if unit.is_none() && !tail.is_empty() {
        let (tok, _) = next_token(tail);
        return Some((n, Some(tok.to_string())));
    }
    Some((n, unit))
}

/// 下一个记号：ASCII 按空白切；CJK 试最长三字、两字、一字里能认出的量级/货币词，
/// 都认不出就取到下一个空白为止
fn next_token(s: &str) -> (&str, &str) {
    let first = s.chars().next().unwrap_or(' ');
    if first.is_ascii() {
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        return (&s[..end], &s[end..]);
    }
    let idx: Vec<usize> = s
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()))
        .collect();
    for len in [4usize, 3, 2, 1] {
        if idx.len() > len {
            let cand = &s[..idx[len]];
            if magnitude(cand).is_some() || currency_unit(cand).is_some() {
                return (cand, &s[idx[len]..]);
            }
        }
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (&s[..end], &s[end..])
}

/// 一个属性值落库时的样子：按 datatype 归一成 `{"value": …}`；失败返回 None，调用方记
/// `attr_datatype`。
///
/// 日期属性上一个**相对**的值（#681 §4）：解不成日期、模型又标了 `relative`、写的是一段非空
/// 文字时，照原文收下，值里带 `"relative": true`。它不是日期，从不当日期比较或排序。解得成
/// 日期的照日期存，标错了也不当相对；没标的非日期值仍然不收
pub fn attr_object_value(
    datatype: &str,
    raw: &serde_json::Value,
    relative: bool,
) -> Option<serde_json::Value> {
    if let Some(value) = normalize_attr_value(datatype, raw) {
        return Some(serde_json::json!({ "value": value }));
    }
    let written = raw.as_str().map(str::trim).filter(|s| !s.is_empty())?;
    (datatype == "date" && relative)
        .then(|| serde_json::json!({ "value": written, "relative": true }))
}

/// 属性值按 datatype 归一。失败返回 None——宁缺勿脏，调用方跳过并记日志。
/// number 容忍千分位/空格；date 收规则 3 的格式（YYYY[-MM[-DD]]、带时区的时刻，原样保留），
/// 也收写法说得清是哪天的日期（[`written_date`]），换成规则 3 的样子；bool 宽容 yes/no。
pub fn normalize_attr_value(datatype: &str, raw: &serde_json::Value) -> Option<serde_json::Value> {
    match datatype {
        "number" => match raw {
            // 模型给的 JSON 数也过一遍 f64：`65` 与 "65%" 解出来的 `65.0` 是同一个数，
            // 而 serde_json 把整数和浮点当两种值——实测同一条边上 65 撞 65.0 记成了冲突
            serde_json::Value::Number(n) => n
                .as_f64()
                .filter(|f| f.is_finite())
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number),
            serde_json::Value::String(s) => {
                let cleaned: String = s
                    .chars()
                    .filter(|c| !matches!(c, ',' | ' ' | '_'))
                    .collect();
                cleaned
                    .parse::<f64>()
                    .ok()
                    // 清洗解不动的再当量解：`$5 billion`、`52%` 这些整体就是数，
                    // 只是带着符号与量级词。单位不在这里落笔——它随事实走
                    // （见 `parse_quantity`），这一档只负责把值变成可比的数
                    // 清洗解不动的再当量解。**这一档已经声明了 datatype = number**，
                    // 问的不是「是不是数」而是「那个数是多少」，所以用宽的那套：
                    // `$5 billion` → 5e9，`1,250 people` → 1250，
                    // `42% from customers in Europe` → 42
                    .or_else(|| parse_leading_quantity(s).map(|(n, _)| n))
                    .filter(|f| f.is_finite())
                    .and_then(serde_json::Number::from_f64)
                    .map(serde_json::Value::Number)
            }
            _ => None,
        },
        // 按规则 3 写的原样留着（精度随写了几位，带时区的时刻也在内）；写成别的样子、又读得
        // 出来的日期（#688）换成规则 3 的样子——同一天只该有一种写法，比较和去重才对得上
        "date" => {
            let s = raw.as_str()?.trim();
            match written_date(s) {
                Some((date, precision)) => Some(serde_json::Value::String(match precision {
                    "month" => date.format("%Y-%m").to_string(),
                    _ => date.format("%Y-%m-%d").to_string(),
                })),
                None => parse_time(s).map(|_| serde_json::Value::String(s.to_string())),
            }
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

/// 解析时间字符串 → (UTC 时间, 精度)。
///
/// 日期：YYYY / YYYY-MM / YYYY-MM-DD，精度随写了几位。带时区的时刻（0024）：
/// `YYYY-MM-DDTHH[:MM[:SS]]` 后跟 `Z` 或 `±HH:MM`，精度到 hour / minute / second，
/// 值截到那一位。**没有时区的钟点不是时刻**——「14:32」是哪里的 14:32 没人知道——
/// 所以只取日期那一半，按天；钟点留在引文里。亚秒一律丢：账本到秒为止。
///
/// 这是规则 3 的契约格式，工具参数、界面上的时刻都只认它。读模型回复用 [`read_time`]
pub fn parse_time(s: &str) -> Option<(DateTime<Utc>, &'static str)> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("null") {
        return None;
    }
    if let Some((date, clock)) = s.split_once(['T', ' ']) {
        let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
        let Some((clock, offset)) = split_zone(clock) else {
            return Some((Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?), "day"));
        };
        let parts: Vec<&str> = clock.split(':').collect();
        let (h, m, sec, precision) = match parts.as_slice() {
            [h] => (*h, "0", "0", "hour"),
            [h, m] => (*h, *m, "0", "minute"),
            [h, m, sec] => (*h, *m, sec.split('.').next().unwrap_or(sec), "second"),
            _ => return None,
        };
        let time =
            chrono::NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, sec.parse().ok()?)?;
        let utc = d.and_time(time) - offset;
        return Some((Utc.from_utc_datetime(&utc), precision));
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

/// 读模型回复里的时间：先按规则 3（[`parse_time`]）；没照它写、但写法说得清是哪天的日期
/// 也收（[`written_date`]，#688）。区间端点、日期属性、边上的日期属性都从这里读
pub fn read_time(s: &str) -> Option<(DateTime<Utc>, &'static str)> {
    parse_time(s).or_else(|| {
        let (date, precision) = written_date(s)?;
        Some((
            Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?),
            precision,
        ))
    })
}

/// 没照规则 3 写、但说得清是哪一天（或哪个月）的日期（#688）。
///
/// 合同、公告里的日期多半这么写，模型常常照抄；从前这些值全被当成「不是日期」丢掉。
/// 收两类，精度随写了几位——只写到月的就是月，不替它补一个日：
/// - 月份写成名字的：`March 17, 2020`、`17 March 2020`、`Mar. 17 2020`、`March 2020`。
///   名字由 chrono 的 `%B` / `%b` 认（整名或三个字母的缩写，不分大小写）
/// - 年在前的数字：`2020/03/17`、`2020.3.17`、`2020年3月17日`、`2020年3月`
///
/// **日、月都是数字而年不在前的不收**：`03/04/2020` 是三月四日还是四月三日，写法本身说
/// 不清，猜错一次就是一个错的截止日。
pub fn written_date(s: &str) -> Option<(NaiveDate, &'static str)> {
    let s = s.trim();
    if let Some(found) = year_first_date(s) {
        return found;
    }
    // 月份写成名字的：句点（缩写后面那个）与逗号只是标点
    let words: Vec<&str> = s
        .split(|c: char| c.is_whitespace() || c == ',' || c == '.')
        .filter(|w| !w.is_empty())
        .collect();
    let is_number = |w: &str| w.chars().all(|c| c.is_ascii_digit());
    let (day, month, year) = match words.as_slice() {
        [m, d, y] if !is_number(m) && is_number(d) && is_number(y) => (Some(*d), *m, *y),
        [d, m, y] if is_number(d) && !is_number(m) && is_number(y) => (Some(*d), *m, *y),
        [m, y] if !is_number(m) && is_number(y) => (None, *m, *y),
        _ => return None,
    };
    if year.len() != 4 || day.is_some_and(|d| d.len() > 2) {
        return None;
    }
    let month = ["%B", "%b"].iter().find_map(|f| {
        NaiveDate::parse_from_str(&format!("1 {month} 2000"), &format!("%d {f} %Y"))
            .ok()
            .map(|d| chrono::Datelike::month(&d))
    })?;
    let year = year.parse().ok()?;
    match day {
        Some(d) => NaiveDate::from_ymd_opt(year, month, d.parse().ok()?).map(|date| (date, "day")),
        None => NaiveDate::from_ymd_opt(year, month, 1).map(|date| (date, "month")),
    }
}

/// 年在前的数字日期。外层 `None` = 不是这种写法，交给下一种；`Some(None)` = 是这种写法
/// 但不是真实的日子
fn year_first_date(s: &str) -> Option<Option<(NaiveDate, &'static str)>> {
    let marked = s.contains('年');
    let separated = s.contains(['/', '.']) && !s.contains(char::is_whitespace);
    if !marked && !separated {
        return None;
    }
    let parts: Vec<&str> = s
        .trim_end_matches('日')
        .split(['/', '.', '年', '月'])
        .filter(|p| !p.is_empty())
        .collect();
    if !parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    if parts.first().is_none_or(|y| y.len() != 4) {
        // 年不在前：日月顺序说不清，不交给别的写法去猜
        return Some(None);
    }
    let number = |p: &str| p.parse::<u32>().ok();
    let year = parts[0].parse::<i32>().ok()?;
    Some(match parts[1..] {
        [m, d] if m.len() <= 2 && d.len() <= 2 => {
            NaiveDate::from_ymd_opt(year, number(m)?, number(d)?).map(|date| (date, "day"))
        }
        // 只到月：写了「年」「月」才算（`2020/03` 太像别的东西）
        [m] if marked && s.ends_with('月') && m.len() <= 2 => {
            NaiveDate::from_ymd_opt(year, number(m)?, 1).map(|date| (date, "month"))
        }
        _ => None,
    })
}

/// 钟点后面的时区：`Z` 或 `±HH[:]MM` / `±HH`。返回 (钟点, 相对 UTC 的偏移)；
/// 没有时区返回 None——调用方据此只记那一天
fn split_zone(clock: &str) -> Option<(&str, chrono::Duration)> {
    if let Some(c) = clock.strip_suffix(['Z', 'z']) {
        return Some((c, chrono::Duration::zero()));
    }
    let i = clock.rfind(['+', '-'])?;
    let (c, zone) = clock.split_at(i);
    let sign: i64 = if zone.starts_with('-') { -1 } else { 1 };
    let digits: String = zone[1..].chars().filter(|ch| ch.is_ascii_digit()).collect();
    let (hh, mm) = match digits.len() {
        2 => (&digits[..2], "0"),
        4 => (&digits[..2], &digits[2..]),
        _ => return None,
    };
    let (h, m): (i64, i64) = (hh.parse().ok()?, mm.parse().ok()?);
    Some((c, chrono::Duration::minutes(sign * (h * 60 + m))))
}

#[cfg(test)]
mod prompt_shape_tests {
    use super::*;

    /// 第十份补充协议把截止日改成「触发日后 45 天」：模型标 relative，服务端照原文收；
    /// 没标的、不是日期属性的、空的都不走这条
    #[test]
    fn a_relative_date_is_kept_as_written_only_when_marked() {
        let raw = serde_json::json!("45 days after the Trigger Date");
        assert_eq!(
            attr_object_value("date", &raw, true),
            Some(
                serde_json::json!({ "value": "45 days after the Trigger Date", "relative": true })
            )
        );
        assert_eq!(attr_object_value("date", &raw, false), None, "没标就不收");
        assert_eq!(
            attr_object_value("date", &serde_json::json!("  "), true),
            None
        );
        // 解得成日期的照日期存，标了 relative 也不当相对
        assert_eq!(
            attr_object_value("date", &serde_json::json!("2020-06-23"), true),
            Some(serde_json::json!({ "value": "2020-06-23" }))
        );
        // 不是日期属性：只按它自己的 datatype 归一，relative 不起作用
        assert_eq!(
            attr_object_value("bool", &serde_json::json!("45 days after"), true),
            None
        );
        assert_eq!(
            attr_object_value("number", &serde_json::json!("1,250"), true),
            Some(serde_json::json!({ "value": 1250.0 }))
        );
        let fact: ExtractedFact = serde_json::from_value(serde_json::json!({
            "subject": "Lease", "predicate": "expansion_option_deadline",
            "value": "45 days after the Trigger Date", "relative": true
        }))
        .unwrap();
        assert!(fact.relative);
        let plain: ExtractedFact = serde_json::from_value(serde_json::json!({
            "subject": "Lease", "predicate": "expansion_option_deadline", "value": "2020-06-23"
        }))
        .unwrap();
        assert!(!plain.relative, "没写就不是");
        let msgs = build_messages(
            &[],
            &[],
            &["lease.deadline (date)".into()],
            None,
            "a.txt",
            &[],
            "text",
        );
        assert!(msgs[0].content.contains("add \"relative\": true"));
    }

    fn rel(key: &str, description: &str, signature: &str) -> PromptRelation {
        PromptRelation {
            key: key.into(),
            label: key.replace('_', " "),
            description: description.into(),
            signature: signature.into(),
            temporal: "state".into(),
            qualifiers: vec![],
        }
    }

    fn timed(key: &str, description: &str, temporal: &str) -> PromptRelation {
        PromptRelation {
            temporal: temporal.into(),
            ..rel(key, description, "")
        }
    }

    /// 事件与恒常在清单里带标记，说明只出现一次（0031）
    #[test]
    fn an_event_and_an_eternal_relation_are_marked() {
        let rels = vec![
            rel("works_at", "受雇于某个组织。", "person → organization"),
            timed("acquired", "One company buys another.", "event"),
            timed("capital_of", "", "eternal"),
        ];
        let msgs = build_messages(&[], &rels, &[], None, "a.txt", &[], "text");
        let s = &msgs[0].content;
        assert!(s.contains("- works_at (person → organization): 受雇于某个组织。"));
        assert!(s.contains("- acquired [event]: One company buys another."));
        // 没有描述时括号里是 label，标记跟在括号后面
        assert!(s.contains("- capital_of (capital of) [eternal]"));
        assert!(s.contains("A relation marked [event] happens at one moment"));
        assert!(s.contains("leave valid_to null"));
    }

    /// **全是状态的库，提示词一字不变**：不标、不解释
    #[test]
    fn a_base_of_states_pays_nothing_for_the_marks() {
        let rels = vec![rel("works_at", "d", "")];
        let msgs = build_messages(&[], &rels, &[], None, "a.txt", &[], "text");
        let s = &msgs[0].content;
        assert!(!s.contains("[event]"));
        assert!(!s.contains("[eternal]"));
        assert!(!s.contains("happens at one moment"));
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

    #[test]
    fn an_obligation_belongs_to_the_agreement_that_imposes_it() {
        // 主租约里写着「签二期租约的截止日」，模型时而把截止日挂到二期租约上：
        // 主租约的时间线上就少了这次改期（#681 §3）
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &[], "text");
        assert!(msgs[0]
            .content
            .contains("belongs to the agreement, law or decision that imposes it"));
    }

    #[test]
    fn a_literal_keeps_its_units_but_a_date_takes_the_contract_format() {
        // 8a 从前说「字面值按原文写」并把日期列在字面值里，而规则 3 与属性规则要求
        // YYYY-MM-DD：两条互相打架，模型写出「June 23, 2020」，服务端按格式不合整条丢掉。
        // Blackbaud 总部租约链上各轮累计丢了二十多次
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &[], "text");
        let system = &msgs[0].content;
        assert!(system.contains("except a date, which is always written in the format of rule 3"));
    }

    /// 补充协议把旧条款与新日期排成一张对照表，模型把表格里读到的新日期标 0.7（当成
    /// 推断），低于 0.75 的值不许接替前一个——截止日就一直停在旧值上。规则 6 说清楚：
    /// 原文写着的值不论排成什么样都是明写
    #[test]
    fn a_value_written_in_a_table_is_stated() {
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &[], "text");
        let system = &msgs[0].content;
        assert!(system.contains("stated whatever the layout"));
        assert!(system.contains("a table cell"));
    }

    /// 规则编号各不相同，「按规则 N」指得到唯一的一条。从前有两条 8c、两条 10，
    /// 「as rule 10 says」说的是哪条要靠猜（#689 评审）
    #[test]
    fn every_rule_has_its_own_number_and_every_reference_lands() {
        let rels = vec![PromptRelation {
            key: "acquired".into(),
            label: "acquired".into(),
            description: String::new(),
            signature: String::new(),
            temporal: "event".into(),
            qualifiers: vec![],
        }];
        let attrs = vec!["lease.option_deadline (date)".to_string()];
        let msgs = build_messages(&[], &rels, &attrs, None, "a.txt", &[], "text");
        let system = &msgs[0].content;
        let mut labels = Vec::new();
        for line in system.lines() {
            let Some((label, _)) = line.trim_start().split_once(". ") else {
                continue;
            };
            let digits = label.trim_end_matches(|c: char| c.is_ascii_lowercase());
            if !digits.is_empty()
                && digits.chars().all(|c| c.is_ascii_digit())
                && label.len() - digits.len() <= 1
            {
                labels.push(label.to_string());
            }
        }
        let unique: std::collections::BTreeSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "规则编号重复：{labels:?}");
        for (i, _) in system.match_indices("rule ") {
            let n: String = system[i + 5..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            assert!(
                labels.contains(&n),
                "「rule {n}」指不到任何一条：{labels:?}"
            );
        }
    }

    #[test]
    fn a_later_chunk_reads_the_opening_of_its_document() {
        let opening = "FIFTH AMENDMENT TO LEASE AGREEMENT entered into as of February 18, 2020";
        let msgs = build_messages_with_opening(
            &[],
            &[],
            &[],
            None,
            "a.html",
            &[],
            Some(opening),
            "The Existing Dates are extended to March 17, 2020.",
        );
        let user = &msgs[1].content;
        let at_opening = user.find(opening).expect("opening is in the user message");
        let at_text = user
            .find("The Existing Dates")
            .expect("text is in the user message");
        assert!(
            at_opening < at_text,
            "the opening comes before the text it frames"
        );
        // 没有开头时，提示词与从前一字不差
        let plain = build_messages(&[], &[], &[], None, "a.html", &[], "t");
        let framed = build_messages_with_opening(&[], &[], &[], None, "a.html", &[], None, "t");
        assert_eq!(plain[1].content, framed[1].content);
    }

    #[test]
    fn a_long_opening_is_cut_on_a_character_boundary() {
        let long = "租".repeat(OPENING_BUDGET_CHARS + 10);
        let block = opening_block(Some(&long));
        assert_eq!(block.matches('租').count(), OPENING_BUDGET_CHARS);
        assert!(block.contains(" …"));
        assert_eq!(opening_block(Some("   ")), "");
    }

    /// 已知实体必须落在 **user** 消息里、紧挨着正文。
    ///
    /// 理由是服从性不是缓存：抽象规则打不过挨着它的具体块。清单放进 system 的
    /// 规则区，就会隔着输出格式、十条规则、文件名，离它要管的正文最远。
    #[test]
    fn known_entities_stay_out_of_the_system_message() {
        // 用一个规则 1 的例子里没有的名字：规则 1 也提"星云科技上海研究院"，
        // 拿它断言等于测不出清单到底在哪条消息里
        let known = vec![KnownEntity {
            handle: "k1".into(),
            type_key: "organization".into(),
            name: "华瑞集团智能制造研究院".into(),
        }];
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &known, "text");
        assert_eq!(msgs[0].role, "system");
        assert!(!msgs[0].content.contains("Already recorded"));
        assert!(!msgs[0].content.contains("华瑞集团智能制造研究院"));
        assert!(msgs[1]
            .content
            .contains("k1 [organization]: 华瑞集团智能制造研究院"));
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
        let known = vec![KnownEntity {
            handle: "k1".into(),
            type_key: "person".into(),
            name: "陈立".into(),
        }];
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

    /// 边上的属性（0037）：清单里跟在关系后面，回复里挂在事实上。
    #[test]
    fn a_relation_lists_its_qualifiers_and_a_fact_carries_them() {
        use serde_json::json;
        let mut r = PromptRelation {
            key: "invested_in".into(),
            label: "invested in".into(),
            description: "money into a company".into(),
            signature: "organization → organization".into(),
            temporal: "event".into(),
            qualifiers: vec!["amount: number $".into(), "stake: number %".into()],
        };
        let msgs = build_messages(
            &[],
            std::slice::from_ref(&r),
            &[],
            None,
            "a.txt",
            &[],
            "text",
        );
        let prompt = format!("{:?}", msgs);
        // 签名、标记、属性清单三段顺序固定：`(签名) [event] {属性}`
        assert!(prompt.contains(
            "- invested_in (organization → organization) [event] {amount: number $, stake: number %}: money into a company"
        ), "{prompt}");
        // 不带属性的关系不多一个花括号
        r.qualifiers.clear();
        let prompt = format!(
            "{:?}",
            build_messages(
                &[],
                std::slice::from_ref(&r),
                &[],
                None,
                "a.txt",
                &[],
                "text"
            )
        );
        assert!(
            prompt.contains("- invested_in (organization → organization) [event]: money"),
            "{prompt}"
        );
        assert!(!prompt.contains("[event] {"));

        // 回复：qualifiers 挂在关系事实上；没写的是 None，旧回复不受影响
        let reply = r#"{"entities":[],"facts":[
            {"subject":"Vega","predicate":"invested_in","object":"Northwind",
             "qualifiers":{"amount":"$5 billion"},"confidence":0.9},
            {"subject":"Vega","predicate":"invested_in","object":"Kestrel","confidence":0.9}
        ]}"#;
        let parsed = parse_response(reply).unwrap();
        assert_eq!(parsed.facts.len(), 2);
        assert_eq!(
            parsed.facts[0]
                .qualifiers
                .as_ref()
                .and_then(|q| q.get("amount")),
            Some(&json!("$5 billion"))
        );
        assert!(parsed.facts[1].qualifiers.is_none());
    }

    #[test]
    fn a_quantity_is_the_whole_string_or_nothing() {
        // 整体就是一个量：符号、量级词、千分位都读得动
        assert_eq!(parse_quantity("$5 billion"), Some((5e9, Some("$".into()))));
        assert_eq!(
            parse_quantity("€1.5 million"),
            Some((1.5e6, Some("€".into())))
        );
        assert_eq!(parse_quantity("52%"), Some((52.0, Some("%".into()))));
        assert_eq!(parse_quantity("3.5 million"), Some((3.5e6, None)));
        assert_eq!(parse_quantity("35,000"), Some((35000.0, None)));
        assert_eq!(parse_quantity("  42 "), Some((42.0, None)));
        // 币种：符号、ISO 码、中英文单词，统一成符号；量级：英文全写与中文千万亿
        assert_eq!(
            parse_quantity("EUR 30 million"),
            Some((3e7, Some("€".into())))
        );
        assert_eq!(
            parse_quantity("30 million euros"),
            Some((3e7, Some("€".into())))
        );
        assert_eq!(
            parse_quantity("USD 5 billion"),
            Some((5e9, Some("$".into())))
        );
        assert_eq!(parse_quantity("2亿美元"), Some((2e8, Some("$".into()))));
        assert_eq!(
            parse_quantity("15亿元人民币"),
            Some((1.5e9, Some("¥".into())))
        );
        assert_eq!(parse_quantity("3000万元"), Some((3e7, Some("¥".into()))));
        assert_eq!(parse_quantity("1.5亿"), Some((1.5e8, None)));
        // 乘过量级的数收成整数：9.2 亿不是 919999999.9999999
        assert_eq!(
            parse_quantity("9.2亿元"),
            Some((920000000.0, Some("¥".into())))
        );
        assert_eq!(
            parse_quantity("$2.5 billion"),
            Some((2500000000.0, Some("$".into())))
        );

        // 尾巴上还有实词：含义不再只是那个数，宁可当实体也不当量
        assert_eq!(parse_quantity("900 million weekly active users"), None);
        assert_eq!(parse_quantity("2025 Atlantic hurricane season"), None);
        assert_eq!(parse_quantity("$10 billion investment"), None);
        assert_eq!(parse_quantity("8GW data center"), None);
        // 单字母后缀不认：3M 是一家公司，读成三百万就把一个真实体吃掉了
        assert_eq!(parse_quantity("3M"), None);
        assert_eq!(parse_quantity("5k"), None);
        // 两个记号撞一起，不是量
        assert_eq!(parse_quantity("$5%"), None);
        assert_eq!(parse_quantity(""), None);
        assert_eq!(parse_quantity("杭州"), None);
    }

    #[test]
    fn a_declared_number_reads_past_the_unit() {
        // 属性已经声明了 datatype = number，问的是「那个数是多少」。
        // 卡住过的两条都在这里
        assert_eq!(
            parse_leading_quantity("1,250 people"),
            Some((1250.0, Some("people".into())))
        );
        assert_eq!(
            parse_leading_quantity("42% from customers in Europe"),
            Some((42.0, Some("%".into())))
        );
        assert_eq!(
            parse_leading_quantity("3,400 people worldwide"),
            Some((3400.0, Some("people".into())))
        );
        assert_eq!(
            parse_leading_quantity("900 million weekly active users"),
            Some((9e8, Some("weekly".into())))
        );
        // 整体就是量的仍走严的那套：单位是 `$`，不是 `billion`
        assert_eq!(
            parse_leading_quantity("$5 billion"),
            Some((5e9, Some("$".into())))
        );
        // 开头不是数就还是不认
        // 币种在尾巴上也认；认不出的词才落到「单位是第一个词」
        assert_eq!(
            parse_leading_quantity("30 million euros in cash"),
            Some((3e7, Some("€".into())))
        );
        assert_eq!(
            parse_leading_quantity("15亿元人民币的投资"),
            Some((1.5e9, Some("¥".into())))
        );
        assert_eq!(
            parse_leading_quantity("30 million francs"),
            Some((3e7, Some("francs".into())))
        );
        assert_eq!(parse_leading_quantity("about ten"), None);
        assert_eq!(parse_leading_quantity(""), None);

        // **严的那套一点没松**：它要判「是不是一个东西」，判错会吃掉真实体
        assert_eq!(parse_quantity("1,250 people"), None);
        assert_eq!(parse_quantity("2025 Atlantic hurricane season"), None);
    }

    #[test]
    fn a_number_attribute_takes_a_written_quantity() {
        use serde_json::json;
        // 采纳属性时按 datatype 换算，量也要换得动——否则 `$5 billion`
        // 会一路「换不动」，事实永远拿不到谓词
        assert_eq!(
            normalize_attr_value("number", &json!("$5 billion")),
            Some(json!(5e9))
        );
        assert_eq!(
            normalize_attr_value("number", &json!("52%")),
            Some(json!(52.0))
        );
        // 原来就认的两种写法不受影响
        assert_eq!(
            normalize_attr_value("number", &json!("35,000")),
            Some(json!(35000.0))
        );
        assert_eq!(normalize_attr_value("number", &json!("about ten")), None);
    }

    #[test]
    fn a_description_is_cut_at_its_first_sentence() {
        assert_eq!(
            first_sentence(
                "  The date on which the CreativeWork was created. See also dateModified.\n\nExample: 2020-01-01."
            ),
            "The date on which the CreativeWork was created."
        );
        assert_eq!(
            first_sentence("一个有名有姓的人。可以是虚构的。"),
            "一个有名有姓的人。"
        );
        assert_eq!(first_sentence("A person"), "A person");
        assert_eq!(first_sentence("   "), "");
    }

    #[test]
    fn parse_time_precisions() {
        assert_eq!(parse_time("2024").unwrap().1, "year");
        assert_eq!(parse_time("2024-07").unwrap().1, "month");
        assert_eq!(parse_time("2024-07-15").unwrap().1, "day");
        // 带时区的钟点：到分、到时、到秒，值截到那一位，偏移换回 UTC
        let (t, p) = parse_time("2026-06-01T14:32Z").unwrap();
        assert_eq!(
            (t.to_rfc3339(), p),
            ("2026-06-01T14:32:00+00:00".to_string(), "minute")
        );
        assert_eq!(parse_time("2026-06-01T14Z").unwrap().1, "hour");
        let (t, p) = parse_time("2026-06-01T14:32:07.382+08:00").unwrap();
        assert_eq!(
            (t.to_rfc3339(), p),
            ("2026-06-01T06:32:07+00:00".to_string(), "second")
        );
        // 没时区的钟点不是时刻：只记那一天
        let (t, p) = parse_time("2026-06-01T14:32").unwrap();
        assert_eq!(
            (t.to_rfc3339(), p),
            ("2026-06-01T00:00:00+00:00".to_string(), "day")
        );
        assert!(parse_time("null").is_none());
        assert!(parse_time("").is_none());
        assert!(parse_time("下个月").is_none());
        // 契约格式之外的写法不归它：工具参数里的「August 2024」不猜
        assert!(parse_time("June 23, 2020").is_none());
        // 读模型回复的那一个收写出来的日期（#688），精度随写了几位
        let (t, p) = read_time("June 23, 2020").unwrap();
        assert_eq!(
            (t.to_rfc3339(), p),
            ("2020-06-23T00:00:00+00:00".to_string(), "day")
        );
        assert_eq!(read_time("March 2020").unwrap().1, "month");
        assert_eq!(read_time("2024-07").unwrap().1, "month");
        assert!(read_time("03/04/2020").is_none());
    }

    /// 合同与公告里的日期写法（#688）：说得清是哪天的都收成规则 3 的样子，说不清的不猜
    #[test]
    fn a_written_date_is_read_only_when_its_form_says_which_day() {
        use serde_json::json;
        let day = |s: &str| normalize_attr_value("date", &json!(s));
        for written in [
            "March 17, 2020",
            "March 17 2020",
            "march 17, 2020",
            "MARCH 17, 2020",
            "Mar 17, 2020",
            "Mar. 17, 2020",
            "17 March 2020",
            "17 Mar. 2020",
            "17 March, 2020",
            "  March 17, 2020 ",
            "2020/03/17",
            "2020.3.17",
            "2020年3月17日",
        ] {
            assert_eq!(day(written), Some(json!("2020-03-17")), "{written}");
        }
        // 只写到月的是月，不补日
        for written in ["March 2020", "Mar. 2020", "2020年3月"] {
            assert_eq!(day(written), Some(json!("2020-03")), "{written}");
        }
        // 已经照规则 3 写的原样留着
        assert_eq!(day("2020-03-17"), Some(json!("2020-03-17")));
        assert_eq!(day("2020"), Some(json!("2020")));
        // 日月都是数字、年不在前：说不清是几月几号
        for ambiguous in ["03/04/2020", "3.4.2020", "04-03-2020", "3/4/20"] {
            assert_eq!(day(ambiguous), None, "{ambiguous}");
        }
        // 不是日期，或不是一个真实的日子
        for not_a_date in [
            "45 days after the Trigger Date",
            "Q3 2020",
            "Sometime 2020",
            "February 30, 2020",
            "March 17, 20",
            "March 123, 2020",
            "2020/13/01",
            "2020/03",
            "next March",
        ] {
            assert_eq!(day(not_a_date), None, "{not_a_date}");
        }
        // 区间端点读的是同一个解析
        assert_eq!(
            read_time("17 Mar 2020").map(|(t, p)| (t.date_naive().to_string(), p)),
            Some(("2020-03-17".to_string(), "day"))
        );
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
        // JSON 里的整数也落成同一种数：`42` 与 "42" 解出来是同一个值
        assert_eq!(
            normalize_attr_value("number", &json!(42)),
            Some(json!(42.0))
        );
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

    /// 片段字段可有可无：老模型输出没有它们，照常解析
    #[test]
    fn spans_parse_and_default_to_none() {
        let with = parse_response(
            r#"{"entities":[],"facts":[{"subject":"OpenAI","predicate":"founded","object":"Anthropic","subject_span":"Former OpenAI personnel","object_span":"Anthropic"}]}"#,
        )
        .unwrap();
        assert_eq!(
            with.facts[0].subject_span.as_deref(),
            Some("Former OpenAI personnel")
        );
        assert_eq!(with.facts[0].object_span.as_deref(), Some("Anthropic"));
        let without = parse_response(
            r#"{"entities":[],"facts":[{"subject":"OpenAI","predicate":"founded","object":"Anthropic"}]}"#,
        )
        .unwrap();
        assert!(without.facts[0].subject_span.is_none());
        assert!(without.facts[0].object_span.is_none());
    }

    #[test]
    fn names_are_parsed_and_a_malformed_one_is_skipped() {
        let raw = r#"{"entities":[{"local_id":"e1","name":"海洋探测器1号","type":"equipment"}],
            "facts":[],
            "names":[{"ref":"e1","name":"海探1","quote":"海洋探测器1号（简称“海探1”）"},
                     {"name":"no ref"}]}"#;
        let x = parse_response(raw).unwrap();
        assert_eq!(x.names.len(), 1);
        assert_eq!(x.names[0].entity_ref, "e1");
        assert_eq!(x.names[0].name, "海探1");
        assert_eq!(x.skipped_entities, 0, "a bad name is not a skipped entity");
    }

    #[test]
    fn the_contract_asks_for_other_names_and_forbids_descriptions() {
        let msgs = build_messages(&[], &[], &[], None, "f.txt", &[], "text");
        let system = &msgs[0].content;
        assert!(system.contains("\"names\":[{\"ref\""));
        assert!(system.contains("1b. Every other name the text gives an entity"));
    }

    #[test]
    fn parse_response_with_fence() {
        let raw = "好的，结果如下：\n```json\n{\"entities\":[{\"name\":\"张三\",\"type\":\"person\"}],\"facts\":[]}\n```";
        let e = parse_response(raw).unwrap();
        assert_eq!(e.entities.len(), 1);
        assert_eq!(e.entities[0].type_key, "person");
    }

    #[test]
    fn handles_and_fact_refs_are_optional_and_legacy_compatible() {
        let handled = parse_response(
            r#"{"entities":[{"local_id":"e1","name":"Zhang Wei","type":"person"}],
                "facts":[{"subject":"Zhang Wei","subject_ref":"e1","predicate":"leads",
                           "object":"Finance","object_ref":"e2"}]}"#,
        )
        .unwrap();
        assert_eq!(handled.entities[0].local_id.as_deref(), Some("e1"));
        assert_eq!(handled.facts[0].subject_ref.as_deref(), Some("e1"));
        assert_eq!(handled.facts[0].object_ref.as_deref(), Some("e2"));

        let legacy = parse_response(
            r#"{"entities":[{"name":"Zhang Wei","type":"person"}],
                "facts":[{"subject":"Zhang Wei","predicate":"leads","object":"Finance"}]}"#,
        )
        .unwrap();
        assert_eq!(legacy.entities[0].local_id, None);
        assert_eq!(legacy.facts[0].subject_ref, None);
        assert_eq!(legacy.facts[0].object_ref, None);
    }

    #[test]
    fn duplicate_handle_definitions_are_all_malformed() {
        let x = parse_response(
            r#"{"entities":[
                  {"local_id":"e1","name":"Zhang Wei","type":"person"},
                  {"local_id":"e1","name":"John Smith","type":"person"},
                  {"local_id":"e2","name":"Finance","type":"organization"}],
                "facts":[]}"#,
        )
        .unwrap();
        assert_eq!(x.skipped_entities, 2);
        assert_eq!(x.entities.len(), 1);
        assert_eq!(x.entities[0].local_id.as_deref(), Some("e2"));
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

    #[test]
    fn extraction_contract_explains_handle_identity_without_forcing_splits() {
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &[], "text");
        let system = &msgs[0].content;
        assert!(system.contains("\"local_id\":\"e1\""));
        assert!(system.contains("\"subject_ref\":\"e1\""));
        // #578：跟 X 有关的一群人不是 X
        assert!(system.contains("subject_span and object_span are the exact words"));
        assert!(system.contains("unique within this response"));
        assert!(system.contains("Reuse the same local_id"));
        assert!(system.contains("permanent identity is proven"));
        assert!(system.contains("A shared name alone neither proves sameness nor requires a split"));
    }

    #[test]
    fn same_name_known_entities_keep_distinct_response_handles() {
        let known = vec![
            KnownEntity {
                handle: "k1".into(),
                type_key: "person".into(),
                name: "Zhang Wei".into(),
            },
            KnownEntity {
                handle: "k2".into(),
                type_key: "person".into(),
                name: "Zhang Wei".into(),
            },
        ];
        let msgs = build_messages(&[], &[], &[], None, "a.txt", &known, "text");
        let user = &msgs[1].content;
        assert!(user.contains("k1 [person]: Zhang Wei"));
        assert!(user.contains("k2 [person]: Zhang Wei"));
        assert!(user.contains("subject_ref/object_ref"));
    }
}

#[cfg(test)]
mod a_number_is_one_number {
    use super::normalize_attr_value;
    use serde_json::json;

    /// 模型写 `65` 还是 "65%"，落下来都是同一个数——不然同一条边上会记成冲突
    #[test]
    fn a_number_is_one_number_however_it_is_written() {
        let a = normalize_attr_value("number", &json!(65)).unwrap();
        let b = normalize_attr_value("number", &json!("65%")).unwrap();
        let c = normalize_attr_value("number", &json!("65")).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a.as_f64(), Some(65.0));
    }
}
