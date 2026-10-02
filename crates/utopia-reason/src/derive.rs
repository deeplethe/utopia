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

use crate::{Axioms, Edge, Kind, MAX_DEPTH};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// 单个谓词上派生的条数上限。
///
/// **不是防御性编程,是拿数量过的**:4.5 倍膨胀出现在一个 185 条边的谓词上,
/// 而膨胀是超线性的。封顶之后被截掉多少条要**说出来**（见 [`Derivation::capped`]）
/// ——悄悄截断会让「推完了」和「推了一部分」长得一模一样。
pub const MAX_DERIVED_PER_PREDICATE: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// 一条业务规则推出的关系边（0047）。不是公理：`derive()` 从不产出它，
    /// 它只作为**候选**进矛盾检查，让撞上断言或别的派生时能像公理派生一样被报出来。
    /// 触发它的规则行在 `attribute_rules` 里，不在公理规则表里
    Business,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Transitive => "transitive",
            Rule::Symmetric => "symmetric",
            Rule::Inverse => "inverse",
            Rule::SubProperty => "sub_property",
            Rule::Business => "business_rule",
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
/// 单独一个类型而不是给 `Edge` 加字段:环与自环用不上时间,R0 的互斥三类要看
/// 两条是否同时成立(#634),R1 每一步都要算交集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedEdge {
    pub edge: Edge,
    /// 半开区间 `[from, to)`。两端都可为空 = 不知道/一直
    pub from: Option<i64>,
    pub to: Option<i64>,
}

/// 交集。`None` 表示无界那一侧。
pub fn overlap(
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

/// 邻接表里的一条边：(另一端, 起, 止, 支撑它的事实 id 列表)。
///
/// 第四项是**列表**而不是单个 id：这一项也会装进派生出来的边，而派生的证明可以
/// 有好几条前提。只留第一条的话，传递再接一条派生边时，证明就短了——链上少一条
/// 前提，而 `validity` 只对留下来的那几条求交集，于是结论的有效期比实际宽。
type Hop = (Uuid, Option<i64>, Option<i64>, Vec<Uuid>);

/// 半开区间 `[from, to)`，两端可空。
type Span = (Option<i64>, Option<i64>);

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

// 同一个 triple 可以有几段互不包含的有效期；被更宽区间完全覆盖的那段不再另留一份。

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

    // 断言过的时序三元组。**派生撞上它就让路**——asserted > derived 是硬性的；
    // 判断的是区间覆盖，不只是等值，所以无时间/更宽的一段也能挡住窄的派生。
    let mut asserted: HashMap<Triple, Vec<Span>> = HashMap::new();
    for e in edges {
        asserted
            .entry((e.edge.predicate, e.edge.subject, e.edge.object))
            .or_default()
            .push((e.from, e.to));
    }

    // 已经推出来的 → 怎么来的。继续展开时既看区间也看证明长度：覆盖同一段时间
    // 的短证明能在深度上限前再多走几步，不能被先到的长证明挡掉。
    let mut reached: HashMap<Triple, Vec<(Span, Reached)>> = HashMap::new();
    // **封顶仍按谓词计**：那个常量的含义没变（一个谓词最多推两万条），
    // 而 `Derivation::capped` 回的也是谓词列表。跨谓词之后若改成全局一个数，
    // 界面上「哪个谓词太密」就答不出来了
    let mut per_pred: HashMap<Uuid, usize> = HashMap::new();
    let mut capped: HashSet<Uuid> = HashSet::new();

    // 从 (谓词, 主语) 出发能走的边，传递用。派生出来的也进来——它们已经是
    // 我们的断言了，链上不该因为「来路不同」断掉
    let mut adj: HashMap<(Uuid, Uuid), Vec<Hop>> = HashMap::new();
    // 新边也可能是链的右半段：左半段已离开 frontier，不能等它再展开一次。
    // 按宾语索引入边，才能把后到的 B→C 接到已经见过的 A→B 上。
    let mut incoming: HashMap<(Uuid, Uuid), Vec<Hop>> = HashMap::new();
    for e in edges {
        adj.entry((e.edge.predicate, e.edge.subject))
            .or_default()
            .push((e.edge.object, e.from, e.to, vec![e.edge.fact]));
        incoming
            .entry((e.edge.predicate, e.edge.object))
            .or_default()
            .push((e.edge.subject, e.from, e.to, vec![e.edge.fact]));
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
    // 起始 frontier 的顺序跟着入参走，而入参顺序不保证——按完整事实身份排一次，
    // 同一个库两次推导才给得出同一份结果
    frontier.sort_by_key(|(t, acc)| (*t, acc.from, acc.to));

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
                    &mut incoming,
                ) {
                    continue;
                }
            }

            // ---- 传递：新边既能作左半段，也能作右半段
            if ax.transitive {
                let outs = adj.get(&(pred, obj)).cloned().unwrap_or_default();
                for (c, from, to, hop_premises) in outs {
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
                        Some(&hop_premises),
                        &asserted,
                        &mut reached,
                        &mut per_pred,
                        &mut capped,
                        &mut out,
                        &mut next,
                        &mut adj,
                        &mut incoming,
                    );
                }
                let ins = incoming.get(&(pred, subj)).cloned().unwrap_or_default();
                for (a, from, to, premises) in ins {
                    if a == obj {
                        continue;
                    }
                    let Some((nf, nt)) = overlap((from, to), (acc.from, acc.to)) else {
                        continue;
                    };
                    // emit 按左、右顺序拼证明；后到的是右边，不能把整条路径倒过来。
                    let prefix = Reached { from, to, premises };
                    emit(
                        (pred, a, obj),
                        pred,
                        Rule::Transitive,
                        &prefix,
                        nf,
                        nt,
                        Some(&acc.premises),
                        &asserted,
                        &mut reached,
                        &mut per_pred,
                        &mut capped,
                        &mut out,
                        &mut next,
                        &mut adj,
                        &mut incoming,
                    );
                }
            }
        }
        next.sort_by_key(|(t, acc)| (*t, acc.from, acc.to));
        frontier = next;
    }

    let mut capped: Vec<Uuid> = capped.into_iter().collect();
    capped.sort();
    out.capped = capped;
    out
}

/// `outer` 是否覆盖 `inner`。两端都为 `None` 表示无界，因此能覆盖任何同向区间。
fn span_contains(outer: Span, inner: Span) -> bool {
    outer
        .0
        .is_none_or(|outer| inner.0.is_some_and(|inner| outer <= inner))
        && outer
            .1
            .is_none_or(|outer| inner.1.is_some_and(|inner| outer >= inner))
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
    // 传递多用掉的那一条边所依赖的全部前提；一跳规则没有
    extra_premises: Option<&[Uuid]>,
    asserted: &HashMap<Triple, Vec<Span>>,
    reached: &mut HashMap<Triple, Vec<(Span, Reached)>>,
    per_pred: &mut HashMap<Uuid, usize>,
    capped: &mut HashSet<Uuid>,
    out: &mut Derivation,
    next: &mut Vec<(Triple, Reached)>,
    adj: &mut HashMap<(Uuid, Uuid), Vec<Hop>>,
    incoming: &mut HashMap<(Uuid, Uuid), Vec<Hop>>,
) -> bool {
    let (pred, subj, obj) = t;
    // 自环不推，任何规则都一样：`A p A` 是矛盾不是知识
    if subj == obj {
        return false;
    }
    let span = (from, to);
    // 断言优先；无时间断言也覆盖日期的派生。
    if asserted
        .get(&t)
        .into_iter()
        .flatten()
        .copied()
        .any(|outer| span_contains(outer, span))
    {
        return false;
    }
    let mut premises = acc.premises.clone();
    if let Some(extra) = extra_premises {
        premises.extend_from_slice(extra);
    }
    // 深度上限在这里再判一次。展开 frontier 时那条 `premises.len() >= MAX_DEPTH`
    // 只看得到当前这一份；传递接上来的那条边可能自带好几条前提，一次就能把长度
    // 顶过上限，所以**拼完之后**才是判得上限的地方
    if premises.len() > MAX_DEPTH {
        return false;
    }

    let previous = reached.entry(t).or_default();
    // 同一段时间的长证明只能挡住重复展示，不能挡住短证明继续展开：否则先走了
    // 绕路，就可能在 12 条前提处停下，而本来存在一条 12 条以内能走完的捷径。
    // 区间与长度都被覆盖才丢掉；逆的互指仍会在同长的证明处收敛。
    let covered = previous
        .iter()
        .any(|(outer, _)| span_contains(*outer, span));
    if previous
        .iter()
        .any(|(outer, proof)| span_contains(*outer, span) && proof.premises.len() <= premises.len())
    {
        return false;
    }
    if !covered {
        let n = per_pred.entry(pred).or_insert(0);
        if *n >= MAX_DERIVED_PER_PREDICATE {
            capped.insert(pred);
            return true;
        }
        *n += 1;
    }

    let r = Reached {
        from,
        to,
        premises: premises.clone(),
    };
    previous.retain(|(old_span, proof)| {
        !(span_contains(span, *old_span) && premises.len() <= proof.premises.len())
    });
    previous.push((span, r.clone()));
    // 派生出来的边也能被后续传递接上。**整份证明都要带上**：邻接表里的这一项
    // 以后会被当作前提拼进下一条派生，只留首条会让链上的证明越拼越短
    if !premises.is_empty() {
        for (index, key, other) in [(adj, (pred, subj), obj), (incoming, (pred, obj), subj)] {
            let hops = index.entry(key).or_default();
            hops.retain(|(end, f, t, proof)| {
                *end != other || !(span_contains(span, (*f, *t)) && premises.len() <= proof.len())
            });
            hops.push((other, from, to, premises.clone()));
        }
    }
    if !covered {
        out.facts.push(Derived {
            predicate: pred,
            via,
            subject: subj,
            object: obj,
            rule,
            premises,
        });
    }
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

// ===================== 矛盾：派生撞上了什么（0017） =====================

/// 一条派生撞上了一条断言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clash {
    /// `Derivation::facts` 里的下标
    pub derived: usize,
    /// 撞在哪条公理上：`Functional`、`InverseFunctional`、`Asymmetry`、`SelfLoop`
    pub axiom: Kind,
    /// 被撞的断言。自环没有对方，取派生的最后一条前提
    pub against: Uuid,
}

/// 两条规则加在一起产出了互相矛盾的派生。
///
/// **按规则对聚合，不逐对报**：`ceo_of ⊑ works_at` 加 `works_at` functional，每个有
/// 两个 ceo 的组织就撞一对——根子是那两条声明，逐对进队列只会淹掉 Review。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleClash {
    /// (声明所在的谓词, 规则种类)，两条按 (谓词, 种类) 排过序，a ≤ b
    pub a: (Uuid, Rule),
    pub b: (Uuid, Rule),
    pub axiom: Kind,
    /// 互撞的派生对，按 `Derivation::facts` 的下标
    pub pairs: Vec<(usize, usize)>,
}

/// (谓词, 一端) → 另一端的边：(另一端, 事实, 区间)。functional 两个方向各一份
type ByEnd = HashMap<(Uuid, Uuid), Vec<(Uuid, Uuid, Span)>>;
/// 一条规则的身份：声明所在的谓词 + 规则种类
type RuleSide = (Uuid, Rule);
/// 互撞的派生对，按 (规则 a, 规则 b, 撞在哪条公理上) 分组
type Grouped = HashMap<(RuleSide, RuleSide, Kind), Vec<(usize, usize)>>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contradictions {
    pub with_assertions: Vec<Clash>,
    pub between_derivations: Vec<RuleClash>,
}

impl Contradictions {
    /// 不该落地的派生下标：撞过断言的，和撞过别的派生的。**写图宁少勿错**（0002）
    pub fn blocked(&self) -> HashSet<usize> {
        let mut out: HashSet<usize> = self.with_assertions.iter().map(|c| c.derived).collect();
        for rc in &self.between_derivations {
            for (i, j) in &rc.pairs {
                out.insert(*i);
                out.insert(*j);
            }
        }
        out
    }
}

/// 拿公理量一遍派生：与断言撞的逐条列出，派生之间撞的按规则对聚合。
///
/// 只查四类——`functional`（含 inverse）、`asymmetric`、`irreflexive`——因为只有它们
/// 能由**两条边**判出矛盾；传递环那类要走闭包，派生本身就是闭包的一部分，R0 对断言
/// 查过就够了。functional 与 asymmetric 都要求**有效区间重叠**：Mira 走了 Devin
/// 接任，两条 `ceo_of` 区间不交，那是接任，不是矛盾。
///
/// 撞上断言的派生一律**不落地**（asserted > derived，硬性）；这一步把「让路」这件事
/// 从静默变成可见——0002 那张表里写了没做的那一行。
pub fn contradictions(
    derivation: &Derivation,
    edges: &[TimedEdge],
    axioms: &HashMap<Uuid, Axioms>,
    spans: &HashMap<Uuid, (Option<i64>, Option<i64>)>,
) -> Contradictions {
    // 断言的三份索引：(谓词, 主) → 宾；(谓词, 宾) → 主；(谓词, 主, 宾) → 边
    let mut by_ps: ByEnd = HashMap::new();
    let mut by_po: ByEnd = HashMap::new();
    let mut by_spo: HashMap<(Uuid, Uuid, Uuid), Vec<(Uuid, Span)>> = HashMap::new();
    for e in edges {
        let span = (e.from, e.to);
        let x = e.edge;
        by_ps
            .entry((x.predicate, x.subject))
            .or_default()
            .push((x.object, x.fact, span));
        by_po
            .entry((x.predicate, x.object))
            .or_default()
            .push((x.subject, x.fact, span));
        by_spo
            .entry((x.predicate, x.subject, x.object))
            .or_default()
            .push((x.fact, span));
    }

    let mut out = Contradictions::default();
    // 派生的区间：与落库那一侧同一个函数算，算不出的（前提区间不交）本来就不会落
    let derived_spans: Vec<Option<Span>> = derivation
        .facts
        .iter()
        .map(|d| validity(&d.premises, spans))
        .collect();

    for (i, d) in derivation.facts.iter().enumerate() {
        let Some(span) = derived_spans[i] else {
            continue;
        };
        let Some(ax) = axioms.get(&d.predicate) else {
            continue;
        };
        let Some(&last) = d.premises.last() else {
            continue;
        };
        if ax.irreflexive && d.subject == d.object {
            out.with_assertions.push(Clash {
                derived: i,
                axiom: Kind::SelfLoop,
                against: last,
            });
        }
        if ax.asymmetric {
            if let Some(v) = by_spo.get(&(d.predicate, d.object, d.subject)) {
                for (fact, sp) in v {
                    if overlap(span, *sp).is_some() {
                        out.with_assertions.push(Clash {
                            derived: i,
                            axiom: Kind::Asymmetry,
                            against: *fact,
                        });
                    }
                }
            }
        }
        if ax.functional {
            if let Some(v) = by_ps.get(&(d.predicate, d.subject)) {
                for (obj, fact, sp) in v {
                    if *obj != d.object && overlap(span, *sp).is_some() {
                        out.with_assertions.push(Clash {
                            derived: i,
                            axiom: Kind::Functional,
                            against: *fact,
                        });
                    }
                }
            }
        }
        if ax.inverse_functional {
            if let Some(v) = by_po.get(&(d.predicate, d.object)) {
                for (subj, fact, sp) in v {
                    if *subj != d.subject && overlap(span, *sp).is_some() {
                        out.with_assertions.push(Clash {
                            derived: i,
                            axiom: Kind::InverseFunctional,
                            against: *fact,
                        });
                    }
                }
            }
        }
    }

    // 派生之间：同样三份索引，只不过键的是下标
    let mut d_ps: HashMap<(Uuid, Uuid), Vec<usize>> = HashMap::new();
    let mut d_po: HashMap<(Uuid, Uuid), Vec<usize>> = HashMap::new();
    let mut d_spo: HashMap<(Uuid, Uuid, Uuid), Vec<usize>> = HashMap::new();
    for (i, d) in derivation.facts.iter().enumerate() {
        if derived_spans[i].is_none() {
            continue;
        }
        d_ps.entry((d.predicate, d.subject)).or_default().push(i);
        d_po.entry((d.predicate, d.object)).or_default().push(i);
        d_spo
            .entry((d.predicate, d.subject, d.object))
            .or_default()
            .push(i);
    }
    let mut grouped: Grouped = HashMap::new();
    let mut note = |i: usize, j: usize, axiom: Kind| {
        let (i, j) = if i < j { (i, j) } else { (j, i) };
        let ri = (derivation.facts[i].via, derivation.facts[i].rule);
        let rj = (derivation.facts[j].via, derivation.facts[j].rule);
        let (a, b) = if (ri.0, ri.1.as_str()) <= (rj.0, rj.1.as_str()) {
            (ri, rj)
        } else {
            (rj, ri)
        };
        grouped.entry((a, b, axiom)).or_default().push((i, j));
    };
    for (i, d) in derivation.facts.iter().enumerate() {
        let Some(span) = derived_spans[i] else {
            continue;
        };
        let Some(ax) = axioms.get(&d.predicate) else {
            continue;
        };
        let overlapping = |j: usize| derived_spans[j].is_some_and(|s| overlap(span, s).is_some());
        if ax.asymmetric {
            if let Some(v) = d_spo.get(&(d.predicate, d.object, d.subject)) {
                for &j in v {
                    if j > i && overlapping(j) {
                        note(i, j, Kind::Asymmetry);
                    }
                }
            }
        }
        if ax.functional {
            if let Some(v) = d_ps.get(&(d.predicate, d.subject)) {
                for &j in v {
                    if j > i && derivation.facts[j].object != d.object && overlapping(j) {
                        note(i, j, Kind::Functional);
                    }
                }
            }
        }
        if ax.inverse_functional {
            if let Some(v) = d_po.get(&(d.predicate, d.object)) {
                for &j in v {
                    if j > i && derivation.facts[j].subject != d.subject && overlapping(j) {
                        note(i, j, Kind::InverseFunctional);
                    }
                }
            }
        }
    }
    let mut rule_clashes: Vec<RuleClash> = grouped
        .into_iter()
        .map(|((a, b, axiom), mut pairs)| {
            pairs.sort_unstable();
            pairs.dedup();
            RuleClash { a, b, axiom, pairs }
        })
        .collect();
    // 输出排过序——这条路的价值有一半在确定性
    rule_clashes.sort_by(|x, y| {
        (x.a.0, x.a.1.as_str(), x.b.0, x.b.1.as_str()).cmp(&(
            y.a.0,
            y.a.1.as_str(),
            y.b.0,
            y.b.1.as_str(),
        ))
    });
    out.between_derivations = rule_clashes;
    out
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
    fn a_chained_derivation_keeps_every_premise() {
        // 4→1→2 推出 4→2，而 4→2 已经是一条证明；传递再接 2→3 时，那条派生边
        // 自带的两条前提要一起进证明。少了任一条，`validity` 就只对留下的前提求
        // 交集，结论会比实际宽
        let edges = [
            te(1, 1, 2, Some(0), Some(100)),
            te(2, 2, 3, Some(40), Some(50)),
            te(3, 4, 1, Some(0), Some(100)),
        ];
        let d = derive(&edges, &transitive());
        let to3 = d
            .facts
            .iter()
            .find(|x| x.subject == n(4) && x.object == n(3))
            .expect("4→3 应由 4→1→2→3 推出");
        assert_eq!(
            to3.premises,
            vec![f(3), f(1), f(2)],
            "证明要带上链上三条前提，中间那条派生边的两条都在内"
        );
        let by_fact: HashMap<Uuid, _> = edges
            .iter()
            .map(|x| (x.edge.fact, (x.from, x.to)))
            .collect();
        assert_eq!(
            validity(&to3.premises, &by_fact),
            Some((Some(40), Some(50))),
            "有效期应是三条前提的交集，而不是漏掉中间那条的宽区间"
        );
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

    fn edges_with_shortcuts() -> Vec<TimedEdge> {
        // 21→8→16 与 14→1→7→12 都有捷径；先碰到绕路的证明，不该让
        // 后到的短证明失去继续推导的机会。11→20 的最短路径恰好 12 条前提。
        [
            (12, 0),
            (7, 12),
            (15, 10),
            (1, 7),
            (2, 5),
            (8, 16),
            (13, 21),
            (5, 13),
            (6, 14),
            (14, 12),
            (21, 8),
            (0, 20),
            (10, 2),
            (21, 16),
            (14, 1),
            (11, 15),
            (16, 6),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (s, o))| e(i as u8, s, o))
        .collect()
    }

    #[test]
    fn a_shorter_proof_keeps_a_chain_within_the_depth_limit() {
        let edges = edges_with_shortcuts();
        let d = derive(&edges, &transitive());
        let chain = d
            .facts
            .iter()
            .find(|x| x.subject == n(11) && x.object == n(20))
            .expect("12 条前提能走完的链不能被先到的长证明挡住");
        assert_eq!(chain.premises.len(), MAX_DEPTH);
        assert!(d.facts.iter().all(|x| x.premises.len() <= MAX_DEPTH));
        let distinct: HashSet<_> = d.facts.iter().map(|x| (x.subject, x.object)).collect();
        assert_eq!(distinct.len(), d.facts.len(), "短证明不再物化同一段事实");
        assert!(d.capped.is_empty());
    }

    #[test]
    fn shorter_proofs_do_not_spend_the_output_cap_twice() {
        let component = edges_with_shortcuts();
        // 多个互不相连的副本共用一个谓词；每份都含需要重新展开的短证明。
        // 封顶数的是实际产出的事实，不能拿这些替代证明抵扣额度。
        let edges: Vec<_> = (0..300u128)
            .flat_map(|copy| {
                component.iter().enumerate().map(move |(i, e)| TimedEdge {
                    edge: Edge {
                        fact: Uuid::from_u128(100_000 + copy * 32 + i as u128),
                        predicate: e.edge.predicate,
                        subject: Uuid::from_u128(copy * 32 + e.edge.subject.as_bytes()[0] as u128),
                        object: Uuid::from_u128(copy * 32 + e.edge.object.as_bytes()[0] as u128),
                    },
                    ..*e
                })
            })
            .collect();
        let d = derive(&edges, &transitive());
        assert_eq!(d.facts.len(), MAX_DERIVED_PER_PREDICATE);
        assert_eq!(d.capped, vec![n(99)]);
        let distinct: HashSet<_> = d.facts.iter().map(|x| (x.subject, x.object)).collect();
        assert_eq!(distinct.len(), d.facts.len());
        assert!(d.facts.iter().all(|x| x.premises.len() <= MAX_DEPTH));
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

    #[test]
    fn a_late_right_hand_edge_completes_an_existing_chain() {
        for (source, target) in [(Q, P), (P, Q)] {
            for inverse in [false, true] {
                let ax = HashMap::from([
                    (
                        source,
                        Axioms {
                            inverse_of: inverse.then_some(target),
                            sub_property_of: (!inverse).then_some(target),
                            ..Default::default()
                        },
                    ),
                    (
                        target,
                        Axioms {
                            transitive: true,
                            ..Default::default()
                        },
                    ),
                ]);
                let left = ep(target, 1, 1, 2);
                let right = if inverse {
                    ep(source, 2, 3, 2)
                } else {
                    ep(source, 2, 2, 3)
                };
                for edges in [[left, right], [right, left]] {
                    let d = derive(&edges, &ax);
                    let chain = d
                        .facts
                        .iter()
                        .find(|x| x.predicate == target && x.subject == n(1) && x.object == n(3))
                        .expect("右边晚一步推出，不能让已展开的左边再也接不上它");
                    assert_eq!(chain.rule, Rule::Transitive);
                    assert_eq!(chain.via, target);
                    assert_eq!(chain.premises, vec![f(1), f(2)]);
                    assert!(d.capped.is_empty());
                }
            }
        }
    }

    #[test]
    fn a_late_right_hand_edge_keeps_the_whole_prefix_and_its_span() {
        let ax = HashMap::from([
            (
                P,
                Axioms {
                    transitive: true,
                    ..Default::default()
                },
            ),
            (
                Q,
                Axioms {
                    sub_property_of: Some(P),
                    ..Default::default()
                },
            ),
            (
                R,
                Axioms {
                    sub_property_of: Some(Q),
                    ..Default::default()
                },
            ),
        ]);
        let edges = [
            tep(P, 1, 1, 2, Some(0), Some(30)),
            tep(P, 2, 2, 3, Some(10), Some(40)),
            tep(R, 3, 3, 4, Some(20), Some(50)),
            tep(R, 4, 3, 5, Some(30), Some(50)),
        ];
        let spans = edges
            .iter()
            .map(|e| (e.edge.fact, (e.from, e.to)))
            .collect();
        let d = derive(&edges, &ax);
        let chain = d
            .facts
            .iter()
            .find(|x| x.predicate == P && x.subject == n(1) && x.object == n(4))
            .expect("右边跨两轮到达时，已派生的左边也要接得上");
        assert_eq!(chain.premises, vec![f(1), f(2), f(3)]);
        assert_eq!(
            validity(&chain.premises, &spans),
            Some((Some(20), Some(30)))
        );
        assert!(
            !d.facts
                .iter()
                .any(|x| x.predicate == P && x.subject == n(1) && x.object == n(5)),
            "前缀止于 30，右边始于 30，半开区间没有交集"
        );
    }

    /// 同一个 triple 可以有几段时间都成立。第二段不能被第一段挡在传递之外，
    /// 而且输入顺序怎么排，时间闭包都要一样。
    #[test]
    fn distinct_intervals_for_one_triple_survive_axiom_steps_in_any_order() {
        let ax = HashMap::from([
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
                    transitive: true,
                    ..Default::default()
                },
            ),
        ]);
        let early = tep(P, 1, 1, 2, Some(0), Some(10));
        let late = tep(P, 2, 1, 2, Some(20), Some(30));
        let cb = tep(Q, 3, 3, 2, Some(20), Some(30));
        let spans = HashMap::from([
            (f(1), (Some(0), Some(10))),
            (f(2), (Some(20), Some(30))),
            (f(3), (Some(20), Some(30))),
        ]);
        let closure = |edges: [TimedEdge; 3]| {
            let d = derive(&edges, &ax);
            let mut closure: Vec<_> = d
                .facts
                .iter()
                .map(|x| {
                    let (from, to) = validity(&x.premises, &spans).expect("派生区间应当可求");
                    (x.predicate, x.subject, x.object, from, to)
                })
                .collect();
            closure.sort();
            closure
        };

        let early_first = closure([early, late, cb]);
        let late_first = closure([late, early, cb]);

        assert_eq!(early_first, late_first, "闭包不能随 fact 输入顺序改变");
        assert!(
            early_first.contains(&(Q, n(2), n(1), Some(0), Some(10))),
            "第一段 B q A 也要保留"
        );
        assert!(
            early_first.contains(&(Q, n(2), n(1), Some(20), Some(30))),
            "第二段 B q A 要保留"
        );
        assert!(
            early_first.contains(&(Q, n(3), n(1), Some(20), Some(30))),
            "第二段 B q A 必须还能和 C q B 接成 C q A"
        );
    }

    /// 断言的一段区间如果覆盖了派生出来的更窄区间，就不再留下第二条同 triple 的事实。
    #[test]
    fn an_asserted_interval_covers_a_narrower_derivation() {
        let edges = [
            te(1, 1, 2, Some(10), Some(20)),
            te(2, 2, 3, Some(10), Some(20)),
            te(3, 1, 3, Some(0), Some(100)),
        ];
        let d = derive(&edges, &transitive());
        assert!(
            d.facts.is_empty(),
            "断言 [0,100) 已覆盖推出来的 [10,20)，不该再派生一份"
        );
    }

    /// 无时间断言表示一直成立，也覆盖有日期的同 triple 派生。
    #[test]
    fn an_undated_assertion_covers_a_dated_derivation() {
        let edges = [
            te(1, 1, 2, Some(10), Some(20)),
            te(2, 2, 3, Some(10), Some(20)),
            te(3, 1, 3, None, None),
        ];
        let d = derive(&edges, &transitive());
        assert!(
            d.facts.is_empty(),
            "无时间断言覆盖 [10,20)，不该推出一条说得更少的同 triple 事实"
        );
    }

    /// 两条路径分别推出 [0,100) 与 [10,20)：宽的保留，窄的丢掉；输入反序也一样。
    #[test]
    fn a_wider_route_suppresses_a_narrower_derivation() {
        let edges = [
            te(1, 1, 2, Some(0), Some(100)),
            te(2, 2, 4, Some(0), Some(100)),
            te(3, 1, 3, Some(10), Some(20)),
            te(4, 3, 4, Some(10), Some(20)),
        ];
        let by_fact: HashMap<_, _> = edges
            .iter()
            .map(|e| (e.edge.fact, (e.from, e.to)))
            .collect();
        let spans = |edges: [TimedEdge; 4]| {
            let d = derive(&edges, &transitive());
            let mut spans: Vec<_> = d
                .facts
                .iter()
                .filter(|x| x.subject == n(1) && x.object == n(4))
                .map(|x| validity(&x.premises, &by_fact).expect("派生区间应当可求"))
                .collect();
            spans.sort();
            spans
        };

        let broad = (Some(0), Some(100));
        assert_eq!(spans(edges), vec![broad], "只留下覆盖更宽的 [0,100)");
        assert_eq!(
            spans([edges[3], edges[2], edges[1], edges[0]]),
            vec![broad],
            "输入反序不能把被覆盖的窄区间又放回来"
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

    // ---------- 矛盾（0017） ----------

    /// 指定谓词的一条带区间的边
    fn et(pred: Uuid, fact: u8, s: u8, o: u8, from: Option<i64>, to: Option<i64>) -> TimedEdge {
        TimedEdge {
            edge: Edge {
                fact: f(fact),
                predicate: pred,
                subject: n(s),
                object: n(o),
            },
            from,
            to,
        }
    }

    fn spans_of(edges: &[TimedEdge]) -> HashMap<Uuid, (Option<i64>, Option<i64>)> {
        edges
            .iter()
            .map(|e| (e.edge.fact, (e.from, e.to)))
            .collect()
    }

    /// `ceo_of ⊑ works_at`，works_at functional：Mira 的 ceo_of 推出 works_at Acme，
    /// 而账本里说她 works_at Globex——派生撞上断言，指名道姓
    #[test]
    fn a_derivation_that_breaks_functional_names_the_assertion_it_hit() {
        let ax = HashMap::from([
            (
                P,
                Axioms {
                    sub_property_of: Some(Q),
                    ..Default::default()
                },
            ),
            (
                Q,
                Axioms {
                    functional: true,
                    ..Default::default()
                },
            ),
        ]);
        let edges = [ep(P, 1, 1, 2), ep(Q, 2, 1, 3)];
        let d = derive(&edges, &ax);
        assert_eq!(d.facts.len(), 1);
        let c = contradictions(&d, &edges, &ax, &spans_of(&edges));
        assert_eq!(
            c.with_assertions,
            vec![Clash {
                derived: 0,
                axiom: Kind::Functional,
                against: f(2)
            }]
        );
        assert!(c.between_derivations.is_empty());
        assert_eq!(c.blocked(), HashSet::from([0]));
    }

    /// 区间不交就不是矛盾：前任与继任
    #[test]
    fn disjoint_intervals_are_succession_and_stay_silent() {
        let ax = HashMap::from([
            (
                P,
                Axioms {
                    sub_property_of: Some(Q),
                    ..Default::default()
                },
            ),
            (
                Q,
                Axioms {
                    functional: true,
                    ..Default::default()
                },
            ),
        ]);
        let edges = [
            et(P, 1, 1, 2, Some(10), Some(20)),
            et(Q, 2, 1, 3, Some(30), None),
        ];
        let d = derive(&edges, &ax);
        let c = contradictions(&d, &edges, &ax, &spans_of(&edges));
        assert!(c.with_assertions.is_empty(), "{c:?}");
    }

    /// 对称与非对称：`A p B` 对称推出 `B p A`，而 p 又声明 asymmetric——
    /// 每条断言都撞上自己的镜像
    #[test]
    fn a_symmetric_derivation_hits_the_asymmetric_assertion() {
        let ax = HashMap::from([(
            P,
            Axioms {
                symmetric: true,
                asymmetric: true,
                ..Default::default()
            },
        )]);
        let edges = [ep(P, 1, 1, 2)];
        let d = derive(&edges, &ax);
        let c = contradictions(&d, &edges, &ax, &spans_of(&edges));
        assert_eq!(c.with_assertions.len(), 1);
        assert_eq!(c.with_assertions[0].axiom, Kind::Asymmetry);
        assert_eq!(c.with_assertions[0].against, f(1));
    }

    /// 两条派生互撞时按规则对聚合，而且都不落地
    #[test]
    fn derivations_that_disagree_are_grouped_by_the_rules_that_made_them() {
        let ax = HashMap::from([
            (
                P,
                Axioms {
                    sub_property_of: Some(Q),
                    ..Default::default()
                },
            ),
            (
                Q,
                Axioms {
                    functional: true,
                    ..Default::default()
                },
            ),
        ]);
        // 1 ceo_of 2 与 1 ceo_of 3：两条 works_at 由同一条规则推出，互相排斥
        let edges = [ep(P, 1, 1, 2), ep(P, 2, 1, 3), ep(P, 3, 4, 5)];
        let d = derive(&edges, &ax);
        assert_eq!(d.facts.len(), 3);
        let c = contradictions(&d, &edges, &ax, &spans_of(&edges));
        assert!(c.with_assertions.is_empty());
        assert_eq!(c.between_derivations.len(), 1);
        let rc = &c.between_derivations[0];
        assert_eq!(rc.a, (P, Rule::SubProperty));
        assert_eq!(rc.b, (P, Rule::SubProperty));
        assert_eq!(rc.axiom, Kind::Functional);
        assert_eq!(rc.pairs.len(), 1);
        // 第三条（4 works_at 5）没跟谁撞，照常落地
        assert_eq!(c.blocked().len(), 2);
        assert!(!c.blocked().contains(&2));
    }

    /// 谓词上没有公理就没有矛盾可言
    #[test]
    fn a_predicate_without_axioms_cannot_contradict() {
        let ax = HashMap::from([(
            P,
            Axioms {
                sub_property_of: Some(Q),
                ..Default::default()
            },
        )]);
        let edges = [ep(P, 1, 1, 2), ep(Q, 2, 1, 3)];
        let d = derive(&edges, &ax);
        let c = contradictions(&d, &edges, &ax, &spans_of(&edges));
        assert_eq!(c, Contradictions::default());
    }
}
