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

pub mod derive;
pub mod ontology;
pub mod rules;

use derive::TimedEdge;
use std::collections::hash_map::Entry;
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
    /// `p⁻¹ = q`：这个谓词的逆是哪一个。**跨谓词的规则从这里来**——
    /// `A p B` 推出 `B q A`，而 R0 的检查也要看它（自己是自己的逆等于对称）
    pub inverse_of: Option<Uuid>,
    /// `p ⊑ q`：断言了具体的，通用的也成立。链要防成环，R0 那边查
    pub sub_property_of: Option<Uuid>,
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
    /// 环取首尾两条（中间那些在 `path` 里）；互斥的三类（asymmetry、functional、
    /// inverse_functional）取整组里 id 最小与最大的两条。
    pub left: Uuid,
    pub right: Uuid,
    /// 环的完整路径，按事实排列；互斥的三类是整组事实，按 id 排序；自环为空。
    /// 留着是因为「A→B→C→A」比「A 与 C 矛盾」有用得多——人要顺着看一遍才知道
    /// 该撤哪一条
    pub path: Vec<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `A p A`，而 p 声明了 irreflexive
    SelfLoop,
    /// `A p B` 且 `B p A`，而 p 声明了 asymmetric
    Asymmetry,
    /// `A p B p … p A`，而 p 声明了 transitive——闭包里推出 `A p A`
    Cycle,
    /// 同一主语与谓词在同一段时间里指向两个宾语，而 p 声明了 functional
    Functional,
    /// 同一宾语在同一段时间里被两个主语指，而 p 声明了 inverse_functional。
    ///
    /// 与 `Functional` 分开记（#634）：从前两个方向共用一个种类，一个项目同时有两位
    /// 负责人被写成「该只有一个值，却有两个」，人照着去主语那一侧找，找的是错的一端
    InverseFunctional,
    /// `A p B`，而 A 不在 p 声明的 domain 里、或 B 不在 range 里（#190 / #196）。
    ///
    /// **这一类不由本 crate 算出**：它要看实体的类型与 domain / range 的闭包，那是
    /// 库里的东西，`utopia-store::reasoning::signature_breaks` 用 SQL 量。列在这里
    /// 是为了与其它四类走同一条落库、清陈、裁决的路——left 与 right 同一条事实，
    /// 与自反那类同款
    Signature,
    /// 一条派生撞上了一条断言（0017）：推出来的 `A p B` 与账本里的某条断言在 p 的
    /// 公理上不能并存。派生不落地，这一行把它摆到人面前。`left` 是被撞的断言，
    /// `right` 是派生的最后一条前提，`path` 是全部前提；推出来的三元组本身在
    /// `axiom_violations.detail` 里——它没有落库，没有 id 可指
    DerivedContradiction,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::SelfLoop => "self_loop",
            Kind::Asymmetry => "asymmetry",
            Kind::Cycle => "cycle",
            Kind::Functional => "functional",
            Kind::InverseFunctional => "inverse_functional",
            Kind::Signature => "signature",
            Kind::DerivedContradiction => "derived_contradiction",
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

/// 一个谓词上最多报多少个环（#642）。
///
/// 上千个环不是上千件事：它们挤在同一团互相可达的节点里，根子多半是这个谓词不该
/// 声明 transitive，或者有一两条边方向写反了。逐个端进 Review 只会淹掉队列，与
/// `MAX_CLASHES_PER_PREDICATE` 同一个道理
pub const MAX_CYCLES_PER_PREDICATE: usize = 1000;

/// 一个谓词上找环最多走多少步（每看一条边算一步，#642）。
///
/// **环数上限管不住工作量。** 一团稠密的强连通分量里，深度 12 以内的简单路径是
/// 指数级的，大多数根本不回到起点——步数没有上限，一次检查就能把请求挂住、把内存
/// 撑满。量过：500 个节点、2000 条边、两成随机回边，从前 60 秒没跑完。
pub const MAX_CYCLE_STEPS: usize = 1_000_000;

/// 一次检查的完整结果：违规，以及哪些谓词的环没搜完。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checked {
    pub violations: Vec<Violation>,
    /// 撞上 `MAX_CYCLES_PER_PREDICATE` 或 `MAX_CYCLE_STEPS`、环没搜完的谓词，排过序。
    /// **必须回给调用方**：没搜完不能说没有——落库那边据此不清这些谓词上旧的环，
    /// 界面据此说「这个谓词的环太多，先看公理」
    pub cycles_capped: Vec<Uuid>,
}

/// 拿公理量一遍这批边。
///
/// 每个谓词各查各的：公理是挂在谓词上的，跨谓词的边之间没有可比性
/// （`A part_of B` 与 `B produces A` 同时成立不是矛盾）。
///
/// **互斥的三类要求两条事实同时成立**（#634）。functional、inverse_functional、
/// asymmetric 说的都是「同一时刻」：Lin Zhao 的薪水在 2024-02-20 从 28000 变成
/// 32000，两段首尾相接，那是一次调薪。从前这里只收 `Edge`，区间在调用方就丢了，
/// 于是每一次接任都进了 Review。判据与 `derive::contradictions` 同一个——派生那一侧
/// 一直是按区间重叠判的，断言这一侧跟它对齐。
///
/// **环要整条路径在同一时刻成立**（#636）。A 在 2019–2021 年并入 B、2022 年起 B
/// 又并入 A，图在任何一刻都没有这个环；两两重叠也不够，三条边可以两两相交而三者
/// 无交。所以沿路径求交集，与 `derive::validity` 对前提做的是同一件事。
///
/// 区间照 `TimedEdge` 的读法：半开 `[from, to)`，`None` 是那一侧无界（恒常谓词两端
/// 都是 `None`，于是退回到只看形状）。自环不看时间：`A p A` 哪一刻成立都是错的。
pub fn check(edges: &[TimedEdge], axioms: &HashMap<Uuid, Axioms>) -> Vec<Violation> {
    check_all(edges, axioms).violations
}

/// 同 [`check`]，另外说出哪些谓词的环没搜完（#642）。
pub fn check_all(edges: &[TimedEdge], axioms: &HashMap<Uuid, Axioms>) -> Checked {
    let mut out = Vec::new();
    let mut cycles_capped = Vec::new();
    let mut by_pred: HashMap<Uuid, Vec<TimedEdge>> = HashMap::new();
    for t in edges {
        let Some(ax) = axioms.get(&t.edge.predicate) else {
            continue;
        };
        if ax.says_nothing() {
            continue;
        }
        by_pred.entry(t.edge.predicate).or_default().push(*t);
    }
    for (pred, group) in by_pred {
        let ax = axioms[&pred];
        let shapes: Vec<Edge> = group.iter().map(|t| t.edge).collect();
        if ax.irreflexive {
            out.extend(self_loops(&shapes));
        }
        if ax.asymmetric {
            out.extend(asymmetries(&group));
        }
        if ax.transitive {
            let (found, capped) = cycles(&group);
            out.extend(found);
            if capped {
                cycles_capped.push(pred);
            }
        }
        if ax.functional {
            out.extend(clashes(
                &group,
                Kind::Functional,
                |e| (e.subject, Uuid::nil()),
                |a, b| a.object != b.object,
            ));
        }
        if ax.inverse_functional {
            out.extend(clashes(
                &group,
                Kind::InverseFunctional,
                |e| (e.object, Uuid::nil()),
                |a, b| a.subject != b.subject,
            ));
        }
    }
    cycles_capped.sort();
    Checked {
        violations: out,
        cycles_capped,
    }
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

/// 反对称：同一对节点在同一段时间里两个方向都断言了。
///
/// 键是无序的那一对节点，「不同」是方向相反——与 functional 同一个形状，所以走
/// 同一个函数。
fn asymmetries(edges: &[TimedEdge]) -> Vec<Violation> {
    // 自环由 irreflexive 那一档负责；反对称在这里报一遍是重复
    let two_way: Vec<TimedEdge> = edges
        .iter()
        .filter(|t| t.edge.subject != t.edge.object)
        .copied()
        .collect();
    clashes(
        &two_way,
        Kind::Asymmetry,
        |e| (e.subject.min(e.object), e.subject.max(e.object)),
        |a, b| a.subject != b.subject,
    )
}

/// 找环。**每个环只报一次，而且只以一种形状报**：报出来的路径转到最小的那条事实
/// 起头（见 `walk` 里的注释）。
///
/// 用深度优先而不是半朴素闭包求值：两者都能发现环，但闭包只告诉你「A 推出了
/// A」，而人要的是**路径**——顺着 `A→B→C→A` 看一遍才知道该撤哪一条。闭包丢掉
/// 的正是这个。
///
/// **只在强连通分量里找，每个环只从它最小的节点找一次**（#642）。从前从每个节点出发
/// 把深度 12 以内的路径全走一遍，再按事实集合去重。环只可能落在一个强连通分量里，
/// 而真实的 `part_of` 大体是一棵层级、只有几条回边：分量很小，绝大多数路径一路往下
/// 走进 DAG、永远回不来。量过：一个宽 4、14 层的层级加一条回边（209 条边、256 个环）
/// 从前要 10 秒，2 万个节点的层级要 8 秒。先算分量、只走分量内的边，这些路径一步都
/// 不走；再规定环只从它编号最小的节点出发、途中只经过比起点大的节点，每个环恰好被
/// 走到一次，去重的集合也省了。
///
/// 真正稠密的大分量里环本身就是指数级的，这两条救不了——那时撞上
/// `MAX_CYCLES_PER_PREDICATE` / `MAX_CYCLE_STEPS` 就停，第二个返回值为真。起点与每个
/// 节点的出边都排过序，所以停在哪儿只取决于数据：同一份数据跑多少遍、以什么顺序
/// 读进来，报出来的都是同一批环。
///
/// R1 物化推导要的是闭包本身，那时再建；R0 要的是「哪几条边凑成了环」。
fn cycles(edges: &[TimedEdge]) -> (Vec<Violation>, bool) {
    // 自环不是这里的事（irreflexive 那一档管），也不可能在两个以上节点的环上
    let mut next: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for t in edges.iter().filter(|t| t.edge.subject != t.edge.object) {
        next.entry(t.edge.subject).or_default().push(t.edge.object);
    }
    let mut nodes: Vec<Uuid> = next.keys().copied().collect();
    nodes.sort_unstable();
    let component = components(&nodes, &next);

    // 分量内的边才可能在环上；出边按 (宾语, 事实) 排，遍历顺序只取决于数据
    let mut adj: HashMap<Uuid, Vec<&TimedEdge>> = HashMap::new();
    for t in edges {
        let (s, o) = (t.edge.subject, t.edge.object);
        let same = matches!((component.get(&s), component.get(&o)), (Some(a), Some(b)) if a == b);
        if s != o && same {
            adj.entry(s).or_default().push(t);
        }
    }
    for out in adj.values_mut() {
        out.sort_unstable_by_key(|t| (t.edge.object, t.edge.fact));
    }
    let mut starts: Vec<Uuid> = adj.keys().copied().collect();
    starts.sort_unstable();

    let mut search = Search {
        adj: &adj,
        path: Vec::new(),
        on_path: HashSet::new(),
        out: Vec::new(),
        steps: 0,
        capped: false,
    };
    for start in starts {
        search.walk(start, start, (None, None));
        if search.capped {
            break;
        }
    }
    (search.out, search.capped)
}

/// 强连通分量：每个节点所在分量的编号。
///
/// Tarjan，**迭代版**：递归版的调用深度等于最长的那条链，几万个节点的层级在它面前
/// 就是几万层递归，栈会炸。这里自己维护调用栈 `(节点, 下一个要看的后继)`。
fn components(nodes: &[Uuid], next: &HashMap<Uuid, Vec<Uuid>>) -> HashMap<Uuid, usize> {
    let mut index: HashMap<Uuid, usize> = HashMap::new();
    let mut low: HashMap<Uuid, usize> = HashMap::new();
    let mut on_stack: HashSet<Uuid> = HashSet::new();
    let mut stack: Vec<Uuid> = Vec::new();
    let mut component: HashMap<Uuid, usize> = HashMap::new();
    let (mut counter, mut count) = (0usize, 0usize);
    for &root in nodes {
        let Entry::Vacant(slot) = index.entry(root) else {
            continue;
        };
        slot.insert(counter);
        low.insert(root, counter);
        counter += 1;
        stack.push(root);
        on_stack.insert(root);
        let mut calls: Vec<(Uuid, usize)> = vec![(root, 0)];
        while let Some(&(v, i)) = calls.last() {
            let succ = next.get(&v).map(Vec::as_slice).unwrap_or(&[]);
            if let Some(&w) = succ.get(i) {
                if let Some(frame) = calls.last_mut() {
                    frame.1 += 1;
                }
                if let Entry::Vacant(slot) = index.entry(w) {
                    slot.insert(counter);
                    low.insert(w, counter);
                    counter += 1;
                    stack.push(w);
                    on_stack.insert(w);
                    calls.push((w, 0));
                } else if on_stack.contains(&w) {
                    let lw = index[&w];
                    if let Some(lv) = low.get_mut(&v) {
                        *lv = (*lv).min(lw);
                    }
                }
                continue;
            }
            calls.pop();
            let lv = low[&v];
            if let Some(&(parent, _)) = calls.last() {
                if let Some(lp) = low.get_mut(&parent) {
                    *lp = (*lp).min(lv);
                }
            }
            if lv == index[&v] {
                while let Some(w) = stack.pop() {
                    on_stack.remove(&w);
                    component.insert(w, count);
                    if w == v {
                        break;
                    }
                }
                count += 1;
            }
        }
    }
    component
}

/// 一次找环的状态：当前路径、找到的环、走了多少步、是否撞上了上限。
struct Search<'a> {
    adj: &'a HashMap<Uuid, Vec<&'a TimedEdge>>,
    path: Vec<&'a TimedEdge>,
    on_path: HashSet<Uuid>,
    out: Vec<Violation>,
    steps: usize,
    capped: bool,
}

impl<'a> Search<'a> {
    /// 从 `at` 往下走。`span` 是路径上已走过的边的区间交集；起点是全时间
    fn walk(&mut self, start: Uuid, at: Uuid, span: (Option<i64>, Option<i64>)) {
        if self.path.len() >= MAX_DEPTH || self.capped {
            return;
        }
        let adj = self.adj;
        let Some(next) = adj.get(&at) else { return };
        for &t in next {
            self.steps += 1;
            if self.steps > MAX_CYCLE_STEPS || self.out.len() >= MAX_CYCLES_PER_PREDICATE {
                self.capped = true;
                return;
            }
            let e = &t.edge;
            // 只经过比起点大的节点：每个环只在从它最小的节点出发时被走到，恰好一次。
            // 出边按宾语排过序，这里本可以二分跳过，但出边通常只有几条
            if e.object < start {
                continue;
            }
            // 接上这条边，路径就没有哪一刻是整条成立的：不管是成环还是往下走，都不必了。
            // 没日期的事件区间为空（0031），在这里自然被剪掉
            let Some(span) = derive::overlap(span, (t.from, t.to)) else {
                continue;
            };
            if e.object == start {
                if self.path.is_empty() {
                    continue;
                }
                let mut facts: Vec<Uuid> = self.path.iter().map(|x| x.edge.fact).collect();
                facts.push(e.fact);
                // **报之前把环转到规范位置。**环是一个圈，从哪条边开始读都是同一个环；
                // 而 `axiom_violations` 里环是按整条 `path` 唯一的（0054）：换一种旋转
                // 就是换一行。后果不是"多一行"，是**人的裁决会悄悄失效**：重算删掉的是
                // open 的行，裁过的那行留着，同一个环以新键插成 open，重开那一支匹配
                // 不上就不会触发（#618）。起点是最小的节点，却未必是最小的事实——转到
                // 最小的事实 id 起头，键就成了环自己的函数
                let at = facts
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, f)| **f)
                    .map(|(i, _)| i)
                    .unwrap();
                facts.rotate_left(at);
                self.out.push(Violation {
                    kind: Kind::Cycle,
                    left: facts[0],
                    right: *facts.last().unwrap(),
                    path: facts,
                });
                continue;
            }
            if self.on_path.contains(&e.object) {
                continue;
            }
            self.on_path.insert(e.object);
            self.path.push(t);
            self.walk(start, e.object, span);
            self.path.pop();
            self.on_path.remove(&e.object);
            if self.capped {
                return;
            }
        }
    }
}

/// 互斥：同一个键下，两条「不同」的事实同时成立。
///
/// functional（键是主语，不同是宾语不同）、inverse_functional（键是宾语，不同是
/// 主语不同）、asymmetric（键是无序的一对节点，不同是方向相反）都是这个形状，
/// 调用方给键和「不同」。
///
/// **一组报一处。** 同一个主语同一段时间里指了五个宾语，是一件事；两两组合报十处
/// 只是把它说十遍。组是冲突关系的连通块：a 与 b 同时成立、b 与 c 同时成立，三条
/// 就是一组，哪怕 a 与 c 首尾相接。先后出现、互不重叠的两场冲突是两组。
///
/// **身份只由组里有哪些事实决定**（#624）。`axiom_violations` 按
/// `(kind, left_fact, right_fact)` 唯一；从前 left/right 取遇到的头两条，而遇到的
/// 顺序来自 HashMap 与取数顺序——同一处冲突换个顺序就是另一行，人裁过的那行被绕开，
/// 与 #618 的环是同一个病。现在 `path` 是排过序的整组，left/right 是它的首尾。
fn clashes(
    edges: &[TimedEdge],
    kind: Kind,
    key: fn(&Edge) -> (Uuid, Uuid),
    differ: fn(&Edge, &Edge) -> bool,
) -> Vec<Violation> {
    let mut by_key: HashMap<(Uuid, Uuid), Vec<&TimedEdge>> = HashMap::new();
    for t in edges {
        // 空区间（没有日期的事件，0031）哪一刻都不成立，跟谁都不同时
        if matches!((t.from, t.to), (Some(f), Some(to)) if f >= to) {
            continue;
        }
        by_key.entry(key(&t.edge)).or_default().push(t);
    }
    let mut out = Vec::new();
    for mut group in by_key.into_values() {
        if group.len() < 2 {
            continue;
        }
        // 按起点扫一遍，手里留着还没结束的：它们正是与这一条同时成立的。一长串前后
        // 相接的历史值（逐月的薪水）手里始终只有一两条，不会两两比一遍
        group.sort_by_key(|t| (t.from.unwrap_or(i64::MIN), t.edge.fact));
        let mut parent: Vec<usize> = (0..group.len()).collect();
        let mut clashed = vec![false; group.len()];
        let mut open: Vec<usize> = Vec::new();
        for i in 0..group.len() {
            let start = group[i].from.unwrap_or(i64::MIN);
            // 半开区间：终点正好是这一条起点的，已经结束了——首尾相接是接任
            open.retain(|&j| group[j].to.is_none_or(|to| to > start));
            for &j in &open {
                if differ(&group[i].edge, &group[j].edge) {
                    clashed[i] = true;
                    clashed[j] = true;
                    let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                    parent[a] = b;
                }
            }
            open.push(i);
        }
        let mut blocks: HashMap<usize, Vec<Uuid>> = HashMap::new();
        for i in 0..group.len() {
            if clashed[i] {
                let r = root(&mut parent, i);
                blocks.entry(r).or_default().push(group[i].edge.fact);
            }
        }
        for mut facts in blocks.into_values() {
            facts.sort_unstable();
            facts.dedup();
            out.push(Violation {
                kind,
                left: facts[0],
                right: facts[facts.len() - 1],
                path: facts,
            });
        }
    }
    out
}

/// 并查集找根，顺手压缩路径
fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
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
    /// 不带时间地查：两端都无界，任意两条都同时成立，检查只剩形状。大部分用例问的
    /// 就是形状；问时间的用例用 `at` 造边，直接调 `super::check`
    fn check(edges: &[Edge], axioms: &HashMap<Uuid, Axioms>) -> Vec<Violation> {
        let timed: Vec<TimedEdge> = edges
            .iter()
            .map(|&edge| TimedEdge {
                edge,
                from: None,
                to: None,
            })
            .collect();
        super::check(&timed, axioms)
    }
    /// 带区间的边，`[from, to)`
    fn at(fact: u8, s: u8, o: u8, from: Option<i64>, to: Option<i64>) -> TimedEdge {
        TimedEdge {
            edge: e(fact, s, o),
            from,
            to,
        }
    }
    /// 输入的全排列。顺序是这一类 bug 的全部来源，所以每种顺序都要跑到
    fn permutations<T: Copy>(items: &[T]) -> Vec<Vec<T>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut out = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let head = rest.remove(i);
            for mut tail in permutations(&rest) {
                tail.insert(0, head);
                out.push(tail);
            }
        }
        out
    }
    /// 一次检查的结果，排成与顺序无关的形状，好拿来比
    fn shape(v: Vec<Violation>) -> Vec<(&'static str, Uuid, Uuid, Vec<Uuid>)> {
        let mut out: Vec<_> = v
            .into_iter()
            .map(|x| (x.kind.as_str(), x.left, x.right, x.path))
            .collect();
        out.sort();
        out
    }
    fn kinds(v: &[Violation]) -> Vec<Kind> {
        let mut k: Vec<Kind> = v.iter().map(|x| x.kind).collect();
        k.sort_by_key(|x| x.as_str());
        k
    }

    /// **同一个环，每次都是同一行。**环的 `left`/`right` 从前取这一次遍历的首尾，
    /// 而起点来自 HashMap 的迭代顺序——同一份输入跑两次可以给出不同的旋转，而
    /// `axiom_violations` 按 `(kind, left_fact, right_fact)` 唯一，换个旋转就是换一行，
    /// 人裁过的那一行于是被绕开（#618）。
    ///
    /// 跑一百次而不是一次：顺序是随机的，一次跑不出问题来。
    #[test]
    fn a_cycle_is_reported_the_same_way_every_time() {
        let edges = [e(1, 1, 2), e(2, 2, 3), e(3, 3, 1)];
        let ax = with(Axioms {
            transitive: true,
            ..Default::default()
        });
        let shapes: std::collections::HashSet<(Uuid, Uuid, Vec<Uuid>)> = (0..100)
            .map(|_| {
                let v: Vec<Violation> = check(&edges, &ax)
                    .into_iter()
                    .filter(|v| v.kind == Kind::Cycle)
                    .collect();
                assert_eq!(v.len(), 1, "一个环只报一次");
                (v[0].left, v[0].right, v[0].path.clone())
            })
            .collect();
        assert_eq!(shapes.len(), 1, "同一个环给出了不止一种形状: {shapes:?}");
        // 规范形状是从最小的事实起头，路径仍然是一圈
        let (left, right, path) = shapes.into_iter().next().unwrap();
        assert_eq!(left, *path.iter().min().unwrap());
        assert_eq!(left, path[0]);
        assert_eq!(right, *path.last().unwrap());
        assert_eq!(path.len(), 3);
    }

    /// **共用首尾两条边的两个环是两个环**（#641）。A→B 起头、X→A 收尾，中间一条走 C、
    /// 一条走 D：两个环的 left 都是 A→B、right 都是 X→A。从前库里按这两列定键，一个
    /// 覆盖另一个；这里钉的是检查器给出的就是两个、路径不同——落库那边按整条 path 定键
    #[test]
    fn two_cycles_sharing_their_ends_are_two_cycles() {
        let edges = [
            e(1, 1, 2),
            e(2, 2, 3),
            e(3, 2, 4),
            e(4, 3, 5),
            e(5, 4, 5),
            e(6, 5, 1),
        ];
        let v: Vec<Violation> = check(
            &edges,
            &with(Axioms {
                transitive: true,
                ..Default::default()
            }),
        )
        .into_iter()
        .filter(|v| v.kind == Kind::Cycle)
        .collect();
        let paths: HashSet<Vec<Uuid>> = v.iter().map(|c| c.path.clone()).collect();
        assert_eq!(
            paths,
            HashSet::from([vec![f(1), f(2), f(4), f(6)], vec![f(1), f(3), f(5), f(6)]])
        );
        assert!(v.iter().all(|c| (c.left, c.right) == (f(1), f(6))));
    }

    /// **层级里的一条回边，不让找环走遍整个层级**（#642）。宽 4、14 层的层级 DAG，
    /// 第 5 层一条边回到第 0 层：环只有 256 个（第 0 层那个点经四层每层四选一到第 5 层
    /// 那个点），全在前六层；而从前从每个节点出发把深度 12 以内的路径都走一遍，光一个
    /// 起点就有 4^12 条，10 秒。只在强连通分量里找，那几万条路一步都不走——这里断言
    /// 的是结果完整、没有撞上步数上限：若分量剪枝失效，这张图必然撞上
    #[test]
    fn a_back_edge_in_a_hierarchy_does_not_walk_the_hierarchy() {
        let (width, layers) = (4u16, 14u16);
        let node = |l: u16, x: u16| Uuid::from_u64_pair(0, (l * width + x) as u64);
        let mut edges = Vec::new();
        let mut fact = 0u64;
        let mut push = |s: Uuid, o: Uuid, edges: &mut Vec<TimedEdge>| {
            fact += 1;
            edges.push(TimedEdge {
                edge: Edge {
                    fact: Uuid::from_u64_pair(1, fact),
                    predicate: n(99),
                    subject: s,
                    object: o,
                },
                from: None,
                to: None,
            });
        };
        for l in 0..layers - 1 {
            for x in 0..width {
                for y in 0..width {
                    push(node(l, x), node(l + 1, y), &mut edges);
                }
            }
        }
        push(node(5, 0), node(0, 0), &mut edges);
        let checked = super::check_all(
            &edges,
            &with(Axioms {
                transitive: true,
                ..Default::default()
            }),
        );
        assert!(checked.cycles_capped.is_empty(), "不该撞上上限");
        let cycles = checked
            .violations
            .iter()
            .filter(|v| v.kind == Kind::Cycle)
            .count();
        assert_eq!(
            cycles,
            4usize.pow(4),
            "第 0 层那个点经四层、每层四选一到第 5 层那个点"
        );
    }

    /// **稠密的分量里撞上上限就停，并且说出来**（#642）；停下时报出的那一批环只取决于
    /// 数据，与读进来的顺序无关——否则同一份数据每跑一遍 Review 里就换一批环。
    #[test]
    fn a_dense_loop_stops_at_the_cap_and_says_so() {
        // 12 个节点两两互指：深度 12 以内的简单环是天文数字
        let mut edges = Vec::new();
        let mut fact = 0u8;
        for s in 1..=12u8 {
            for o in 1..=12u8 {
                if s != o {
                    fact += 1;
                    edges.push(TimedEdge {
                        edge: e(fact, s, o),
                        from: None,
                        to: None,
                    });
                }
            }
        }
        let ax = with(Axioms {
            transitive: true,
            ..Default::default()
        });
        let first = super::check_all(&edges, &ax);
        assert_eq!(first.cycles_capped, vec![n(99)]);
        let cycles = first
            .violations
            .iter()
            .filter(|v| v.kind == Kind::Cycle)
            .count();
        assert!(cycles <= MAX_CYCLES_PER_PREDICATE, "报出 {cycles} 个");
        assert!(cycles > 0, "撞上上限之前找到的照报");

        let mut rng = Rng(0x0642);
        for _ in 0..5 {
            let again = super::check_all(&shuffled(&mut rng, &edges), &ax);
            assert_eq!(again.cycles_capped, first.cycles_capped);
            assert_eq!(
                shape(again.violations),
                shape(first.violations.clone()),
                "撞上上限时报出的那一批环随输入顺序变了"
            );
        }
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

        // 同一个宾语被两个主语指：记成它自己的种类，不冒充 functional（#634）
        let inn = [e(1, 2, 1), e(2, 3, 1)];
        assert_eq!(
            kinds(&check(
                &inn,
                &with(Axioms {
                    inverse_functional: true,
                    ..Default::default()
                })
            )),
            vec![Kind::InverseFunctional]
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

    /// 同一个主语指五个宾语只报一处。报十处（两两组合）是把同一件事说十遍；
    /// 那一处带着全部五条，人撤哪条都有按钮。
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
        assert_eq!(v[0].path, vec![f(1), f(2), f(3), f(4), f(5)]);
        assert_eq!((v[0].left, v[0].right), (f(1), f(5)));
    }

    /// **一处冲突，不管以什么顺序读进来，都是同一行**（#624）。从前 left/right 取
    /// 遇到的头两条：三个值的六种顺序给出六种键，反对称两种顺序给出两种键——而
    /// `axiom_violations` 按键唯一，换个顺序，人裁过的那行就被绕开了。
    ///
    /// 跑全排列而不是跑一百次：顺序是这里唯一的变量，每一种都要到。
    #[test]
    fn the_same_clash_is_the_same_row_whatever_the_order() {
        let functional = with(Axioms {
            functional: true,
            ..Default::default()
        });
        let inverse = with(Axioms {
            inverse_functional: true,
            ..Default::default()
        });
        let asymmetric = with(Axioms {
            asymmetric: true,
            ..Default::default()
        });
        let cases: [(&[Edge], &HashMap<Uuid, Axioms>); 3] = [
            (&[e(1, 1, 2), e(2, 1, 3), e(3, 1, 4)], &functional),
            (&[e(1, 2, 1), e(2, 3, 1), e(3, 4, 1)], &inverse),
            (&[e(1, 1, 2), e(2, 2, 1), e(3, 1, 2)], &asymmetric),
        ];
        for (edges, ax) in cases {
            let shapes: HashSet<_> = permutations(edges)
                .iter()
                .map(|p| shape(check(p, ax)))
                .collect();
            assert_eq!(shapes.len(), 1, "顺序换出了不同的行: {shapes:?}");
            let only = shapes.into_iter().next().unwrap();
            assert_eq!(only.len(), 1);
            let (_, left, right, path) = &only[0];
            assert_eq!(path, &vec![f(1), f(2), f(3)], "整组，按 id 排序");
            assert_eq!((*left, *right), (f(1), f(3)), "首尾是最小与最大");
        }
    }

    /// **环要整条路径同一时刻成立**（#636）。A 在 2019–2021 年属于 B，2022 年起 B
    /// 属于 A：图在任何一刻都没有这个环。三元环更容易看走眼——三条边可以两两重叠，
    /// 三者却没有共同的一刻，这时逐对比较会误报，只有沿路径求交才对。
    #[test]
    fn a_cycle_must_hold_at_one_moment() {
        let transitive = with(Axioms {
            transitive: true,
            ..Default::default()
        });
        let cycles = |edges: &[TimedEdge]| -> Vec<Violation> {
            super::check(edges, &transitive)
                .into_iter()
                .filter(|v| v.kind == Kind::Cycle)
                .collect()
        };

        // 两条边一前一后
        let swapped = [
            at(1, 1, 2, Some(0), Some(100)),
            at(2, 2, 1, Some(100), None),
        ];
        assert!(cycles(&swapped).is_empty(), "前后相接的两段凑不成环");

        // 三条边两两重叠，三者无交：[0,100) ∩ [50,150) ∩ [100,200) = ∅
        let pairwise = [
            at(1, 1, 2, Some(0), Some(100)),
            at(2, 2, 3, Some(50), Some(150)),
            at(3, 3, 1, Some(100), Some(200)),
        ];
        assert!(cycles(&pairwise).is_empty(), "两两重叠不等于同时成立");

        // 三者共有 [90,100) 这一段：是环，而且照旧只报一次、规范形状
        let together = [
            at(1, 1, 2, Some(0), Some(100)),
            at(2, 2, 3, Some(50), Some(150)),
            at(3, 3, 1, Some(90), Some(200)),
        ];
        let v = cycles(&together);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, vec![f(1), f(2), f(3)]);

        // 没日期的事件哪一刻都不成立，不进任何环
        let undated = [at(1, 1, 2, Some(50), Some(50)), at(2, 2, 1, None, None)];
        assert!(cycles(&undated).is_empty());
    }

    // ---------- 对拍：随机小图上，与暴力做法逐个比 ----------

    /// 确定性的伪随机数（xorshift64*）。对拍要可复现：挂了的那一张图，种子一样就能
    /// 原样再造出来；不为这个引依赖
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// 一张随机小图：节点少、边多，平行边、自环、反向边都常见。端点落在 0..=60 的
    /// 十格上，于是首尾相接、完全重合、空区间（起点不早于终点）、无界都经常出现——
    /// 这些正是边界
    fn random_graph(rng: &mut Rng, nodes: u64, max_edges: u64) -> Vec<TimedEdge> {
        let count = rng.below(max_edges + 1) as u8;
        let bound = |rng: &mut Rng| {
            if rng.below(4) == 0 {
                None
            } else {
                Some(rng.below(7) as i64 * 10)
            }
        };
        (1..=count)
            .map(|i| {
                let (s, o) = (rng.below(nodes) as u8 + 1, rng.below(nodes) as u8 + 1);
                let (from, to) = (bound(rng), bound(rng));
                at(i, s, o, from, to)
            })
            .collect()
    }

    fn shuffled(rng: &mut Rng, edges: &[TimedEdge]) -> Vec<TimedEdge> {
        let mut v = edges.to_vec();
        for i in (1..v.len()).rev() {
            v.swap(i, rng.below(i as u64 + 1) as usize);
        }
        v
    }

    /// 暴力找环：从每个节点起，枚举所有节点不重复、至少两条边的回路，整条路径的
    /// 区间交集非空才算。不剪枝，不看遍历顺序，结果按事实集合收
    fn cycles_by_brute_force(edges: &[TimedEdge]) -> HashSet<Vec<Uuid>> {
        fn extend(
            edges: &[TimedEdge],
            start: Uuid,
            at: Uuid,
            path: &mut Vec<usize>,
            seen: &mut Vec<Uuid>,
            out: &mut HashSet<Vec<Uuid>>,
        ) {
            for (i, t) in edges.iter().enumerate() {
                if t.edge.subject != at {
                    continue;
                }
                if t.edge.object == start {
                    if path.is_empty() {
                        continue;
                    }
                    let mut all = path.clone();
                    all.push(i);
                    let holds = all.iter().try_fold((None, None), |acc, &j| {
                        derive::overlap(acc, (edges[j].from, edges[j].to))
                    });
                    if holds.is_some() {
                        let mut facts: Vec<Uuid> =
                            all.iter().map(|&j| edges[j].edge.fact).collect();
                        facts.sort();
                        out.insert(facts);
                    }
                    continue;
                }
                if seen.contains(&t.edge.object) {
                    continue;
                }
                seen.push(t.edge.object);
                path.push(i);
                extend(edges, start, t.edge.object, path, seen, out);
                path.pop();
                seen.pop();
            }
        }
        let mut out = HashSet::new();
        let starts: HashSet<Uuid> = edges.iter().map(|t| t.edge.subject).collect();
        for start in starts {
            extend(
                edges,
                start,
                start,
                &mut Vec::new(),
                &mut vec![start],
                &mut out,
            );
        }
        out
    }

    /// 暴力分组：两两比，同键、「不同」、区间有交就连一条，取连通块
    fn groups_by_brute_force(
        edges: &[TimedEdge],
        conflict: impl Fn(&Edge, &Edge) -> bool,
    ) -> HashSet<Vec<Uuid>> {
        let n = edges.len();
        let mut parent: Vec<usize> = (0..n).collect();
        let mut clashed = vec![false; n];
        for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (&edges[i], &edges[j]);
                if conflict(&a.edge, &b.edge)
                    && derive::overlap((a.from, a.to), (b.from, b.to)).is_some()
                {
                    clashed[i] = true;
                    clashed[j] = true;
                    let (x, y) = (root(&mut parent, i), root(&mut parent, j));
                    parent[x] = y;
                }
            }
        }
        let mut blocks: HashMap<usize, Vec<Uuid>> = HashMap::new();
        for i in 0..n {
            if clashed[i] {
                let r = root(&mut parent, i);
                blocks.entry(r).or_default().push(edges[i].edge.fact);
            }
        }
        blocks
            .into_values()
            .map(|mut v| {
                v.sort();
                v
            })
            .collect()
    }

    fn found(v: &[Violation], kind: Kind) -> Vec<Vec<Uuid>> {
        v.iter()
            .filter(|x| x.kind == kind)
            .map(|x| {
                let mut facts = x.path.clone();
                facts.sort();
                facts
            })
            .collect()
    }

    /// **环：几千张随机小图上与暴力枚举逐个一致**（#636）。剪枝是个"证明了才敢做"的
    /// 优化——交集只会越求越窄，所以剪掉的分支凑不出真环。这里不信证明，信对拍：
    /// 带剪枝的深搜报出的环集合必须与不剪枝、整条求交的暴力枚举完全相同，一个不多
    /// 一个不少；每个环只报一次，报出来的形状是规范的（从最小事实起头、首尾对得上
    /// 路径、路径真的首尾相连）；把输入打乱，输出逐字节不变。
    #[test]
    fn cycles_agree_with_brute_force_on_random_graphs() {
        let transitive = with(Axioms {
            transitive: true,
            ..Default::default()
        });
        let mut rng = Rng(0x0636_C0FF_EE00_0001);
        let (mut with_cycles, mut filtered_by_time) = (0, 0);
        for round in 0..6000 {
            let (nodes, max_edges) = if round % 2 == 0 { (3, 9) } else { (4, 12) };
            let edges = random_graph(&mut rng, nodes, max_edges);
            let v = super::check(&edges, &transitive);
            let got = found(&v, Kind::Cycle);
            let unique: HashSet<Vec<Uuid>> = got.iter().cloned().collect();
            assert_eq!(got.len(), unique.len(), "同一个环报了两次: {edges:?}");
            assert_eq!(
                unique,
                cycles_by_brute_force(&edges),
                "与暴力枚举不一致: {edges:?}"
            );
            if !unique.is_empty() {
                with_cycles += 1;
            }
            // 同一张图抹掉时间再暴力找一遍：形状上的环比带时间的多，说明这张图上
            // 时间真的起了作用——剪枝与求交走到了
            let timeless: Vec<TimedEdge> = edges
                .iter()
                .map(|t| TimedEdge {
                    from: None,
                    to: None,
                    ..*t
                })
                .collect();
            if cycles_by_brute_force(&timeless).len() > unique.len() {
                filtered_by_time += 1;
            }

            let by_fact: HashMap<Uuid, &TimedEdge> =
                edges.iter().map(|t| (t.edge.fact, t)).collect();
            for c in v.iter().filter(|x| x.kind == Kind::Cycle) {
                assert_eq!(c.left, *c.path.iter().min().unwrap(), "从最小的事实起头");
                assert_eq!((c.left, c.right), (c.path[0], *c.path.last().unwrap()));
                for w in 0..c.path.len() {
                    let here = by_fact[&c.path[w]].edge;
                    let next = by_fact[&c.path[(w + 1) % c.path.len()]].edge;
                    assert_eq!(here.object, next.subject, "路径要真的首尾相连: {c:?}");
                }
            }

            assert_eq!(
                shape(v),
                shape(super::check(&shuffled(&mut rng, &edges), &transitive)),
                "打乱输入换了输出: {edges:?}"
            );
        }
        // 对拍要真的碰到环才算数：随机图若几乎不成环，这个测试什么也没证明
        eprintln!("{with_cycles} 张图有环，{filtered_by_time} 张图上时间筛掉过环");
        assert!(with_cycles > 1000, "只有 {with_cycles} 张图有环，样本太稀");
        assert!(
            filtered_by_time > 1000,
            "只有 {filtered_by_time} 张图上时间筛掉过环，剪枝没怎么走到"
        );
    }

    /// **互斥三类：几千张随机小图上与两两比较的暴力分组逐个一致**（#634、#624）。
    /// 按起点扫一遍、手里只留还没结束的——这个扫描省掉了两两比较，也最容易在边界上
    /// 错：首尾相接、无界的起点、空区间、同一时刻开始的几条。暴力做法把每一对都比
    /// 一遍再取连通块，两边给出的组必须完全相同；打乱输入，输出逐字节不变。
    #[test]
    fn clashes_agree_with_brute_force_on_random_graphs() {
        let all = with(Axioms {
            asymmetric: true,
            functional: true,
            inverse_functional: true,
            ..Default::default()
        });
        let mut rng = Rng(0x0634_BEEF_0000_0002);
        let mut seen = [0usize; 3];
        for round in 0..4000 {
            let (nodes, max_edges) = if round % 2 == 0 { (2, 8) } else { (4, 12) };
            let edges = random_graph(&mut rng, nodes, max_edges);
            let v = super::check(&edges, &all);
            // 不捕获任何东西的闭包就是函数指针
            type Conflict = fn(&Edge, &Edge) -> bool;
            let cases: [(Kind, Conflict); 3] = [
                (Kind::Functional, |a, b| {
                    a.subject == b.subject && a.object != b.object
                }),
                (Kind::InverseFunctional, |a, b| {
                    a.object == b.object && a.subject != b.subject
                }),
                (Kind::Asymmetry, |a, b| {
                    a.subject != a.object && a.subject == b.object && a.object == b.subject
                }),
            ];
            for (k, (kind, conflict)) in cases.into_iter().enumerate() {
                let got = found(&v, kind);
                let unique: HashSet<Vec<Uuid>> = got.iter().cloned().collect();
                assert_eq!(
                    got.len(),
                    unique.len(),
                    "{kind:?} 同一组报了两次: {edges:?}"
                );
                assert_eq!(
                    unique,
                    groups_by_brute_force(&edges, conflict),
                    "{kind:?} 与暴力分组不一致: {edges:?}"
                );
                seen[k] += unique.len();
            }
            for x in v.iter().filter(|x| x.kind != Kind::Cycle) {
                let mut sorted = x.path.clone();
                sorted.sort();
                assert_eq!(x.path, sorted, "组按 id 排序");
                assert_eq!((x.left, x.right), (x.path[0], *x.path.last().unwrap()));
            }
            assert_eq!(
                shape(v),
                shape(super::check(&shuffled(&mut rng, &edges), &all)),
                "打乱输入换了输出: {edges:?}"
            );
        }
        assert!(seen.iter().all(|&n| n > 500), "有一类几乎没碰到: {seen:?}");
    }

    /// **剪枝丢不了真环。** 同一对节点之间有两条边，一条的区间让路径无交、另一条
    /// 不会：前一条被剪掉之后，后一条照样把环走出来。输入的每一种顺序都要到——
    /// 剪枝发生在遍历里，顺序决定先碰到哪一条。
    #[test]
    fn pruning_a_dead_branch_keeps_the_live_one() {
        let transitive = with(Axioms {
            transitive: true,
            ..Default::default()
        });
        let edges = [
            at(1, 1, 2, Some(0), Some(100)),
            // 与 1 无交：经过它的路径被剪掉
            at(2, 2, 3, Some(200), Some(300)),
            // 与 1 有交：环走这一条
            at(3, 2, 3, Some(50), Some(150)),
            at(4, 3, 1, Some(60), None),
        ];
        for order in permutations(&edges) {
            let v: Vec<Violation> = super::check(&order, &transitive)
                .into_iter()
                .filter(|v| v.kind == Kind::Cycle)
                .collect();
            assert_eq!(v.len(), 1, "{order:?}");
            assert_eq!(v[0].path, vec![f(1), f(3), f(4)]);
        }
    }

    /// **前后相接是接任，不是矛盾**（#634）。Lin Zhao 的薪水 `[2023-06, 2024-02)`
    /// 是 28000、`[2024-02, 现在)` 是 32000：一个人任何一刻都只有一份薪水。从前
    /// 检查不看时间，每次调薪都进 Review。重叠哪怕一秒，才是同时有两个值。
    #[test]
    fn a_succession_is_not_a_contradiction() {
        let functional = with(Axioms {
            functional: true,
            ..Default::default()
        });
        let (jun23, feb24) = (1_685_577_600, 1_708_387_200);

        let raise = [
            at(1, 1, 2, Some(jun23), Some(feb24)),
            at(2, 1, 3, Some(feb24), None),
        ];
        assert!(super::check(&raise, &functional).is_empty(), "首尾相接");

        let overlap = [
            at(1, 1, 2, Some(jun23), Some(feb24 + 1)),
            at(2, 1, 3, Some(feb24), None),
        ];
        assert_eq!(
            kinds(&super::check(&overlap, &functional)),
            vec![Kind::Functional]
        );

        // 宾语侧同理：一个项目换了负责人，中间还空了一年
        let handover = [
            at(1, 2, 9, Some(jun23), Some(feb24)),
            at(2, 3, 9, Some(feb24 + 31_536_000), None),
        ];
        let inverse = with(Axioms {
            inverse_functional: true,
            ..Default::default()
        });
        assert!(super::check(&handover, &inverse).is_empty());

        // 反对称同理：先是 A 管 B，后来 B 管 A
        let reversal = [
            at(1, 1, 2, Some(jun23), Some(feb24)),
            at(2, 2, 1, Some(feb24), None),
        ];
        let asymmetric = with(Axioms {
            asymmetric: true,
            ..Default::default()
        });
        assert!(super::check(&reversal, &asymmetric).is_empty());
    }

    /// 无界的那一侧跟谁都重叠：一条不知道从何时起、一直成立的事实，与任何一段
    /// 都同时成立。没有日期的事件区间为空（0031），哪一刻都不成立，跟谁都不冲突。
    #[test]
    fn an_unbounded_span_meets_everything_and_an_empty_one_meets_nothing() {
        let functional = with(Axioms {
            functional: true,
            ..Default::default()
        });
        let always = [at(1, 1, 2, None, None), at(2, 1, 3, Some(100), Some(200))];
        assert_eq!(
            kinds(&super::check(&always, &functional)),
            vec![Kind::Functional]
        );

        let undated_event = [
            at(1, 1, 2, Some(150), Some(150)),
            at(2, 1, 3, Some(100), Some(200)),
        ];
        assert!(super::check(&undated_event, &functional).is_empty());
    }

    /// **组是冲突的连通块。** a 与 b 重叠、b 与 c 重叠，a 与 c 首尾相接：三条是一组
    /// ——撤掉 b 两场冲突一起消失，分开报会让人以为是两件事。两场彼此隔开的冲突是两组。
    #[test]
    fn a_clash_is_a_connected_group_and_separate_clashes_stay_separate() {
        let functional = with(Axioms {
            functional: true,
            ..Default::default()
        });
        let chained = [
            at(1, 1, 2, Some(0), Some(100)),
            at(2, 1, 3, Some(50), Some(150)),
            at(3, 1, 4, Some(100), Some(200)),
        ];
        let v = super::check(&chained, &functional);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, vec![f(1), f(2), f(3)]);

        let apart = [
            at(1, 1, 2, Some(0), Some(100)),
            at(2, 1, 3, Some(50), Some(100)),
            at(3, 1, 4, Some(500), Some(600)),
            at(4, 1, 5, Some(550), Some(600)),
            // 与谁都不重叠的一条不进任何一组
            at(5, 1, 6, Some(300), Some(400)),
        ];
        let v = shape(super::check(&apart, &functional));
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].3, vec![f(1), f(2)]);
        assert_eq!(v[1].3, vec![f(3), f(4)]);
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
