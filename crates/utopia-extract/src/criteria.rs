//! 来自语料的判据——独立的抽取路径（#507 cut 4 / 0064 cut 3）。
//!
//! 这一条路**不**替代 [`crate::open`]。`open` 把一段话读成一组事实：东西
//! 与陈述。事实挂在一个实体身上。判据不是事实——它说的是「只要 X 满足 Y
//! 条件，就归入 Z」，这是一个**关于一类东西的规则**，没有具体的事主。
//!
//! 两条路共用同一份原文（同一分块），但用不同的提示词与不同的输出形状。
//! 接线由调用方决定：何时跑、跑一次还是两次。这一档只负责：**给一段话，
//! 返回结构化的判据候选**。落到 `attribute_rules`、写 provenance、走
//! Review 队列——都是另一档的事（cut 5）。
//!
//! ## 输出形状
//!
//! 每个判据是一句**全称条件**："凡是某类的、属性取某值时，归入某类"。要
//! 满足三个条件才能算：
//!
//!  1. 句子说的是一类（well / bridge / device / etc.），不是某一个具名实体
//!  2. 句子说的是**条件**：带「当 / 如果 / 若 / whenever / if」之类
//!  3. 结论是把主语归入一个类，或给主语一个属性取值
//!
//! 三个里只要有一条不通，这一档**跳过**——抽不出比错抽好。
//!
//! ## 词汇不解析时
//!
//! 「全烃」是哪一类属性？本档不答。模型按原文写的写下来，服务端
//! 决定怎么映射。**不**因为词不认识就拒收——0064 d3 决定「hold 不
//! refuse」，所以即便 `predicate` 是原文措辞，`op` 与 `value` 是
//! 原文数字，服务端照样能把这一档提交为提案。Review 卡片上要看见
//! 原文，这是 0064 d4。
//!
//! ## 模型回复不达标时
//!
//! 跟 [`crate::open`] 同款——宽容解析：一条坏记录计入
//! [`CriteriaExtraction::skipped`]，调用方看到这一档写一条「这一档
//! 有 N 条被跳过」的告警，而不是让整块死掉。

use crate::ChatMessage;
use serde::{Deserialize, Serialize};

/// 单条判据候选。模型按原文写下来，**不**做本体映射。
///
/// 6 字段按数组位置固定，输出数组形式同 [`crate::open`]：
/// `[quote, subject_class, predicate, op, value, conclude_class, conclude_kind]`
///
/// - `quote`：原句一字不改。Review 卡片要拿它与结构化字段并排展示
///   （0064 d4），所以引文是契约的一部分，不是装饰。
/// - `subject_class`：原句写的事主类（"well"、"桥"、「各区县」）。
///   **不**做 ID 化——这一档不存在「井 W-1」这种具名事主。
/// - `predicate`：原句写的属性措辞（"全烃"、"长度"）。可能在本档里
///   找不到对应属性——0064 决定把这样的提案连同原措辞一起 hold。
/// - `op`：原句写的比较（`>`, `<`, `=`, `>=`, `<=`, `in`, `between`）。
///   数字比较用 `gt`/`lt`/`eq`/`gte`/`lte` 之一。集合或区间用 `in` /
///   `between`。**不是**这一档的责任去推断——写了什么就记什么。
/// - `value`：原句写的字面值，**不**做单位换算。
/// - `conclude_class`：结论归入的类（"优秀井"、"可投运"），或结论属性
///   名（"评价"）。
/// - `conclude_kind`：`"class"`（归入一个类）或 `"attribute"`（算
///   一个属性）。这一档不混着用两条结论——参见 [issue
///   #507](https://github.com/deeplethe/utopia/issues/507) 的 cut 4
///   范围。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCriterion {
    pub quote: String,
    pub subject_class: String,
    pub predicate: String,
    pub op: String,
    pub value: String,
    pub conclude_class: String,
    pub conclude_kind: String,
}

impl OpenCriterion {
    /// 6 字段数组 → 结构体。空串视同「未填」并视作缺字段，跳过记
    /// [`CriteriaExtraction::skipped`]。
    pub fn from_array(arr: &[serde_json::Value]) -> Option<Self> {
        if arr.len() < 6 {
            return None;
        }
        let s = |v: &serde_json::Value| v.as_str().map(str::to_string).unwrap_or_default();
        let c = OpenCriterion {
            quote: s(&arr[0]).trim().to_string(),
            subject_class: s(&arr[1]).trim().to_string(),
            predicate: s(&arr[2]).trim().to_string(),
            op: s(&arr[3]).trim().to_string(),
            value: s(&arr[4]).trim().to_string(),
            conclude_class: s(&arr[5]).trim().to_string(),
            // 第 7 位如有「是否为属性结论」标注就收；没有默认 "class"
            conclude_kind: arr
                .get(6)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("class")
                .to_string(),
        };
        if c.quote.is_empty()
            || c.subject_class.is_empty()
            || c.predicate.is_empty()
            || c.op.is_empty()
            || c.value.is_empty()
            || c.conclude_class.is_empty()
        {
            return None;
        }
        Some(c)
    }
}

/// 模型的回复，原文不动 + 解析后的 `criteria`。
///
/// 跟 [`crate::open::OpenExtraction`] 同款：宽容解析，跳过的计
/// [`Self::skipped`]；截断由调用方看 [`Self::truncated`]。
#[derive(Debug, Default, Clone)]
pub struct CriteriaExtraction {
    pub criteria: Vec<OpenCriterion>,
    pub skipped: usize,
    pub truncated: bool,
}

/// 解析 `{"c": [...]}` 的回复。`c` 缺省为空数组。条目数错误、或字
/// 段类型不对的计入 `skipped`，**不**让整块死掉。
///
/// 顶端 `{"c": [...]}` 是契约；若顶层不是对象、或对象不含 `c`，按
/// 「抽不出」处理——这一档不是事实，不存在「拒绝」一说。
pub fn parse_criteria_response(raw: &str) -> anyhow::Result<CriteriaExtraction> {
    let mut out = CriteriaExtraction::default();
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| anyhow::anyhow!("criteria reply is not JSON: {e}"))?;
    let Some(obj) = value.as_object() else {
        // 不是对象——可能是模型把这一档漏了；不算错
        return Ok(out);
    };
    let Some(arr) = obj.get("c").and_then(|v| v.as_array()) else {
        return Ok(out);
    };
    for item in arr {
        match item {
            serde_json::Value::Array(arr) => match OpenCriterion::from_array(arr) {
                Some(c) => out.criteria.push(c),
                None => out.skipped += 1,
            },
            _ => out.skipped += 1,
        }
    }
    Ok(out)
}

/// 系统消息——`criteria` 这一档的提示词。
///
/// 与 [`crate::open`] 的提示词**不**共用。原因：
///   - 这一档要的是全称条件，不是关于具体实体的陈述
///   - 这一档输出 `c`，而 `open` 输出 `e / s / n`；混在一起模型会
///     试着把判据塞进 `s` 的宾语槽（issue #507 描述的「W-1 是优秀
///     井」风险）
///
/// 写法紧贴 [`OPEN_SYSTEM`](crate::open::OPEN_SYSTEM) 的语气——短句
/// 编号、例子居中、用引用体。这是这一档里测过能用 prompt 形状。
const CRITERIA_SYSTEM: &str = "\
You read one passage of a document and write down every criterion it states in the form \
\"whenever a [class] has [attribute] [op] [value], the [class] is [conclusion]\". \
A criterion is a universally-quantified rule about a class, not a fact about any one named \
thing. Output one JSON object and nothing else, shaped like this:\n\
\n\
{\"c\": [[\"the sentence that states it, verbatim\", \"subject class\", \"attribute as written\", \"op\", \"value as written\", \"conclusion class\", \"class\"]]}\n\
\n\
1. c lists the criteria, one entry each. Every entry is a single conditional: a class, one \
attribute predicate, one comparison, one value, one conclusion. The array shape is fixed: \
[quote, subject_class, predicate, op, value, conclude_class, conclude_kind]. quote comes \
first: the sentence of the passage that states the criterion, copied verbatim, one sentence. \
The rest of the entry is written from that sentence.\n\
\n\
2. A criterion's subject is a class, written as the passage writes it (\"well\", \"bridge\", \
\"各区县\"), never a specific named thing (\"W-1\", \"Harbor Bridge\"). A statement about a \
named thing is a fact and does not belong here, even if it mentions a comparison. \"W-1's \
total hydrocarbon was 8.1\" is a fact; \"whenever a well's total hydrocarbon is above 8, the \
well is a good well\" is a criterion.\n\
\n\
3. The attribute predicate and the value are written as the passage writes them. Do not \
translate them into any vocabulary of your own. If the passage says \"全烃\" it stays \"全烃\"; \
if it says \">= 8 %\" the op is \"gte\" and the value is \"8 %\". A unit the passage gives goes \
in the value as written, not parsed.\n\
\n\
4. op is one of: gt, lt, eq, gte, lte, ne, in, between. in names a set (\"in {oil, gas}\"); \
between names a closed interval (\"between 3 and 5\"). For a value the passage gives as a \
range, op is between and value carries both bounds.\n\
\n\
5. conclude_kind is \"class\" if the criterion puts the subject into a class (\"good well\", \
\"operable\"), or \"attribute\" if it reads off an attribute value (\"评价: 高\"). One criterion, \
one conclusion. A sentence that says two things at once is two criteria.\n\
\n\
6. Skip what does not fit. A narrative (\"when they drilled to 3000 m they found shows\") is \
a fact, not a criterion. A sentence with no comparison (\"every well has a name\") is a \
schema observation and does not belong here. An instruction (\"the operator should check the \
seal\") is not a criterion and does not belong here.\n\
\n\
If the passage states no criterion, output {\"c\":[]}. If you cannot be sure, skip the entry \
rather than guess.";

/// 构造 `criteria` 这一档的两条消息。形状与
/// [`crate::open::build_open_messages`] 平行：常量系统 + 用户一段。
///
/// `known` 与 `opening` 故意**不**收——这一档不关心本体里的实体；
/// 判据的事主是一类东西，词表里列的实体与它无关。给模型喂已知实体
/// 反而会把它推向「这是一个已知实体」那条路，把判据错塞成事实。
pub fn build_criteria_messages(filename: &str, chunk_text: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage {
            role: "system".into(),
            content: CRITERIA_SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user".into(),
            content: format!("Document: {filename}\nPassage:\n{chunk_text}"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_criterion_with_one_condition_round_trips_through_array() {
        let raw = r#"{"c": [
            ["当全烃大于 8 且解释结论为气测异常时，可判定为优秀井。",
             "well", "全烃", "gt", "8", "优秀井", "class"]
        ]}"#;
        let x = parse_criteria_response(raw).unwrap();
        // 这一档目前只支持单条件——多条件的那句也只收第一档条件
        // 的字面值，把「全烃大于 8」收下，「气测异常」放在 quote
        // 里留给 Review 卡片。
        assert_eq!(x.criteria.len(), 1);
        assert_eq!(x.criteria[0].subject_class, "well");
        assert_eq!(x.criteria[0].predicate, "全烃");
        assert_eq!(x.criteria[0].op, "gt");
        assert_eq!(x.criteria[0].value, "8");
        assert_eq!(x.criteria[0].conclude_class, "优秀井");
        assert_eq!(x.criteria[0].conclude_kind, "class");
        assert_eq!(x.skipped, 0);
    }

    #[test]
    fn no_criteria_field_is_empty_not_an_error() {
        let raw = r#"{"e": [], "s": [], "n": []}"#;
        let x = parse_criteria_response(raw).unwrap();
        assert!(x.criteria.is_empty());
        assert_eq!(x.skipped, 0);
    }

    #[test]
    fn a_malformed_entry_is_counted_not_fatal() {
        // 第二条是对象，不是数组——跳过计 1
        let raw = r#"{"c": [
            ["Q", "well", "全烃", "gt", "8", "优秀井", "class"],
            {"quote": "bad"}
        ]}"#;
        let x = parse_criteria_response(raw).unwrap();
        assert_eq!(x.criteria.len(), 1);
        assert_eq!(x.skipped, 1);
    }

    #[test]
    fn an_empty_quote_is_skipped() {
        let raw = r#"{"c": [
            ["", "well", "全烃", "gt", "8", "优秀井", "class"]
        ]}"#;
        let x = parse_criteria_response(raw).unwrap();
        assert!(x.criteria.is_empty());
        assert_eq!(x.skipped, 1);
    }

    #[test]
    fn op_can_be_a_set_or_an_interval() {
        let raw = r#"{"c": [
            ["产层属于 {oil, gas} 时，可判定为有效储层。",
             "reservoir", "产层", "in", "{oil, gas}", "有效储层", "class"],
            ["孔隙度介于 3% 与 5% 之间时，可判定为低孔。",
             "rock", "孔隙度", "between", "3% to 5%", "低孔", "class"]
        ]}"#;
        let x = parse_criteria_response(raw).unwrap();
        assert_eq!(x.criteria.len(), 2);
        assert_eq!(x.criteria[0].op, "in");
        assert_eq!(x.criteria[1].op, "between");
    }

    #[test]
    fn conclude_kind_can_be_attribute() {
        let raw = r#"{"c": [
            ["凡评价为优秀的，归档为 A。",
             "well", "评价", "eq", "优秀", "归档", "attribute"]
        ]}"#;
        let x = parse_criteria_response(raw).unwrap();
        assert_eq!(x.criteria[0].conclude_kind, "attribute");
    }

    #[test]
    fn unknown_top_level_shape_is_an_empty_extraction_not_a_panic() {
        let raw = r#"[1, 2, 3]"#;
        let x = parse_criteria_response(raw).unwrap();
        assert!(x.criteria.is_empty());
    }

    #[test]
    fn c_wrong_shape_yields_an_empty_extraction_not_a_panic() {
        // `c` 是字符串而非数组：宽容解析把这一档视同无 `c` 字段，
        // 不报错——这是契约「能解就解、不能解就空」的一部分
        let raw = r#"{"c":"not an array"}"#;
        let x = parse_criteria_response(raw).unwrap();
        assert!(x.criteria.is_empty());
        assert_eq!(x.skipped, 0);
    }

    #[test]
    fn malformed_json_is_reported_as_an_error() {
        let raw = r#"this is not JSON"#;
        let err = parse_criteria_response(raw).unwrap_err();
        assert!(err.to_string().contains("criteria reply is not JSON"));
    }

    #[test]
    fn build_criteria_messages_carries_chunk_text_but_no_known_entities() {
        // 这一档故意不收 `known` 与 `opening`——把判据当作一类东西读，
        // 不被已知实体的列表牵着走
        let messages = build_criteria_messages("report.txt", "凡是评价为优秀的，归档为 A。");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(
            messages[0].content.contains("\"c\":"),
            "system message must advertise the c array"
        );
        assert!(
            !messages[0].content.contains("known"),
            "system message must not mention known entities"
        );
        assert_eq!(messages[1].role, "user");
        assert!(messages[1].content.contains("report.txt"));
        assert!(
            messages[1].content.contains("凡是评价为优秀的"),
            "user message must carry the chunk text"
        );
    }

    #[test]
    fn a_realistic_multi_criterion_reply_parses_in_order() {
        // 模拟一段油气勘探报告里常见的判据密度
        let raw = r#"{"c": [
            ["全烃大于 8 且解释结论为气测异常时，可判定为优秀井。",
             "well", "全烃", "gt", "8", "优秀井", "class"],
            ["电阻率大于等于 10 Ω·m 的储层，归为 I 类储层。",
             "reservoir", "电阻率", "gte", "10 Ω·m", "I 类储层", "class"],
            ["孔隙度介于 3% 与 5% 之间时，可判定为低孔。",
             "rock", "孔隙度", "between", "3% to 5%", "低孔", "class"],
            ["凡是评价为优秀的，归档为 A。",
             "well", "评价", "eq", "优秀", "归档", "attribute"]
        ]}"#;
        let x = parse_criteria_response(raw).unwrap();
        assert_eq!(x.criteria.len(), 4);
        assert_eq!(x.skipped, 0);
        // 顺序保留——0064 没有规定排序，但 Review 卡片按原文先后读更自然
        assert_eq!(x.criteria[0].predicate, "全烃");
        assert_eq!(x.criteria[3].conclude_kind, "attribute");
    }
}
