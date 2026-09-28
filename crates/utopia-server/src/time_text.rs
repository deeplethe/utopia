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
pub fn world(t: DateTime<Utc>, precision: Option<&str>) -> String {
    match precision {
        Some("year") => t.format("%Y").to_string(),
        Some("month") => t.format("%Y-%m").to_string(),
        Some("day") => t.format("%Y-%m-%d").to_string(),
        Some("hour") => t.format("%Y-%m-%dT%HZ").to_string(),
        Some("minute") => t.format("%Y-%m-%dT%H:%MZ").to_string(),
        Some("second") => t.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        _ => instant(t),
    }
}

/// 一条事实的两端：原文说的（`valid_*` 与精度）和读出来的（`holds_*`，0022），以及日期
/// 从哪来（#970）：终点是时间线推出来的、区间是人改过的，这两样都不在那一行 `[n]` 打开的
/// 原句里。
#[derive(Debug, Clone, Copy, Default)]
pub struct Span<'a> {
    pub valid_from: Option<DateTime<Utc>>,
    pub from_precision: Option<&'a str>,
    pub valid_to: Option<DateTime<Utc>>,
    pub to_precision: Option<&'a str>,
    pub holds_from: Option<DateTime<Utc>>,
    pub holds_to: Option<DateTime<Utc>>,
    /// 终点是后一条事实开始时时间线关上的（`facts.end_derived`）
    pub end_derived: bool,
    /// 人改过哪一端：`start` / `end` / `both`（`facts.corrected_ends`）；None 是没改过
    pub corrected: Option<&'a str>,
    /// 关上它的那一行（#970 第二步）：接任的那一端，和它证据的号（对话里是 ` [n]`，MCP 里空）
    pub closed_by: Option<(&'a str, &'a str)>,
    /// 人改区间时写下的备注
    pub correction_note: Option<&'a str>,
}

/// 备注在行上最多这么多字：再长就是一段话，不是一个说明
const NOTE_CHARS: usize = 120;

/// 备注折成一行、去掉两端空白，过长截断。空的就是没写
fn note_text(note: &str) -> Option<String> {
    // 方括号换掉：备注是人写的话，里面一个 `[2]` 落在行上就成了一个像引用的号
    let flat = note
        .replace('[', "(")
        .replace(']', ")")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if flat.is_empty() {
        return None;
    }
    if flat.chars().count() > NOTE_CHARS {
        return Some(flat.chars().take(NOTE_CHARS).collect::<String>() + "…");
    }
    Some(flat)
}

/// `from → to`，给模型看。
///
/// 起点：原文给了按精度写；没给但有锚点写 `attested <时刻>`——模型该知道这条事实
/// 只是从那份证据起有据可查。终点：原文给了按精度写；结束了不知哪天写
/// `ended by <锚点>`（没有锚点时写 `ended, date unknown`）；否则 `now`。
/// 两端都无可写时返回空串，调用方据此决定不带括号。
///
/// 日期不是原文说的就跟在后面说出来（#970）：人改过的那一端写 `start corrected` 或
/// `end corrected`，两端都改过写 `corrected`——没改的那一端仍是原文说的；写了备注就带上
/// 备注（`start corrected: …`，人的话不是原文段落，不带号）；时间线推出来的终点写
/// `superseded by X [m]`——`[m]` 打开的是接任那条事实读出来的原句，那里写着这个
/// 日期——找不到接任的那一行时写 `end derived`。这两样只在有终点时说。行尾的 `[n]` 仍是
/// 这条事实自己读出来的原句
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
    let derived = s.end_derived && to.is_some();
    let range = match (from, to) {
        (None, None) => String::new(),
        (Some(f), None) => format!("{f} → now"),
        (None, Some(t)) => format!("→ {t}"),
        (Some(f), Some(t)) => format!("{f} → {t}"),
    };
    let mut marks: Vec<String> = Vec::new();
    if let Some(ends) = s.corrected {
        let what = match ends {
            "start" => "start corrected",
            "end" => "end corrected",
            _ => "corrected",
        };
        marks.push(match s.correction_note.and_then(note_text) {
            Some(note) => format!("{what}: {note}"),
            None => what.to_string(),
        });
    }
    if derived {
        marks.push(match s.closed_by {
            Some((who, mark)) => format!("superseded by {who}{mark}"),
            None => "end derived".to_string(),
        });
    }
    match (range.is_empty(), marks.is_empty()) {
        (_, true) => range,
        (true, false) => marks.join(", "),
        (false, false) => format!("{range}, {}", marks.join(", ")),
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
                ..Span::default()
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
                ..Span::default()
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

    /// 日期不是原文说的，就在区间后面说出来（#970）：时间线关上的终点、人改过的区间。
    /// 没有终点就没有「推出来的终点」；区间为空却改过，只说改过
    #[test]
    fn a_span_says_when_a_date_is_not_the_passages() {
        let day = |s: &str| Some(t(s));
        let closed = Span {
            valid_from: day("2024-07-05T00:00:00Z"),
            from_precision: Some("day"),
            valid_to: day("2025-09-01T00:00:00Z"),
            to_precision: Some("day"),
            ..Span::default()
        };
        assert_eq!(
            span(Span {
                end_derived: true,
                ..closed
            }),
            "2024-07-05 → 2025-09-01, end derived"
        );
        assert_eq!(
            span(Span {
                corrected: Some("both"),
                ..closed
            }),
            "2024-07-05 → 2025-09-01, corrected"
        );
        assert_eq!(
            span(Span {
                corrected: Some("both"),
                end_derived: true,
                ..closed
            }),
            "2024-07-05 → 2025-09-01, corrected, end derived"
        );
        // 时间线关在一个没说起点的后任上：结束了不知哪天，也是推出来的
        assert_eq!(
            span(Span {
                valid_to: None,
                to_precision: Some("unknown"),
                end_derived: true,
                ..closed
            }),
            "2024-07-05 → ended, date unknown, end derived"
        );
        // 开着的行没有终点可推
        assert_eq!(
            span(Span {
                valid_to: None,
                to_precision: None,
                end_derived: true,
                ..closed
            }),
            "2024-07-05 → now"
        );
        assert_eq!(
            span(Span {
                corrected: Some("both"),
                ..Span::default()
            }),
            "corrected"
        );
    }
    /// 找得到关上它的那一行时，推出来的终点说出谁接任、带上那一行的号（#970 第二步）；
    /// MCP 没有号。人改过的区间带着备注：折成一行，过长截断，空的等于没写
    #[test]
    fn a_derived_end_names_what_closed_it_and_a_correction_its_note() {
        let day = |s: &str| Some(t(s));
        let closed = Span {
            valid_from: day("2024-07-05T00:00:00Z"),
            from_precision: Some("day"),
            valid_to: day("2025-09-01T00:00:00Z"),
            to_precision: Some("day"),
            end_derived: true,
            ..Span::default()
        };
        assert_eq!(
            span(Span {
                closed_by: Some(("Zhou Qi", " [3]")),
                ..closed
            }),
            "2024-07-05 → 2025-09-01, superseded by Zhou Qi [3]"
        );
        assert_eq!(
            span(Span {
                closed_by: Some(("Zhou Qi", "")),
                ..closed
            }),
            "2024-07-05 → 2025-09-01, superseded by Zhou Qi"
        );
        // 开着的行没有终点，也就没有谁接任
        assert_eq!(
            span(Span {
                valid_to: None,
                to_precision: None,
                closed_by: Some(("Zhou Qi", " [3]")),
                ..closed
            }),
            "2024-07-05 → now"
        );
        let corrected = Span {
            corrected: Some("both"),
            end_derived: false,
            ..closed
        };
        assert_eq!(
            span(Span {
                correction_note: Some("  The charter date\n was the approval date "),
                ..corrected
            }),
            "2024-07-05 → 2025-09-01, corrected: The charter date was the approval date"
        );
        assert_eq!(
            span(Span {
                correction_note: Some("   "),
                ..corrected
            }),
            "2024-07-05 → 2025-09-01, corrected"
        );
        let long = "x".repeat(200);
        assert_eq!(
            span(Span {
                correction_note: Some(&long),
                ..corrected
            }),
            format!("2024-07-05 → 2025-09-01, corrected: {}…", "x".repeat(120))
        );
        assert_eq!(
            span(Span {
                corrected: Some("both"),
                correction_note: Some("approval date"),
                closed_by: Some(("Zhou Qi", " [3]")),
                ..closed
            }),
            "2024-07-05 → 2025-09-01, corrected: approval date, superseded by Zhou Qi [3]"
        );
    }

    /// 人只改了一端就只说那一端（#976 的评审）：没改的那一端仍是原文说的，终点仍可以是
    /// 时间线推出来的
    #[test]
    fn a_correction_names_the_end_a_person_changed() {
        let day = |s: &str| Some(t(s));
        let row = Span {
            valid_from: day("2023-02-01T00:00:00Z"),
            from_precision: Some("day"),
            valid_to: day("2024-07-05T00:00:00Z"),
            to_precision: Some("day"),
            ..Span::default()
        };
        assert_eq!(
            span(Span {
                corrected: Some("start"),
                correction_note: Some("The charter date was the approval date"),
                ..row
            }),
            "2023-02-01 → 2024-07-05, start corrected: The charter date was the approval date"
        );
        assert_eq!(
            span(Span {
                corrected: Some("end"),
                ..row
            }),
            "2023-02-01 → 2024-07-05, end corrected"
        );
        assert_eq!(
            span(Span {
                corrected: Some("start"),
                end_derived: true,
                closed_by: Some(("Li Si", " [2]")),
                ..row
            }),
            "2023-02-01 → 2024-07-05, start corrected, superseded by Li Si [2]"
        );
    }
}
