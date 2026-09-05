//! 世界轴谓词（0022）：`holds_at(T)`——**T 时刻哪些事实成立**。
//!
//! 写入侧早就分得清「仍在持续」与「结束了，不知哪天」，也从不替原文编一个起点。
//! 读出侧却把两者都读回「随时成立」：`valid_from IS NULL` 被读成自古如此，
//! `valid_to IS NULL` 不看旁边的精度就读成至今仍是。问一个证据出现之前的时刻，
//! 或一个原文说已结束、只是没给日期的时刻，图都会理直气壮地回答——引的恰恰是
//! 那条说它不该成立的行（#345、#352）。
//!
//! **谓词只在这里拼**，理由与 `record_axis` 相同：散在每个读点的防御，漏一处就
//! 无声无息。前端也不再自己算一遍——边和事实带着 `holds_from` / `holds_to`
//! （按这里同一套表达式投影出来的「读出来的区间」），滑杆只按它们过滤。
//!
//! 未知的一端**读到证据为止**：`attested_at` 是这一行的各次观察里最早那份文档的
//! 日期。没有起点 → 从它起成立；结束了不知哪天 → 到它为止。两端的不对称是故意
//! 的：开放的结束端仍读作「直到有人说它结束」——结束会以记录的形式到来（后面的
//! 文档、人的修正），把行关上；而缺失的起点没有这样的修正者，不会有谁来说
//! 「2023 年它还没开始」。所以事实从有证据的那一刻起成立，之前的诚实答案是没有。
//!
//! `at` 为 NULL 在世界轴上是**每一刻**（画布画的是历史，滑杆负责收窄），与记录轴
//! 的「NULL 即现在」不同：没有人持有一个晚于此刻的信念，而没有时刻的图是全部
//! 时间的图。

/// `facts`：读出来的下界——原文给了起点用起点，否则从最早的证据起。
pub fn facts_holds_from(alias: &str) -> String {
    format!("COALESCE({alias}.valid_from, {alias}.attested_at)")
}

/// `facts`：读出来的上界——原文给了终点用终点；说结束了但不知哪天，到最早说出它
/// 的那份文档为止；否则开放（NULL）。
pub fn facts_holds_to(alias: &str) -> String {
    format!(
        "CASE WHEN {alias}.valid_to IS NOT NULL THEN {alias}.valid_to \
              WHEN {alias}.valid_to_precision = 'unknown' THEN {alias}.attested_at END"
    )
}

/// `facts`：断言在 T 时刻成立。`$param` 为 NULL 即不过滤。
pub fn facts_hold_at(alias: &str, param: usize) -> String {
    format!(
        "(${param}::timestamptz IS NULL \
          OR ({from} <= ${param} AND ({to} IS NULL OR {to} > ${param})))",
        from = facts_holds_from(alias),
        to = facts_holds_to(alias),
    )
}

/// 纯粹的区间包含，NULL 一端即开放。派生行与幽灵边（0017 §3，区间在 `detail` 里）
/// 都用它——它们的两端不是原文说的，是引擎按前提算出来的。
pub fn interval_holds_at(from: &str, to: &str, param: usize) -> String {
    format!(
        "(${param}::timestamptz IS NULL \
          OR (({from} IS NULL OR {from} <= ${param}) AND ({to} IS NULL OR {to} > ${param})))"
    )
}

/// `derived_facts`：派生在 T 时刻成立。两端由求值器按前提**读出来的**区间求交写入
/// （0022 第 4 条，第二刀），所以这里是纯粹的包含。第一刀里派生行仍带着前提原样的
/// NULL 起点，那一半随第二刀补上。
pub fn derived_hold_at(alias: &str, param: usize) -> String {
    interval_holds_at(
        &format!("{alias}.valid_from"),
        &format!("{alias}.valid_to"),
        param,
    )
}
