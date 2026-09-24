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

/// 一条参与规则连接的实体—实体边。
#[derive(Debug, Clone, PartialEq)]
pub struct RuleEdge {
    pub id: Uuid,
    pub predicate: Uuid,
    pub subject: Uuid,
    pub object: Uuid,
}

/// 连接边的两侧。`X` 是规则的主语；`Y` 是这条边把它连到的另一个实体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    X,
    Y,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::X => "x",
            Side::Y => "y",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "x" | "X" => Side::X,
            "y" | "Y" => Side::Y,
            _ => return None,
        })
    }
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
    /// 反过来的 In。**它判的仍然是一条存在的事实**，所以照样有前提、有区间；
    /// 「压根没有这个属性」是另一件事，不在这套里（0029 末节）
    NotIn,
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
            Op::NotIn => "not_in",
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
            "not_in" => Op::NotIn,
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
    /// 算出来的数（0032）：拿这个实体别的读数算一个门槛，而不是写死一个
    Calc(Expr),
    None,
}

/// 四则。**只有这四个**：再多一个就得回答它对缺读数、对除零、对单位各是什么
/// 语义，而那是一门语言的开头（0002 / 0032）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
}

impl Arith {
    pub fn as_str(self) -> &'static str {
        match self {
            Arith::Add => "add",
            Arith::Sub => "sub",
            Arith::Mul => "mul",
            Arith::Div => "div",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "add" => Arith::Add,
            "sub" => Arith::Sub,
            "mul" => Arith::Mul,
            "div" => Arith::Div,
            _ => return None,
        })
    }
}

/// 一个数是怎么算出来的（0032）。叶子要么是这个实体的一条属性读数，要么是
/// 写死的数；中间是四则。
///
/// **树，不是字符串。** 存下来的就是这棵树，界面照着它渲染，求值照着它算——
/// 两者不会漂移，而这正是 0002 拒掉「用户自定义规则语言」时真正在守的东西。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// 这个实体在这个谓词上的读数
    Attr(Uuid),
    Const(f64),
    Arith {
        op: Arith,
        l: Box<Expr>,
        r: Box<Expr>,
    },
}

/// 一棵算式最深几层。太深的树是「有人在这里写程序」的信号，编译时就该拦下
pub const MAX_EXPR_DEPTH: usize = 4;

impl Expr {
    /// 这棵树读了哪些谓词，按出现顺序、去重。求值前要为它们各留一个槽位
    pub fn predicates(&self, out: &mut Vec<Uuid>) {
        match self {
            Expr::Attr(p) => {
                if !out.contains(p) {
                    out.push(*p);
                }
            }
            Expr::Const(_) => {}
            Expr::Arith { l, r, .. } => {
                l.predicates(out);
                r.predicates(out);
            }
        }
    }

    pub fn depth(&self) -> usize {
        match self {
            Expr::Attr(_) | Expr::Const(_) => 1,
            Expr::Arith { l, r, .. } => 1 + l.depth().max(r.depth()),
        }
    }

    /// 按这一轮选中的读数算一个数。
    ///
    /// **算不出来就是 None，不是 0**：读数不在（没记 ≠ 零）、值不是数、除零，
    /// 三种情况一律没有值，于是这条规则在这个组合上不出结论（0032）。把它们
    /// 当零，等于在无知的地方填一个确定的答案。
    pub fn eval(&self, bound: &HashMap<Uuid, &AttrFact>) -> Option<f64> {
        Some(match self {
            Expr::Attr(p) => num(&bound.get(p)?.value)?,
            Expr::Const(n) => *n,
            Expr::Arith { op, l, r } => {
                let (a, b) = (l.eval(bound)?, r.eval(bound)?);
                match op {
                    Arith::Add => a + b,
                    Arith::Sub => a - b,
                    Arith::Mul => a * b,
                    Arith::Div => {
                        if b == 0.0 {
                            return None;
                        }
                        a / b
                    }
                }
            }
        })
        .filter(|n: &f64| n.is_finite())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    /// 同组的条件用「与」连，组与组之间用「或」连（0029）。老规则全是第 0 组
    pub group: i32,
    /// 这一条件读连接的哪一侧。没有连接的老规则读的永远是 X
    pub side: Side,
    pub predicate: Uuid,
    pub op: Op,
    pub operand: Operand,
}

/// 结论。两种形状对应 `attribute_rules.conclusion` 的两支。
#[derive(Debug, Clone, PartialEq)]
pub enum Conclusion {
    /// 派生归类：结论是一个类，落成 `is_a` 上的字面值
    Typing { class: String },
    /// 派生属性：算出来的数（0032）。与上面那支的区别只在值从哪来——
    /// 算式读的那几条读数**一并进前提**，所以结论仍然说得出「凭什么」
    Computed { predicate: Uuid, expr: Expr },
    /// 派生属性：某个属性谓词上的一个字面值
    Attribute {
        predicate: Uuid,
        value: serde_json::Value,
    },
    /// 派生关系：从 X 经规则声明的连接指向 Y（0047）
    Relation { predicate: Uuid },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BusinessRule {
    pub id: Uuid,
    pub conclusion: Conclusion,
    /// 把 X 与 Y 连起来的那条谓词。`None` 就是老的单实体规则
    pub join_predicate: Option<Uuid>,
    /// 条件。**组内合取、组间析取**（0029）：同一组里全部满足才算这一组成立，
    /// 任何一组成立这条规则就命中。空条件集永不命中——一条没有判据的规则应当
    /// 什么都不推，而不是把整个类都归进去
    pub conditions: Vec<Condition>,
}

/// 一次命中。前提是**真正让它成立的那几条事实**，不是这个实体的全部属性。
#[derive(Debug, Clone, PartialEq)]
pub struct RuleHit {
    pub rule: Uuid,
    pub subject: Uuid,
    /// 关系结论的宾语；其他结论没有实体宾语
    pub object: Option<Uuid>,
    pub premises: Vec<Uuid>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    /// 算出来的结论值（0032）。**每个组合各算各的**——两条 revenue 三条 cost
    /// 就是六个数、六段区间、六行结论；常量结论这里是 None
    pub value: Option<f64>,
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
///
/// **组内与、组间或**（0029）：上面那一段是一组的算法，整条规则就是把它按组
/// 跑几遍。封顶按组算——一组展不开不该拖累另一组；两组推出同一区间时按区间
/// 去重，留先到的那组当证明（多条证明路径对读的人没有区别，全存下来只会让
/// 证明树跟着路径数长）。
pub fn evaluate(
    rules: &[BusinessRule],
    facts: &[AttrFact],
    spans: &HashMap<Uuid, (Option<i64>, Option<i64>)>,
    edges: &[RuleEdge],
) -> (Vec<RuleHit>, RuleReport) {
    evaluate_with_pool(rules, facts, facts, spans, edges)
}

/// Evaluate rules with the facts available to a joined `Y` supplied separately.
///
/// `facts` stays scoped to the conclusion's subject type. A one-hop join can
/// reach another entity type, so its conditions read from `pool_facts`.
pub fn evaluate_with_pool(
    rules: &[BusinessRule],
    facts: &[AttrFact],
    pool_facts: &[AttrFact],
    spans: &HashMap<Uuid, (Option<i64>, Option<i64>)>,
    edges: &[RuleEdge],
) -> (Vec<RuleHit>, RuleReport) {
    let mut report = RuleReport {
        rules: rules.len(),
        ..Default::default()
    };
    let mut hits: Vec<RuleHit> = Vec::new();

    for rule in rules {
        if rule.conditions.is_empty() {
            continue;
        }
        if let Some(join_predicate) = rule.join_predicate {
            joined_evaluate(
                rule,
                join_predicate,
                facts,
                pool_facts,
                spans,
                edges,
                &mut hits,
                &mut report,
            );
            continue;
        }

        // 按实体分组：这条规则谈的还是一个实体自己的属性
        let mut by_subject: HashMap<Uuid, Vec<&AttrFact>> = HashMap::new();
        for f in facts {
            by_subject.entry(f.subject).or_default().push(f);
        }
        // 按组切开，组序保持稳定：同一区间被两组同时推出时，留下的是**组序在前**
        // 的那条证明，而不是 HashMap 顺序决定的随机一条
        let groups = group_conditions(&rule.conditions);
        for (subject, own) in &by_subject {
            // 同一实体上，一条规则的多个组可能推出同一段区间。去重跨组，
            // 因为它们落成的是同一行派生事实
            // 去重的键带上算出来的值（0032）：同一区间上两个不同的值是两行
            let mut seen: Vec<(Option<i64>, Option<i64>, Option<u64>)> = Vec::new();
            // 这条规则在这个实体上有没有哪一组因组合太多没展开完。**按 (规则, 实体)
            // 记一次**：报出来的是「这里少推了东西」，不是「少推了几组」
            let mut capped_here = false;
            for group in &groups {
                // ---- 槽位：这一组要各挑一条读数的那几处。
                //
                // 前 group.len() 个是条件自己的谓词（**一个条件一个槽**，与
                // 0032 之前一样：两个写在同一属性上的条件仍然各挑各的读数）；
                // 后面是算式里读到、而条件没覆盖的谓词（0032）。算式引用一个
                // 谓词时绑到**第一个**同谓词的槽——`thc > thc * 0.5` 两边说的
                // 就是同一条读数，而不是同一属性的两条
                let mut slots: Vec<Uuid> = group.iter().map(|c| c.predicate).collect();
                let mut extra: Vec<Uuid> = Vec::new();
                for c in group {
                    if let Operand::Calc(e) = &c.operand {
                        e.predicates(&mut extra);
                    }
                }
                if let Conclusion::Computed { expr, .. } = &rule.conclusion {
                    expr.predicates(&mut extra);
                }
                for p in extra {
                    if !slots.contains(&p) {
                        slots.push(p);
                    }
                }

                // 每个槽位的候选读数。任何一个为空，这一组在这个实体上就不成立
                let mut per_slot: Vec<Vec<Uuid>> = Vec::with_capacity(slots.len());
                let mut satisfiable = true;
                for p in &slots {
                    let matched: Vec<Uuid> = own
                        .iter()
                        .filter(|f| f.predicate == *p)
                        .map(|f| f.id)
                        .collect();
                    if matched.is_empty() {
                        satisfiable = false;
                        break;
                    }
                    per_slot.push(matched);
                }
                if !satisfiable {
                    continue;
                }

                // 封顶按组算：一组展不开，不该把另一组也拦下
                let combos: usize = per_slot.iter().map(|v| v.len()).product();
                if combos > MAX_COMBOS {
                    capped_here = true;
                    continue;
                }

                let by_id: HashMap<Uuid, &AttrFact> = own.iter().map(|f| (f.id, *f)).collect();

                // 笛卡尔积。**条件要等组合定下来才判得了**（0032）：一个算出来的
                // 门槛读的是这一轮选中的那几条读数，判断不再只看一条事实。
                // 同一区间可能由多个组合得出（同一读数报了两遍），按区间去重
                for combo in cartesian(&per_slot) {
                    // 槽位 → 这一轮选中的那条读数。算式与条件都照着它读
                    let mut bound: HashMap<Uuid, &AttrFact> = HashMap::new();
                    for (i, id) in combo.iter().enumerate() {
                        let Some(f) = by_id.get(id) else { continue };
                        bound.entry(slots[i]).or_insert(f);
                    }
                    let holds = group.iter().enumerate().all(|(i, c)| {
                        combo
                            .get(i)
                            .and_then(|id| by_id.get(id))
                            .is_some_and(|f| satisfies(c, &f.value, &bound))
                    });
                    if !holds {
                        continue;
                    }
                    // 算出来的结论：这个组合上算不出数就不出结论（缺读数、
                    // 不是数、除零），而不是落一行没有值的派生
                    let value = match &rule.conclusion {
                        Conclusion::Computed { expr, .. } => match expr.eval(&bound) {
                            Some(v) => Some(v),
                            None => continue,
                        },
                        _ => None,
                    };
                    let Some((from, to)) = validity(&combo, spans) else {
                        continue;
                    };
                    // 去重按 (区间, 值)：算出来的结论同一区间可以有不同的值，
                    // 那是两行，不是一行
                    let key = (from, to, value.map(f64::to_bits));
                    if seen.contains(&key) {
                        continue;
                    }
                    seen.push(key);
                    hits.push(RuleHit {
                        rule: rule.id,
                        subject: *subject,
                        object: None,
                        premises: combo,
                        from,
                        to,
                        value,
                    });
                }
            }
            if capped_here {
                report.capped += 1;
            }
        }
    }
    report.hits = hits.len();
    (hits, report)
}

/// 求值一条跨两个实体的规则（0047）。
///
/// **一条连接边是一对 `(X, Y)`，不是一次笛卡尔连接。** ADR 只放行一跳，并且
/// 让每条边自己成为前提：两条同谓词边哪怕首尾一样，也可能各有一段成立期。
/// 封顶因此按 `(rule, X, Y)` 报，与单实体规则的 `(rule, X)` 保持同一种含义。
#[allow(clippy::too_many_arguments)]
fn joined_evaluate(
    rule: &BusinessRule,
    join_predicate: Uuid,
    facts: &[AttrFact],
    pool_facts: &[AttrFact],
    spans: &HashMap<Uuid, (Option<i64>, Option<i64>)>,
    edges: &[RuleEdge],
    hits: &mut Vec<RuleHit>,
    report: &mut RuleReport,
) {
    // X is already scoped to the rule's subject type. The join, however, is
    // the way the rule reaches a differently typed Y.
    let mut x_by_subject: HashMap<Uuid, Vec<&AttrFact>> = HashMap::new();
    for f in facts {
        x_by_subject.entry(f.subject).or_default().push(f);
    }
    let mut pool_by_subject: HashMap<Uuid, Vec<&AttrFact>> = HashMap::new();
    for f in pool_facts {
        pool_by_subject.entry(f.subject).or_default().push(f);
    }

    // 连接边按 X 分桶，一次扫完：对每个 X 再去全部边里找它的，是 X 数乘边数——
    // 十万对上量出来是 5.9 s 对 1 万对的 82 ms，分桶之后随对数线性
    let mut edges_by_x: HashMap<Uuid, Vec<&RuleEdge>> = HashMap::new();
    for e in edges.iter().filter(|e| e.predicate == join_predicate) {
        edges_by_x.entry(e.subject).or_default().push(e);
    }
    let mut x_subjects: Vec<Uuid> = edges_by_x.keys().copied().collect();
    x_subjects.sort_unstable();
    let groups = group_conditions(&rule.conditions);

    for x in x_subjects {
        let x_facts = x_by_subject.get(&x).map(Vec::as_slice).unwrap_or_default();
        for edge in &edges_by_x[&x] {
            let y = edge.object;
            let y_facts = pool_by_subject
                .get(&y)
                .map(Vec::as_slice)
                .unwrap_or_default();
            // 同一对上的多个组可能推出同一结论。留先到的组作证明，与单实体
            // 规则的去重规则一致
            let mut seen: Vec<(Option<i64>, Option<i64>, Option<u64>)> = Vec::new();
            let mut capped_here = false;
            for group in &groups {
                let mut slots: Vec<(Side, Uuid)> =
                    group.iter().map(|c| (c.side, c.predicate)).collect();
                let mut extra: Vec<(Side, Uuid)> = Vec::new();
                for c in group {
                    if let Operand::Calc(e) = &c.operand {
                        expr_predicates(e, c.side, &mut extra);
                    }
                }
                if let Conclusion::Computed { expr, .. } = &rule.conclusion {
                    expr_predicates(expr, Side::X, &mut extra);
                }
                for slot in extra {
                    if !slots.contains(&slot) {
                        slots.push(slot);
                    }
                }

                let mut per_slot: Vec<Vec<Uuid>> = Vec::with_capacity(slots.len());
                let mut satisfiable = true;
                for (side, predicate) in &slots {
                    let side_facts = match side {
                        Side::X => x_facts,
                        Side::Y => y_facts,
                    };
                    let matched: Vec<Uuid> = side_facts
                        .iter()
                        .filter(|f| f.predicate == *predicate)
                        .map(|f| f.id)
                        .collect();
                    if matched.is_empty() {
                        satisfiable = false;
                        break;
                    }
                    per_slot.push(matched);
                }
                if !satisfiable {
                    continue;
                }
                let combos: usize = per_slot.iter().map(|v| v.len()).product();
                if combos > MAX_COMBOS {
                    capped_here = true;
                    continue;
                }

                let x_by_id: HashMap<Uuid, &AttrFact> =
                    x_facts.iter().map(|f| (f.id, *f)).collect();
                let y_by_id: HashMap<Uuid, &AttrFact> =
                    y_facts.iter().map(|f| (f.id, *f)).collect();
                for combo in cartesian(&per_slot) {
                    let mut x_bound: HashMap<Uuid, &AttrFact> = HashMap::new();
                    let mut y_bound: HashMap<Uuid, &AttrFact> = HashMap::new();
                    for ((side, predicate), id) in slots.iter().zip(combo.iter()) {
                        let bound = match side {
                            Side::X => &mut x_bound,
                            Side::Y => &mut y_bound,
                        };
                        let facts_by_id = match side {
                            Side::X => &x_by_id,
                            Side::Y => &y_by_id,
                        };
                        if let Some(f) = facts_by_id.get(id) {
                            bound.entry(*predicate).or_insert(*f);
                        }
                    }
                    let holds = group.iter().enumerate().all(|(i, c)| {
                        let bound = match c.side {
                            Side::X => &x_bound,
                            Side::Y => &y_bound,
                        };
                        combo
                            .get(i)
                            .and_then(|id| match c.side {
                                Side::X => x_by_id.get(id),
                                Side::Y => y_by_id.get(id),
                            })
                            .is_some_and(|f| satisfies(c, &f.value, bound))
                    });
                    if !holds {
                        continue;
                    }
                    // Relation is the only joined conclusion. Store validation
                    // keeps computed conclusions on the old single-entity path,
                    // so this branch need not invent a value for an edge.
                    if !matches!(rule.conclusion, Conclusion::Relation { .. }) {
                        continue;
                    }
                    let mut premises = combo;
                    premises.push(edge.id);
                    let Some((from, to)) = validity(&premises, spans) else {
                        continue;
                    };
                    let key = (from, to, None);
                    if seen.contains(&key) {
                        continue;
                    }
                    seen.push(key);
                    hits.push(RuleHit {
                        rule: rule.id,
                        subject: x,
                        object: Some(y),
                        premises,
                        from,
                        to,
                        value: None,
                    });
                }
            }
            if capped_here {
                report.capped += 1;
            }
        }
    }
}

fn expr_predicates(expr: &Expr, side: Side, out: &mut Vec<(Side, Uuid)>) {
    match expr {
        Expr::Attr(predicate) => {
            let slot = (side, *predicate);
            if !out.contains(&slot) {
                out.push(slot);
            }
        }
        Expr::Const(_) => {}
        Expr::Arith { l, r, .. } => {
            expr_predicates(l, side, out);
            expr_predicates(r, side, out);
        }
    }
}

/// 按 `group` 切成几组，**组序按 group_seq 升序**——两组推出同一区间时，
/// 留下的证明得是稳定的那一条，不能随存储顺序变。
fn group_conditions(conditions: &[Condition]) -> Vec<Vec<&Condition>> {
    let mut keys: Vec<i32> = conditions.iter().map(|c| c.group).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter()
        .map(|g| conditions.iter().filter(|c| c.group == g).collect())
        .collect()
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
fn satisfies(c: &Condition, value: &serde_json::Value, bound: &HashMap<Uuid, &AttrFact>) -> bool {
    // 算出来的门槛：先按这一轮选中的读数算个数，算不出来就是不满足（0032）
    if let Operand::Calc(e) = &c.operand {
        let Some(n) = e.eval(bound) else { return false };
        let Some(v) = num(value) else { return false };
        return match c.op {
            Op::Gt => v > n,
            Op::Gte => v >= n,
            Op::Lt => v < n,
            Op::Lte => v <= n,
            // 算式给的是一个数：区间、集合、有记录都不是它能填的位置
            _ => false,
        };
    }
    match (&c.op, &c.operand) {
        (Op::Present, _) => !value.is_null(),
        // 值不在集合里就算满足；**没有值不算**——那是「没记」，不是「不是它」
        (Op::NotIn, Operand::Set(set)) => {
            !value.is_null() && text(value).is_some_and(|v| !set.iter().any(|s| s == &v))
        }
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
mod joined_tests {
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

    /// A joined rule sees a second entity across exactly one declared edge.
    /// Both sides' readings, and the edge itself, are premises.
    #[test]
    fn a_rule_joins_one_entity_and_concludes_a_relation() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: Some(id(1)),
            conclusion: Conclusion::Relation { predicate: id(2) },
            conditions: vec![
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(50.0),
                },
                Condition {
                    group: 0,
                    side: Side::Y,
                    predicate: id(11),
                    op: Op::Gt,
                    operand: Operand::Num(50.0),
                },
            ],
        };
        let facts = vec![fact(1, 50, 10, json!(60.0)), fact(2, 51, 11, json!(70.0))];
        let edges = vec![RuleEdge {
            id: id(9),
            predicate: id(1),
            subject: id(50),
            object: id(51),
        }];
        let spans = HashMap::from([
            (id(1), (Some(100), Some(200))),
            (id(2), (Some(150), Some(300))),
            (id(9), (Some(120), None)),
        ]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &edges);

        assert_eq!(hits.len(), 1, "one edge yields one joined conclusion");
        assert_eq!(hits[0].subject, id(50));
        assert_eq!(hits[0].object, Some(id(51)));
        assert_eq!((hits[0].from, hits[0].to), (Some(150), Some(200)));
        assert!(hits[0].premises.contains(&id(1)));
        assert!(hits[0].premises.contains(&id(2)));
        assert!(hits[0].premises.contains(&id(9)));
    }

    /// A condition on Y cannot manufacture the edge that binds it to X.
    #[test]
    fn a_joined_condition_without_the_edge_fires_nothing() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: Some(id(1)),
            conclusion: Conclusion::Relation { predicate: id(2) },
            conditions: vec![Condition {
                group: 0,
                side: Side::Y,
                predicate: id(11),
                op: Op::Gt,
                operand: Operand::Num(50.0),
            }],
        };
        let facts = vec![fact(2, 51, 11, json!(70.0))];
        let spans = HashMap::from([(id(2), (Some(100), Some(200)))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert!(hits.is_empty());
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
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(8.0),
                },
                Condition {
                    group: 0,
                    side: Side::X,
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
        let (hits, report) = evaluate(&[rule], &facts, &spans, &[]);
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
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(8.0),
                },
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(11),
                    op: Op::Present,
                    operand: Operand::None,
                },
            ],
        };
        let facts = vec![fact(1, 50, 10, json!(12.3))];
        let spans = HashMap::from([(id(1), (Some(100), None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert!(hits.is_empty(), "第二个条件没有任何事实，不该命中");
    }

    /// 同一属性的两次读数各自成立 → 两段区间，而不是一行翻转（0021 决策 4）
    #[test]
    fn two_readings_give_two_intervals() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
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
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
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
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(8.0),
                },
                Condition {
                    group: 0,
                    side: Side::X,
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
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert!(hits.is_empty(), "两条前提没有同时成立的时段");
    }

    /// 两组「或」：任一组成立就命中。第二组的前提里**没有**第一组那条读数——
    /// 一条派生事实带的是真正让它成立的那几条，不是这个实体的全部属性
    #[test]
    fn either_group_can_fire_and_carries_only_its_own_premises() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![
                // 第 0 组：全烃 > 8 且 解释 ∈ {气测异常}
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(8.0),
                },
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(11),
                    op: Op::In,
                    operand: Operand::Set(vec!["气测异常".into()]),
                },
                // 第 1 组：综合解释 ∈ {气层}
                Condition {
                    side: Side::X,
                    group: 1,
                    predicate: id(12),
                    op: Op::In,
                    operand: Operand::Set(vec!["气层".into()]),
                },
            ],
        };
        // 只有第二组的那条读数
        let facts = vec![fact(3, 50, 12, json!("气层"))];
        let spans = HashMap::from([(id(3), (Some(100), Some(200)))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert_eq!(hits.len(), 1, "第二组独自成立");
        assert_eq!(hits[0].premises, vec![id(3)]);
        assert_eq!((hits[0].from, hits[0].to), (Some(100), Some(200)));
    }

    /// 两组同时成立、且推出同一段区间：**一条命中，不是两条**——它们落成的是
    /// 同一行派生事实，留下的证明是组序在前的那一组
    #[test]
    fn two_groups_on_the_same_interval_are_one_hit() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(8.0),
                },
                Condition {
                    side: Side::X,
                    group: 1,
                    predicate: id(11),
                    op: Op::In,
                    operand: Operand::Set(vec!["气层".into()]),
                },
            ],
        };
        let facts = vec![fact(1, 50, 10, json!(12.3)), fact(2, 50, 11, json!("气层"))];
        // 同一段区间
        let spans = HashMap::from([
            (id(1), (Some(100), Some(200))),
            (id(2), (Some(100), Some(200))),
        ]);
        let (hits, report) = evaluate(&[rule], &facts, &spans, &[]);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].premises,
            vec![id(1)],
            "留下的是组序在前那一组的证明"
        );
        assert_eq!(report.hits, 1);
    }

    /// 一组展不开不该拖累另一组：**封顶按组算**，而且这个 (规则, 实体) 对
    /// 只报一次「没展完」
    #[test]
    fn a_capped_group_does_not_stop_the_other() {
        let mut facts: Vec<AttrFact> = Vec::new();
        let mut spans = HashMap::new();
        // 第 0 组：同一个属性上 100 条读数，组合数 100 > 64
        for i in 1..=100u8 {
            facts.push(fact(i, 50, 10, json!(12.3)));
            spans.insert(id(i), (Some(100), Some(200)));
        }
        // 第 1 组：一条就够
        facts.push(fact(200, 50, 11, json!("气层")));
        spans.insert(id(200), (Some(100), Some(200)));
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(10),
                    op: Op::Gt,
                    operand: Operand::Num(8.0),
                },
                Condition {
                    side: Side::X,
                    group: 1,
                    predicate: id(11),
                    op: Op::In,
                    operand: Operand::Set(vec!["气层".into()]),
                },
            ],
        };
        let (hits, report) = evaluate(&[rule], &facts, &spans, &[]);
        assert_eq!(hits.len(), 1, "第二组照样出结论");
        assert_eq!(hits[0].premises, vec![id(200)]);
        assert_eq!(report.capped, 1, "(规则, 实体) 只报一次");
    }

    /// `is not one of`：值在集合外算满足；**没有这条读数不算**——那是「没记」，
    /// 不是「不是它」
    #[test]
    fn not_one_of_needs_a_reading_to_be_true() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "NonGas".into(),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(11),
                op: Op::NotIn,
                operand: Operand::Set(vec!["气层".into()]),
            }],
        };
        let spans = HashMap::from([(id(1), (None, None))]);
        let (hit, _) = evaluate(
            std::slice::from_ref(&rule),
            &[fact(1, 50, 11, json!("水层"))],
            &spans,
            &[],
        );
        assert_eq!(hit.len(), 1, "读数在集合外");
        let (miss, _) = evaluate(
            std::slice::from_ref(&rule),
            &[fact(1, 50, 11, json!("气层"))],
            &spans,
            &[],
        );
        assert!(miss.is_empty(), "读数在集合里");
        let (none, _) = evaluate(&[rule], &[], &HashMap::new(), &[]);
        assert!(none.is_empty(), "没有这条读数：不成立，而不是「不是它」");
    }

    /// 数字被抽成字符串照样比得动。**这一条挡的是最难发现的失效**：
    /// 规则从不报错，只是永远不命中
    #[test]
    fn a_number_in_quotes_still_compares() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Attribute {
                predicate: id(20),
                value: json!("good"),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(10),
                op: Op::Gte,
                operand: Operand::Num(12.0),
            }],
        };
        let facts = vec![fact(1, 50, 10, json!(" 12.3 "))];
        let spans = HashMap::from([(id(1), (None, None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert_eq!(hits.len(), 1);
    }

    /// 阈值抬高到读数之上，同一份数据就不再命中——降/升阈值重跑是 0021 的验收项
    #[test]
    fn raising_the_threshold_clears_the_hit() {
        let facts = vec![fact(1, 50, 10, json!(12.3))];
        let spans = HashMap::from([(id(1), (None, None))]);
        let mk = |threshold: f64| BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "GasBearingWell".into(),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(10),
                op: Op::Gt,
                operand: Operand::Num(threshold),
            }],
        };
        assert_eq!(evaluate(&[mk(8.0)], &facts, &spans, &[]).0.len(), 1);
        assert!(evaluate(&[mk(20.0)], &facts, &spans, &[]).0.is_empty());
    }

    /// 没有条件的规则什么都不推。空合取在逻辑上恒真，会把整个类归进去——
    /// 那是「规则还没写完」最坏的失败方式
    #[test]
    fn a_rule_without_conditions_concludes_nothing() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Typing { class: "X".into() },
            conditions: vec![],
        };
        let facts = vec![fact(1, 50, 10, json!(12.3))];
        let spans = HashMap::from([(id(1), (None, None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert!(hits.is_empty());
    }

    /// 组合数超上限时**报出来**，不是静默少推
    #[test]
    fn too_many_combinations_are_reported() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Typing { class: "X".into() },
            conditions: vec![
                Condition {
                    group: 0,
                    side: Side::X,
                    predicate: id(10),
                    op: Op::Present,
                    operand: Operand::None,
                },
                Condition {
                    group: 0,
                    side: Side::X,
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
        let (hits, report) = evaluate(&[rule], &facts, &spans, &[]);
        assert_eq!(report.capped, 1, "100 种组合超过上限，要计数");
        assert!(hits.is_empty());
    }

    fn attr(p: u8) -> Expr {
        Expr::Attr(id(p))
    }
    fn arith(op: Arith, l: Expr, r: Expr) -> Expr {
        Expr::Arith {
            op,
            l: Box::new(l),
            r: Box::new(r),
        }
    }

    /// 结论是算出来的：margin = revenue − cost。**算式读的两条读数都进前提**，
    /// 所以这条结论说得出凭什么，也会在任一条读数变了的时候退场（0032）
    #[test]
    fn a_computed_conclusion_carries_the_readings_it_read() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Computed {
                predicate: id(12),
                expr: arith(Arith::Sub, attr(10), attr(11)),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(10),
                op: Op::Present,
                operand: Operand::None,
            }],
        };
        let facts = vec![fact(1, 50, 10, json!(300.0)), fact(2, 50, 11, json!(120.0))];
        let spans = HashMap::from([(id(1), (Some(100), None)), (id(2), (Some(100), None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].value, Some(180.0));
        // 条件只提到 revenue，可 cost 也读了——它照样是前提
        assert_eq!(hits[0].premises.len(), 2);
        assert!(hits[0].premises.contains(&id(1)) && hits[0].premises.contains(&id(2)));
    }

    /// **每个组合各算各的。** 两条 revenue 一条 cost，就是两个数、两段区间、
    /// 两行结论——而不是挑一条算一次（0032 里说的「行数才是真代价」）
    #[test]
    fn each_combination_of_readings_computes_its_own_value() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Computed {
                predicate: id(12),
                expr: arith(Arith::Sub, attr(10), attr(11)),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(10),
                op: Op::Present,
                operand: Operand::None,
            }],
        };
        let facts = vec![
            fact(1, 50, 10, json!(300.0)),
            fact(2, 50, 10, json!(400.0)),
            fact(3, 50, 11, json!(120.0)),
        ];
        let spans = HashMap::from([
            (id(1), (Some(100), Some(200))),
            (id(2), (Some(200), Some(300))),
            (id(3), (Some(100), Some(300))),
        ]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert_eq!(hits.len(), 2, "两条 revenue 各算一个 margin");
        let mut values: Vec<f64> = hits.iter().filter_map(|h| h.value).collect();
        values.sort_by(f64::total_cmp);
        assert_eq!(values, vec![180.0, 280.0]);
    }

    /// 缺一条读数就不出结论——**不是当零**。没记不等于零，在无知的地方填一个
    /// 确定的答案正是这套账本最不该做的事
    #[test]
    fn a_missing_reading_computes_nothing_rather_than_zero() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Computed {
                predicate: id(12),
                expr: arith(Arith::Sub, attr(10), attr(11)),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(10),
                op: Op::Present,
                operand: Operand::None,
            }],
        };
        let facts = vec![fact(1, 50, 10, json!(300.0))];
        let spans = HashMap::from([(id(1), (Some(100), None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert!(hits.is_empty(), "cost 没记，margin 就不该有");
    }

    /// 除零同样什么都不出。一条算不出来的结论与一条不成立的结论一样：没有
    #[test]
    fn dividing_by_zero_computes_nothing() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Computed {
                predicate: id(12),
                expr: arith(Arith::Div, attr(10), attr(11)),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(10),
                op: Op::Present,
                operand: Operand::None,
            }],
        };
        let facts = vec![fact(1, 50, 10, json!(300.0)), fact(2, 50, 11, json!(0.0))];
        let spans = HashMap::from([(id(1), (Some(100), None)), (id(2), (Some(100), None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert!(hits.is_empty());
    }

    /// 门槛也能是算出来的：`revenue > cost × 1.5`。判断要等组合定下来才做得了，
    /// 因为门槛读的正是这一轮选中的那条 cost
    #[test]
    fn a_threshold_can_be_computed_from_another_reading() {
        let rule = |factor: f64| BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Typing {
                class: "Healthy".into(),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(10),
                op: Op::Gt,
                operand: Operand::Calc(arith(Arith::Mul, attr(11), Expr::Const(factor))),
            }],
        };
        let facts = vec![fact(1, 50, 10, json!(300.0)), fact(2, 50, 11, json!(120.0))];
        let spans = HashMap::from([(id(1), (Some(100), None)), (id(2), (Some(100), None))]);

        let (hits, _) = evaluate(&[rule(1.5)], &facts, &spans, &[]);
        assert_eq!(hits.len(), 1, "300 > 120 × 1.5");
        assert_eq!(hits[0].premises.len(), 2, "门槛读的那条也是前提");

        let (none, _) = evaluate(&[rule(3.0)], &facts, &spans, &[]);
        assert!(none.is_empty(), "300 不大于 120 × 3");
    }

    /// 同一区间上两个不同的值是**两行**，不是一行。去重的键带上值，否则
    /// 后算出来的那个会被当成重复丢掉
    #[test]
    fn two_values_on_one_interval_are_two_hits() {
        let rule = BusinessRule {
            id: id(90),
            join_predicate: None,
            conclusion: Conclusion::Computed {
                predicate: id(12),
                expr: attr(10),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(10),
                op: Op::Present,
                operand: Operand::None,
            }],
        };
        let facts = vec![fact(1, 50, 10, json!(300.0)), fact(2, 50, 10, json!(400.0))];
        let spans = HashMap::from([(id(1), (Some(100), None)), (id(2), (Some(100), None))]);
        let (hits, _) = evaluate(&[rule], &facts, &spans, &[]);
        assert_eq!(hits.len(), 2, "同一段区间，两个值");
    }

    /// 算式的深度算得对——编译那一侧靠它拦住「有人在这里写程序」
    #[test]
    fn depth_counts_the_deepest_branch() {
        assert_eq!(attr(10).depth(), 1);
        assert_eq!(arith(Arith::Add, attr(10), attr(11)).depth(), 2);
        assert_eq!(
            arith(Arith::Div, arith(Arith::Sub, attr(10), attr(11)), attr(10)).depth(),
            3
        );
    }
}
