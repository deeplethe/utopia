//! 种类词绑定到类（0044 决定 3–4 的第一步）：类型化图谱是从开放图谱按签名的绑定算出来的，
//! 而种类词——开放抽取写下的「它是个什么」——是最简单的签名：一个词对一个类，或者
//! 对不上。绑定按库缓存、按词复用，所以代价随不同的说法增长，不随文档增长。
//!
//! 对齐读来源（决定 3）：每个词带着文档里见过的拼写、这一类东西的例子（名字）和它们
//! 参与的关系短语进提示词，模型判的是「**每一个**这种东西都是这个类的实例吗」。类可以
//! 比词宽（面包房是组织），不能比词窄、也不能只是沾边（合同不是「文件」，如果类的定义
//! 说文件是一个 file）。角色词、描述词、一词两义都答 null。候选类的定义照库里的写法给
//! （中文或英文都行），答案里的键照抄。
//!
//! 回复是紧凑 JSON：`{"b": [[id, "key" | null]]}`。解析逐项宽容：坏的一对计数、不毁掉
//! 整批；键不在那一项的候选里、id 不在批里、同一个 id 的第二次都算坏；截断的回复退到
//! 最后一个完整的对。没答到的 id 就是没答到——调用方按 id 对账，缺席是「再问」，
//! 不是 null。

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use utopia_llm::ChatMessage;

use crate::{json_block, json_text, open::repair_truncated_compact};

/// 一个候选类：键照抄进答案；标签与定义是模型判断的依据，照库里的语言给
#[derive(Debug, Clone, Copy)]
pub struct ClassCandidate<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub description: &'a str,
}

/// 一个待绑定的种类词签名：词、它在文档里的样子、这一类东西的例子与短语、候选类
#[derive(Debug, Clone)]
pub struct KindWordItem<'a> {
    pub id: i64,
    /// the normalised kind word ("company")
    pub kind_word: &'a str,
    /// spellings seen in documents ("Company", "company", "公司")
    pub spellings: &'a [String],
    /// example entity names of this kind ("Brightway Builders", "Harbor Estates")
    pub examples: &'a [String],
    /// relation phrases these things are subjects of ("is based in", "was founded by")
    pub phrases: &'a [String],
    pub candidates: Vec<ClassCandidate<'a>>,
}

/// 模型对一项的裁决：Some = 那个候选的键（从候选里抄的，不是模型写的）；None = 不绑
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindWordChoice {
    pub id: i64,
    pub key: Option<String>,
}

/// 系统消息。例子是中性的（面包房、桥、镇议会），不出自任何测量语料；规则 1 的
/// 「每一个这种东西」是整条路的判据，宽可以、窄不行、沾边不行
const KIND_WORD_SYSTEM: &str = "\
You bind the words documents use for kinds of things to the classes of a knowledge base's \
ontology. Each numbered item is one kind word as the documents wrote it: the word, the \
spellings seen, some things of this kind by name, the phrases those things take part in, and \
the candidate classes, each with its key, its label and its definition. For each item, answer \
with the key of the one candidate class that every thing of this kind is an instance of, or \
null.\n\
\n\
Output exactly one JSON object and nothing else, one pair per item:\n\
{\"b\": [[12, \"organization\"], [13, null]]}\n\
The first element of a pair is the item's id; the second is the chosen key, copied exactly as \
listed under that item, or null.\n\
\n\
1. Choose a candidate only when every thing of this kind is an instance of that class by the \
class's definition. The class may be broader than the word: a bakery is an organization, a \
bridge is a structure. It is never narrower and never merely related: a contract is not a \
document when the definition says a document is a file; a town council is not a town; a \
bakery is not a building because bakeries are in buildings.\n\
2. Judge by the definition as given, not by the label or the key alone. Labels and \
definitions may be written in any language, and so may the kind word; a word in another \
script or language is the same word when it names the same kind.\n\
3. Answer null when no candidate fits; when the word names a role, a part or a description \
rather than a kind of thing (\"participants\", \"results\", \"the northern wing\"); or when the \
examples and phrases show two kinds of thing under one word (a \"bank\" that is a lender in one \
document and a riverbank in another).\n\
4. The spellings, examples and phrases are the evidence of what the word means in these \
documents: things that \"were founded by\" and \"are based in\" are organizations; things that \
\"were signed on\" and \"expire\" are agreements. Read them before the label.\n\
5. Never invent a key, never answer with a label or a word of your own, and never choose for \
an item a key that is not among its candidates. One pair per item, every item answered.";

/// 构造两条消息：常量系统消息 + 逐项的用户消息。每项：id、词、拼写、例子、短语、候选。
/// 空清单写 `(none)`，模型知道那一栏存在但这次没有证据
pub fn build_kind_word_messages(items: &[KindWordItem<'_>]) -> Vec<ChatMessage> {
    let mut user = String::new();
    for item in items {
        let candidates = if item.candidates.is_empty() {
            " (none)".to_string()
        } else {
            let lines: Vec<String> = item.candidates.iter().map(candidate_line).collect();
            format!("\n{}", lines.join("\n"))
        };
        user.push_str(&format!(
            "Item {}: \"{}\"\nSpellings: {}\nExamples: {}\nPhrases: {}\nCandidates:{candidates}\n\n",
            item.id,
            item.kind_word,
            quoted_list(item.spellings),
            quoted_list(item.examples),
            quoted_list(item.phrases),
        ));
    }
    vec![
        ChatMessage {
            role: "system".into(),
            content: KIND_WORD_SYSTEM.to_string(),
        },
        ChatMessage {
            role: "user".into(),
            content: user.trim_end().to_string(),
        },
    ]
}

/// `"a", "b", "c"`；空清单是 `(none)`。带引号是因为名字里可能有逗号
fn quoted_list(values: &[String]) -> String {
    if values.is_empty() {
        return "(none)".to_string();
    }
    values
        .iter()
        .map(|v| format!("\"{}\"", v.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `- key: label — description`。标签与键相同就不重复；定义为空就不写破折号
fn candidate_line(c: &ClassCandidate<'_>) -> String {
    let key = c.key.trim();
    let mut line = format!("- {key}");
    let label = c.label.trim();
    if !label.is_empty() && label != key {
        line.push_str(&format!(": {label}"));
    }
    let description = c.description.trim();
    if !description.is_empty() {
        line.push_str(&format!(" — {description}"));
    }
    line
}

/// 逐项宽容地解回复。返回（裁决，坏项数）。
///
/// 一对坏在：不是数组、不足两格、id 不是整数、id 不在这批里、键不是 null 也不是
/// 字符串、键不在那一项的候选里、同一个 id 已经答过（留第一次）。键先按原样对候选，
/// 对不上再不分大小写对——对上了写回的是候选自己的键，不是模型写的那个。
/// 截断的回复先退到最后一个完整的对；连一对都没有才报错
pub fn parse_kind_word_response(
    raw: &str,
    items: &[KindWordItem<'_>],
) -> anyhow::Result<(Vec<KindWordChoice>, usize)> {
    let value = parse_value(raw)?;
    let by_id: HashMap<i64, &KindWordItem<'_>> = items.iter().map(|i| (i.id, i)).collect();
    let mut choices = Vec::new();
    let mut malformed = 0usize;
    let mut seen = HashSet::new();
    for pair in pairs(&value) {
        match parse_pair(pair, &by_id) {
            Some(choice) if seen.insert(choice.id) => choices.push(choice),
            _ => malformed += 1,
        }
    }
    Ok((choices, malformed))
}

/// 先按常规取块（第一个 `{` 到最后一个 `}`）；解不开才从第一个 `{` 取到结尾去修补。
/// 取块与修补的分工同 `open.rs`：紧凑回复里 `}` 只在结尾出现，截断的回复要么没有 `}`，
/// 要么最后一个 `}` 不是结尾
pub(crate) fn parse_value(raw: &str) -> anyhow::Result<Value> {
    let block = json_block(raw)
        .and_then(|b| serde_json::from_str::<Value>(&b).map_err(anyhow::Error::from));
    match block {
        Ok(v) => Ok(v),
        Err(e) => {
            let text = json_text(raw);
            let fixed = text
                .find('{')
                .map(|s| &text[s..])
                .and_then(repair_truncated_compact)
                .ok_or_else(|| anyhow::anyhow!("Failed to parse kind-word binding JSON: {e}"))?;
            serde_json::from_str::<Value>(&fixed)
                .map_err(|e| anyhow::anyhow!("Failed to parse kind-word binding JSON: {e}"))
        }
    }
}

/// 顶层 `b` 下的数组；缺了或不是数组就当空——那不是坏项，是一项都没答
fn pairs(value: &Value) -> &[Value] {
    value
        .get("b")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// `[id, key | null]`
fn parse_pair(v: &Value, by_id: &HashMap<i64, &KindWordItem<'_>>) -> Option<KindWordChoice> {
    let arr = v.as_array()?;
    if arr.len() < 2 {
        return None;
    }
    let id = item_id(&arr[0])?;
    let item = by_id.get(&id)?;
    let key = match &arr[1] {
        Value::Null => None,
        Value::String(written) => Some(candidate_key(item, written)?),
        _ => return None,
    };
    Some(KindWordChoice { id, key })
}

/// id 是 JSON 整数；模型偶尔把它写成字符串，照数字读
pub(crate) fn item_id(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// 模型写的键对回这一项的候选：先原样，再不分大小写（只在唯一命中时）；返回的是
/// 候选自己的键。空串与不在候选里的都是 None，由调用方计入坏项
fn candidate_key(item: &KindWordItem<'_>, written: &str) -> Option<String> {
    let written = written.trim();
    if written.is_empty() {
        return None;
    }
    let keys = || item.candidates.iter().map(|c| c.key.trim());
    if let Some(exact) = keys().find(|k| *k == written) {
        return Some(exact.to_string());
    }
    let lower = written.to_lowercase();
    let mut hits = keys().filter(|k| k.to_lowercase() == lower);
    let first = hits.next()?;
    hits.next().is_none().then(|| first.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    /// 三项：公司（应绑组织）、参与者（角色词，应 null）、桥（候选里有结构）
    struct Fixture {
        company_spellings: Vec<String>,
        company_examples: Vec<String>,
        company_phrases: Vec<String>,
        participant_spellings: Vec<String>,
        bridge_spellings: Vec<String>,
        bridge_examples: Vec<String>,
        bridge_phrases: Vec<String>,
    }

    impl Fixture {
        fn new() -> Self {
            Fixture {
                company_spellings: strings(&["Company", "company", "公司"]),
                company_examples: strings(&["Brightway Builders", "Harbor Estates"]),
                company_phrases: strings(&["is based in", "was founded by"]),
                participant_spellings: strings(&["participants"]),
                bridge_spellings: strings(&["bridge", "Bridge"]),
                bridge_examples: strings(&["Harbor Bridge"]),
                bridge_phrases: strings(&["spans", "was completed"]),
            }
        }

        fn items(&self) -> Vec<KindWordItem<'_>> {
            let organization = ClassCandidate {
                key: "organization",
                label: "Organization",
                description: "An organized group of people with a shared purpose, such as a company, a council or a club.",
            };
            let person = ClassCandidate {
                key: "person",
                label: "Person",
                description: "A human being.",
            };
            let structure = ClassCandidate {
                key: "structure",
                label: "建筑物",
                description: "人建造的固定物：桥、楼、水坝。",
            };
            vec![
                KindWordItem {
                    id: 12,
                    kind_word: "company",
                    spellings: &self.company_spellings,
                    examples: &self.company_examples,
                    phrases: &self.company_phrases,
                    candidates: vec![organization, person],
                },
                KindWordItem {
                    id: 13,
                    kind_word: "participant",
                    spellings: &self.participant_spellings,
                    examples: &[],
                    phrases: &[],
                    candidates: vec![person, organization],
                },
                KindWordItem {
                    id: 14,
                    kind_word: "bridge",
                    spellings: &self.bridge_spellings,
                    examples: &self.bridge_examples,
                    phrases: &self.bridge_phrases,
                    candidates: vec![structure],
                },
            ]
        }
    }

    const FULL: &str = r#"{"b": [[12, "organization"], [13, null], [14, "structure"]]}"#;

    fn choice(id: i64, key: Option<&str>) -> KindWordChoice {
        KindWordChoice {
            id,
            key: key.map(str::to_string),
        }
    }

    #[test]
    fn a_full_reply_parses_with_keys_and_nulls() {
        let f = Fixture::new();
        let items = f.items();
        let (choices, malformed) = parse_kind_word_response(FULL, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![
                choice(12, Some("organization")),
                choice(13, None),
                choice(14, Some("structure")),
            ]
        );
    }

    /// 围栏与前后废话照旧容忍（走的是同一个 json_block）
    #[test]
    fn a_fenced_reply_parses() {
        let f = Fixture::new();
        let items = f.items();
        let raw = format!("Here you go:\n```json\n{FULL}\n```\nDone.");
        let (choices, malformed) = parse_kind_word_response(&raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(choices.len(), 3);
    }

    /// 键不在那一项的候选里就是坏项：别的项有这个候选也不算
    #[test]
    fn a_key_not_among_the_items_candidates_is_malformed() {
        let f = Fixture::new();
        let items = f.items();
        let raw = r#"{"b": [[12, "place"], [14, "organization"], [13, "person"]]}"#;
        let (choices, malformed) = parse_kind_word_response(raw, &items).unwrap();
        assert_eq!(malformed, 2);
        assert_eq!(choices, vec![choice(13, Some("person"))]);
    }

    #[test]
    fn an_unknown_id_is_malformed() {
        let f = Fixture::new();
        let items = f.items();
        let raw = r#"{"b": [[99, "organization"], [12, "organization"]]}"#;
        let (choices, malformed) = parse_kind_word_response(raw, &items).unwrap();
        assert_eq!(malformed, 1);
        assert_eq!(choices, vec![choice(12, Some("organization"))]);
    }

    /// 截在第三对的键中间：前两对留下，第三项缺席（不是 null，也不算坏）
    #[test]
    fn a_truncated_reply_is_repaired_to_the_last_complete_pair() {
        let f = Fixture::new();
        let items = f.items();
        let cut = FULL.find("\"struc").unwrap();
        let (choices, malformed) = parse_kind_word_response(&FULL[..cut], &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![choice(12, Some("organization")), choice(13, None)]
        );
    }

    /// 截在一对的 id 之后（`[14`）：退到上一个完整的 `]`
    #[test]
    fn a_reply_cut_inside_a_pair_keeps_the_pairs_before_it() {
        let f = Fixture::new();
        let items = f.items();
        let raw = r#"{"b": [[12, "organization"], [13, null], [14"#;
        let (choices, malformed) = parse_kind_word_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(choices.len(), 2);
    }

    /// 连一对都没有时仍然报失败——容错不是把空结果说成成功
    #[test]
    fn a_reply_with_nothing_complete_still_fails() {
        let f = Fixture::new();
        let items = f.items();
        assert!(parse_kind_word_response(r#"{"b": [[12, "org"#, &items).is_err());
        assert!(parse_kind_word_response("no json here", &items).is_err());
    }

    /// 没有 `b` 或不是数组：一项都没答，不是坏项
    #[test]
    fn a_reply_without_pairs_gives_no_choices() {
        let f = Fixture::new();
        let items = f.items();
        let (choices, malformed) = parse_kind_word_response(r#"{"b": {}}"#, &items).unwrap();
        assert!(choices.is_empty());
        assert_eq!(malformed, 0);
        let (choices, malformed) = parse_kind_word_response(r#"{"x": 1}"#, &items).unwrap();
        assert!(choices.is_empty());
        assert_eq!(malformed, 0);
    }

    /// 同一个 id 答了两次：留第一次，第二次计入坏项
    #[test]
    fn a_duplicate_id_keeps_the_first() {
        let f = Fixture::new();
        let items = f.items();
        let raw = r#"{"b": [[12, "organization"], [12, null], [12, "person"]]}"#;
        let (choices, malformed) = parse_kind_word_response(raw, &items).unwrap();
        assert_eq!(choices, vec![choice(12, Some("organization"))]);
        assert_eq!(malformed, 2);
    }

    /// 坏项逐个计数：不是数组、只有一格、键是数字、键是空串、id 是对象；
    /// id 写成字符串照数字读
    #[test]
    fn malformed_pairs_are_counted_not_fatal() {
        let f = Fixture::new();
        let items = f.items();
        let raw = r#"{"b": ["not a pair", [12], [13, 7], [14, ""], [{"id": 12}, null], ["12", "organization"]]}"#;
        let (choices, malformed) = parse_kind_word_response(raw, &items).unwrap();
        assert_eq!(malformed, 5);
        assert_eq!(choices, vec![choice(12, Some("organization"))]);
    }

    /// 键的大小写写错了：对回候选自己的键；首尾空白也去掉
    #[test]
    fn a_key_in_the_wrong_case_is_read_back_as_the_candidates_key() {
        let f = Fixture::new();
        let items = f.items();
        let raw = r#"{"b": [[12, "Organization"], [14, " structure "]]}"#;
        let (choices, malformed) = parse_kind_word_response(raw, &items).unwrap();
        assert_eq!(malformed, 0);
        assert_eq!(
            choices,
            vec![
                choice(12, Some("organization")),
                choice(14, Some("structure"))
            ]
        );
    }

    /// 提示词列出每一项的 id、词、拼写、例子、短语、候选键与定义，并带着「每一个
    /// 这种东西」的判据；空清单写 `(none)`
    #[test]
    fn the_prompt_lists_every_item_and_carries_the_rule() {
        let f = Fixture::new();
        let items = f.items();
        let msgs = build_kind_word_messages(&items);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        let system = &msgs[0].content;
        let user = &msgs[1].content;

        assert!(system.contains("every thing of this kind is an instance of that class"));
        assert!(system.contains("{\"b\": [[12, \"organization\"], [13, null]]}"));
        assert!(system.contains("never merely related"));
        assert!(system.contains("Never invent a key"));

        assert!(user.contains("Item 12: \"company\"\n"));
        assert!(user.contains("Spellings: \"Company\", \"company\", \"公司\"\n"));
        assert!(user.contains("Examples: \"Brightway Builders\", \"Harbor Estates\"\n"));
        assert!(user.contains("Phrases: \"is based in\", \"was founded by\"\n"));
        assert!(user.contains(
            "- organization: Organization — An organized group of people with a shared purpose"
        ));
        assert!(user.contains("- person: Person — A human being."));

        assert!(user.contains("Item 13: \"participant\"\n"));
        assert!(user.contains("Spellings: \"participants\"\nExamples: (none)\nPhrases: (none)\n"));

        assert!(user.contains("Item 14: \"bridge\"\n"));
        assert!(user.contains("Examples: \"Harbor Bridge\"\n"));
        assert!(user.contains("Phrases: \"spans\", \"was completed\"\n"));
        assert!(
            user.contains("- structure: 建筑物 — 人建造的固定物：桥、楼、水坝。"),
            "候选的标签与定义照库里的语言给: {user}"
        );

        let a = user.find("Item 12").unwrap();
        let b = user.find("Item 13").unwrap();
        let c = user.find("Item 14").unwrap();
        assert!(a < b && b < c);
        assert!(!user.ends_with('\n'));
    }

    /// 标签与键相同不重复；定义为空不写破折号；没有候选写 `(none)`
    #[test]
    fn a_candidate_line_omits_what_is_empty() {
        let f = Fixture::new();
        let spellings = strings(&["council"]);
        let mut items = f.items();
        items.truncate(1);
        items[0].candidates = vec![
            ClassCandidate {
                key: "organization",
                label: "organization",
                description: "",
            },
            ClassCandidate {
                key: "place",
                label: "",
                description: "A location.",
            },
        ];
        items.push(KindWordItem {
            id: 15,
            kind_word: "council",
            spellings: &spellings,
            examples: &[],
            phrases: &[],
            candidates: vec![],
        });
        let user = &build_kind_word_messages(&items)[1].content;
        assert!(user.contains("Candidates:\n- organization\n- place — A location.\n"));
        assert!(user.contains("Item 15: \"council\"\n"));
        assert!(user.ends_with("Candidates: (none)"));
    }

    /// 提示词的例子是中性的：不出现任何测量语料的名字
    #[test]
    fn the_prompt_names_no_benchmark_corpus() {
        let f = Fixture::new();
        let items = f.items();
        let msgs = build_kind_word_messages(&items);
        for m in &msgs {
            for name in [
                "NVIDIA",
                "Nvidia",
                "EDGAR",
                "CUAD",
                "DocRED",
                "Holmes",
                "Wikidata",
                "ai-timeline",
                "OpenAI",
                "Pfizer",
                "State of the Union",
            ] {
                assert!(!m.content.contains(name), "{name} in prompt: {}", m.content);
            }
        }
    }
}
