//! 本体自己的自洽性:**不碰事实,只看定义**。
//!
//! 与 [`crate::check`] 是两件事。那一层问「事实与定义抵触吗」,这一层问
//! 「定义自己站得住吗」。分开不是分类癖:一个自相矛盾的本体会让事实层的
//! 结论全部可疑——若某个谓词同时声明了 symmetric 与 asymmetric,那么据它
//! 报出来的每一条反对称违规都建立在一个本来就不成立的前提上。所以这一层
//! 的结论要**排在前面**给人看。
//!
//! 便宜也是理由:输入只有几千行本体,不用扫账本。
//!
//! **同样是「没声明就不查」。** 这里查的每一条都对应本体里写下来的东西,
//! 没有一条是我们替用户假设的。

use crate::Axioms;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// 类层级上溯的深度上限。与 [`crate::MAX_DEPTH`] 同一个理由:本体里可以有环
/// （建表时只挡得住自环,`A → B → A` 拦不住），没有上限就不终止。
pub const MAX_ANCESTRY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Defect {
    /// 同一个谓词既声明 symmetric 又声明 asymmetric。
    ///
    /// OWL 里这两者只能同时对**空**属性成立——一旦有一条边 `A p B`，对称说
    /// `B p A` 必须成立，反对称说它必须不成立。所以但凡这个谓词上有事实，
    /// 声明就一定错了一个。
    SymmetricAndAsymmetric,
    /// 传递 + 函数性。OWL 2 DL 明文禁止（函数性属性不得声明为传递），
    /// 因为两者一起会让推理跳出可判定的片段。
    ///
    /// 直觉上也讲得通：函数性说「主语侧只有一个值」，传递说「顺着链一直推」，
    /// 而链上第二跳就给同一个主语推出了第二个值。
    TransitiveAndFunctional,
    /// subClassOf 绕成了环。建表时的 CHECK 只挡得住 `A → A`。
    ///
    /// 环意味着环上所有类互为子类，即它们其实是同一个类——而它们有各自的
    /// 标签、描述、属性，界面上也各画一行。
    SubclassCycle,
    /// 一个类跟自己的祖先声明了互斥 → 这个类**永远不可能有实例**。
    /// 它继承了祖先的身份，又声明与之互斥。
    DisjointWithAncestor,
    /// 一个类的两个祖先互相互斥 → 同上，不可满足。
    /// 多父继承下这个形状不罕见：两支各自合理，合起来就矛盾了。
    InheritsDisjoint,
    /// 一个谓词声明自己是自己的逆。**等价于 symmetric**——推理照跑，
    /// 只是读的人要多想一步。提示改写成 `symmetric` 更直白
    InverseOfItself,
    /// `p⁻¹ = q` 而 `q⁻¹ = r`，两边指得不一样。
    ///
    /// 载入公理时只补空缺、不覆盖人写的（**人写的优先于推出来的**），
    /// 所以这个矛盾不会被悄悄抹平，留到这里报。
    InverseNotMutual,
    /// subPropertyOf 绕成了环。与 [`Defect::SubclassCycle`] 同形：
    /// 环上所有谓词互为子属性 = 它们其实是同一个谓词，而各自有标签、有事实。
    SubPropertyCycle,
}

impl Defect {
    pub fn as_str(self) -> &'static str {
        match self {
            Defect::SymmetricAndAsymmetric => "symmetric_and_asymmetric",
            Defect::TransitiveAndFunctional => "transitive_and_functional",
            Defect::SubclassCycle => "subclass_cycle",
            Defect::DisjointWithAncestor => "disjoint_with_ancestor",
            Defect::InheritsDisjoint => "inherits_disjoint",
            Defect::InverseOfItself => "inverse_of_itself",
            Defect::InverseNotMutual => "inverse_not_mutual",
            Defect::SubPropertyCycle => "sub_property_cycle",
        }
    }
}

/// 本体里的一处自相矛盾。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OntologyDefect {
    pub kind: Defect,
    /// 出问题的那个对象：谓词（前两类）或类（后三类）
    pub subject: Uuid,
    /// 另一方：互斥的那个类。前两类与环没有第二方
    pub other: Option<Uuid>,
    /// 环的路径（按类排列），或从类到那个祖先的路径。其余为空
    pub path: Vec<Uuid>,
}

/// 量一遍本体自己。
///
/// `parents` 是 `(子, 父)` 对，`disjoint` 是互斥对——**导入侧已经把对称性
/// 展开成两行**，所以这里两个方向都会看到，去重靠有序键。
pub fn check_ontology(
    axioms: &HashMap<Uuid, Axioms>,
    parents: &[(Uuid, Uuid)],
    disjoint: &[(Uuid, Uuid)],
) -> Vec<OntologyDefect> {
    let mut out = Vec::new();

    // ---- 谓词上的两处自相矛盾。**排序后再报**：HashMap 的遍历顺序每次不同，
    // 而同一个本体两次检查出来的结果该是同一份
    let mut preds: Vec<(&Uuid, &Axioms)> = axioms.iter().collect();
    preds.sort_by_key(|(id, _)| **id);
    for (&pred, ax) in preds {
        if ax.symmetric && ax.asymmetric {
            out.push(OntologyDefect {
                kind: Defect::SymmetricAndAsymmetric,
                subject: pred,
                other: None,
                path: Vec::new(),
            });
        }
        if ax.transitive && (ax.functional || ax.inverse_functional) {
            out.push(OntologyDefect {
                kind: Defect::TransitiveAndFunctional,
                subject: pred,
                other: None,
                path: Vec::new(),
            });
        }
        // 自己是自己的逆 = 对称。**不是错，是绕远路**——推理照跑，
        // 但读本体的人得自己想一步才明白。提示改用 `symmetric` 更直白
        if ax.inverse_of == Some(pred) {
            out.push(OntologyDefect {
                kind: Defect::InverseOfItself,
                subject: pred,
                other: None,
                path: Vec::new(),
            });
        }
        // 逆 + 反对称：`A p B` 推出 `B p A`（自己的逆），而反对称说这不成立。
        // 与 symmetric+asymmetric 同一个矛盾，换了个写法进来
        if ax.inverse_of == Some(pred) && ax.asymmetric {
            out.push(OntologyDefect {
                kind: Defect::SymmetricAndAsymmetric,
                subject: pred,
                other: None,
                path: Vec::new(),
            });
        }
        // 两边各自声明了逆，却指向不同的谓词。**不在载入时悄悄改一致**
        // （`reasoning::axioms` 那边只补空缺，不覆盖人写的），所以在这里报
        if let Some(inv) = ax.inverse_of {
            if let Some(back) = axioms.get(&inv).and_then(|a| a.inverse_of) {
                if back != pred {
                    out.push(OntologyDefect {
                        kind: Defect::InverseNotMutual,
                        subject: pred,
                        other: Some(inv),
                        path: vec![pred, inv, back],
                    });
                }
            }
        }
    }

    // ---- subPropertyOf 成环。与 subClassOf 的环是同一个形状：
    // 环上所有谓词互为子属性 = 它们其实是同一个谓词，而各自有标签、有事实
    {
        let parent_of: HashMap<Uuid, Uuid> = axioms
            .iter()
            .filter_map(|(id, ax)| ax.sub_property_of.map(|p| (*id, p)))
            .collect();
        let mut starts: Vec<Uuid> = parent_of.keys().copied().collect();
        starts.sort();
        let mut reported: HashSet<Uuid> = HashSet::new();
        for start in starts {
            if reported.contains(&start) {
                continue;
            }
            let mut seen: Vec<Uuid> = Vec::new();
            let mut cur = start;
            for _ in 0..MAX_ANCESTRY {
                if seen.contains(&cur) {
                    // 环上每个成员都标记过，整条环只报一次
                    for m in &seen {
                        reported.insert(*m);
                    }
                    out.push(OntologyDefect {
                        kind: Defect::SubPropertyCycle,
                        subject: start,
                        other: None,
                        path: seen.clone(),
                    });
                    break;
                }
                seen.push(cur);
                match parent_of.get(&cur) {
                    Some(&p) => cur = p,
                    None => break,
                }
            }
        }
    }

    let up = adjacency(parents);
    out.extend(subclass_cycles(&up));
    out.extend(unsatisfiable(&up, disjoint));
    out
}

/// 子 → 父的邻接表。同一对重复声明只留一次。
fn adjacency(parents: &[(Uuid, Uuid)]) -> HashMap<Uuid, Vec<Uuid>> {
    let mut up: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for &(child, parent) in parents {
        let slot = up.entry(child).or_default();
        if !slot.contains(&parent) {
            slot.push(parent);
        }
    }
    // 排序保证同一份本体每次算出同一条路径
    for v in up.values_mut() {
        v.sort();
    }
    up
}

/// subClassOf 的环。每个环只报一次，键取环上类的有序集合。
fn subclass_cycles(up: &HashMap<Uuid, Vec<Uuid>>) -> Vec<OntologyDefect> {
    let mut reported: HashSet<Vec<Uuid>> = HashSet::new();
    let mut out = Vec::new();
    let mut starts: Vec<Uuid> = up.keys().copied().collect();
    starts.sort();
    for start in starts {
        let mut path = Vec::new();
        let mut on_path = HashSet::new();
        climb(
            start,
            start,
            up,
            &mut path,
            &mut on_path,
            &mut reported,
            &mut out,
        );
    }
    out
}

fn climb(
    start: Uuid,
    at: Uuid,
    up: &HashMap<Uuid, Vec<Uuid>>,
    path: &mut Vec<Uuid>,
    on_path: &mut HashSet<Uuid>,
    reported: &mut HashSet<Vec<Uuid>>,
    out: &mut Vec<OntologyDefect>,
) {
    if path.len() >= MAX_ANCESTRY {
        return;
    }
    let Some(ups) = up.get(&at) else {
        return;
    };
    for &parent in ups {
        if parent == start && !path.is_empty() {
            // 回到起点：这是一个环。`path` 此刻是 start 之后的那几个类
            let mut ring = vec![start];
            ring.extend(path.iter().copied());
            let mut key = ring.clone();
            key.sort();
            key.dedup();
            if reported.insert(key) {
                out.push(OntologyDefect {
                    kind: Defect::SubclassCycle,
                    subject: start,
                    other: None,
                    path: ring,
                });
            }
            continue;
        }
        if on_path.contains(&parent) || parent == start {
            // 别处的环，等它自己那一轮报；这里只是别走进去
            continue;
        }
        path.push(parent);
        on_path.insert(parent);
        climb(start, parent, up, path, on_path, reported, out);
        on_path.remove(&parent);
        path.pop();
    }
}

/// 不可满足的类：它的祖先集合里出现了一对互斥。
///
/// 两种形状分开报，因为**给人的话不一样**：跟自己的祖先互斥是「这条 disjoint
/// 声明写反了」，而两个祖先互斥是「这个类不该同时挂在这两支下」。
fn unsatisfiable(up: &HashMap<Uuid, Vec<Uuid>>, disjoint: &[(Uuid, Uuid)]) -> Vec<OntologyDefect> {
    let pairs: HashSet<(Uuid, Uuid)> = disjoint.iter().copied().collect();
    if pairs.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut classes: Vec<Uuid> = up.keys().copied().collect();
    classes.sort();
    for class in classes {
        let anc = ancestors(class, up);
        // 一、跟自己的祖先互斥
        for &a in &anc {
            if pairs.contains(&(class, a)) {
                out.push(OntologyDefect {
                    kind: Defect::DisjointWithAncestor,
                    subject: class,
                    other: Some(a),
                    path: Vec::new(),
                });
            }
        }
        // 二、两个祖先互相互斥。有序对去重，否则展开成两行的 disjoint 会报两遍
        let mut sorted: Vec<Uuid> = anc.iter().copied().collect();
        sorted.sort();
        for (i, &a) in sorted.iter().enumerate() {
            for &b in &sorted[i + 1..] {
                if pairs.contains(&(a, b)) {
                    out.push(OntologyDefect {
                        kind: Defect::InheritsDisjoint,
                        subject: class,
                        other: Some(b),
                        path: vec![a],
                    });
                }
            }
        }
    }
    out
}

/// 一个类的全部祖先（不含自己）。有环也不会转不出来——`seen` 挡住。
fn ancestors(class: Uuid, up: &HashMap<Uuid, Vec<Uuid>>) -> HashSet<Uuid> {
    let mut seen = HashSet::new();
    let mut queue = vec![(class, 0usize)];
    while let Some((at, depth)) = queue.pop() {
        if depth >= MAX_ANCESTRY {
            continue;
        }
        let Some(ups) = up.get(&at) else {
            continue;
        };
        for &parent in ups {
            if parent != class && seen.insert(parent) {
                queue.push((parent, depth + 1));
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(i: u8) -> Uuid {
        Uuid::from_bytes([i; 16])
    }
    fn kinds(v: &[OntologyDefect]) -> Vec<Defect> {
        let mut k: Vec<Defect> = v.iter().map(|d| d.kind).collect();
        k.sort();
        k
    }
    fn ax(f: impl Fn(&mut Axioms)) -> HashMap<Uuid, Axioms> {
        let mut a = Axioms::default();
        f(&mut a);
        HashMap::from([(c(99), a)])
    }

    #[test]
    fn a_property_cannot_be_both_symmetric_and_asymmetric() {
        let a = ax(|a| {
            a.symmetric = true;
            a.asymmetric = true;
        });
        let d = check_ontology(&a, &[], &[]);
        assert_eq!(kinds(&d), vec![Defect::SymmetricAndAsymmetric]);
        assert_eq!(d[0].subject, c(99));
    }

    #[test]
    fn either_one_alone_is_fine() {
        assert!(check_ontology(&ax(|a| a.symmetric = true), &[], &[]).is_empty());
        assert!(check_ontology(&ax(|a| a.asymmetric = true), &[], &[]).is_empty());
        // 传递 + 反对称是**正常的**——它正是环检测有意义的前提
        let both = ax(|a| {
            a.transitive = true;
            a.asymmetric = true;
        });
        assert!(check_ontology(&both, &[], &[]).is_empty());
    }

    #[test]
    fn transitive_and_functional_is_forbidden_by_owl2() {
        let a = ax(|a| {
            a.transitive = true;
            a.functional = true;
        });
        assert_eq!(
            kinds(&check_ontology(&a, &[], &[])),
            vec![Defect::TransitiveAndFunctional]
        );
        // 反函数性同理
        let b = ax(|a| {
            a.transitive = true;
            a.inverse_functional = true;
        });
        assert_eq!(
            kinds(&check_ontology(&b, &[], &[])),
            vec![Defect::TransitiveAndFunctional]
        );
    }

    #[test]
    fn subclass_of_can_form_a_ring() {
        let none = HashMap::new();
        // 1 → 2 → 3 → 1
        let ring = [(c(1), c(2)), (c(2), c(3)), (c(3), c(1))];
        let d = check_ontology(&none, &ring, &[]);
        assert_eq!(kinds(&d), vec![Defect::SubclassCycle], "三个类绕成一圈");
        assert_eq!(d.len(), 1, "同一个环只报一次，不是每个类各报一次");
        assert_eq!(d[0].path.len(), 3, "路径要带上环上全部三个类");
    }

    #[test]
    fn a_tree_is_not_a_ring() {
        let none = HashMap::new();
        // 多父也不是环：4 同时挂在 2 与 3 下
        let tree = [(c(1), c(2)), (c(1), c(3)), (c(4), c(2)), (c(4), c(3))];
        assert!(check_ontology(&none, &tree, &[]).is_empty());
    }

    #[test]
    fn a_class_disjoint_with_its_own_ancestor_can_never_exist() {
        let none = HashMap::new();
        let parents = [(c(1), c(2)), (c(2), c(3))];
        // 导入侧把 disjoint 的对称性展开成两行，这里照样给两行
        let dis = [(c(1), c(3)), (c(3), c(1))];
        let d = check_ontology(&none, &parents, &dis);
        assert_eq!(kinds(&d), vec![Defect::DisjointWithAncestor]);
        assert_eq!(d[0].subject, c(1));
        assert_eq!(d[0].other, Some(c(3)), "指出跟哪个祖先互斥——人要去改那一条");
    }

    #[test]
    fn two_disjoint_ancestors_make_a_class_unsatisfiable() {
        let none = HashMap::new();
        // 1 同时是 2 与 3 的子类，而 2 与 3 互斥
        let parents = [(c(1), c(2)), (c(1), c(3))];
        let dis = [(c(2), c(3)), (c(3), c(2))];
        let d = check_ontology(&none, &parents, &dis);
        assert_eq!(kinds(&d), vec![Defect::InheritsDisjoint]);
        assert_eq!(d.len(), 1, "展开成两行的 disjoint 不该报两遍");
        assert_eq!(d[0].subject, c(1));
    }

    #[test]
    fn disjoint_between_unrelated_branches_is_the_point_of_disjoint() {
        let none = HashMap::new();
        // 2 与 3 互斥，而 1 只挂在 2 下、4 只挂在 3 下——这正是 disjoint 的正常用法
        let parents = [(c(1), c(2)), (c(4), c(3))];
        let dis = [(c(2), c(3)), (c(3), c(2))];
        assert!(check_ontology(&none, &parents, &dis).is_empty());
    }

    #[test]
    fn a_ring_does_not_hang_the_ancestor_walk() {
        let none = HashMap::new();
        let ring = [(c(1), c(2)), (c(2), c(1))];
        let dis = [(c(1), c(2)), (c(2), c(1))];
        // 环 + 互斥同时存在：既要报环，也不能在上溯时转不出来
        let d = check_ontology(&none, &ring, &dis);
        assert!(d.iter().any(|x| x.kind == Defect::SubclassCycle));
        assert!(d.iter().any(|x| x.kind == Defect::DisjointWithAncestor));
    }

    #[test]
    fn nothing_declared_means_nothing_reported() {
        assert!(check_ontology(&HashMap::new(), &[], &[]).is_empty());
        // 类层级齐全但一条 disjoint 都没有 → 没有判据
        let parents = [(c(1), c(2)), (c(2), c(3))];
        assert!(check_ontology(&HashMap::new(), &parents, &[]).is_empty());
    }
}

#[cfg(test)]
mod inverse_and_sub_property_tests {
    use super::*;

    fn c(i: u8) -> Uuid {
        Uuid::from_bytes([i; 16])
    }
    fn kinds(v: &[OntologyDefect]) -> Vec<Defect> {
        let mut k: Vec<Defect> = v.iter().map(|d| d.kind).collect();
        k.sort();
        k
    }
    fn only(id: Uuid, a: Axioms) -> HashMap<Uuid, Axioms> {
        HashMap::from([(id, a)])
    }

    /// 自己是自己的逆 —— 合法但绕远路，提示改用 symmetric。
    #[test]
    fn a_predicate_that_is_its_own_inverse_should_just_say_symmetric() {
        let a = Axioms {
            inverse_of: Some(c(1)),
            ..Default::default()
        };
        let d = check_ontology(&only(c(1), a), &[], &[]);
        assert_eq!(kinds(&d), vec![Defect::InverseOfItself]);
    }

    /// 自己的逆 + 反对称 = 与 symmetric+asymmetric 同一个矛盾，换了个写法。
    #[test]
    fn its_own_inverse_and_asymmetric_is_the_same_contradiction_in_disguise() {
        let a = Axioms {
            inverse_of: Some(c(1)),
            asymmetric: true,
            ..Default::default()
        };
        let d = check_ontology(&only(c(1), a), &[], &[]);
        assert!(
            kinds(&d).contains(&Defect::SymmetricAndAsymmetric),
            "**要报成同一类**：读的人不该因为写法不同就以为是两回事"
        );
    }

    /// 互指一致 —— 干净，什么都不该报。
    #[test]
    fn a_mutual_pair_is_clean() {
        let ax = HashMap::from([
            (
                c(1),
                Axioms {
                    inverse_of: Some(c(2)),
                    ..Default::default()
                },
            ),
            (
                c(2),
                Axioms {
                    inverse_of: Some(c(1)),
                    ..Default::default()
                },
            ),
        ]);
        assert!(check_ontology(&ax, &[], &[]).is_empty());
    }

    /// `p⁻¹ = q` 而 `q⁻¹ = r` —— 两边指得不一样。
    ///
    /// **载入公理时只补空缺不覆盖**，所以这个矛盾不会被悄悄抹平；
    /// 它必须在这里被报出来，否则没有任何地方会提。
    #[test]
    fn an_inverse_that_does_not_point_back_is_reported() {
        let ax = HashMap::from([
            (
                c(1),
                Axioms {
                    inverse_of: Some(c(2)),
                    ..Default::default()
                },
            ),
            (
                c(2),
                Axioms {
                    inverse_of: Some(c(3)),
                    ..Default::default()
                },
            ),
        ]);
        let d = check_ontology(&ax, &[], &[]);
        let one = d
            .iter()
            .find(|x| x.kind == Defect::InverseNotMutual)
            .expect("该报 InverseNotMutual");
        assert_eq!(one.subject, c(1));
        assert_eq!(one.other, Some(c(2)));
        assert_eq!(one.path, vec![c(1), c(2), c(3)], "路径要说清指到哪去了");
    }

    /// subPropertyOf 成环。
    #[test]
    fn a_sub_property_ring_is_a_single_predicate_wearing_three_hats() {
        let mk = |parent: u8| Axioms {
            sub_property_of: Some(c(parent)),
            ..Default::default()
        };
        let ax = HashMap::from([(c(1), mk(2)), (c(2), mk(3)), (c(3), mk(1))]);
        let d = check_ontology(&ax, &[], &[]);
        let ring: Vec<&OntologyDefect> = d
            .iter()
            .filter(|x| x.kind == Defect::SubPropertyCycle)
            .collect();
        assert_eq!(ring.len(), 1, "**整条环只报一次**，不是每个成员报一遍");
        assert_eq!(ring[0].path.len(), 3);
    }

    /// 一条不成环的链不该被误报。
    #[test]
    fn a_chain_that_ends_is_not_a_ring() {
        let ax = HashMap::from([
            (
                c(1),
                Axioms {
                    sub_property_of: Some(c(2)),
                    ..Default::default()
                },
            ),
            (
                c(2),
                Axioms {
                    sub_property_of: Some(c(3)),
                    ..Default::default()
                },
            ),
        ]);
        assert!(check_ontology(&ax, &[], &[]).is_empty());
    }
}
