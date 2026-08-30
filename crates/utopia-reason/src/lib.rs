//! 一致性检查：拿本体声明的公理去量已经落库的事实（见 `docs/decisions/0002` R0）。
//!
//! **不写 `facts` 表，也不碰数据库。** 这一层只做判断：输入是边与公理，输出是
//! 「哪几条事实互相矛盾」。取数与落库在 `utopia-store` / `utopia-server`。
//!
//! 这样分不是洁癖。ADR 说 R0 的价值在于「引擎的难点——规则表示、求值、终止——
//! 全部建成并验证，而风险面为零」，而那些难点是纯逻辑：不起数据库就能跑几百个
//! 用例，包括那些真实语料里未必凑得出来的形状（十一个节点的环、自环套在环里、
//! 同一对节点被两条不同谓词连着）。
//!
//! **没有公理就没有依据。** 四类检查每一类都由本体里的一位布尔决定要不要查，
//! 没声明就不查——不报矛盾比猜一个公理出来安全。所以一个没装本体包的库跑出来
//! 是零，那是实情不是故障。

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// 一条参与检查的事实：谁、什么关系、指向谁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub fact: Uuid,
    pub predicate: Uuid,
    pub subject: Uuid,
    pub object: Uuid,
}

/// 一个谓词声明了哪些公理。四位都为假的谓词根本不进检查。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Axioms {
    pub transitive: bool,
    pub symmetric: bool,
    pub asymmetric: bool,
    pub irreflexive: bool,
    pub functional: bool,
    pub inverse_functional: bool,
}

impl Axioms {
    /// 一位都没声明的谓词不必检查——**这是性能上的事，也是语义上的事**：
    /// 没有公理就没有判据，扫它只会白扫。
    fn says_nothing(&self) -> bool {
        *self == Axioms::default()
    }
}

/// 查出来的一处矛盾。落库在 `axiom_violations`——**不进 `fact_conflicts`**：
/// 那张表问的是「哪条对」，而公理违规问的是「错在数据还是错在定义」，
/// 后者的出路可能是去改本体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub kind: Kind,
    /// 涉及的事实。**自反违反只有一条**——它跟自己矛盾，不需要第二条。
    /// 环取首尾两条（中间那些在 `path` 里）。
    pub left: Uuid,
    pub right: Uuid,
    /// 环的完整路径，按事实排列；其余三类为空。
    /// 留着是因为「A→B→C→A」比「A 与 C 矛盾」有用得多——人要顺着看一遍才知道
    /// 该撤哪一条
    pub path: Vec<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `A p A`，而 p 声明了 irreflexive
    SelfLoop,
    /// `A p B` 且 `B p A`，而 p 声明了 asymmetric
    Asymmetry,
    /// `A p B p … p A`，而 p 声明了 transitive——闭包里推出 `A p A`
    Cycle,
    /// 同一主语与谓词同时指向两个宾语，而 p 声明了 functional
    /// （inverse_functional 则是同一宾语被两个主语指）
    Functional,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::SelfLoop => "self_loop",
            Kind::Asymmetry => "asymmetry",
            Kind::Cycle => "cycle",
            Kind::Functional => "functional",
        }
    }
}

/// 环检测的深度上限。
///
/// **必须有，不是防御性编程。** 0002 在真实语料上量过：`part_of` 的传递闭包
/// 从 185 条膨胀到 828 条且**不收敛**——深度分布 `1:185 2:181 3:141 4:45
/// 5:52 6:40 7:52 8:40 9:52 10:40`，第 5 层起振荡而不是衰减，那是有环的形状。
/// 没有上限，一个环就能让求值不终止。
pub const MAX_DEPTH: usize = 12;

/// 拿公理量一遍这批边。
///
/// 每个谓词各查各的：公理是挂在谓词上的，跨谓词的边之间没有可比性
/// （`A part_of B` 与 `B produces A` 同时成立不是矛盾）。
pub fn check(edges: &[Edge], axioms: &HashMap<Uuid, Axioms>) -> Vec<Violation> {
    let mut out = Vec::new();
    let mut by_pred: HashMap<Uuid, Vec<Edge>> = HashMap::new();
    for e in edges {
        let Some(ax) = axioms.get(&e.predicate) else {
            continue;
        };
        if ax.says_nothing() {
            continue;
        }
        by_pred.entry(e.predicate).or_default().push(*e);
    }
    for (pred, group) in by_pred {
        let ax = axioms[&pred];
        if ax.irreflexive {
            out.extend(self_loops(&group));
        }
        if ax.asymmetric {
            out.extend(asymmetries(&group));
        }
        if ax.transitive {
            out.extend(cycles(&group));
        }
        if ax.functional {
            out.extend(too_many(&group, |e| e.subject, |e| e.object));
        }
        if ax.inverse_functional {
            out.extend(too_many(&group, |e| e.object, |e| e.subject));
        }
    }
    out
}

fn self_loops(edges: &[Edge]) -> Vec<Violation> {
    edges
        .iter()
        .filter(|e| e.subject == e.object)
        .map(|e| Violation {
            kind: Kind::SelfLoop,
            // 两列填同一条：它跟自己矛盾，没有第二条事实可指
            left: e.fact,
            right: e.fact,
            path: Vec::new(),
        })
        .collect()
}

fn asymmetries(edges: &[Edge]) -> Vec<Violation> {
    let mut seen: HashMap<(Uuid, Uuid), Uuid> = HashMap::new();
    let mut out = Vec::new();
    for e in edges {
        if e.subject == e.object {
            // 自环由 irreflexive 那一档负责；反对称在这里报一遍是重复
            continue;
        }
        if let Some(&other) = seen.get(&(e.object, e.subject)) {
            out.push(Violation {
                kind: Kind::Asymmetry,
                left: other,
                right: e.fact,
                path: Vec::new(),
            });
        }
        seen.insert((e.subject, e.object), e.fact);
    }
    out
}

/// 找环。**每个环只报一次**，从环上最小的节点起算。
///
/// 用深度优先而不是半朴素闭包求值：两者都能发现环，但闭包只告诉你「A 推出了
/// A」，而人要的是**路径**——顺着 `A→B→C→A` 看一遍才知道该撤哪一条。闭包丢掉
/// 的正是这个。
///
/// R1 物化推导要的是闭包本身，那时再建；R0 要的是「哪几条边凑成了环」。
fn cycles(edges: &[Edge]) -> Vec<Violation> {
    let mut adj: HashMap<Uuid, Vec<&Edge>> = HashMap::new();
    for e in edges {
        adj.entry(e.subject).or_default().push(e);
    }
    let mut reported: HashSet<Vec<Uuid>> = HashSet::new();
    let mut out = Vec::new();
    let nodes: Vec<Uuid> = adj.keys().copied().collect();
    for start in nodes {
        let mut path: Vec<&Edge> = Vec::new();
        let mut on_path: HashSet<Uuid> = HashSet::new();
        walk(
            start,
            start,
            &adj,
            &mut path,
            &mut on_path,
            &mut reported,
            &mut out,
        );
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    start: Uuid,
    at: Uuid,
    adj: &HashMap<Uuid, Vec<&'a Edge>>,
    path: &mut Vec<&'a Edge>,
    on_path: &mut HashSet<Uuid>,
    reported: &mut HashSet<Vec<Uuid>>,
    out: &mut Vec<Violation>,
) {
    if path.len() >= MAX_DEPTH {
        return;
    }
    let Some(next) = adj.get(&at) else { return };
    for e in next {
        if e.object == start && !path.is_empty() {
            // 回到起点：成环。**按事实 id 排序去重**——同一个环从不同节点
            // 出发会被走到 n 次，报 n 遍就是让人把同一件事看 n 次
            let mut facts: Vec<Uuid> = path.iter().map(|x| x.fact).collect();
            facts.push(e.fact);
            let mut key = facts.clone();
            key.sort();
            if reported.insert(key) {
                out.push(Violation {
                    kind: Kind::Cycle,
                    left: facts[0],
                    right: *facts.last().unwrap(),
                    path: facts,
                });
            }
            continue;
        }
        if e.object == start || on_path.contains(&e.object) {
            continue;
        }
        on_path.insert(e.object);
        path.push(e);
        walk(start, e.object, adj, path, on_path, reported, out);
        path.pop();
        on_path.remove(&e.object);
    }
}

/// 函数性违反：同一个「一端」指向了两个不同的「另一端」。
///
/// `functional` 与 `inverse_functional` 是同一个判断的两个方向，所以共用这一个
/// 函数，由调用方决定哪一端是键。
fn too_many(edges: &[Edge], key: fn(&Edge) -> Uuid, val: fn(&Edge) -> Uuid) -> Vec<Violation> {
    let mut by_key: HashMap<Uuid, Vec<&Edge>> = HashMap::new();
    for e in edges {
        by_key.entry(key(e)).or_default().push(e);
    }
    let mut out = Vec::new();
    for (_, group) in by_key {
        // 只报第一对。同一个主语指了五个宾语时报十对（两两组合）只是把同一件事
        // 说十遍——人要处理的是「这里有冲突」，看一对就够去查了
        let mut distinct: Vec<&Edge> = Vec::new();
        for e in group {
            if !distinct.iter().any(|d| val(d) == val(e)) {
                distinct.push(e);
            }
        }
        if distinct.len() > 1 {
            out.push(Violation {
                kind: Kind::Functional,
                left: distinct[0].fact,
                right: distinct[1].fact,
                path: Vec::new(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造边的简写。节点用小整数编出稳定的 uuid，读断言时看得出谁是谁。
    fn n(i: u8) -> Uuid {
        Uuid::from_bytes([i; 16])
    }
    fn f(i: u8) -> Uuid {
        // 与节点 id 分段:节点用低位,事实用高位。`u8` 装得下 0..=255,
        // 而测试里造过 30 条边的链——用 `200 + i` 会溢出
        Uuid::from_bytes([i; 16].map(|b| b ^ 0xF0))
    }
    fn e(fact: u8, s: u8, o: u8) -> Edge {
        Edge {
            fact: f(fact),
            predicate: n(99),
            subject: n(s),
            object: n(o),
        }
    }
    fn with(ax: Axioms) -> HashMap<Uuid, Axioms> {
        HashMap::from([(n(99), ax)])
    }
    fn kinds(v: &[Violation]) -> Vec<Kind> {
        let mut k: Vec<Kind> = v.iter().map(|x| x.kind).collect();
        k.sort_by_key(|x| x.as_str());
        k
    }

    /// **没声明公理的谓词一条都不查。** 这是整套检查的地基：没有依据就不报矛盾。
    ///
    /// 反过来说也成立——一个没装本体包的库跑出来是零，那是实情不是故障。
    #[test]
    fn a_predicate_that_declares_nothing_is_never_checked() {
        let edges = [e(1, 1, 1), e(2, 1, 2), e(3, 2, 1)];
        assert!(check(&edges, &with(Axioms::default())).is_empty());
        // 连 axioms 里都没有这个谓词时同样不查（本体里没有这一行）
        assert!(check(&edges, &HashMap::new()).is_empty());
    }

    #[test]
    fn a_self_loop_needs_irreflexive_to_be_a_problem() {
        let edges = [e(1, 1, 1)];
        assert!(check(
            &edges,
            &with(Axioms {
                transitive: true,
                ..Default::default()
            })
        )
        .is_empty());
        let v = check(
            &edges,
            &with(Axioms {
                irreflexive: true,
                ..Default::default()
            }),
        );
        assert_eq!(kinds(&v), vec![Kind::SelfLoop]);
        // 只有一条事实：两列填同一个 id
        assert_eq!(v[0].left, v[0].right);
    }

    /// 反对称那一档**不重复报自环**。`A p A` 同时满足「有反向边」的字面意思，
    /// 不挡掉的话一条自环会在两档里各报一次，人看到两条要处理的东西而其实是一件。
    #[test]
    fn a_self_loop_is_reported_once_not_twice() {
        let edges = [e(1, 1, 1)];
        let v = check(
            &edges,
            &with(Axioms {
                irreflexive: true,
                asymmetric: true,
                ..Default::default()
            }),
        );
        assert_eq!(kinds(&v), vec![Kind::SelfLoop]);
    }

    #[test]
    fn a_pair_pointing_both_ways_needs_asymmetric() {
        let edges = [e(1, 1, 2), e(2, 2, 1)];
        assert!(check(
            &edges,
            &with(Axioms {
                transitive: true,
                ..Default::default()
            })
        )
        .iter()
        .all(|v| v.kind != Kind::Asymmetry));
        let v = check(
            &edges,
            &with(Axioms {
                asymmetric: true,
                ..Default::default()
            }),
        );
        assert_eq!(kinds(&v), vec![Kind::Asymmetry]);
        assert_eq!((v[0].left, v[0].right), (f(1), f(2)));
    }

    /// 长环要报出**路径**，而不只是「首尾矛盾」——人要顺着看一遍才知道撤哪一条。
    #[test]
    fn a_long_cycle_reports_the_whole_path() {
        let edges = [e(1, 1, 2), e(2, 2, 3), e(3, 3, 4), e(4, 4, 1)];
        let v = check(
            &edges,
            &with(Axioms {
                transitive: true,
                ..Default::default()
            }),
        );
        assert_eq!(v.len(), 1, "一个环只报一次");
        assert_eq!(v[0].path.len(), 4, "四条边都该在路径里");
    }

    /// **同一个环从不同节点出发会被走到 n 次。** 去重是这个函数存在的一半理由：
    /// 报四遍就是让人把同一件事看四遍。
    #[test]
    fn one_cycle_is_one_finding_however_many_ways_in() {
        let edges = [e(1, 1, 2), e(2, 2, 3), e(3, 3, 1)];
        let v = check(
            &edges,
            &with(Axioms {
                transitive: true,
                ..Default::default()
            }),
        );
        assert_eq!(v.len(), 1);
    }

    /// 两个互不相干的环各报一次。
    #[test]
    fn separate_cycles_stay_separate() {
        let edges = [e(1, 1, 2), e(2, 2, 1), e(3, 5, 6), e(4, 6, 5)];
        let v = check(
            &edges,
            &with(Axioms {
                transitive: true,
                ..Default::default()
            }),
        );
        assert_eq!(v.len(), 2);
    }

    /// **深度上限挡得住不收敛。** 0002 在真实语料上量到 `part_of` 闭包深度到 10
    /// 仍在振荡；没有上限，一条长链加一个环就能让求值不终止。
    ///
    /// 这里造一条比上限更长的链再闭合——它不该让检查挂住，报不报得出那个环是
    /// 次要的，**不挂住是首要的**。
    #[test]
    fn a_chain_longer_than_the_limit_still_terminates() {
        let mut edges: Vec<Edge> = (0..30).map(|i| e(i, i, i + 1)).collect();
        edges.push(e(60, 30, 0));
        let v = check(
            &edges,
            &with(Axioms {
                transitive: true,
                ..Default::default()
            }),
        );
        // 断言的是"跑完了"，长度不作要求
        assert!(v.len() <= 1);
    }

    #[test]
    fn functional_and_its_inverse_are_two_directions_of_one_check() {
        // 同一个主语指了两个宾语
        let out = [e(1, 1, 2), e(2, 1, 3)];
        assert_eq!(
            kinds(&check(
                &out,
                &with(Axioms {
                    functional: true,
                    ..Default::default()
                })
            )),
            vec![Kind::Functional]
        );
        assert!(check(
            &out,
            &with(Axioms {
                inverse_functional: true,
                ..Default::default()
            })
        )
        .is_empty());

        // 同一个宾语被两个主语指
        let inn = [e(1, 2, 1), e(2, 3, 1)];
        assert_eq!(
            kinds(&check(
                &inn,
                &with(Axioms {
                    inverse_functional: true,
                    ..Default::default()
                })
            )),
            vec![Kind::Functional]
        );
        assert!(check(
            &inn,
            &with(Axioms {
                functional: true,
                ..Default::default()
            })
        )
        .is_empty());
    }

    /// 同一个主语指五个宾语只报一对。报十对（两两组合）是把同一件事说十遍——
    /// 人要处理的是「这里有冲突」，看一对就够去查了。
    #[test]
    fn one_finding_per_conflicting_key_not_one_per_pair() {
        let edges = [e(1, 1, 2), e(2, 1, 3), e(3, 1, 4), e(4, 1, 5), e(5, 1, 6)];
        let v = check(
            &edges,
            &with(Axioms {
                functional: true,
                ..Default::default()
            }),
        );
        assert_eq!(v.len(), 1);
    }

    /// 同一个主语两次指向**同一个**宾语不是冲突——重复断言而已。
    #[test]
    fn saying_the_same_thing_twice_is_not_a_contradiction() {
        let edges = [e(1, 1, 2), e(2, 1, 2)];
        assert!(check(
            &edges,
            &with(Axioms {
                functional: true,
                ..Default::default()
            })
        )
        .is_empty());
    }

    /// **公理挂在谓词上，跨谓词的边之间没有可比性。**
    /// `A p B` 与 `B q A` 同时成立不是矛盾，哪怕 p 声明了反对称。
    #[test]
    fn axioms_do_not_leak_across_predicates() {
        let a = Edge {
            fact: f(1),
            predicate: n(90),
            subject: n(1),
            object: n(2),
        };
        let b = Edge {
            fact: f(2),
            predicate: n(91),
            subject: n(2),
            object: n(1),
        };
        let ax = HashMap::from([
            (
                n(90),
                Axioms {
                    asymmetric: true,
                    ..Default::default()
                },
            ),
            (
                n(91),
                Axioms {
                    asymmetric: true,
                    ..Default::default()
                },
            ),
        ]);
        assert!(check(&[a, b], &ax).is_empty());
    }
}
