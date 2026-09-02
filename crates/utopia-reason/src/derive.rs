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
    /// `A p B` ∧ `p⁻¹ = q` ⟹ `B q A`。**主宾对调且换谓词**——
    /// 两件事一起发生，只做一件是这条规则最容易写错的地方
    Inverse,
    /// `A p B` ∧ `p ⊑ q` ⟹ `A q B`。主宾不动，只升谓词
    SubProperty,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Transitive => "transitive",
            Rule::Symmetric => "symmetric",
            Rule::Inverse => "inverse",
            Rule::SubProperty => "sub_property",
        }
    }
}

/// 一条要落地的派生事实,连同它的证明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub predicate: Uuid,
    /// **哪个谓词的声明触发了它。**
    ///
    /// 传递与对称不换谓词，`via == predicate`；而 `inverseOf` 与
    /// `subPropertyOf` 换——`ceo_of ⊑ works_at` 推出的事实谓词是 `works_at`，
    /// 而声明写在 `ceo_of` 上。
    ///
    /// 落库时按 `via` 找规则行。**这里踩过一次**：原先按 `predicate` 找，
    /// 前两条规则一直对（两者相同），加了跨谓词的两条之后查不到规则，
    /// 于是 `continue` 静默丢弃——推出来了却不落库，最难查的那一种。
    pub via: Uuid,
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

/// 一条三元组的身份：(谓词, 主语, 宾语)。
///
/// **谓词进了 key，这是这一版最要紧的改动。** 从前推导按谓词分组、组内自成一体，
/// 因为传递与对称都不换谓词；而 `inverseOf` 与 `subPropertyOf` 天生跨谓词——
/// `A works_at B` 推出的是 `B employs A`，落在另一个谓词上。分组一做，这两条
/// 规则就无处安放。
type Triple = (Uuid, Uuid, Uuid);

/// 拿公理推一遍这批边。
///
/// **全局不动点，不再按谓词分组。** 三条规则会串起来：
///
/// ```text
/// A ceo_of B  --(subPropertyOf)-->  A works_at B  --(inverseOf)-->  B employs A
/// ```
///
/// 各谓词各算各的话，这条链在第一步就断了。所以改成半朴素求值扫全集：每一轮拿上
/// 一轮的新边（frontier）再推一遍，没有新增就停。
///
/// 三条一跳规则（对称／逆／子属性）与传递放在同一轮里，因为它们互为输入——
/// 逆推出来的边可能让某条传递链接得上，反之亦然。
pub fn derive(edges: &[TimedEdge], axioms: &HashMap<Uuid, Axioms>) -> Derivation {
    let mut out = Derivation::default();

    // 断言过的三元组。**派生撞上它就让路**——asserted > derived 是硬性的
    let asserted: HashSet<Triple> = edges
        .iter()
        .map(|e| (e.edge.predicate, e.edge.subject, e.edge.object))
        .collect();

    // 已经推出来的 → 怎么来的。同一条只留第一条证明：多条路径都能推出同一件事
    // 时，展示哪一条对用户没有区别，而全存下来会让证明树的规模跟着路径数走
    let mut reached: HashMap<Triple, Reached> = HashMap::new();
    // **封顶仍按谓词计**：那个常量的含义没变（一个谓词最多推两万条），
    // 而 `Derivation::capped` 回的也是谓词列表。跨谓词之后若改成全局一个数，
    // 界面上「哪个谓词太密」就答不出来了
    let mut per_pred: HashMap<Uuid, usize> = HashMap::new();
    let mut capped: HashSet<Uuid> = HashSet::new();

    // 从 (谓词, 主语) 出发能走的边，传递用。派生出来的也进来——它们已经是
    // 我们的断言了，链上不该因为「来路不同」断掉
    let mut adj: HashMap<(Uuid, Uuid), Vec<Hop>> = HashMap::new();
    for e in edges {
        adj.entry((e.edge.predicate, e.edge.subject))
            .or_default()
            .push((e.edge.object, e.from, e.to, e.edge.fact));
    }

    let mut frontier: Vec<(Triple, Reached)> = edges
        .iter()
        .map(|e| {
            (
                (e.edge.predicate, e.edge.subject, e.edge.object),
                Reached {
                    from: e.from,
                    to: e.to,
                    premises: vec![e.edge.fact],
                },
            )
        })
        .collect();
    // 起始 frontier 的顺序跟着入参走，而入参顺序不保证——排一次，
    // 同一个库两次推导才给得出同一份结果
    frontier.sort_by_key(|(t, _)| *t);

    // 轮数上限是**兜底**，真正的界在下面那条 `premises.len() >= MAX_DEPTH`：
    // 常量的含义是「路径最长 12」，按前提条数算才对得上。轮数只防病态输入
    for _ in 0..MAX_DEPTH {
        if frontier.is_empty() {
            break;
        }
        let mut next: Vec<(Triple, Reached)> = Vec::new();

        for (triple, acc) in frontier.drain(..) {
            let (pred, subj, obj) = triple;
            let Some(ax) = axioms.get(&pred) else {
                continue;
            };
            // 再接一条就超了：这一条不再往下延，但它自己已经产出过
            if acc.premises.len() >= MAX_DEPTH {
                continue;
            }

            // ---- 一跳的三条：换主宾（对称）、换谓词（逆 / 子属性）
            let mut hops: Vec<(Triple, Rule)> = Vec::new();
            if ax.symmetric {
                hops.push(((pred, obj, subj), Rule::Symmetric));
            }
            if let Some(inv) = ax.inverse_of {
                // `A p B ⟹ B p⁻¹ A`。主宾对调**且**谓词换掉——两件事一起发生，
                // 只做一件是这条规则最容易写错的地方
                hops.push(((inv, obj, subj), Rule::Inverse));
            }
            if let Some(sup) = ax.sub_property_of {
                // `A p B ∧ p ⊑ q ⟹ A q B`。主宾不动，只升谓词
                hops.push(((sup, subj, obj), Rule::SubProperty));
            }
            for (t, rule) in hops {
                if emit(
                    t,
                    pred,
                    rule,
                    &acc,
                    acc.from,
                    acc.to,
                    None,
                    &asserted,
                    &mut reached,
                    &mut per_pred,
                    &mut capped,
                    &mut out,
                    &mut next,
                    &mut adj,
                ) {
                    continue;
                }
            }

            // ---- 传递：需要接一条同谓词的出边
            if ax.transitive {
                let outs = adj.get(&(pred, obj)).cloned().unwrap_or_default();
                for (c, from, to, fact) in outs {
                    // **不推自环。** `A p A` 在一个传递+反对称的谓词上是矛盾
                    // 而不是知识，R0 那边会把这个环连路径一起报出来
                    if subj == c {
                        continue;
                    }
                    let Some((nf, nt)) = overlap((acc.from, acc.to), (from, to)) else {
                        continue;
                    };
                    emit(
                        (pred, subj, c),
                        pred,
                        Rule::Transitive,
                        &acc,
                        nf,
                        nt,
                        Some(fact),
                        &asserted,
                        &mut reached,
                        &mut per_pred,
                        &mut capped,
                        &mut out,
                        &mut next,
                        &mut adj,
                    );
                }
            }
        }
        next.sort_by_key(|(t, _)| *t);
        frontier = next;
    }

    let mut capped: Vec<Uuid> = capped.into_iter().collect();
    capped.sort();
    out.capped = capped;
    out
}

/// 落一条派生，并把它接进邻接表供后续传递使用。返回 true = 这个谓词封顶了。
///
/// 参数多得难看，但把它抽出来是必要的：四条规则各自的落地动作一模一样
/// （查断言、查已推、算区间、记证明、进 frontier、进邻接表），而上一版
/// 正是因为对称与传递各写一遍，两处的「跳过条件」慢慢长得不一样了。
#[allow(clippy::too_many_arguments)]
fn emit(
    t: Triple,
    // 触发它的那个谓词的声明。四条规则都是「当前展开的这条边的谓词」
    via: Uuid,
    rule: Rule,
    acc: &Reached,
    from: Option<i64>,
    to: Option<i64>,
    // 传递多用掉的那一条前提；一跳规则没有
    extra_premise: Option<Uuid>,
    asserted: &HashSet<Triple>,
    reached: &mut HashMap<Triple, Reached>,
    per_pred: &mut HashMap<Uuid, usize>,
    capped: &mut HashSet<Uuid>,
    out: &mut Derivation,
    next: &mut Vec<(Triple, Reached)>,
    adj: &mut HashMap<(Uuid, Uuid), Vec<Hop>>,
) -> bool {
    let (pred, subj, obj) = t;
    // 自环不推，任何规则都一样：`A p A` 是矛盾不是知识
    if subj == obj {
        return false;
    }
    // 断言优先；已经推过的不重复推——**逆的互指靠这一条收敛**：
    // `p⁻¹ = q` 且 `q⁻¹ = p` 时，第二轮推回来的那条已经在 reached 里
    if asserted.contains(&t) || reached.contains_key(&t) {
        return false;
    }
    let n = per_pred.entry(pred).or_insert(0);
    if *n >= MAX_DERIVED_PER_PREDICATE {
        capped.insert(pred);
        return true;
    }
    *n += 1;

    let mut premises = acc.premises.clone();
    if let Some(p) = extra_premise {
        premises.push(p);
    }
    let r = Reached {
        from,
        to,
        premises: premises.clone(),
    };
    reached.insert(t, r.clone());
    // 派生出来的边也能被后续传递接上
    if let Some(&first) = premises.first() {
        adj.entry((pred, subj))
            .or_default()
            .push((obj, from, to, first));
    }
    out.facts.push(Derived {
        predicate: pred,
        via,
        subject: subj,
        object: obj,
        rule,
        premises,
    });
    next.push((t, r));
    false
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

    // ---- 跨谓词的两条规则（inverseOf / subPropertyOf）----
    //
    // 上面那些用的都是单谓词 `n(99)`；这两条规则天生要三个谓词才说得清，
    // 所以另起一组常量与构造器

    const P: Uuid = Uuid::from_bytes([1; 16]);
    const Q: Uuid = Uuid::from_bytes([2; 16]);
    const R: Uuid = Uuid::from_bytes([3; 16]);

    /// 指定谓词的一条无时间边
    fn ep(pred: Uuid, fact: u8, s: u8, o: u8) -> TimedEdge {
        TimedEdge {
            edge: Edge {
                fact: f(fact),
                predicate: pred,
                subject: n(s),
                object: n(o),
            },
            from: None,
            to: None,
        }
    }
    /// 指定谓词、带区间
    fn tep(pred: Uuid, fact: u8, s: u8, o: u8, from: Option<i64>, to: Option<i64>) -> TimedEdge {
        TimedEdge {
            from,
            to,
            ..ep(pred, fact, s, o)
        }
    }
    /// `p⁻¹ = q` 且 `q⁻¹ = p`——互指，收敛性靠它测
    fn inverse_pair() -> HashMap<Uuid, Axioms> {
        HashMap::from([
            (
                P,
                Axioms {
                    inverse_of: Some(Q),
                    ..Default::default()
                },
            ),
            (
                Q,
                Axioms {
                    inverse_of: Some(P),
                    ..Default::default()
                },
            ),
        ])
    }
    /// `p ⊑ q`
    fn sub_property() -> HashMap<Uuid, Axioms> {
        HashMap::from([(
            P,
            Axioms {
                sub_property_of: Some(Q),
                ..Default::default()
            },
        )])
    }

    /// `A works_at B` ⟹ `B employs A`：主宾对调**且**换谓词。
    /// 只做一件就是这条规则最常见的写错方式，所以两件都断言。
    #[test]
    fn the_inverse_swaps_the_ends_and_the_predicate() {
        let d = derive(&[ep(P, 1, 1, 2)], &inverse_pair());
        assert_eq!(d.facts.len(), 1, "一条边只推出一条逆");
        let got = &d.facts[0];
        assert_eq!(got.predicate, Q, "**谓词换了**");
        assert_eq!((got.subject, got.object), (n(2), n(1)), "**主宾也对调了**");
        assert_eq!(got.rule, Rule::Inverse);
        assert_eq!(got.via, P, "声明写在 P 上，产出落在 Q 上");
        assert_eq!(got.premises, vec![f(1)], "证明就是那一条原边");
    }

    /// `p⁻¹ = q` 且 `q⁻¹ = p` —— 互指。推回来的那条已经断言过，
    /// 必须收敛而不是来回震荡。
    #[test]
    fn a_mutual_inverse_settles_instead_of_bouncing() {
        let d = derive(&[ep(P, 1, 1, 2), ep(Q, 2, 2, 1)], &inverse_pair());
        assert!(
            d.facts.is_empty(),
            "两个方向都已经断言过，一条都不该推——**断言优先**"
        );
    }

    /// `p ⊑ q`：断言具体的，通用的也成立。主宾不动。
    #[test]
    fn a_sub_property_lifts_the_predicate_and_keeps_the_ends() {
        let d = derive(&[ep(P, 1, 1, 2)], &sub_property());
        assert_eq!(d.facts.len(), 1);
        let got = &d.facts[0];
        assert_eq!(got.predicate, Q, "升到父属性");
        assert_eq!((got.subject, got.object), (n(1), n(2)), "主宾不动");
        assert_eq!(got.rule, Rule::SubProperty);
        assert_eq!(
            got.via, P,
            "**via 是声明公理的那个谓词**，不是推出来的那个——落库按它找规则行"
        );
    }

    /// **不换谓词的两条规则，`via` 必须等于 `predicate`。**
    ///
    /// 这条看着是废话，而它正是那个 bug 藏得住的原因：落库从前按 `predicate`
    /// 找规则行，对传递与对称一直是对的，所以没人发现键选错了。跨谓词的两条
    /// 一加，`ceo_of ⊑ works_at` 推出的事实就查不到规则、被静默丢弃。
    #[test]
    fn for_the_same_predicate_rules_via_is_the_predicate() {
        let d = derive(&[e(1, 1, 2), e(2, 2, 3)], &transitive());
        assert!(!d.facts.is_empty());
        for f in &d.facts {
            assert_eq!(f.via, f.predicate, "传递不换谓词");
        }
    }

    /// **这一条是整次改造的理由**：三条规则串起来。
    ///
    /// `A ceo_of B` ∧ `ceo_of ⊑ works_at` ∧ `works_at⁻¹ = employs`
    ///   ⟹ `A works_at B` ⟹ `B employs A`
    ///
    /// 按谓词分组的旧结构在第一步就断了。
    #[test]
    fn a_sub_property_feeds_the_inverse() {
        let mut ax = HashMap::new();
        // ceo_of ⊑ works_at
        ax.insert(
            P,
            Axioms {
                sub_property_of: Some(Q),
                ..Default::default()
            },
        );
        // works_at⁻¹ = employs
        ax.insert(
            Q,
            Axioms {
                inverse_of: Some(R),
                ..Default::default()
            },
        );
        let d = derive(&[ep(P, 1, 1, 2)], &ax);
        let mut got: Vec<(Uuid, u8, u8)> = d
            .facts
            .iter()
            .map(|x| (x.predicate, x.subject.as_bytes()[0], x.object.as_bytes()[0]))
            .collect();
        got.sort();
        assert!(got.contains(&(Q, 1, 2)), "先升成 works_at");
        assert!(
            got.contains(&(R, 2, 1)),
            "**再转成 employs 的反方向**——跨了两个谓词，旧结构做不到"
        );
        assert_eq!(got.len(), 2);
        // 证明要跟着长：第二跳用掉两条前提里的第一条
        let employs = d.facts.iter().find(|x| x.predicate == R).unwrap();
        assert_eq!(employs.premises, vec![f(1)], "根还是那条原始断言");
    }

    /// 逆推出来的边要能被传递接上：`p` 传递、`q` 是它的逆，
    /// `B q A` ∧ `C q B` 应当推出 `C q A`（若 q 也传递）。
    #[test]
    fn what_the_inverse_produces_can_still_be_chained() {
        let mut ax = HashMap::new();
        ax.insert(
            P,
            Axioms {
                inverse_of: Some(Q),
                ..Default::default()
            },
        );
        ax.insert(
            Q,
            Axioms {
                transitive: true,
                ..Default::default()
            },
        );
        // A p B、B p C  ⟹  B q A、C q B  ⟹（q 传递）⟹ C q A
        let d = derive(&[ep(P, 1, 1, 2), ep(P, 2, 2, 3)], &ax);
        let got: Vec<(Uuid, u8, u8)> = d
            .facts
            .iter()
            .map(|x| (x.predicate, x.subject.as_bytes()[0], x.object.as_bytes()[0]))
            .collect();
        assert!(got.contains(&(Q, 2, 1)));
        assert!(got.contains(&(Q, 3, 2)));
        assert!(
            got.contains(&(Q, 3, 1)),
            "**逆产出的边要进邻接表**，否则传递接不上它"
        );
    }

    /// 区间照旧取交集，跨谓词也一样。
    #[test]
    fn the_inverse_carries_the_same_span() {
        let d = derive(&[tep(P, 1, 1, 2, Some(10), Some(20))], &inverse_pair());
        assert_eq!(d.facts.len(), 1);
        let v = validity(
            &d.facts[0].premises,
            &HashMap::from([(f(1), (Some(10), Some(20)))]),
        );
        assert_eq!(v, Some((Some(10), Some(20))), "逆不改变有效期");
    }

    /// 自己是自己的逆 = 对称，但不该推出自环。
    #[test]
    fn a_predicate_that_is_its_own_inverse_still_refuses_self_loops() {
        let ax = HashMap::from([(
            P,
            Axioms {
                inverse_of: Some(P),
                ..Default::default()
            },
        )]);
        let d = derive(&[ep(P, 1, 1, 1)], &ax);
        assert!(d.facts.is_empty(), "`A p A` 的逆还是 `A p A`——自环不推");
    }
}
