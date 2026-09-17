//! utopia-extract：抽取用的提示词与解析。
//!
//! 开放抽取（[`open`]）是唯一的抽取契约（0044 决定 2）：模型用文档自己的话写陈述，
//! 不读本体、不算日期。这里留的是它借用的零件（已知实体的句柄、文件开头的预算、
//! JSON 取块与补括号）、实体消解的攒批裁决，以及量与时间的读法——本体属性的采纳、
//! 工具参数、连接器都用它们。

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::Deserialize;
use utopia_llm::ChatMessage;

pub mod governor;
pub mod open;
pub mod time;

/// Response-scoped reference to a persistent entity; database UUIDs must never enter prompts.
pub struct KnownEntity {
    pub handle: String,
    pub type_key: String,
    pub name: String,
}

/// 文件开头进提示词的字符预算。一份补充协议的标题、生效日、当事方和「修订的是哪份
/// 协议」通常在头一千字符里；新闻稿的电头与导语也是
pub(crate) const OPENING_BUDGET_CHARS: usize = 1500;

/// 文件开头排版成提示词里的一段。开头为空（或只有空白）时返回空串；
/// 「这一块就是开头本身」由调用方判断，那时它传 `None`。
///
/// 按字符截：不会截断一个字符，但会截在词中间——英文的最后一个词可能只剩半个
pub(crate) fn opening_block(opening: Option<&str>) -> String {
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

/// 已在本文档中出现过的实体，放进提示词的字符预算。
///
/// 超出就截断（保留先出现的）。中文商业文本先出全称、主角先出场，所以
/// **首次出现顺序天然偏向那些后面会被简称的名字**。
pub(crate) const KNOWN_BUDGET_CHARS: usize = 1200;

/// 回复里可能是 JSON 的那段文字：切掉思考过程与代码围栏。前后的废话留给调用方按
/// 括号定位——[`json_block`] 取第一个 `{` 到最后一个 `}`；开放抽取的截断修补则从
/// 第一个 `{` 取到结尾，那边的记录是数组，最后一个 `}` 不是可靠的结尾
pub(crate) fn json_text(raw: &str) -> &str {
    let text = raw.trim();
    // 推理模型的思考过程（#690）：`LlmClient::chat` 那边会先切，但取块这一层
    // 自己认得标记才是最后的保障——`chat_tools` 那条路就不经过 `chat`。
    // 不切的话，思考过程里的大括号会把下面"第一个 `{`"的起点提前，
    // 而修补截断的逻辑认不出夹在中间的废话，整块直接作废。
    let text = match text.rfind("</think>") {
        Some(pos) => text[pos + "</think>".len()..].trim(),
        None => text,
    };
    text.strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .map(|s| s.trim_end_matches("```"))
        .unwrap_or(text)
}

/// 从 LLM 回复中稳健地取出 JSON 块（容忍代码围栏与前后废话）。
pub fn json_block(raw: &str) -> anyhow::Result<String> {
    let cleaned = json_text(raw);
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
pub(crate) fn close_brackets(head: &str) -> Option<String> {
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
pub(crate) fn currency_unit(tok: &str) -> Option<&'static str> {
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
mod tests {
    use super::*;

    /// #690：思考过程里的大括号不能把 JSON 的起止带偏。不切掉标记，"第一个 `{`" 的起点
    /// 会提前到思考过程里；代码围栏与前后的废话同样不算数
    #[test]
    fn json_block_ignores_a_think_block_and_a_fence_before_the_json() {
        let raw = "先想想 {\"a\": 1，再回答。\n</think>{\"entities\":[{\"name\":\"张三\"}]}";
        assert_eq!(
            json_block(raw).unwrap(),
            "{\"entities\":[{\"name\":\"张三\"}]}"
        );
        let fenced = "好的，结果如下：\n```json\n{\"entities\":[]}\n```";
        assert_eq!(json_block(fenced).unwrap(), "{\"entities\":[]}");
        assert!(json_block("no json here").is_err());
    }

    /// 括号出现在字符串里不算结构——`"a[b"` 不是一个开括号；断在字符串中间的那一截不可用
    #[test]
    fn brackets_inside_strings_are_not_structure() {
        assert_eq!(
            close_brackets(r#"{"s": [{"a": "a[b{c"}"#).as_deref(),
            Some(r#"{"s": [{"a": "a[b{c"}]}"#)
        );
        assert!(close_brackets(r#"{"s": "cut in the mid"#).is_none());
        assert!(close_brackets("]").is_none());
    }

    #[test]
    fn a_long_opening_is_cut_on_a_character_boundary() {
        let long = "租".repeat(OPENING_BUDGET_CHARS + 10);
        let block = opening_block(Some(&long));
        assert_eq!(block.matches('租').count(), OPENING_BUDGET_CHARS);
        assert!(block.contains(" …"));
        assert_eq!(opening_block(Some("   ")), "");
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
