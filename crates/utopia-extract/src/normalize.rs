//! 模型回复落库前的**形状检查**：只看结构，不看词。
//!
//! **分工。**读懂原文里的时间、判断一段话是不是一个东西——这是语言问题，归模型，
//! 契约（提示词 3c）说清楚它该怎么写。这里只核对输出有没有照契约的形状写，判据一律
//! 是结构性的：引文里有没有这段字、值是不是只有标点、一侧是不是契约的日期格式、
//! 同一句里有没有另一条边。**不认任何一种语言的词**——第一版按英文词表认「季度」
//!「N months ended」，中文财报一条都认不出，还把四份报告的标题当成期间删了。
//!
//! 每条规则做了什么都返回给服务端记进丢弃表：违约多常见、出在哪个模型，量得出来，
//! 契约该怎么改看数说话。

use crate::{parse_time, ExtractedFact, Extraction};
use std::collections::HashSet;

/// 形状检查做了什么；服务端按条记信号
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Normalization {
    /// 值只有破折号（`—`）或是空的：表里的「无」，不是一个值，不落。**只认破折号**：
    /// `☒`、`✓` 这类符号没有字母数字，却是一格写着的内容（勾选了），照值落
    NoValue { predicate: String, written: String },
    /// 值后面有一截引文里没有的字：只留引文里有的那段。模型读对了表头、却把期间
    /// 写进了值（`(6,176) for three months ended July 27, 2025`），引文只有 `(6,176)`
    ValueTrimmed {
        predicate: String,
        kept: String,
        dropped: String,
    },
    /// 没有宾语、没有值、只带边属性：每个属性落成主语上的一条值事实。
    /// 从前整条以 object_missing 丢掉，写对了的数跟着没了
    QualifiersWithoutObject { predicate: String, values: usize },
    /// 宾语是契约格式的日期（`2026-06-30`）：时间不是实体。边上的数落成值、日期进有效期；
    /// 没带数的，把写出来的那段落成值——`2028`（「2028 年起上线」）、`4000`（人数）都
    /// 解析得成年份，丢掉就把一条信息整个丢了
    TimeAsObject {
        predicate: String,
        written: String,
        values: usize,
    },
    /// 主语是契约格式的日期：数是某个东西在那一刻的数，那个东西是谁回复里没说。不落
    TimeAsSubject { predicate: String, written: String },
    /// 宾语的名字包住了另一个声明实体，而同一句、同主语、同谓词已有一条指向那个实体的边：
    /// 它**可能**是那个实体的描述。**只记，不删**：「non-GAAP net income, or earnings, per
    /// diluted share」包住了「non-GAAP net income」，却是另一个指标（每股收益）。结构上
    /// 分不出描述与另一个东西，删错了就是实体连事实一起没了
    ObjectDescribesDeclared {
        predicate: String,
        name: String,
        head: String,
    },
    /// 上面几条去掉事实之后，没有任何事实再引用的声明：不建，否则就是一个孤点
    OrphanDeclaration { name: String },
}

/// 比对用的形态：空白折叠、小写
fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 比对引文用的词元：一段连续的数字（中间的 `,` `.` 算在数里，`13,237`、`89.0`），或者
/// 一段连续的非数字字母。其余字符都是分隔。
///
/// **按词元比，不按子串比**：从前 `10 to 15 GW` 对着「10–15 GW」，`10` 作为子串在引文
/// 里，尾巴 `to 15 GW` 作为整段不在，于是被剪成 `10`。数字与汉字之间也切开——中文里数
/// 贴着字写（「营收为12,345元」），不切的话一个数永远对不上
fn tokens(t: &str) -> Vec<String> {
    let chars: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_digit = false;
    for (i, &c) in chars.iter().enumerate() {
        let digit = c.is_numeric();
        let joins_number =
            (c == ',' || c == '.') && cur_digit && chars.get(i + 1).is_some_and(|n| n.is_numeric());
        if joins_number {
            cur.push(c);
            continue;
        }
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if !cur.is_empty() && digit != cur_digit {
            out.push(std::mem::take(&mut cur));
        }
        cur_digit = digit;
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `needle` 的词元是否作为连续的一段出现在 `hay` 的词元里
fn contains_tokens(hay: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn has_digit(t: &str) -> bool {
    t.chars().any(char::is_numeric)
}

/// 值后面是否挂着一截不属于它的字。返回 (保留的前缀, 去掉的尾巴)。
///
/// 两个条件同时成立才剪，都是结构，不认词，比的都是整词元（见 [`tokens`]）：
/// - **保留的那段在引文里，而且是一格的写法**——含数字、词元连续地出现在引文里，是原文
///   写的那个数；或者只有破折号（`—`），是原文那一格写的「没有」；
/// - **尾巴自己含数字，而且那些数一个都不在引文里**——它是另一条信息（一个期间、一个
///   日期、另一个百分比），不是这个数的单位。尾巴里有一个数在引文里，就说明它是原文的
///   一部分换了写法（`10 to 15 GW` 对着「10–15 GW」），不剪。
///
/// 第二条要数字，是因为挂在数后面、引文里又没有的，还有一类是对的：表头上的量级与
/// 单位（`53,954 million USD`，这一行引文只有 `53,954`，`million` 在表头「in millions」）、
/// 模型换了写法的单位（`10 gigawatts`）、缩写的头衔（`founder and CEO`）。它们不带数字，
/// 不剪。整个值在引文里的，一个字不动。
fn ungrounded_tail<'a>(value: &'a str, quote: &str) -> Option<(&'a str, &'a str)> {
    let q = norm(quote);
    if q.is_empty() || q.contains(&norm(value)) {
        return None;
    }
    let quote_tokens = tokens(quote);
    let foreign_figures = |tail: &str| {
        let numbers: Vec<String> = tokens(tail).into_iter().filter(|w| has_digit(w)).collect();
        !numbers.is_empty() && numbers.iter().all(|w| !quote_tokens.contains(w))
    };
    let bounds: Vec<usize> = value
        .char_indices()
        .filter(|(i, c)| c.is_whitespace() && *i > 0)
        .map(|(i, _)| i)
        .collect();
    bounds
        .iter()
        .rev()
        .map(|&i| {
            (
                value[..i]
                    .trim()
                    .trim_end_matches([',', ';', ':', '\u{3001}', '\u{FF0C}']),
                value[i..].trim(),
            )
        })
        .find(|(p, r)| {
            if p.is_empty() || r.is_empty() || !foreign_figures(r) {
                return false;
            }
            if is_no_value(p) {
                return q.contains(&norm(p));
            }
            has_digit(p) && contains_tokens(&quote_tokens, &tokens(p))
        })
}

/// 表格里表示「没有」的那一格：空的，或者只有破折号。
///
/// **只认破折号（Unicode 的 Pd 类）**，不是「没有字母数字就算」：`☒`、`✓`、`☐` 也没有
/// 字母数字，却是那一格真写着的内容，从前被当成空格子丢了
fn is_no_value(written: &str) -> bool {
    written.chars().filter(|c| !c.is_whitespace()).all(|c| {
        matches!(
            c,
            '-' | '\u{058A}' | '\u{05BE}' | '\u{1400}' | '\u{1806}' | '\u{2010}'
                ..='\u{2015}'
                    | '\u{2E17}'
                    | '\u{2E1A}'
                    | '\u{2E3A}'
                    | '\u{2E3B}'
                    | '\u{2E40}'
                    | '\u{301C}'
                    | '\u{3030}'
                    | '\u{30A0}'
                    | '\u{FE31}'
                    | '\u{FE32}'
                    | '\u{FE58}'
                    | '\u{FE63}'
                    | '\u{FF0D}'
        )
    })
}

/// 边属性里不是值、是单位的那几个键（与服务端同一张表）
fn is_unit_key(k: &str) -> bool {
    matches!(
        k.trim().to_lowercase().as_str(),
        "currency" | "币种" | "货币" | "unit" | "单位"
    )
}

fn qualifier_values(f: &ExtractedFact) -> Vec<(String, serde_json::Value)> {
    f.qualifiers
        .as_ref()
        .map(|q| {
            q.iter()
                .filter(|(k, v)| !v.is_null() && !is_unit_key(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn value_fact(
    f: &ExtractedFact,
    predicate: String,
    value: serde_json::Value,
    valid_from: Option<String>,
) -> ExtractedFact {
    ExtractedFact {
        subject: f.subject.clone(),
        subject_ref: f.subject_ref.clone(),
        predicate,
        object: None,
        object_ref: None,
        value: Some(value),
        qualifiers: None,
        valid_from: valid_from.or_else(|| f.valid_from.clone()),
        valid_to: f.valid_to.clone(),
        confidence: f.confidence,
        quote: f.quote.clone(),
        subject_span: f.subject_span.clone(),
        object_span: None,
    }
}

/// 一侧写的是契约格式的时间（`YYYY` / `YYYY-MM` / `YYYY-MM-DD`，带时区的时刻）
fn is_contract_time(s: &str) -> bool {
    parse_time(s.trim()).is_some()
}

pub fn normalize_facts(x: &mut Extraction) -> Vec<Normalization> {
    let mut out = Vec::new();
    let entities = std::mem::take(&mut x.entities);

    // 一侧绑到的声明名：有句柄按句柄，没有按写出来的名字
    let handle_name = |h: Option<&String>| {
        h.and_then(|h| {
            entities
                .iter()
                .find(|e| e.local_id.as_deref().map(str::trim) == Some(h.trim()))
                .map(|e| e.name.trim().to_string())
        })
    };
    let referenced = |facts: &[ExtractedFact]| -> HashSet<String> {
        facts
            .iter()
            .flat_map(|f| {
                [
                    handle_name(f.subject_ref.as_ref())
                        .or_else(|| Some(f.subject.trim().to_string())),
                    handle_name(f.object_ref.as_ref())
                        .or_else(|| f.object.as_deref().map(|o| o.trim().to_string())),
                ]
            })
            .flatten()
            .map(|n| n.to_lowercase())
            .collect()
    };
    let before = referenced(&x.facts);

    let mut facts: Vec<ExtractedFact> = Vec::with_capacity(x.facts.len());
    for mut f in std::mem::take(&mut x.facts) {
        let quote = f.quote.clone().unwrap_or_default();

        // ---- 值 ----
        if let Some(written) = f.value.as_ref().and_then(|v| v.as_str()).map(str::to_owned) {
            // 先剪再看空：`— for three months ended July 27, 2025` 剪掉尾巴才露出那一格是空的
            let figure = ungrounded_tail(&written, &quote).map_or(written.as_str(), |(k, _)| k);
            if is_no_value(figure) {
                out.push(Normalization::NoValue {
                    predicate: f.predicate.clone(),
                    written,
                });
                continue;
            }
            if let Some((kept, dropped)) = ungrounded_tail(&written, &quote) {
                out.push(Normalization::ValueTrimmed {
                    predicate: f.predicate.clone(),
                    kept: kept.to_string(),
                    dropped: dropped.to_string(),
                });
                f.value = Some(serde_json::Value::String(kept.to_string()));
            }
        }

        // ---- 主语是时间 ----
        let subject =
            handle_name(f.subject_ref.as_ref()).unwrap_or_else(|| f.subject.trim().to_string());
        if is_contract_time(&subject) {
            out.push(Normalization::TimeAsSubject {
                predicate: f.predicate.clone(),
                written: subject,
            });
            continue;
        }

        let values = qualifier_values(&f);
        let object = handle_name(f.object_ref.as_ref())
            .or_else(|| f.object.as_deref().map(|o| o.trim().to_string()))
            .filter(|o| !o.is_empty());
        let has_value = f.value.as_ref().is_some_and(|v| !v.is_null());

        // ---- 只有边属性 ----
        if object.is_none() && !has_value && !values.is_empty() {
            for (key, value) in &values {
                let predicate = format!("{}.{}", f.predicate.trim(), key.trim());
                facts.push(value_fact(&f, predicate, value.clone(), None));
            }
            out.push(Normalization::QualifiersWithoutObject {
                predicate: f.predicate.clone(),
                values: values.len(),
            });
            continue;
        }

        // ---- 宾语是时间 ----
        if let Some(o) = object.as_deref().filter(|o| is_contract_time(o)) {
            if values.is_empty() {
                // 没带数：写出来的那段就是值。`2028`（「2028 年起上线」）、`4000`（人数）都解析
                // 得成年份，从前整条丢掉，一条信息就没了。宾语那个声明没人引用，下面按孤点去掉
                facts.push(value_fact(
                    &f,
                    f.predicate.trim().to_string(),
                    serde_json::Value::String(o.to_string()),
                    None,
                ));
                out.push(Normalization::TimeAsObject {
                    predicate: f.predicate.clone(),
                    written: o.to_string(),
                    values: 0,
                });
                continue;
            }
            let several = values.len() > 1;
            for (key, value) in &values {
                let predicate = if several {
                    format!("{}.{}", f.predicate.trim(), key.trim())
                } else {
                    f.predicate.trim().to_string()
                };
                facts.push(value_fact(
                    &f,
                    predicate,
                    value.clone(),
                    Some(o.to_string()),
                ));
            }
            out.push(Normalization::TimeAsObject {
                predicate: f.predicate.clone(),
                written: o.to_string(),
                values: values.len(),
            });
            continue;
        }

        facts.push(f);
    }

    // ---- 描述：名字包住另一个声明实体，本尊那条边同句已在。只记，不删 ----
    let declared: Vec<String> = entities.iter().map(|e| e.name.trim().to_string()).collect();
    let side = |r: Option<&String>, w: Option<&str>| {
        handle_name(r)
            .or_else(|| w.map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
    };
    let objects: Vec<Option<String>> = facts
        .iter()
        .map(|f| side(f.object_ref.as_ref(), f.object.as_deref()))
        .collect();
    let subjects: Vec<Option<String>> = facts
        .iter()
        .map(|f| side(f.subject_ref.as_ref(), Some(f.subject.as_str())))
        .collect();
    // 名字边界：拉丁字母数字前后不能粘着字母数字；汉字之间本来就没有空格，不设边界
    let contains_name = |outer: &str, inner: &str| {
        let (o, i) = (norm(outer), norm(inner));
        !i.is_empty()
            && o.len() > i.len()
            && o.match_indices(&i).any(|(at, _)| {
                let glued = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
                let edge_in = i.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
                let edge_out = i
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric());
                !(edge_in && glued(o[..at].chars().next_back()))
                    && !(edge_out && glued(o[at + i.len()..].chars().next()))
            })
    };
    for i in 0..facts.len() {
        let Some(name) = objects[i].as_deref() else {
            continue;
        };
        let quote_i = norm(facts[i].quote.as_deref().unwrap_or(""));
        let head = declared.iter().find(|h| {
            contains_name(name, h)
                && (0..facts.len()).any(|j| {
                    j != i
                        && facts[j].predicate.eq_ignore_ascii_case(&facts[i].predicate)
                        && subjects[j] == subjects[i]
                        && objects[j]
                            .as_deref()
                            .is_some_and(|o| o.eq_ignore_ascii_case(h))
                        && {
                            let quote_j = norm(facts[j].quote.as_deref().unwrap_or(""));
                            quote_i.contains(&quote_j) || quote_j.contains(&quote_i)
                        }
                })
        });
        if let Some(head) = head {
            // 不删（见枚举上的说明）：结构分不出「SB Energy 的发展」与「每股收益」这类
            // 包住了另一个名字的真指标。记下来，量得出这种形状多常见、有多少是描述
            out.push(Normalization::ObjectDescribesDeclared {
                predicate: facts[i].predicate.clone(),
                name: name.to_string(),
                head: head.clone(),
            });
        }
    }
    x.facts = facts;

    // ---- 被上面几条弄成孤点的声明 ----
    // 只去掉「原来有事实引用、现在没有了」的：模型一开始就只声明不连边的，不归这里管
    let after = referenced(&x.facts);
    let mut orphans = Vec::new();
    let mut kept_entities = entities;
    kept_entities.retain(|e| {
        let n = e.name.trim().to_lowercase();
        let orphan = before.contains(&n) && !after.contains(&n);
        if orphan {
            orphans.push(e.name.trim().to_string());
        }
        !orphan
    });
    x.entities = kept_entities;
    out.extend(
        orphans
            .into_iter()
            .map(|name| Normalization::OrphanDeclaration { name }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExtractedEntity;

    fn fact(subject: &str, predicate: &str, object: Option<&str>, quote: &str) -> ExtractedFact {
        ExtractedFact {
            subject: subject.into(),
            subject_ref: None,
            predicate: predicate.into(),
            object: object.map(str::to_string),
            object_ref: None,
            value: None,
            qualifiers: None,
            valid_from: None,
            valid_to: None,
            confidence: Some(0.9),
            quote: Some(quote.into()),
            subject_span: Some(subject.into()),
            object_span: object.map(str::to_string),
        }
    }
    fn valued(subject: &str, predicate: &str, value: &str, quote: &str) -> ExtractedFact {
        let mut f = fact(subject, predicate, None, quote);
        f.value = Some(serde_json::Value::String(value.into()));
        f
    }
    fn entity(id: &str, name: &str) -> ExtractedEntity {
        ExtractedEntity {
            local_id: Some(id.into()),
            name: name.into(),
            type_key: "organization".into(),
            specific_type: None,
        }
    }
    fn quals(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect()
    }
    fn run(
        entities: Vec<ExtractedEntity>,
        facts: Vec<ExtractedFact>,
    ) -> (Extraction, Vec<Normalization>) {
        let mut x = Extraction {
            entities,
            facts,
            skipped_entities: 0,
            skipped_facts: 0,
            truncated: false,
        };
        let n = normalize_facts(&mut x);
        (x, n)
    }
    fn value_of(f: &ExtractedFact) -> &str {
        f.value.as_ref().unwrap().as_str().unwrap()
    }

    /// 截图里那 21 条：模型读对了表头，却把期间写进了值。引文里只有数
    #[test]
    fn a_tail_the_quote_does_not_contain_is_not_the_value() {
        let (x, n) = run(
            vec![entity("e1", "NVIDIA")],
            vec![
                valued(
                    "NVIDIA",
                    "net_cash_used",
                    "(6,176) for three months ended July 27, 2025",
                    "Net cash used in financing activities (6,176)",
                ),
                // 同样的形状，中文：不认词，照样剪
                valued(
                    "NVIDIA",
                    "经营现金流",
                    "12,345 截至2025年7月27日止三个月",
                    "经营活动产生的现金流量净额 | 12,345",
                ),
                valued(
                    "NVIDIA",
                    "revenue",
                    "$89.0 billion, up 18% from the previous quarter",
                    "Second-quarter revenue was $89.0 billion",
                ),
            ],
        );
        assert_eq!(value_of(&x.facts[0]), "(6,176)");
        assert_eq!(value_of(&x.facts[1]), "12,345");
        // 数后面接着另一个数：「up 18%」是另一条事实，不是这个数的一部分；逗号跟着前缀走
        assert_eq!(value_of(&x.facts[2]), "$89.0 billion");
        assert!(
            matches!(&n[0], Normalization::ValueTrimmed { dropped, .. } if dropped == "for three months ended July 27, 2025")
        );
    }

    #[test]
    fn a_value_grounded_in_the_quote_is_left_alone() {
        let (x, n) = run(
            vec![entity("e1", "Tench Coxe")],
            vec![
                // 整个值在引文里
                valued(
                    "NVIDIA",
                    "operating_expenses",
                    "$9.2 billion",
                    "expected to be approximately $9.2 billion and $9.0 billion",
                ),
                // 尾巴 shares 在引文别处出现过：原文的单位，不剪
                valued(
                    "Tench Coxe",
                    "votes_for",
                    "15,411,252,412 shares",
                    "Number of shares For | 15,411,252,412",
                ),
                // 规范化过的值，前缀一个词都对不上：不是这条规则管的
                valued("Vega", "amount", "$5 billion", "invested 5 billion dollars"),
                // 表头上的量级与币种：引文那一行只有数，尾巴不带数字，是它的单位，不剪
                valued(
                    "NVIDIA",
                    "net_income",
                    "53,954 million USD",
                    "Net income | $ | 53,954",
                ),
                // 模型换了写法的单位、缩写的头衔：不带数字，不剪
                valued(
                    "SB Energy",
                    "capacity",
                    "10 gigawatts",
                    "at least 10 GW of new generation",
                ),
                valued(
                    "Jensen Huang",
                    "job_title",
                    "founder and CEO",
                    "Jensen Huang, founder and chief executive officer",
                ),
            ],
        );
        let got: Vec<&str> = x.facts.iter().map(value_of).collect();
        assert_eq!(
            got,
            [
                "$9.2 billion",
                "15,411,252,412 shares",
                "$5 billion",
                "53,954 million USD",
                "10 gigawatts",
                "founder and CEO"
            ]
        );
        assert!(n.is_empty(), "{n:?}");
    }

    #[test]
    fn a_dash_is_no_value() {
        let (x, n) = run(
            vec![entity("e1", "NVIDIA")],
            vec![
                valued("NVIDIA", "dividend", "—", "Dividends | —"),
                // 勾选框是那一格写着的内容，不是空格子：没有字母数字，照值落
                valued(
                    "NVIDIA",
                    "large_accelerated_filer",
                    "☒",
                    "Large accelerated filer | ☒",
                ),
                valued(
                    "NVIDIA",
                    "emerging_growth_company",
                    "☐",
                    "Emerging growth company | ☐",
                ),
                valued("NVIDIA", "dividend", " – ", "Dividends | –"),
                valued("NVIDIA", "dividend", "$0.01", "Dividends | $0.01"),
                // 空的那一格后面挂着列头上的期间：剪掉尾巴，剩下的仍是空
                valued(
                    "NVIDIA",
                    "amount",
                    "— for three months ended July 27, 2025",
                    "Purchases of marketable securities | — | (6,176)",
                ),
                // 反例：前缀只有标点、在引文里，可尾巴上的数也在引文里——那个数才是值
                valued(
                    "NVIDIA",
                    "net_income",
                    "$ 53,954 million",
                    "Net income | $ | 53,954",
                ),
            ],
        );
        let kept: Vec<&str> = x.facts.iter().map(value_of).collect();
        assert_eq!(kept, ["☒", "☐", "$0.01", "$ 53,954 million"]);
        assert_eq!(
            n.iter()
                .filter(|v| matches!(v, Normalization::NoValue { .. }))
                .count(),
            3
        );
    }

    /// Coxe 的形状：`vote_result` 带着数，没有宾语也没有值
    #[test]
    fn figures_on_an_edge_with_no_other_end_land_on_the_subject() {
        let mut f = fact(
            "Tench Coxe",
            "vote_result",
            None,
            "Number of shares For | 15,411,252,412",
        );
        f.qualifiers = Some(quals(&[
            ("for", "15,411,252,412"),
            ("against", "1,399,727,580"),
            ("unit", "shares"),
        ]));
        let (x, n) = run(vec![entity("e1", "Tench Coxe")], vec![f]);
        let mut got: Vec<(String, String)> = x
            .facts
            .iter()
            .map(|f| (f.predicate.clone(), value_of(f).to_string()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                (
                    "vote_result.against".to_string(),
                    "1,399,727,580".to_string()
                ),
                ("vote_result.for".to_string(), "15,411,252,412".to_string()),
            ]
        );
        assert_eq!(
            n,
            vec![Normalization::QualifiersWithoutObject {
                predicate: "vote_result".into(),
                values: 2
            }]
        );
    }

    /// 时间做了宾语：边上的数落成值，时间进有效期，那个时间节点不建
    #[test]
    fn a_time_object_becomes_the_validity_and_leaves_no_node() {
        let mut margin = fact(
            "NVIDIA",
            "gross_margin",
            Some("2026-06"),
            "Gross margin | 75.0%",
        );
        margin.object_ref = Some("e7".into());
        margin.qualifiers = Some(quals(&[("percentage", "75.0%")]));
        let mut bare = fact("NVIDIA", "reported_in", Some("2026"), "reported in 2026");
        bare.object_ref = Some("e8".into());
        let (x, n) = run(
            vec![
                entity("e1", "NVIDIA"),
                entity("e7", "2026-06"),
                entity("e8", "2026"),
            ],
            vec![margin, bare],
        );
        // 带数的：数落成值、日期进有效期；没带数的：写出来的那段落成值，不丢
        assert_eq!(x.facts.len(), 2);
        assert_eq!(x.facts[0].predicate, "gross_margin");
        assert_eq!(value_of(&x.facts[0]), "75.0%");
        assert_eq!(x.facts[0].valid_from.as_deref(), Some("2026-06"));
        assert_eq!(
            (x.facts[1].predicate.as_str(), value_of(&x.facts[1])),
            ("reported_in", "2026")
        );
        assert!(x.facts[1].object.is_none() && x.facts[1].object_ref.is_none());
        let names: Vec<&str> = x.entities.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["NVIDIA"]);
        assert_eq!(
            n.iter()
                .filter(|v| matches!(v, Normalization::OrphanDeclaration { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn a_time_subject_is_not_placed() {
        let f = valued(
            "2026-07-26",
            "revenue",
            "$89.0 billion",
            "revenue was $89.0 billion",
        );
        let (x, n) = run(vec![entity("e1", "NVIDIA")], vec![f]);
        assert!(x.facts.is_empty());
        assert!(matches!(&n[0], Normalization::TimeAsSubject { .. }));
    }

    /// 不是契约格式的期间名（`Q2 FY27`、`第二季度`）不归这里判：那是模型的事，契约 3c 管
    #[test]
    fn a_period_name_that_is_not_a_date_is_not_second_guessed() {
        let mut f = fact(
            "NVIDIA",
            "gross_margin_for_period",
            Some("Q2 FY27"),
            "Gross margin | 75.0 | %",
        );
        f.qualifiers = Some(quals(&[("percentage", "75.0%")]));
        let (x, n) = run(
            vec![entity("e1", "NVIDIA"), entity("e7", "Q2 FY27")],
            vec![f],
        );
        assert_eq!(x.facts.len(), 1);
        assert_eq!(x.entities.len(), 2);
        assert!(n.is_empty());
    }

    /// SB Energy 那句：名字包住了另一个声明实体，同句同主语同谓词已有一条指向它的边。
    /// 记一条信号，事实与声明都留着——结构分不出它与下面「每股收益」那种真指标
    #[test]
    fn a_description_beside_its_head_is_flagged_not_removed() {
        let quote = "NVIDIA to invest $1.5B in SB Energy now to support SB Energy\u{2019}s growth and commitments to the Ohio community";
        let mut head = fact("NVIDIA", "invested_in", Some("SB Energy"), quote);
        head.object_ref = Some("e2".into());
        let mut desc = fact(
            "NVIDIA",
            "invested_in",
            Some("SB Energy's growth and commitments to the Ohio community"),
            quote,
        );
        desc.object_ref = Some("e6".into());
        let (x, n) = run(
            vec![
                entity("e1", "NVIDIA"),
                entity("e2", "SB Energy"),
                entity(
                    "e6",
                    "SB Energy's growth and commitments to the Ohio community",
                ),
            ],
            vec![head, desc],
        );
        // 只记不删：两条都在，那个声明也在（它仍被引用）
        assert_eq!(x.facts.len(), 2);
        assert_eq!(x.entities.len(), 3);
        assert!(n.iter().any(
            |v| matches!(v, Normalization::ObjectDescribesDeclared { head, .. } if head == "SB Energy")
        ));
    }

    /// 同样的结构，中文：不靠 's，照样认得出这个形状（只记）
    #[test]
    fn a_description_is_recognised_without_a_possessive() {
        let quote = "英伟达向星辰能源投资15亿美元，支持星辰能源在俄亥俄的发展";
        let (x, n) = run(
            vec![
                entity("e1", "英伟达"),
                entity("e2", "星辰能源"),
                entity("e3", "星辰能源在俄亥俄的发展"),
            ],
            vec![
                fact("英伟达", "投资", Some("星辰能源"), quote),
                fact("英伟达", "投资", Some("星辰能源在俄亥俄的发展"), quote),
            ],
        );
        assert_eq!(x.facts.len(), 2);
        assert_eq!(x.entities.len(), 3);
        assert!(n
            .iter()
            .any(|v| matches!(v, Normalization::ObjectDescribesDeclared { .. })));
    }

    /// **四处误伤，逐个钉住**（#637 实测）：每一处从前都把一条真信息丢了或剪坏了
    #[test]
    fn what_the_shape_checks_used_to_break_is_kept() {
        // 一、短数字被当子串匹配：`10 to 15 GW` 对着「10–15 GW」，从前剪成 `10`
        let (x, n) = run(
            vec![entity("e1", "SB Energy")],
            vec![
                valued(
                    "SB Energy",
                    "planned_capacity",
                    "10 to 15 GW",
                    "SB Energy plans 10–15 GW of new generation",
                ),
                // 词元不是子串：`5` 不在「25 GW」里
                valued(
                    "SB Energy",
                    "planned_capacity",
                    "5 GW by 2030",
                    "SB Energy plans 25 GW by 2030",
                ),
                // 中文里数贴着字写，照样按词元对得上，尾巴那个日期不在引文里，剪
                valued(
                    "SB Energy",
                    "营收",
                    "12,345 截至2025年7月27日止三个月",
                    "营收为12,345元",
                ),
            ],
        );
        let got: Vec<&str> = x.facts.iter().map(value_of).collect();
        assert_eq!(got, ["10 to 15 GW", "5 GW by 2030", "12,345"]);
        assert_eq!(n.len(), 1, "{n:?}");

        // 二、勾选框：见 a_dash_is_no_value

        // 三、四位数的宾语：`2028`、`4000` 解析得成年份，落成值，不丢
        let mut launch = fact(
            "Aurora",
            "launch_year",
            Some("2028"),
            "Aurora goes live from 2028",
        );
        launch.object_ref = Some("e2".into());
        let mut staff = fact(
            "Acme",
            "employees",
            Some("4000"),
            "Acme employs 4000 people",
        );
        staff.object_ref = Some("e3".into());
        let (x, _) = run(
            vec![
                entity("e1", "Aurora"),
                entity("e2", "2028"),
                entity("e3", "4000"),
                entity("e4", "Acme"),
            ],
            vec![launch, staff],
        );
        let got: Vec<(&str, &str)> = x
            .facts
            .iter()
            .map(|f| (f.predicate.as_str(), value_of(f)))
            .collect();
        assert_eq!(got, [("launch_year", "2028"), ("employees", "4000")]);
        let names: Vec<&str> = x.entities.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Aurora", "Acme"], "年份那两个声明不建成节点");

        // 四、包住了另一个指标名的真指标：每股收益不是净利润的描述，留着
        let quote = "non-GAAP net income was $26.4 billion and non-GAAP net income, or earnings, per diluted share was $1.05";
        let (x, n) = run(
            vec![
                entity("e1", "NVIDIA"),
                entity("e2", "non-GAAP net income"),
                entity("e3", "non-GAAP net income, or earnings, per diluted share"),
            ],
            vec![
                fact(
                    "NVIDIA",
                    "reported_metric",
                    Some("non-GAAP net income"),
                    quote,
                ),
                fact(
                    "NVIDIA",
                    "reported_metric",
                    Some("non-GAAP net income, or earnings, per diluted share"),
                    quote,
                ),
            ],
        );
        assert_eq!(x.facts.len(), 2);
        assert_eq!(x.entities.len(), 3);
        assert!(n
            .iter()
            .any(|v| matches!(v, Normalization::ObjectDescribesDeclared { .. })));
    }

    /// 包住了别的名字、但旁边没有指向本尊的同一条边：可能真是一个东西，不动
    #[test]
    fn a_name_that_contains_another_without_a_sibling_edge_is_left_alone() {
        let quote = "Sam Altman was removed by OpenAI's board of directors";
        let (x, n) = run(
            vec![
                entity("e1", "Sam Altman"),
                entity("e2", "OpenAI"),
                entity("e3", "OpenAI's board of directors"),
            ],
            vec![fact(
                "Sam Altman",
                "removed_by",
                Some("OpenAI's board of directors"),
                quote,
            )],
        );
        assert_eq!(x.facts.len(), 1);
        assert_eq!(x.entities.len(), 3);
        assert!(n.is_empty());
    }
}
