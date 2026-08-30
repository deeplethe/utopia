//! R1:物化推导。**这一层会往图里加东西**,所以它的每一条约束都是必要的。
//!
//! 与 R0 的分水岭:R0 只指出问题,风险面为零;R1 写事实。ADR 0002 把这一步
//! 单列一档,并且给了三条硬性规矩,下面逐条落在代码里。
//!
//! **一、规则只从本体公理编译。** 没有用户自定义 DSL——那是另一个产品。
//! 今天能编译的只有 `TransitiveProperty` 与 `SymmetricProperty`:`inverseOf`
//! 与 `subPropertyOf` 投影侧还没落库,所以这里也就没有。**少一条规则不是
//! 缺陷,是「没声明就不推」的同一条**。
//!
//! **二、断言优先于派生,硬性。** 已经断言过的三元组不再派生一遍——不是为了
//! 省行数,是为了让「这条是谁说的」有唯一答案。
//!
//! **三、深度上限 + 环检测,实测必需。** ADR 在真实语料上量过:`part_of` 的
//! 传递闭包从 185 条膨胀到 828 条且**不收敛**,深度分布第 5 层起振荡而不是
//! 衰减——那是有环的形状。所以这里既不推自环（`A → A` 是矛盾不是知识,交给
//! R0 报),也在轮数上封顶。
//!
//! 还有一条 ADR 列在开放问题里、这里必须给出答案的:**有效时间取交集**。
//! 前提 A `[2020,2023)`、前提 B `[2022,∞)` → 派生 `[2022,2023)`。交集为空
//! 就不推——两段没有重叠的时候,链本身在任何时刻都不成立。

use crate::{Axioms, Edge, MAX_DEPTH};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// 单个谓词上派生的条数上限。
///
/// **不是防御性编程,是拿数量过的**:4.5 倍膨胀出现在一个 185 条边的谓词上,
/// 而膨胀是超线性的。封顶之后被截掉多少条要**说出来**（见 [`Derivation::capped`]）
/// ——悄悄截断会让「推完了」和「推了一部分」长得一模一样。
pub const MAX_DERIVED_PER_PREDICATE: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// `A p B` ∧ `B p C` ⟹ `A p C`
    Transitive,
    /// `A p B` ⟹ `B p A`
    Symmetric,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Transitive => "transitive",
            Rule::Symmetric => "symmetric",
        }
    }
}

/// 一条要落地的派生事实,连同它的证明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub predicate: Uuid,
    pub subject: Uuid,
    pub object: Uuid,
    pub rule: Rule,
    /// 用到的前提,按推导顺序。**这是证明树的一层**——R2 展开解释时顺着它走,
    /// 而前提失效时也靠它知道该让哪些派生跟着失效
    pub premises: Vec<Uuid>,
}

/// 一次推导的产出。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Derivation {
    pub facts: Vec<Derived>,
    /// 撞上上限、没推完的谓词。**必须回给调用方**:界面上要说得出
    /// 「这个谓词太密,只推了两万条」,而不是让人以为推完了
    pub capped: Vec<Uuid>,
}

/// 一条参与推导的边,比 [`Edge`] 多带有效期。
///
/// 单独一个类型而不是给 `Edge` 加字段:R0 完全用不上时间——公理是否被违反
/// 与它什么时候成立无关，而 R1 每一步都要算交集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedEdge {
    pub edge: Edge,
    /// 半开区间 `[from, to)`。两端都可为空 = 不知道/一直
    pub from: Option<i64>,
    pub to: Option<i64>,
}

/// 交集。`None` 表示无界那一侧。
fn overlap(
    a: (Option<i64>, Option<i64>),
    b: (Option<i64>, Option<i64>),
) -> Option<(Option<i64>, Option<i64>)> {
    let from = match (a.0, b.0) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    };
    let to = match (a.1, b.1) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    };
    // 空交集不推。两段没有重叠时,这条链在任何时刻都不成立——推出来的是一条
    // 从不为真的事实,比不推更糟
    if let (Some(f), Some(t)) = (from, to) {
        if f >= t {
            return None;
        }
    }
    Some((from, to))
}

/// 从某个主语出发的一条边：(宾语, 起, 止, 事实 id)。
type Hop = (Uuid, Option<i64>, Option<i64>, Uuid);

/// 派生的中间态:一个 (主语, 宾语) 对是怎么来的。
#[derive(Clone)]
struct Reached {
    from: Option<i64>,
    to: Option<i64>,
    premises: Vec<Uuid>,
}

/// 拿公理推一遍这批边。
pub fn derive(edges: &[TimedEdge], axioms: &HashMap<Uuid, Axioms>) -> Derivation {
    let mut out = Derivation::default();
    let mut by_pred: HashMap<Uuid, Vec<TimedEdge>> = HashMap::new();
    for e in edges {
        let Some(ax) = axioms.get(&e.edge.predicate) else {
            continue;
        };
        if !ax.transitive && !ax.symmetric {
            continue;
        }
        by_pred.entry(e.edge.predicate).or_default().push(*e);
    }
    // 谓词按 id 排序：同一个库两次推导该给出同一份结果
    let mut preds: Vec<Uuid> = by_pred.keys().copied().collect();
    preds.sort();
    for pred in preds {
        let group = &by_pred[&pred];
        let ax = axioms[&pred];
        one_predicate(pred, group, ax, &mut out);
    }
    out
}

fn one_predicate(pred: Uuid, group: &[TimedEdge], ax: Axioms, out: &mut Derivation) {
    // 断言过的三元组。**派生撞上它就让路**——asserted > derived 是硬性的
    let asserted: HashSet<(Uuid, Uuid)> = group
        .iter()
        .map(|e| (e.edge.subject, e.edge.object))
        .collect();

    // 已经推出来的 (主, 宾) → 怎么来的。同一对只留第一条证明:
    // 多条路径都能推出同一件事时，展示哪一条对用户没有区别，而全存下来
    // 会让证明树的规模跟着路径数走
    let mut reached: HashMap<(Uuid, Uuid), Reached> = HashMap::new();
    let mut emitted: Vec<Derived> = Vec::new();
    let mut capped = false;

    // ---- 对称：一跳就够，不进迭代
    if ax.symmetric {
        for e in group {
            let pair = (e.edge.object, e.edge.subject);
            if pair.0 == pair.1 || asserted.contains(&pair) || reached.contains_key(&pair) {
                continue;
            }
            reached.insert(
                pair,
                Reached {
                    from: e.from,
                    to: e.to,
                    premises: vec![e.edge.fact],
                },
            );
            emitted.push(Derived {
                predicate: pred,
                subject: pair.0,
                object: pair.1,
                rule: Rule::Symmetric,
                premises: vec![e.edge.fact],
            });
        }
    }

    // ---- 传递：半朴素求值。frontier 是上一轮新产生的，只有它需要再往下接
    if ax.transitive {
        // 从主语出发能走的边。对称推出来的那些也算基础事实——它们已经是我们
        // 的断言了，链上不该因为「来路不同」断掉
        let mut base: HashMap<Uuid, Vec<Hop>> = HashMap::new();
        for e in group {
            base.entry(e.edge.subject).or_default().push((
                e.edge.object,
                e.from,
                e.to,
                e.edge.fact,
            ));
        }
        for ((s, o), r) in reached.iter() {
            if let Some(&p) = r.premises.first() {
                base.entry(*s).or_default().push((*o, r.from, r.to, p));
            }
        }

        let mut frontier: Vec<((Uuid, Uuid), Reached)> = group
            .iter()
            .map(|e| {
                (
                    (e.edge.subject, e.edge.object),
                    Reached {
                        from: e.from,
                        to: e.to,
                        premises: vec![e.edge.fact],
                    },
                )
            })
            .collect();

        // **`MAX_DEPTH - 1` 轮,不是 MAX_DEPTH 轮。** 起始 frontier 里每条已经
        // 带着一条前提,此后每轮加一条——跑满 MAX_DEPTH 轮会得到 13 条前提的
        // 证明,而那个常量在 R0 那边的含义是「路径最长 12」。两处得是同一个意思
        for _ in 0..MAX_DEPTH.saturating_sub(1) {
            if frontier.is_empty() {
                break;
            }
            let mut next: Vec<((Uuid, Uuid), Reached)> = Vec::new();
            for ((a, b), acc) in frontier.drain(..) {
                let Some(outs) = base.get(&b) else {
                    continue;
                };
                for &(c, from, to, fact) in outs {
                    // **不推自环。** `A p A` 在一个传递+反对称的谓词上是矛盾
                    // 而不是知识，R0 那边会把这个环连路径一起报出来
                    if a == c {
                        continue;
                    }
                    if asserted.contains(&(a, c)) || reached.contains_key(&(a, c)) {
                        continue;
                    }
                    let Some((nf, nt)) = overlap((acc.from, acc.to), (from, to)) else {
                        continue;
                    };
                    if emitted.len() >= MAX_DERIVED_PER_PREDICATE {
                        capped = true;
                        break;
                    }
                    let mut premises = acc.premises.clone();
                    premises.push(fact);
                    reached.insert(
                        (a, c),
                        Reached {
                            from: nf,
                            to: nt,
                            premises: premises.clone(),
                        },
                    );
                    emitted.push(Derived {
                        predicate: pred,
                        subject: a,
                        object: c,
                        rule: Rule::Transitive,
                        premises: premises.clone(),
                    });
                    next.push((
                        (a, c),
                        Reached {
                            from: nf,
                            to: nt,
                            premises,
                        },
                    ));
                }
                if capped {
                    break;
                }
            }
            if capped {
                break;
            }
            frontier = next;
        }
    }

    out.facts.extend(emitted);
    if capped {
        out.capped.push(pred);
    }
}

/// 派生事实的有效期,给落库那一侧用。
///
/// 与 [`derive`] 分开是因为交集已经在推导过程里算过了,而调用方拿到的
/// [`Derived`] 只带前提——重算一次比把区间塞进结果里更省事,也更难错:
/// 前提就是那几条事实,交集是它们的函数。
pub fn validity(
    premises: &[Uuid],
    by_fact: &HashMap<Uuid, (Option<i64>, Option<i64>)>,
) -> Option<(Option<i64>, Option<i64>)> {
    let mut acc = (None, None);
    for p in premises {
        let span = *by_fact.get(p)?;
        acc = overlap(acc, span)?;
    }
    Some(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(i: u8) -> Uuid {
        Uuid::from_bytes([i; 16])
    }
    fn f(i: u8) -> Uuid {
        Uuid::from_bytes([i; 16].map(|b| b ^ 0xF0))
    }
    /// 一条无时间的边
    fn e(fact: u8, s: u8, o: u8) -> TimedEdge {
        TimedEdge {
            edge: Edge {
                fact: f(fact),
                predicate: n(99),
                subject: n(s),
                object: n(o),
            },
            from: None,
            to: None,
        }
    }
    /// 一条带区间的边
    fn te(fact: u8, s: u8, o: u8, from: Option<i64>, to: Option<i64>) -> TimedEdge {
        TimedEdge {
            from,
            to,
            ..e(fact, s, o)
        }
    }
    fn with(ax: Axioms) -> HashMap<Uuid, Axioms> {
        HashMap::from([(n(99), ax)])
    }
    fn transitive() -> HashMap<Uuid, Axioms> {
        with(Axioms {
            transitive: true,
            ..Default::default()
        })
    }
    fn pairs(d: &Derivation) -> Vec<(u8, u8)> {
        let mut p: Vec<(u8, u8)> = d
            .facts
            .iter()
            .map(|x| (x.subject.as_bytes()[0], x.object.as_bytes()[0]))
            .collect();
        p.sort();
        p
    }

    #[test]
    fn nothing_declared_derives_nothing() {
        let edges = [e(1, 1, 2), e(2, 2, 3)];
        assert!(derive(&edges, &HashMap::new()).facts.is_empty());
        // 声明了别的公理也一样——只有 transitive / symmetric 编得出规则
        let irr = with(Axioms {
            irreflexive: true,
            ..Default::default()
        });
        assert!(derive(&edges, &irr).facts.is_empty());
    }

    #[test]
    fn a_chain_closes() {
        // 1→2→3→4，传递应推出 1→3、1→4、2→4
        let edges = [e(1, 1, 2), e(2, 2, 3), e(3, 3, 4)];
        let d = derive(&edges, &transitive());
        assert_eq!(pairs(&d), vec![(1, 3), (1, 4), (2, 4)]);
        assert!(d.capped.is_empty());
    }

    #[test]
    fn the_proof_is_the_premises_in_order() {
        let edges = [e(1, 1, 2), e(2, 2, 3), e(3, 3, 4)];
        let d = derive(&edges, &transitive());
        let long = d
            .facts
            .iter()
            .find(|x| x.subject == n(1) && x.object == n(4))
            .unwrap();
        assert_eq!(
            long.premises,
            vec![f(1), f(2), f(3)],
            "证明要按推导顺序带上三条前提"
        );
        assert_eq!(long.rule, Rule::Transitive);
    }

    #[test]
    fn asserted_beats_derived() {
        // 1→2、2→3 已经推得出 1→3，而 1→3 也被断言过 → 不重复派生
        let edges = [e(1, 1, 2), e(2, 2, 3), e(3, 1, 3)];
        let d = derive(&edges, &transitive());
        assert!(d.facts.is_empty(), "断言过的三元组不该再派生一份");
    }

    #[test]
    fn a_ring_does_not_derive_self_loops_and_does_not_hang() {
        // 1→2→3→1：环。传递闭包里会推出 1→1，而那是矛盾不是知识
        let edges = [e(1, 1, 2), e(2, 2, 3), e(3, 3, 1)];
        let d = derive(&edges, &transitive());
        assert!(
            d.facts.iter().all(|x| x.subject != x.object),
            "自环不该被推出来——R0 会把这个环连路径一起报"
        );
        // 但环上其余的推导是成立的：1→3、2→1、3→2
        assert_eq!(pairs(&d), vec![(1, 3), (2, 1), (3, 2)]);
    }

    #[test]
    fn symmetric_derives_the_other_direction_once() {
        let sym = with(Axioms {
            symmetric: true,
            ..Default::default()
        });
        let edges = [e(1, 1, 2)];
        let d = derive(&edges, &sym);
        assert_eq!(pairs(&d), vec![(2, 1)]);
        assert_eq!(d.facts[0].rule, Rule::Symmetric);
        assert_eq!(d.facts[0].premises, vec![f(1)]);
        // 两个方向都断言过 → 无可派生
        let both = [e(1, 1, 2), e(2, 2, 1)];
        assert!(derive(&both, &sym).facts.is_empty());
    }

    #[test]
    fn validity_is_the_intersection() {
        // 1→2 在 [10,30)，2→3 在 [20,∞) ⟹ 1→3 在 [20,30)
        let edges = [te(1, 1, 2, Some(10), Some(30)), te(2, 2, 3, Some(20), None)];
        let d = derive(&edges, &transitive());
        assert_eq!(pairs(&d), vec![(1, 3)]);
        let by_fact = HashMap::from([(f(1), (Some(10), Some(30))), (f(2), (Some(20), None))]);
        assert_eq!(
            validity(&d.facts[0].premises, &by_fact),
            Some((Some(20), Some(30)))
        );
    }

    #[test]
    fn no_overlap_derives_nothing() {
        // 1→2 只在 [10,20)，2→3 只在 [30,40) —— 这条链在任何时刻都不成立
        let edges = [
            te(1, 1, 2, Some(10), Some(20)),
            te(2, 2, 3, Some(30), Some(40)),
        ];
        let d = derive(&edges, &transitive());
        assert!(
            d.facts.is_empty(),
            "两段不重叠时推出来的是一条从不为真的事实"
        );
    }

    #[test]
    fn a_touching_boundary_is_not_an_overlap() {
        // [10,20) 与 [20,30)：半开区间，端点相接不算重叠
        let edges = [
            te(1, 1, 2, Some(10), Some(20)),
            te(2, 2, 3, Some(20), Some(30)),
        ];
        assert!(derive(&edges, &transitive()).facts.is_empty());
    }

    #[test]
    fn depth_is_bounded() {
        // 一条 40 跳的链，深度上限是 12
        let edges: Vec<TimedEdge> = (1..=40).map(|i| e(i, i, i + 1)).collect();
        let d = derive(&edges, &transitive());
        let longest = d.facts.iter().map(|x| x.premises.len()).max().unwrap();
        assert!(
            longest <= MAX_DEPTH,
            "证明长度不该超过深度上限，实际 {longest}"
        );
        assert!(!d.facts.is_empty(), "有上限不等于什么都不推");
    }

    #[test]
    fn symmetric_feeds_the_transitive_chain() {
        // 同时声明对称与传递：1→2 与 3→2 断言过，对称推出 2→3，
        // 于是传递能接上 1→3
        let both = with(Axioms {
            symmetric: true,
            transitive: true,
            ..Default::default()
        });
        let edges = [e(1, 1, 2), e(2, 3, 2)];
        let d = derive(&edges, &both);
        let got = pairs(&d);
        assert!(got.contains(&(2, 1)) && got.contains(&(2, 3)), "两条对称边");
        assert!(got.contains(&(1, 3)), "对称推出来的边要能继续参与传递");
    }

    #[test]
    fn each_predicate_is_closed_on_its_own() {
        // 99 传递、98 不是：跨谓词不该接成链
        let mut other = e(2, 2, 3);
        other.edge.predicate = n(98);
        let edges = [e(1, 1, 2), other];
        let d = derive(&edges, &transitive());
        assert!(d.facts.is_empty(), "1 →(99) 2 →(98) 3 推不出任何东西");
    }
}
