//! 记录轴谓词（0019）：`held_at(T)`——**T 时刻我们认为哪些行成立**。
//!
//! 世界轴（`valid_from` / `valid_to`）答"那时世界是什么样"，记录轴（`recorded_at` /
//! `invalidated_at`）答"那时我们以为世界是什么样"。写入侧从图谱迁移起就一直记着
//! 两根轴，读出侧只倒得回第一根——三月被改掉的事实，在任何滑杆位置上都不存在。
//!
//! **谓词只在这里拼。** 防御一旦散到每个读点，漏掉一个就悄无声息：SQL 不报错，
//! `cargo check` 也不会说一个字（0009 栽的正是这一跤，`human_type_decisions`
//! 那个测试就是那次留下的）。所以读路径引这里的函数，不自己写 `invalidated_at`。
//!
//! **写路径不用**（0019）：`confirm_fact` / `reject_fact`、采纳的撤销、去重查重
//! 都是对"当前那一行"的守卫——修正永远发生在现在，没有"以三月的身份改一行"这回事。
//!
//! 参数是 `Option<usize>`（绑定参数的序号）：`None` 即"现在"，谓词在 **SQL 文本上**
//! 退化成 `invalidated_at IS NULL`——不只是语义等价，还要让优化器看见它，否则
//! `IS NULL OR > now()` 的双支会让 `WHERE invalidated_at IS NULL` 的部分索引
//! 整棵不可用（`held()` 的注释记着那次 61.6s 的教训）。读路径因此只写一处调用，
//! 而不是为回放和当下各写一条——两条就是下一次漏改的地方。
//!
//! **退化成 `IS NULL` 押在一条写入不变量上**：任何行都不会携带未来的
//! `invalidated_at` / `deleted_at` / `decided_at` / `resolved_at`——撤销永远盖
//! `now()` 的戳。有它，`IS NULL` 才与 `IS NULL OR > now()` 同义；没有 CHECK
//! 守着它，只有这段文字。它若破了（比如预填一个未来的作废时刻），`None` 路径
//! 会把"尚未生效的作废"当成"从未作废"。退化前有一个调用点碰巧对这种违例稳健，
//! 退化后四个谓词全都指望它——所以它必须写在这里，让下一个读者撞上。

/// 起止两列构成的记录轴区间：`since <= T < invalidated_at`。
///
/// **`None` 必须退化成单支谓词**，不能照旧输出 `IS NULL OR > now()` 的双支：
/// 部分索引（`facts_live_subject_idx` 等，`WHERE invalidated_at IS NULL`）只在
/// 查询谓词能推出 `invalidated_at IS NULL` 时可用，`OR > now()` 让优化器
/// 证不出来——读路径退成逐实体全表扫（9311 实体 × 94683 事实的库上
/// `graph/overview` 节点查询 61.6s，退化后 4.3s）。
fn held(alias: &str, since: &str, as_of: Option<usize>) -> String {
    match as_of {
        None => format!("{alias}.invalidated_at IS NULL"),
        Some(param) => format!(
            "{alias}.{since} <= coalesce(${param}, now()) \
             AND ({alias}.invalidated_at IS NULL OR {alias}.invalidated_at > coalesce(${param}, now()))"
        ),
    }
}

/// `facts`：断言在 T 时刻仍被我们持有。
pub fn facts_held_at(alias: &str, as_of: Option<usize>) -> String {
    held(alias, "recorded_at", as_of)
}

/// `derived_facts`：派生在 T 时刻已推出且未被推翻——回放的图上留着**当时**推出的边，
/// 而不是今天这套规则的结论。
pub fn derived_held_at(alias: &str, as_of: Option<usize>) -> String {
    held(alias, "derived_at", as_of)
}

/// `axiom_violations`：违规在 T 时刻还开着。列名与上面两张表不同（`detected_at` /
/// `decided_at` + `status`），但问的是同一个问题——所以也归这里，别在读点上现拼。
///
/// 已裁掉却没留 `decided_at` 的历史行按"当时就不开着"算：宁可少画一条幽灵边，
/// 也不要凭空给三月的图加一条今天才发现的矛盾。
pub fn violation_open_at(alias: &str, as_of: Option<usize>) -> String {
    match as_of {
        None => format!("{alias}.status = 'open'"),
        Some(param) => format!(
            "{alias}.detected_at <= coalesce(${param}, now()) \
             AND ({alias}.status = 'open' OR {alias}.decided_at > coalesce(${param}, now()))"
        ),
    }
}

/// `fact_conflicts`：时态冲突在 T 时刻还开着。
pub fn conflict_open_at(alias: &str, as_of: Option<usize>) -> String {
    match as_of {
        None => format!("{alias}.status = 'open'"),
        Some(param) => format!(
            "{alias}.created_at <= coalesce(${param}, now()) \
             AND ({alias}.status = 'open' OR {alias}.resolved_at > coalesce(${param}, now()))"
        ),
    }
}

/// `documents`：文档在 T 时刻还在库里。删除留墓碑（#268），所以"删掉的文档"
/// 在删除之前的任何时刻都该照常出现——它的分块当时确实是可检索的。
pub fn document_live_at(alias: &str, as_of: Option<usize>) -> String {
    match as_of {
        None => format!("{alias}.deleted_at IS NULL"),
        Some(param) => format!(
            "{alias}.created_at <= coalesce(${param}, now()) \
             AND ({alias}.deleted_at IS NULL OR {alias}.deleted_at > coalesce(${param}, now()))"
        ),
    }
}

/// `chunks`：分块在 T 时刻还是现行版本。证据是否"已消失"要按当时的版本判——
/// 今天被重解析顶掉的段落，在三月的图上仍然是活证据。
pub fn chunk_live_at(alias: &str, as_of: Option<usize>) -> String {
    match as_of {
        None => format!("{alias}.superseded_at IS NULL"),
        Some(param) => format!(
            "{alias}.created_at <= coalesce(${param}, now()) \
             AND ({alias}.superseded_at IS NULL OR {alias}.superseded_at > coalesce(${param}, now()))"
        ),
    }
}

/// `entity_merges`：这次合并在 T 时刻**生效着**吗（0019 第二刀 / #336）。
///
/// 实体身上没有记录轴——`merged_into` 只说合并发生过，不说何时。时刻在这张表上，
/// 而它和别的表问的是同一个问题，所以列名不同、形状一样。
pub fn merge_in_effect_at(alias: &str, as_of: Option<usize>) -> String {
    match as_of {
        None => format!("{alias}.reverted_at IS NULL"),
        Some(param) => format!(
            "{alias}.created_at <= coalesce(${param}, now()) \
             AND ({alias}.reverted_at IS NULL OR {alias}.reverted_at > coalesce(${param}, now()))"
        ),
    }
}

/// 实体在 T 时刻是不是一个独立的节点：那时已经存在，且没有被一次生效中的合并吞掉。
///
/// **取代读路径上的 `merged_into IS NULL`。** 参数为 `None` 时两者等价（已撤销的合并
/// 此刻不生效，那个实体本来就该出现），但传了时刻之后，三月被并掉的实体在二月
/// 会重新长回来——那正是这一刀要的。
pub fn entity_visible_at(alias: &str, as_of: Option<usize>) -> String {
    let merged = merge_in_effect_at("m", as_of);
    match as_of {
        None => format!(
            "NOT EXISTS (SELECT 1 FROM entity_merges m \
                          WHERE m.source_id = {alias}.id AND {merged})"
        ),
        Some(param) => format!(
            "{alias}.created_at <= coalesce(${param}, now()) \
             AND NOT EXISTS (SELECT 1 FROM entity_merges m \
                          WHERE m.source_id = {alias}.id AND {merged})"
        ),
    }
}

/// 一条事实在 T 时刻的主语（`on_object = false`）或宾语。
///
/// **现在这条路不进函数**：`fact_owner_at` 包住列之后索引就用不上了，而
/// 「现在」是每一次画图都要走的路。回放才付这个代价——它本来就少见。
pub fn owner_at(fact_alias: &str, column: &str, as_of: Option<usize>, on_object: bool) -> String {
    match as_of {
        None => format!("{fact_alias}.{column}"),
        Some(param) => {
            format!("fact_owner_at({fact_alias}.id, {fact_alias}.{column}, ${param}, {on_object})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「现在」那条路必须退化成单支。这不是风格问题：`IS NULL OR > now()` 让
    /// 优化器证不出 `invalidated_at IS NULL`，`facts_live_subject_idx` 这类
    /// 部分索引整棵不可用（实测双支走 `facts_live_time_idx` 每次扫 9,877 行，
    /// 单支走 BitmapOr 扫 4 行）。性能这条性质在别处没有守卫，只有这里。
    #[test]
    fn a_predicate_for_now_has_no_second_branch() {
        for (what, sql) in [
            ("facts_held_at", facts_held_at("f", None)),
            ("derived_held_at", derived_held_at("d", None)),
            ("violation_open_at", violation_open_at("v", None)),
            ("conflict_open_at", conflict_open_at("c", None)),
            ("document_live_at", document_live_at("d", None)),
            ("chunk_live_at", chunk_live_at("c", None)),
            ("merge_in_effect_at", merge_in_effect_at("m", None)),
        ] {
            assert!(
                !sql.contains(" OR "),
                "{what} 在「现在」下还留着双支：{sql}"
            );
            assert!(
                !sql.contains('$'),
                "{what} 在「现在」下不该有绑定参数：{sql}"
            );
            assert!(
                !sql.contains("now()"),
                "{what} 在「现在」下不该比时刻：{sql}"
            );
        }
    }

    /// 回放那条路原样保留：带上时刻参数、两支都在。少了哪一支，三月的图上就会
    /// 多出今天才作废的行，或者少掉当时还活着的行。
    #[test]
    fn a_predicate_for_a_moment_keeps_both_branches() {
        let sql = facts_held_at("f", Some(3));
        assert!(
            sql.contains("f.recorded_at <= coalesce($3, now())"),
            "{sql}"
        );
        assert!(
            sql.contains("f.invalidated_at IS NULL OR f.invalidated_at > coalesce($3, now())"),
            "{sql}"
        );
    }

    /// `entity_visible_at(None)` 顶替读路径上的 `merged_into IS NULL`：它换成了
    /// 对 `entity_merges` 的反连接（多一次 hash anti join），但不再按实体行判。
    /// 传了时刻才额外要求「那时已经建出来了」。
    #[test]
    fn an_entity_is_visible_now_unless_a_live_merge_swallowed_it() {
        let now = entity_visible_at("e", None);
        assert!(now.starts_with("NOT EXISTS"), "{now}");
        assert!(now.contains("m.reverted_at IS NULL"), "{now}");
        assert!(!now.contains("e.created_at"), "{now}");
        assert!(entity_visible_at("e", Some(2)).contains("e.created_at <= coalesce($2, now())"));
    }
}
