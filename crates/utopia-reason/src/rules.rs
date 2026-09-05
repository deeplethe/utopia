//! 业务规则：读一个实体自己的属性事实，得出一个结论（见 `docs/decisions/0021`）。
//!
//! **与 `derive()` 分开的一趟。** 那一趟走的是实体—实体的边，在公理下做闭包；
//! 这一趟做的是「拿字面值跟阈值比」。两件事的输入、判据、终止条件都不一样，
//! 塞进一个函数只会让两边都难读。
//!
//! 与那一趟共享的是**区间语义**（`derive::validity`）和「结论是派生的」这条
//! 身份：命中产出的东西照样进 `derived_facts`、照样挂前提、照样在前提消失时失效。
//!
//! 这一层同样不碰数据库。取规则、取属性事实、落库都在 `utopia-store`。

use crate::derive::validity;
use std::collections::HashMap;
use uuid::Uuid;

/// 一个实体、一条属性、一个时段上的字面值。求值器的全部输入。
#[derive(Debug, Clone, PartialEq)]
pub struct AttrFact {
    pub id: Uuid,
    pub subject: Uuid,
    /// `relation_types` 里 kind='attribute' 的那个谓词
    pub predicate: Uuid,
    /// `facts.object_value -> 'value'`，已经从外壳里取出来
    pub value: serde_json::Value,
}

/// 条件的比较方式。与 `attribute_rule_conditions.op` 的 CHECK 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Gt,
    Gte,
    Lt,
    Lte,
    Between,
    In,
    Present,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Gt => "gt",
            Op::Gte => "gte",
            Op::Lt => "lt",
            Op::Lte => "lte",
            Op::Between => "between",
            Op::In => "in",
            Op::Present => "present",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "gt" => Op::Gt,
            "gte" => Op::Gte,
            "lt" => Op::Lt,
            "lte" => Op::Lte,
            "between" => Op::Between,
            "in" => Op::In,
            "present" => Op::Present,
            _ => return None,
        })
    }
}

/// 操作数。形状由 op 决定，读的时候一次解析好，求值热路径上不再碰 JSON。
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Num(f64),
    /// 闭区间 [lo, hi]
    Range(f64, f64),
    /// 类别集合。**逐字匹配**——同一类别换个语言就不算命中，这是 0021 记下的
    /// 未决问题，不在这里偷偷做模糊
    Set(Vec<String>),
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub predicate: Uuid,
    pub op: Op,
    pub operand: Operand,
}

/// 结论。两种形状对应 `attribute_rules.conclusion` 的两支。
#[derive(Debug, Clone, PartialEq)]
pub enum Conclusion {
    /// 派生归类：结论是一个类，落成 `is_a` 上的字面值
    Typing { class: String },
    /// 派生属性：某个属性谓词上的一个字面值
    Attribute {
        predicate: Uuid,
        value: serde_json::Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BusinessRule {
    pub id: Uuid,
    pub conclusion: Conclusion,
    /// 合取：全部满足才算命中。空条件集永不命中——一条没有判据的规则应当
    /// 什么都不推，而不是把整个类都归进去
    pub conditions: Vec<Condition>,
}

/// 一次命中。前提是**真正让它成立的那几条事实**，不是这个实体的全部属性。
#[derive(Debug, Clone, PartialEq)]
pub struct RuleHit {
    pub rule: Uuid,
    pub subject: Uuid,
    pub premises: Vec<Uuid>,
    pub from: Option<i64>,
    pub to: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleReport {
    /// 参与求值的规则数
    pub rules: usize,
    /// 命中数（去重后）
    pub hits: usize,
    /// 因组合数超上限而没有展开完的 (规则, 实体) 对数。**必须报出来**：
    /// 少推几条与「这个实体不满足」在结果里长得一模一样
    pub capped: usize,
}

/// 一个 (规则, 实体) 对最多展开多少种前提组合。
///
/// 组合数是各条件命中集大小的乘积：一口井的同一项读数报过三次、三个条件，
/// 就是 27 种。真实数据里每项通常只有一两条，这个上限只在「同一属性被反复
/// 覆盖几十次」时才够得着，而那种时候多算出来的区间也早就没有阅读价值了。
const MAX_COMBOS: usize = 64;

/// 求值。
///
/// `facts` 是**已经限定在这条规则的主类之内**的属性事实——挑哪些实体参与是
/// 取数那一侧的事（子类展开要查本体），这一层只管判断。
///
/// 语义：每个条件各自挑出满足它的事实集合，再在「每个条件取一条」的组合上
/// 求区间交集，交集非空即一次命中。**所以同一条规则可以在同一个实体上产出
/// 多段区间**——2023 那次读数命中、2025 那次不命中，得到的是两段各自成立的
/// 结论，而不是一行翻来覆去改（0021 决策 4）。
pub fn evaluate(
    rules: &[BusinessRule],
    facts: &[AttrFact],
    spans: &HashMap<Uuid, (Option<i64>, Option<i64>)>,
) -> (Vec<RuleHit>, RuleReport) {
    let mut report = RuleReport {
        rules: rules.len(),
        ..Default::default()
    };
    let mut hits: Vec<RuleHit> = Vec::new();

    // 按实体分组：规则谈的是「一个实体自己的属性」，跨实体不参与
    let mut by_subject: HashMap<Uuid, Vec<&AttrFact>> = HashMap::new();
    for f in facts {
        by_subject.entry(f.subject).or_default().push(f);
    }

    for rule in rules {
        if rule.conditions.is_empty() {
            continue;
        }
        for (subject, own) in &by_subject {
            // 每个条件的命中集。任何一个为空，这条规则在这个实体上就不成立
            let mut per_condition: Vec<Vec<Uuid>> = Vec::with_capacity(rule.conditions.len());
            let mut satisfiable = true;
            for c in &rule.conditions {
                let matched: Vec<Uuid> = own
                    .iter()
                    .filter(|f| f.predicate == c.predicate && satisfies(c, &f.value))
                    .map(|f| f.id)
                    .collect();
                if matched.is_empty() {
                    satisfiable = false;
                    break;
                }
                per_condition.push(matched);
            }
            if !satisfiable {
                continue;
            }

            let combos: usize = per_condition.iter().map(|v| v.len()).product();
            if combos > MAX_COMBOS {
                report.capped += 1;
                continue;
            }

            // 笛卡尔积。同一区间可能由多个组合得出（同一读数报了两遍），
            // 按区间去重——它们本来就会落成同一行
            let mut seen: Vec<(Option<i64>, Option<i64>)> = Vec::new();
            for combo in cartesian(&per_condition) {
                let Some((from, to)) = validity(&combo, spans) else {
                    continue;
                };
                if seen.contains(&(from, to)) {
                    continue;
                }
                seen.push((from, to));
                hits.push(RuleHit {
                    rule: rule.id,
                    subject: *subject,
                    premises: combo,
                    from,
                    to,
                });
            }
        }
    }
    report.hits = hits.len();
    (hits, report)
}

/// 每个条件取一条，穷举组合。调用方已经把上限挡在外面。
fn cartesian(sets: &[Vec<Uuid>]) -> Vec<Vec<Uuid>> {
    let mut out: Vec<Vec<Uuid>> = vec![Vec::new()];
    for set in sets {
        let mut next = Vec::with_capacity(out.len() * set.len());
        for prefix in &out {
            for id in set {
                let mut row = prefix.clone();
                row.push(*id);
                next.push(row);
            }
        }
        out = next;
    }
    out
}

/// 一条属性值满不满足一个条件。
///
/// **类型不对就是不满足，不是报错。** 一个本该是数字的属性被抽成了
/// "十二点三"，这条规则在这个实体上不成立——而不是让整轮物化失败。
fn satisfies(c: &Condition, value: &serde_json::Value) -> bool {
    match (&c.op, &c.operand) {
        (Op::Present, _) => !value.is_null(),
        (Op::Gt, Operand::Num(n)) => num(value).is_some_and(|v| v > *n),
        (Op::Gte, Operand::Num(n)) => num(value).is_some_and(|v| v >= *n),
        (Op::Lt, Operand::Num(n)) => num(value).is_some_and(|v| v < *n),
        (Op::Lte, Operand::Num(n)) => num(value).is_some_and(|v| v <= *n),
        (Op::Between, Operand::Range(lo, hi)) => num(value).is_some_and(|v| v >= *lo && v <= *hi),
        (Op::In, Operand::Set(set)) => text(value).is_some_and(|s| set.iter().any(|x| x == &s)),
        // op 与操作数形状对不上：库里的 CHECK 挡了大部分，剩下的当不满足
        _ => false,
    }
}

/// 数字。**字符串里的数字也认**——抽取把「12.3」存成字符串是常有的事，
/// 而一个阈值规则因为引号而静默失效，是最难被发现的那种失效。
fn num(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        serde_json::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// 类别。数字与布尔也转成字面形态参与集合比较，理由同上。
fn text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.trim().to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn fact(fid: u8, subject: u8, pred: u8, value: serde_json::Value) -> AttrFact {
        AttrFact {
            id: id(fid),
            subject: id(subject),
            predicate: id(pred),
            value,
        }
    }

    /// 全烃 12.3 且解释为气测异常 → 命中，前提正是那两条读数
    #[test]
    fn a_conjunction_fires_and_names_the_two_readings() {
        let rule = BusinessRule {
            id: id(90),
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![
                Condition {
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(8.0),
                },
                Condition {
                    predicate: id(11),
                    op: Op::In,
                    operand: Operand::Set(vec!["气测异常".into(), "气测异常后效".into()]),
                },
            ],
        };
        let facts = vec![
            fact(1, 50, 10, json!(12.3)),
            fact(2, 50, 11, json!("气测异常")),
        ];
        let spans = HashMap::from([(id(1), (Some(100), None)), (id(2), (Some(100), None))]);
        let (hits, report) = evaluate(&[rule], &facts, &spans);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, id(50));
        assert_eq!(hits[0].premises, vec![id(1), id(2)]);
        assert_eq!(hits[0].from, Some(100));
        assert_eq!(report.hits, 1);
    }

    /// 合取里少一条就不成立。**这是最容易写反的地方**：任一条件没有命中的
    /// 事实，整条规则在这个实体上就不成立，而不是「按剩下的条件算」
    #[test]
    fn a_missing_condition_fires_nothing() {
        let rule = BusinessRule {
            id: id(90),
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![
                Condition {
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(8.0),
                },
                Condition {
                    predicate: id(11),
                    op: Op::Present,
                    operand: Operand::None,
                },
            ],
        };
        let facts = vec![fact(1, 50, 10, json!(12.3))];
        let spans = HashMap::from([(id(1), (Some(100), None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans);
        assert!(hits.is_empty(), "第二个条件没有任何事实，不该命中");
    }

    /// 同一属性的两次读数各自成立 → 两段区间，而不是一行翻转（0021 决策 4）
    #[test]
    fn two_readings_give_two_intervals() {
        let rule = BusinessRule {
            id: id(90),
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![Condition {
                predicate: id(10),
                op: Op::Gt,
                operand: Operand::Num(8.0),
            }],
        };
        // 2023 与 2025 两次读数都过阈值，区间不相交
        let facts = vec![fact(1, 50, 10, json!(12.3)), fact(2, 50, 10, json!(9.9))];
        let spans = HashMap::from([
            (id(1), (Some(100), Some(200))),
            (id(2), (Some(300), Some(400))),
        ]);
        let (hits, _) = evaluate(&[rule], &facts, &spans);
        assert_eq!(hits.len(), 2, "两次读数各自成立");
        let mut spans_out: Vec<_> = hits.iter().map(|h| (h.from, h.to)).collect();
        spans_out.sort();
        assert_eq!(
            spans_out,
            vec![(Some(100), Some(200)), (Some(300), Some(400))]
        );
    }

    /// 区间不相交的两条前提凑不成一次命中：全烃是 2023 的，解释是 2025 的，
    /// 这两条从来没有同时成立过
    #[test]
    fn premises_that_never_overlapped_fire_nothing() {
        let rule = BusinessRule {
            id: id(90),
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![
                Condition {
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(8.0),
                },
                Condition {
                    predicate: id(11),
                    op: Op::In,
                    operand: Operand::Set(vec!["气测异常".into()]),
                },
            ],
        };
        let facts = vec![
            fact(1, 50, 10, json!(12.3)),
            fact(2, 50, 11, json!("气测异常")),
        ];
        let spans = HashMap::from([
            (id(1), (Some(100), Some(200))),
            (id(2), (Some(300), Some(400))),
        ]);
        let (hits, _) = evaluate(&[rule], &facts, &spans);
        assert!(hits.is_empty(), "两条前提没有同时成立的时段");
    }

    /// 数字被抽成字符串照样比得动。**这一条挡的是最难发现的失效**：
    /// 规则从不报错，只是永远不命中
    #[test]
    fn a_number_in_quotes_still_compares() {
        let rule = BusinessRule {
            id: id(90),
            conclusion: Conclusion::Attribute {
                predicate: id(20),
                value: json!("good"),
            },
            conditions: vec![Condition {
                predicate: id(10),
                op: Op::Gte,
                operand: Operand::Num(12.0),
            }],
        };
        let facts = vec![fact(1, 50, 10, json!(" 12.3 "))];
        let spans = HashMap::from([(id(1), (None, None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans);
        assert_eq!(hits.len(), 1);
    }

    /// 阈值抬高到读数之上，同一份数据就不再命中——降/升阈值重跑是 0021 的验收项
    #[test]
    fn raising_the_threshold_clears_the_hit() {
        let facts = vec![fact(1, 50, 10, json!(12.3))];
        let spans = HashMap::from([(id(1), (None, None))]);
        let mk = |threshold: f64| BusinessRule {
            id: id(90),
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![Condition {
                predicate: id(10),
                op: Op::Gt,
                operand: Operand::Num(threshold),
            }],
        };
        assert_eq!(evaluate(&[mk(8.0)], &facts, &spans).0.len(), 1);
        assert!(evaluate(&[mk(20.0)], &facts, &spans).0.is_empty());
    }

    /// 没有条件的规则什么都不推。空合取在逻辑上恒真，会把整个类归进去——
    /// 那是「规则还没写完」最坏的失败方式
    #[test]
    fn a_rule_without_conditions_concludes_nothing() {
        let rule = BusinessRule {
            id: id(90),
            conclusion: Conclusion::Typing { class: "X".into() },
            conditions: vec![],
        };
        let facts = vec![fact(1, 50, 10, json!(12.3))];
        let spans = HashMap::from([(id(1), (None, None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans);
        assert!(hits.is_empty());
    }

    /// 组合数超上限时**报出来**，不是静默少推
    #[test]
    fn too_many_combinations_are_reported() {
        let rule = BusinessRule {
            id: id(90),
            conclusion: Conclusion::Typing { class: "X".into() },
            conditions: vec![
                Condition {
                    predicate: id(10),
                    op: Op::Present,
                    operand: Operand::None,
                },
                Condition {
                    predicate: id(11),
                    op: Op::Present,
                    operand: Operand::None,
                },
            ],
        };
        let mut facts = Vec::new();
        let mut spans = HashMap::new();
        for i in 0..10u8 {
            facts.push(fact(i, 50, 10, json!(1)));
            facts.push(fact(i + 100, 50, 11, json!(1)));
            spans.insert(id(i), (None, None));
            spans.insert(id(i + 100), (None, None));
        }
        let (hits, report) = evaluate(&[rule], &facts, &spans);
        assert_eq!(report.capped, 1, "100 种组合超过上限，要计数");
        assert!(hits.is_empty());
    }
}
