//! 把模型说出的谓词落到本体里**已经有**的关系上。
//!
//! 抽取原本只做精确 key 比对（`rel_ids.get(f.predicate)`），对不上就降级成
//! `related_to`，原词留在 `fact_evidence.proposed_predicate` 等人来采纳。
//!
//! 实测这样漏得很凶。ai-timeline 语料（维基百科的 AI 公司条目）跑到一半时，
//! 49.8% 的事实是 `related_to`，`ontology_misses` 里 398 个不同的关系名、895 次使用。
//! 其中一类根本不是缺词汇，是**词汇就在本体里，模型只是说反了或换了时态**：
//! `produced_by | ChatGPT → OpenAI` 想说的就是已有的 `produces`，主宾对调而已。
//!
//! 那个 49.8% 是**测量台把自动扩本体关掉之后**的数字（`run.mjs` 建库即置
//! `auto_extend_ontology=FALSE`，而列默认值是 true）。产品的实际冷启动路径是
//! `bootstrap_ontology` 事后补本体，所以别拿这个占比去说产品有多糟——
//! 它量的是"缺词汇时降级有多频繁"，而这正是本模块要减少的那一部分。
//!
//! 这里补三段，**顺序即优先级**，宽的永远排在窄的后面：
//!
//! 1. 精确 key —— 原有行为，一字不改
//! 2. 写法对齐 —— `acquiredFrom` / `acquired_from` / `Acquired From` 是同一个
//! 3. 屈折归一 —— `produced` 与 `produces` 折到同一串（只削时态与单复数，见 `inflect_base`）
//!
//! 2、3 各再试一次「去掉结尾的 by」，命中就**把主宾对调**：英语里 `_by` 是被动的
//! 明确标记，`X produced_by Y` 与 `Y produces X` 是同一条边。
//!
//! **撞车就不匹配。** 本体里若同时有 `produces` 和 `produced`，两者折到同一串，
//! 这时选谁都是猜——宁可降级，让人去采纳。只有精确 key 不受此限，它本来就唯一。
//!
//! **量到了什么**（同一份 895 次未匹配，两种词表）：种子本体 10 个关系捞回 49 次；
//! schema.org 629 个关系捞回 59 次。捞回的 9 个说法里 6 个正确、3 个方向可疑
//!（`addresses→address`、`funds→funding`、`sponsors→sponsor`），各 1 次，
//! **全部来自光削一个 `s` 那条路**——英语里 `sponsors` 既是动词第三人称也是名词复数，
//! 靠后缀分不出来。没为这三条再加规则：样本太小，加了就是过拟合。
//! 它们的反事实也不是「正确」，而是 `related_to`——原词仍在 proposed_predicate 里。
//!
//! 不做同义判断（`partners_with` 与 `collaborates_with`）：那是检索与模型的活。
//! 摆在这里会把「写法对齐」悄悄变成「意思大概差不多」，而后者错了没人看得见。

use std::collections::HashMap;
use utopia_core::models::RelationType;
use uuid::Uuid;

/// 切词：非字母数字处切，驼峰的大小写交界处也切。
///
/// 驼峰那一半不是可选的：OWL 导入的 key 就是 `acquiredFrom`，不切的话它跟手写的
/// `acquired_from` 折不到一起，而「导入的本体能不能被用上」正是这条路要保的。
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_is_lower_or_digit = false;
    for c in s.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_is_lower_or_digit = false;
            continue;
        }
        if c.is_uppercase() && prev_is_lower_or_digit && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_is_lower_or_digit = c.is_lowercase() || c.is_numeric();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 写法对齐形：切完的词直接拼起来，分隔符不留。与
/// `api::ontology_routes::normalize_name` 同规则。
fn joined(words: &[String]) -> String {
    words.concat()
}

/// 屈折归一：削掉时态与单复数，**不碰派生后缀**。
///
/// 这里一度用的是 Snowball（`rust-stemmers`），实测反了：词表 10 个词时捞回 49 次，
/// 换成 schema.org 的 629 个词只剩 18 次。因为 Snowball 连派生后缀一起削，
/// `producer` 与 `produces` 都成了 `produc`，撞车规则于是拒绝匹配——**词表越大越不敢动**。
/// 而 `producer`（一个人）和 `produces`（一个动作）本来就该是两个关系，
/// 折到一起是词干器的错，不是撞车规则的错。
///
/// 所以只做屈折：`-ies→y`、`-ing`、`-ed`、`-s`（`-ss` 除外），削完再去掉结尾的 `e`。
/// 最后那一步是为了让 `-s` 和 `-ed` 两条路汇合——`produces→produce→produc`、
/// `produced→produc`，否则同一个动词的两种时态永远对不上。
///
/// 长度守卫看的是**削完之后**的结果，不是中间结果。按中间结果卡会让两条路
/// 走岔：`uses` 削 `-s` 剩三个字母过关、再掉个 `e` 变两个，而 `used` 削 `-ed`
/// 当场就是两个、被拦下——同一个动词的两种时态于是永远对不上。
/// 剩不到两个字母就整个不削（`is` 不该变成 `i`，`led` 不该变成 `l`）。
///
/// 中文没有屈折后缀，走这里是恒等变换，所以不必按 `ontology_lang` 分叉。
fn inflect_base(w: &str) -> String {
    let n = w.len();
    let mut s = if w.ends_with("ies") && n > 3 {
        format!("{}y", &w[..n - 3])
    } else if w.ends_with("ing") && n > 3 {
        w[..n - 3].to_string()
    } else if w.ends_with("ed") && n > 2 {
        w[..n - 2].to_string()
    } else if w.ends_with('s') && !w.ends_with("ss") && n > 1 {
        w[..n - 1].to_string()
    } else {
        w.to_string()
    };
    // 结尾的 e 一律去掉：`produces→produce` 与 `produced→produc` 靠这一步汇合，
    // 顺带让 `note` 与 `notes` 落到一起
    if s.ends_with('e') {
        s.pop();
    }
    if s.chars().count() < 2 {
        return w.to_string();
    }
    s
}

/// 领头的轻动词：`has_funding` 与 `funding` 是同一个关系，前缀是命名习惯不是意思。
///
/// 实测（ai-timeline-ends × schema.org）：空谓词事实的原文说法里
/// `has_funding` ×4 落空，而本体里就有 `funding`；`product` ×2 落空，而本体里有
/// `has_product`——**差的只是这一个前缀**。
///
/// 只在还剩词的时候剥：`has` 单独一个词是它自己，不能剥成空。
/// 过度归并不会造成错配——`insert` 遇到一个键落到两个关系上就作废（撞车即作废），
/// 所以最坏情况是退回"匹配不上"，而不是匹配到错的那个。
const LEADING_AUX: &[&str] = &[
    "has", "have", "had", "is", "are", "was", "were", "be", "been",
];

fn stems(words: &[String]) -> Vec<String> {
    let words = match words.split_first() {
        Some((head, rest)) if !rest.is_empty() && LEADING_AUX.contains(&head.as_str()) => rest,
        _ => words,
    };
    words.iter().map(|w| inflect_base(w)).collect()
}

/// 一个说法的**归并键**：同一个关系的不同时态落到同一个键上。
///
/// 给本体采纳那条路用的。抽取时说法要跟**已有**关系比对（上面那套三段匹配），
/// 采纳时要做的是另一件事——把彼此之间是同一个意思的说法先并起来再算票数。
///
/// 不并就要吃亏，而且吃得见：ai-timeline 跑完后还压在兜底谓词上的说法里，
/// `sued` 与 `sues` 是两条（各 11 和 5）、`integrated_with` 与 `integrates_with`
/// 是两条（各 5）、`announced`/`announces`、`supported`/`supports`、
/// `developed`/`develops` 都是。各自算票，各自够不着「出现在 ≥2 篇」的门槛；
/// 并起来之后够格的候选从 70 组涨到 85 组、281 条涨到 355 条。
///
/// **介词不并**：`integrated_with` 与 `integrated_into` 保持两组。`works_at` 与
/// `works_in` 确实可能是两回事，这里宁可漏。
///
/// **`_by` 也不并**（待做）：`founded_by` 与 `founded` 是同一条边的两个方向。
///
/// 当初不并的理由是「采纳路径从旧行原样复制主语，对调不了」——**那个理由已经
/// 不成立**（#109 让 `adopt` 显式绑定主语并支持交换）。现在缺口只剩一处：
/// 两边都还不在本体里时（`founded_by` 42 条、`founded` 4 条，都够票），
/// 采纳前的 `PredicateIndex` 查询谁也匹配不上，于是各建一个、方向相反。
/// 补法是把 `_by` 折进同一组并把整组标成需对调，不再有阻碍。
pub fn merge_key(form: &str) -> Vec<String> {
    stems(&words(form))
}

/// 撞车即作废：同一个形式落到两个不同的关系上，选谁都是猜。
fn insert<K: std::hash::Hash + Eq>(map: &mut HashMap<K, Option<Uuid>>, key: K, id: Uuid) {
    map.entry(key)
        .and_modify(|slot| {
            if *slot != Some(id) {
                *slot = None;
            }
        })
        .or_insert(Some(id));
}

pub struct PredicateIndex {
    exact: HashMap<String, Uuid>,
    by_joined: HashMap<String, Option<Uuid>>,
    by_stems: HashMap<Vec<String>, Option<Uuid>>,
}

impl PredicateIndex {
    /// **只收 `kind == "relation"`。** 属性走字面值通道，模糊匹配跨过去，
    /// `founding_date` 就会变成一条指向实体「2015」的边——那正是本体采纳那条路
    /// 已经专门挡掉的东西，这里不能从后门放回来。
    pub fn build(rtypes: &[RelationType]) -> Self {
        let mut exact = HashMap::new();
        let mut by_joined = HashMap::new();
        let mut by_stems = HashMap::new();
        for r in rtypes.iter().filter(|r| r.kind == "relation") {
            exact.insert(r.key.clone(), r.id);
            let w = words(&r.key);
            if w.is_empty() {
                continue;
            }
            insert(&mut by_joined, joined(&w), r.id);
            insert(&mut by_stems, stems(&w), r.id);
        }
        Self {
            exact,
            by_joined,
            by_stems,
        }
    }

    fn widened(&self, w: &[String]) -> Option<Uuid> {
        if w.is_empty() {
            return None;
        }
        if let Some(hit) = self.by_joined.get(&joined(w)) {
            return *hit;
        }
        *self.by_stems.get(&stems(w))?
    }

    /// 返回 `(关系 id, 主宾是否要对调)`。`None` = 本体里确实没有，该降级。
    pub fn lookup(&self, proposed: &str) -> Option<(Uuid, bool)> {
        if let Some(id) = self.exact.get(proposed) {
            return Some((*id, false));
        }
        let w = words(proposed);
        if let Some(id) = self.widened(&w) {
            return Some((id, false));
        }
        // 被动形：`produced_by` 去掉 by 之后才对得上 `produces`，且主宾要反过来。
        // 要求至少两个词——光一个 `by` 削完是空的。
        if w.len() >= 2 && w[w.len() - 1] == "by" {
            if let Some(id) = self.widened(&w[..w.len() - 1]) {
                return Some((id, true));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `has_funding` 与 `funding` 是同一个关系，前缀是命名习惯不是意思。
    ///
    /// 实测里这两组各自落空：本体有 `funding`、模型写 `has_funding`（×4）；
    /// 本体有 `has_product`、模型写 `product`（×2）。差的只是这一个前缀。
    #[test]
    fn a_leading_auxiliary_does_not_make_a_different_relation() {
        let rels = vec![rel("funding"), rel("has_product")];
        let idx = PredicateIndex::build(&rels);
        assert!(
            idx.lookup("has_funding").is_some(),
            "has_funding 该落到 funding 上"
        );
        assert!(
            idx.lookup("product").is_some(),
            "product 该落到 has_product 上"
        );
        // 两个方向都要通
        assert!(idx.lookup("funding").is_some());
        assert!(idx.lookup("has_product").is_some());
    }

    /// **只在还剩词的时候剥。** `has` 单独一个词是它自己，剥成空就什么都匹配了。
    #[test]
    fn a_bare_auxiliary_is_still_a_word() {
        let rels = vec![rel("has")];
        let idx = PredicateIndex::build(&rels);
        assert!(idx.lookup("has").is_some(), "has 自己该匹配得上");
        assert!(idx.lookup("owns").is_none(), "剥成空会让不相干的词也匹配上");
    }

    /// 撞车仍然作废：本体同时有 `funding` 与 `has_funding` 时，选谁都是猜。
    /// 过度归并的最坏结果是「匹配不上」，不是「匹配到错的那个」。
    #[test]
    fn folding_the_prefix_never_produces_a_wrong_match() {
        let rels = vec![rel("funding"), rel("has_funding")];
        let idx = PredicateIndex::build(&rels);
        // 精确 key 仍然直达
        assert!(idx.lookup("funding").is_some());
        assert!(idx.lookup("has_funding").is_some());
        // 撞车在**词干**那一层：`funding` 与 `has_funding` 剥掉前缀后都是 ["funding"]。
        // 要测它就得给一个走不到写法对齐、只能落到词干的形式——
        // `has_fundings` 拼起来是 hasfundings，本体里没有，于是往下走到词干，撞车作废
        assert!(
            idx.lookup("has_fundings").is_none(),
            "词干层撞车了却还是选了一个"
        );
    }

    fn rel(key: &str) -> RelationType {
        RelationType {
            id: Uuid::new_v4(),
            kb_id: Uuid::nil(),
            key: key.to_string(),
            label: key.to_string(),
            temporal: "state".into(),
            functional: false,
            inverse_functional: false,
            builtin: false,
            description: String::new(),
            iri: None,
            kind: "relation".into(),
            domains: Vec::new(),
            ranges: Vec::new(),
            datatype: None,
            unit: None,
        }
    }

    fn attr(key: &str) -> RelationType {
        RelationType {
            kind: "attribute".into(),
            ..rel(key)
        }
    }

    #[test]
    fn splits_camel_case_and_separators_the_same_way() {
        assert_eq!(words("acquiredFrom"), ["acquired", "from"]);
        assert_eq!(words("acquired_from"), ["acquired", "from"]);
        assert_eq!(words("Acquired From"), ["acquired", "from"]);
        // 全大写不是驼峰边界，别把 IRI 切成 i/r/i
        assert_eq!(words("IRI"), ["iri"]);
        assert_eq!(words("gpt4Model"), ["gpt4", "model"]);
    }

    #[test]
    fn exact_key_still_wins_unchanged() {
        let types = [rel("produces")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("produces"), Some((types[0].id, false)));
    }

    #[test]
    fn separator_and_case_differences_align() {
        let types = [rel("acquired_from")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("acquiredFrom"), Some((types[0].id, false)));
        assert_eq!(idx.lookup("Acquired From"), Some((types[0].id, false)));
    }

    #[test]
    fn tense_folds_without_swapping() {
        let types = [rel("produces")];
        let idx = PredicateIndex::build(&types);
        // 模型写过去式，说的还是同一条边，方向也没变
        assert_eq!(idx.lookup("produced"), Some((types[0].id, false)));
    }

    /// 这条是这个模块存在的理由：`ChatGPT produced_by OpenAI` 与
    /// `OpenAI produces ChatGPT` 是同一条边，只差主宾方向。
    #[test]
    fn passive_form_matches_and_asks_for_a_swap() {
        let types = [rel("produces")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("produced_by"), Some((types[0].id, true)));
        assert_eq!(idx.lookup("producedBy"), Some((types[0].id, true)));
    }

    #[test]
    fn multi_word_passive_matches() {
        let types = [rel("invests_in")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("invested_in"), Some((types[0].id, false)));
    }

    /// **派生不是屈折。** 这条是换掉 Snowball 的理由：schema.org 里 `producer`
    /// 与 `produces` 并存，词干器把两个都折成 `produc`，于是撞车规则把
    /// `produced` 也一并拒了——词表越大捞得越少（实测 49 次掉到 18 次）。
    /// 只削屈折后缀就不会有这个撞车：`producer` 保持原样。
    #[test]
    fn derivational_forms_stay_separate_from_inflected_ones() {
        let types = [rel("produces"), rel("producer"), rel("production_company")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("produced"), Some((types[0].id, false)));
        assert_eq!(idx.lookup("produced_by"), Some((types[0].id, true)));
        assert_eq!(idx.lookup("producer"), Some((types[1].id, false)));
        // 派生词之间也别互相串
        assert_eq!(idx.lookup("producers"), Some((types[1].id, false)));
    }

    /// 单复数与时态两条路要汇合到同一串，否则同一个动词的两种写法永远对不上。
    #[test]
    fn plural_and_past_forms_meet_at_the_same_base() {
        assert_eq!(inflect_base("produces"), inflect_base("produced"));
        assert_eq!(inflect_base("uses"), inflect_base("used"));
        assert_eq!(inflect_base("notes"), inflect_base("note"));
        assert_eq!(inflect_base("studies"), inflect_base("study"));
        // 短词不削：is 不该变成 i
        assert_eq!(inflect_base("is"), "is");
        // -ss 不是复数
        assert_eq!(inflect_base("address"), inflect_base("addresses"));
    }

    /// 撞车宁可不匹配：本体里同时有 `produces` 和 `produced` 时，
    /// `producing` 折到两者共同的词干上，选谁都是猜。
    #[test]
    fn ambiguous_stem_declines_rather_than_guesses() {
        let types = [rel("produces"), rel("produced")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("producing"), None);
        // 但精确 key 不受影响，它本来就唯一
        assert_eq!(idx.lookup("produces"), Some((types[0].id, false)));
        assert_eq!(idx.lookup("produced"), Some((types[1].id, false)));
    }

    /// 属性不能从这条路被匹配上：宾语是字面值，配不出一条边。
    #[test]
    fn attributes_are_not_reachable() {
        let types = [attr("founding_date")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("founding_date"), None);
        assert_eq!(idx.lookup("foundingDate"), None);
    }

    /// 本体里真没有的，照旧降级——这里不做同义判断。
    #[test]
    fn genuinely_missing_vocabulary_still_declines() {
        let types = [rel("produces"), rel("works_at")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("partners_with"), None);
        assert_eq!(idx.lookup("acquired"), None);
        // 介词不同就是不同的关系，别替模型改口
        assert_eq!(idx.lookup("works_in"), None);
    }

    #[test]
    fn lone_by_does_not_strip_to_nothing() {
        let types = [rel("produces")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("by"), None);
        assert_eq!(idx.lookup("_by_"), None);
    }

    /// 中文 key 走词干器是恒等变换，不该被削也不该错配。
    #[test]
    fn chinese_keys_pass_through() {
        let types = [rel("隶属于"), rel("生产")];
        let idx = PredicateIndex::build(&types);
        assert_eq!(idx.lookup("隶属于"), Some((types[0].id, false)));
        assert_eq!(idx.lookup("生产"), Some((types[1].id, false)));
        assert_eq!(idx.lookup("收购"), None);
    }
}
