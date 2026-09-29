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
//! 未知的一端**读到证据为止**，两端各有各的锚点（#393）：`attested_from` 是这一行的
//! 各次观察里最早那份文档的日期——没有起点 → 从它起成立；`attested_to` 是**说出结束**
//! 的那份文档的日期，只在「结束了不知哪天」的行上有——到它为止。一个锚点装不下两头：
//! 没起点的裸行被一句「不再担任」关上时，起点要用第一份证据、终点要用说结束的那份。
//! 两端的不对称仍是故意的：开放的结束端读作「直到有人说它结束」——结束会以记录的形式
//! 到来，把行关上；缺失的起点没有这样的修正者，不会有谁来说「2023 年它还没开始」。
//!
//! `at` 为 NULL 在世界轴上是**每一刻**（画布画的是历史，滑杆负责收窄），与记录轴
//! 的「NULL 即现在」不同：没有人持有一个晚于此刻的信念，而没有时刻的图是全部
//! 时间的图。

/// 谓词的时间语义（0031）。一行自己说不出它是状态、事件还是恒常——它的谓词说；
/// 没有谓词（0010）读作状态，与写入侧 `predicate_temporal` 同一判断
fn temporal_of(alias: &str) -> String {
    format!("(SELECT r.temporal FROM relation_types r WHERE r.id = {alias}.predicate_id)")
}

/// `facts`：读出来的下界——原文给了起点用起点，否则从最早的证据起。
/// 恒常没有下界（NULL 即开放）：证据日期闸的是「从何时起知道」，恒常的东西不从何时起。
///
/// 既没有起点也没有见证的行（0064 决定 5）：文档没说它什么时候成立，也没说自己是哪天的，
/// 下界开放——任何时点都成立。没日期的事件除外，见 [`facts_hold_at`]
pub fn facts_holds_from(alias: &str) -> String {
    format!(
        "CASE WHEN {temporal} = 'eternal' THEN NULL \
              ELSE COALESCE({alias}.valid_from, {alias}.attested_from) END",
        temporal = temporal_of(alias),
    )
}

/// `facts`：读出来的上界——原文给了终点用终点；说结束了但不知哪天，到最早说出它
/// 的那份文档为止；否则开放（NULL）。
///
/// 事件（0031）在它命名的那个桶里成立：上界是那一刻加一个精度单位——`2024-03-15`
/// 到 `2024-03-16` 为止，`2024-03` 到四月为止。没日期的事件上界与下界同为锚点，
/// 区间为空：它发生过，但不知何时，任何时刻都不算成立（0022 对未知的收法）。
/// 0031 之前写下的事件行终点是空的，按起点那个桶读——不必回填。恒常没有上界
pub fn facts_holds_to(alias: &str) -> String {
    format!(
        "CASE WHEN {temporal} = 'eternal' THEN NULL \
              WHEN {temporal} = 'event' THEN \
                   CASE WHEN {alias}.valid_from IS NULL THEN {alias}.attested_from \
                        ELSE COALESCE({alias}.valid_to, {alias}.valid_from) \
                             + ('1 ' || COALESCE(NULLIF({alias}.valid_to_precision, 'unknown'), \
                                                 {alias}.valid_from_precision))::interval END \
              WHEN {alias}.valid_to IS NOT NULL THEN {alias}.valid_to \
              WHEN {alias}.valid_to_precision = 'unknown' THEN {alias}.attested_to END",
        temporal = temporal_of(alias),
    )
}

/// `facts`：断言在 T 时刻成立。`$param` 为 NULL 即不过滤。
/// 两端都可能开放（恒常），所以是纯粹的区间包含
///
/// 没日期的事件发生过但不知何时，任何时点都不算成立（0022）。有见证时它的两端同为见证，
/// 区间是空的；没有见证时（0064 决定 5）两端都是 NULL，读成区间就成了两端开放，所以单独
/// 挡掉——不拿 `infinity` 当端点，那两个值还要投影给读的人。没有谓词的行（开放陈述）
/// 的时间语义是 NULL，读作状态：比较用 `IS NOT DISTINCT FROM`，否则整个谓词是 NULL，
/// 放在 WHERE 里就把每一条开放陈述都滤掉了
pub fn facts_hold_at(alias: &str, param: usize) -> String {
    format!(
        "(${param}::timestamptz IS NULL \
          OR (NOT ({temporal} IS NOT DISTINCT FROM 'event' AND {alias}.valid_from IS NULL \
                   AND {alias}.attested_from IS NULL) \
              AND {holds}))",
        temporal = temporal_of(alias),
        holds = interval_holds_at(&facts_holds_from(alias), &facts_holds_to(alias), param),
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
/// （0022 第 4 条：没起点的前提从锚点起算，结束了不知哪天的到锚点为止），所以这里
/// 是纯粹的包含——派生行自己不需要锚点。
pub fn derived_hold_at(alias: &str, param: usize) -> String {
    interval_holds_at(
        &format!("{alias}.valid_from"),
        &format!("{alias}.valid_to"),
        param,
    )
}
