//! 时间给模型看的写法：**一条规则，所有工具行都走它**。
//!
//! - 世界轴的端点按**自己的精度**写：year → `2023`，month → `2023-06`，day → `2023-06-01`。
//!   从前一律 `%Y-%m-%d`——年精度的事实印成 1 月 1 日，正是 `facts.valid_from_precision`
//!   那条注释说的病：在无知的地方填一个确定的值。多少精度写多少位。
//! - 没有精度的时刻——`at`、`as_of`、`recorded_at`、`doc_time`，以及锚点顶上来的界
//!   （派生行的两端、没起点的事实读出来的起点，0022）——写完整 RFC3339，含小数秒。
//!   记录轴上同一秒内可以先录入再更正（#351），截到天或秒都会把两次认知叠回一起。
//! - 「结束了，不知哪天」写成 `ended by <锚点>`，绝不写 `now`。
//!
//! 界面的 `web/src/time.ts::fmtTime` 与导出的 `rdf::world_time` 是同一条规则的另两份；
//! 三处分叉的话，人看到的、模型看到的、审计者拿到的就不是同一个日期。
use chrono::{DateTime, SecondsFormat, Utc};

/// 一个时刻，完整写出。
pub fn instant(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// 世界轴的一端，按精度写。小时以下用 ISO 8601 的缩略形式（`2026-06-01T14:32Z`），
/// 同一个字符串能解析回同一个值和精度。没有精度就是一个时刻（锚点、派生的界），写完整。
///
/// `"instant"` 是「源端给的精度是完整时刻」——输出和没有精度一样（RFC 3339 整
/// 时刻），但语义和 `"day"` 不同：前者告诉抽取器「这是 `valid_from_precision`
/// 等于 `second` 的事实」，后者告诉抽取器「这是精度等于 `day` 的事实」。两边
/// 的输出字符串是同一个，由 `utopia_extract::parse_time` 区分（#610 提案）
pub fn world(t: DateTime<Utc>, precision: Option<&str>) -> String {
    match precision {
        Some("year") => t.format("%Y").to_string(),
        Some("month") => t.format("%Y-%m").to_string(),
        Some("day") => t.format("%Y-%m-%d").to_string(),
        Some("hour") => t.format("%Y-%m-%dT%HZ").to_string(),
        Some("minute") => t.format("%Y-%m-%dT%H:%MZ").to_string(),
        Some("second") => t.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        // 来源端给的「真实时刻」：写完整，但不假装是哪一天。
        // 与没有精度走同一条 instant 路径，但语义要在调用方约定清楚
        Some("instant") | None => instant(t),
        // 未知精度（写错的字、整型、空串被 serde 解出 `None`）按「无精度」
        // 处理——手滑不该让一行 fact 静默跨午夜。同 `Source::extracts` 的
        // typo 政策（models.rs:156）
        Some(_) => instant(t),
    }
}

/// 一条事实的两端：原文说的（`valid_*` 与精度）和读出来的（`holds_*`，0022）。
#[derive(Debug, Clone, Copy, Default)]
pub struct Span<'a> {
    pub valid_from: Option<DateTime<Utc>>,
    pub from_precision: Option<&'a str>,
    pub valid_to: Option<DateTime<Utc>>,
    pub to_precision: Option<&'a str>,
    pub holds_from: Option<DateTime<Utc>>,
    pub holds_to: Option<DateTime<Utc>>,
}

/// `from → to`，给模型看。
///
/// 起点：原文给了按精度写；没给但有锚点写 `attested <时刻>`——模型该知道这条事实
/// 只是从那份证据起有据可查。终点：原文给了按精度写；结束了不知哪天写
/// `ended by <锚点>`（没有锚点时写 `ended, date unknown`）；否则 `now`。
/// 两端都无可写时返回空串，调用方据此决定不带括号。
pub fn span(s: Span<'_>) -> String {
    let from = match (s.valid_from, s.holds_from) {
        (Some(t), _) => Some(world(t, s.from_precision)),
        // 没有说出来的起点就说没有。从前写 `attested <文档日期>`，模型把那个日期当成
        // 事情发生的日子念出来（"OpenAI 于 2026 年 9 月 6 日被确认为…"）；锚点是过滤用的，
        // 不是给人读的
        (None, Some(_)) => Some("undated".to_string()),
        (None, None) => None,
    };
    let ended_unknown =
        s.valid_to.is_none() && s.to_precision == Some(utopia_store::graph::ENDED_UNKNOWN);
    let to = match (s.valid_to, ended_unknown, s.holds_to) {
        (Some(t), _, _) => Some(world(t, s.to_precision)),
        // 「结束了，不知哪天」照实说；说出它的那份文档的日期同样不给
        (None, true, Some(_)) => Some("ended, date unknown".to_string()),
        (None, true, None) => Some("ended, date unknown".to_string()),
        (None, false, _) => None,
    };
    match (from, to) {
        (None, None) => String::new(),
        (Some(f), None) => format!("{f} → now"),
        (None, Some(t)) => format!("→ {t}"),
        (Some(f), Some(t)) => format!("{f} → {t}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn a_world_bound_shows_exactly_its_precision() {
        let at = t("2023-06-15T00:00:00Z");
        assert_eq!(world(at, Some("year")), "2023");
        assert_eq!(world(at, Some("month")), "2023-06");
        assert_eq!(world(at, Some("day")), "2023-06-15");
        let clock = t("2026-06-01T14:32:07.382Z");
        assert_eq!(world(clock, Some("hour")), "2026-06-01T14Z");
        assert_eq!(world(clock, Some("minute")), "2026-06-01T14:32Z");
        assert_eq!(world(clock, Some("second")), "2026-06-01T14:32:07Z");
        // 「真实时刻」与无精度走同一条 RFC 3339 路径——给抽取器的语义不同，
        // 但写出来的字符串一样；下游靠精度字段区分
        assert_eq!(world(clock, Some("instant")), "2026-06-01T14:32:07.382Z");
        // 写出来的能读回去，且是同一个精度
        for (p, s) in [
            ("hour", "2026-06-01T14Z"),
            ("minute", "2026-06-01T14:32Z"),
            ("second", "2026-06-01T14:32:07Z"),
        ] {
            let (back, bp) = utopia_extract::parse_time(s).unwrap();
            assert_eq!((bp, world(back, Some(p))), (p, s.to_string()));
        }
        // 没有精度 = 一个时刻（锚点、派生的界）：写完整，不冒充哪一天
        assert_eq!(world(at, None), "2023-06-15T00:00:00Z");
    }

    #[test]
    fn a_late_night_event_does_not_cross_midnight_when_written_with_instant_precision() {
        // 23:59:59Z UTC = 第二天 07:59:59 Asia/Shanghai。day 精度会把这件事
        // 移到 9 月 6 日，而 instant 精度保持 9 月 5 日。这一对断言就是
        // 提案 #610 的「不静默跨午夜」承诺
        let late = t("2026-09-05T23:59:59Z");
        assert_eq!(world(late, Some("day")), "2026-09-05");
        assert_eq!(world(late, Some("instant")), "2026-09-05T23:59:59Z");
        // 关键的反向断言：day 写出来的字符串里没有 T23:59:59；instant 写
        // 出来的字符串里没有 9 月 6 日
        assert!(!world(late, Some("day")).contains("T23:59:59"));
        assert!(!world(late, Some("instant")).contains("2026-09-06"));
    }

    #[test]
    fn an_instant_keeps_its_fraction_and_reads_back() {
        for s in [
            "2026-09-05T02:43:53Z",
            "2026-09-05T02:43:53.382Z",
            "2026-09-05T02:43:53.382001Z",
        ] {
            assert_eq!(instant(t(s)), s);
        }
    }

    #[test]
    fn a_span_reads_each_end_by_its_own_rule() {
        let day = |s: &str| Some(t(s));
        // 原文给了两端
        assert_eq!(
            span(Span {
                valid_from: day("2023-01-01T00:00:00Z"),
                from_precision: Some("year"),
                valid_to: day("2024-07-01T00:00:00Z"),
                to_precision: Some("month"),
                holds_from: day("2023-01-01T00:00:00Z"),
                holds_to: day("2024-07-01T00:00:00Z"),
            }),
            "2023 → 2024-07"
        );
        // 没起点：从证据起；仍在继续
        assert_eq!(
            span(Span {
                holds_from: day("2024-02-20T00:00:00Z"),
                ..Span::default()
            }),
            "undated → now"
        );
        // 结束了不知哪天：到说出它的那份文档为止，绝不是 now
        assert_eq!(
            span(Span {
                valid_from: day("2023-06-01T00:00:00Z"),
                from_precision: Some("day"),
                valid_to: None,
                to_precision: Some("unknown"),
                holds_from: day("2023-06-01T00:00:00Z"),
                holds_to: day("2025-10-15T00:00:00Z"),
            }),
            "2023-06-01 → ended, date unknown"
        );
        // 记录轴事件里没有锚点可用
        assert_eq!(
            span(Span {
                to_precision: Some("unknown"),
                ..Span::default()
            }),
            "→ ended, date unknown"
        );
        assert_eq!(span(Span::default()), "");
    }
}
