//! 时间提及按文档解析（0045 决定 2–5）：模型读，代码算。
//!
//! 抽取只记时间**词**（`time_mentions`：原样的字、块、偏移、起/止）。这里是它后面的任务：
//!
//! 1. **文档时间上下文**（决定 3）。一篇文档只做一次：把开头交给模型，它转写文档自己的日期
//!    （落款、备案日、公报的报告年）和文档命名的期间（「fiscal 2027」及其起止）；日期词要在
//!    开头里核对得到才算数。结果存在 `documents.time_context`；文档自己的日期写进 `doc_time`
//!    （来源 `content`）。上传时间永远不进来（#714）。
//! 2. **解释**（决定 2）。把文档的每个时间词连同它所在的那句话，和上下文一起交给模型；模型
//!    只回形状、引用（写明的数字部分 / 锚点加偏移 / 命名的期间 / 没有）和粒度，**从不算日期**。
//!    解释原样存回提及上，错了看得见、改得了。
//! 3. **算**。日历算术、期间到区间、按粒度截断、形状蕴含的区间，都在这里用结构化字段算，
//!    没有一个时间词写在代码里（决定 8）。写明的日期是 A 级，从文档自己给的锚点算出来的是
//!    B 级，锚不到的是 C 级——C 级什么也不写，等锚点（决定 4、6）。
//! 4. 结果落到开放陈述的 `valid_from` / `valid_to`（决定 5 的第一根轴），时间轴上才有它。

use crate::extraction::{chat_retrying_rate_limits_at, span_in_quote};
use crate::llm_util;
use crate::state::AppState;
use chrono::{DateTime, Duration, Months, NaiveDate, NaiveTime, TimeZone, Utc};
use std::collections::HashMap;
use utopia_extract::time::{
    build_dating_messages, build_interpretation_messages, parse_dating_response,
    parse_interpretation_response, Anchor, DateParts, Direction, DocumentDating, Granularity,
    Interpretation, MentionInput, Offset, Reference, Shape, TimeContext, Unit,
};
use utopia_store::graph::{truncate_to, ENDED_UNKNOWN, WORLD_PRECISIONS};
use uuid::Uuid;

/// 一批解释问多少个提及。
const BATCH: usize = 40;
/// 文档开头给模型看多少字：落款、报告年、财年定义都在前面
const OPENING_CHARS: usize = 3000;

/// 一次解析的结果：世界轴上的位置与等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Resolved {
    from: Option<DateTime<Utc>>,
    from_p: Option<&'static str>,
    to: Option<DateTime<Utc>>,
    to_p: Option<&'static str>,
    /// A：写明的；B：从文档自己的锚点算出来的；C：锚不到
    grade: &'static str,
}

const UNRESOLVED: Resolved = Resolved {
    from: None,
    from_p: None,
    to: None,
    to_p: None,
    grade: "C",
};

/// 数字部分 → 时刻与精度。精度是字写到的那一档：只有年就是年。
fn parts_to_time(p: &DateParts) -> Option<(DateTime<Utc>, &'static str)> {
    let date = NaiveDate::from_ymd_opt(p.year, p.month.unwrap_or(1), p.day.unwrap_or(1))?;
    let time = NaiveTime::from_hms_opt(
        p.hour.unwrap_or(0),
        p.minute.unwrap_or(0),
        p.second.unwrap_or(0),
    )?;
    let precision = if p.month.is_none() {
        "year"
    } else if p.day.is_none() {
        "month"
    } else if p.hour.is_none() {
        "day"
    } else if p.minute.is_none() {
        "hour"
    } else if p.second.is_none() {
        "minute"
    } else {
        "second"
    };
    Some((Utc.from_utc_datetime(&date.and_time(time)), precision))
}

fn granularity_str(g: Granularity) -> &'static str {
    match g {
        Granularity::Year => "year",
        Granularity::Month => "month",
        Granularity::Day => "day",
        Granularity::Hour => "hour",
        Granularity::Minute => "minute",
        Granularity::Second => "second",
    }
}

/// 两档精度取粗的那一档：字只写到年，模型说是「天」也只能是年——精度不能比字更细
fn coarser(a: &'static str, b: &'static str) -> &'static str {
    let rank = |p: &str| WORLD_PRECISIONS.iter().position(|x| *x == p).unwrap_or(0);
    if rank(a) <= rank(b) {
        a
    } else {
        b
    }
}

/// 锚点加偏移（决定 2 的算术）。月、季、年按月份加减（月末自动收口）；周和天以下按时长。
fn shift(t: DateTime<Utc>, offset: &Offset) -> Option<DateTime<Utc>> {
    let count = offset.count.unsigned_abs();
    let forward = (offset.direction == Direction::After) == (offset.count >= 0);
    let months = |n: u64| -> Option<DateTime<Utc>> {
        let m = Months::new(u32::try_from(n).ok()?);
        if forward {
            t.checked_add_months(m)
        } else {
            t.checked_sub_months(m)
        }
    };
    let span = |d: Duration| -> Option<DateTime<Utc>> {
        if forward {
            t.checked_add_signed(d)
        } else {
            t.checked_sub_signed(d)
        }
    };
    match offset.unit {
        Unit::Year => months(count.checked_mul(12)?),
        Unit::Quarter => months(count.checked_mul(3)?),
        Unit::Month => months(count),
        Unit::Week => span(Duration::weeks(i64::try_from(count).ok()?)),
        Unit::Day => span(Duration::days(i64::try_from(count).ok()?)),
        Unit::Hour => span(Duration::hours(i64::try_from(count).ok()?)),
        Unit::Minute => span(Duration::minutes(i64::try_from(count).ok()?)),
        Unit::Second => span(Duration::seconds(i64::try_from(count).ok()?)),
    }
}

/// 一个时刻和它的精度（世界轴的一端）。
type Bound = (DateTime<Utc>, &'static str);

/// 一个季度：包含 `t` 的那三个月（有财年结束日就按财年的季，否则按日历季），
/// 起是季首月，止是季末月（终点精度到月）。「本季度」「上季度」都从这里来
fn quarter_around(t: DateTime<Utc>, fiscal_year_end: Option<(u32, u32)>) -> Option<(Bound, Bound)> {
    use chrono::Datelike;
    // 财年从结束日的下一个月算起：结束于 1 月 25 日的财年，季首月是 2、5、8、11 月
    let first_month = fiscal_year_end.map(|(m, _)| m % 12 + 1).unwrap_or(1);
    let month0 = (t.month() as i32 - first_month as i32).rem_euclid(12); // 财年内第几个月（0 起）
    let start_month0 = month0 - month0 % 3;
    let start = t.checked_sub_months(Months::new(u32::try_from(month0 - start_month0).ok()?))?;
    let start = truncate_to(start, Some("month"));
    let end = start.checked_add_months(Months::new(2))?;
    Some(((start, "month"), (end, "month")))
}

/// 一个期间的起止（写明的数字部分）。
fn period_bounds(from: &DateParts, to: &DateParts) -> Option<(Bound, Bound)> {
    Some((parts_to_time(from)?, parts_to_time(to)?))
}

/// 形状决定字在世界轴上占哪一段（决定 7：形状另存，这里只算区间）。
/// 点：一刻，两端同值（0031 事件的写法）；自：起点；至：终点；区间：两端；
/// 截至：观察到的起点；时长：没有位置；结束但不知何时：终点精度 unknown
fn place(
    shape: Shape,
    at: Option<(DateTime<Utc>, &'static str)>,
    until: Option<(DateTime<Utc>, &'static str)>,
    grade: &'static str,
) -> Resolved {
    let trunc = |(t, p): (DateTime<Utc>, &'static str)| (truncate_to(t, Some(p)), p);
    let at = at.map(trunc);
    let until = until.map(trunc);
    let mut r = Resolved {
        grade,
        ..UNRESOLVED
    };
    match shape {
        Shape::Point => {
            if let Some((t, p)) = at {
                r.from = Some(t);
                r.from_p = Some(p);
                r.to = Some(t);
                r.to_p = Some(p);
            }
        }
        Shape::Since | Shape::AsOf => {
            if let Some((t, p)) = at {
                r.from = Some(t);
                r.from_p = Some(p);
            }
        }
        Shape::Until => {
            if let Some((t, p)) = until.or(at) {
                r.to = Some(t);
                r.to_p = Some(p);
            }
        }
        Shape::Interval => {
            if let Some((t, p)) = at {
                r.from = Some(t);
                r.from_p = Some(p);
            }
            if let Some((t, p)) = until {
                r.to = Some(t);
                r.to_p = Some(p);
            }
        }
        Shape::Duration => return UNRESOLVED,
        Shape::EndedUnknown => {
            r.to_p = Some(ENDED_UNKNOWN);
        }
    }
    if r.from.is_none() && r.to.is_none() && r.to_p.is_none() {
        return UNRESOLVED;
    }
    r
}

/// 一个锚点在世界轴上的位置：文档日期、另一个提及、命名的期间。
fn anchor_time(
    anchor: &Anchor,
    ctx: &DocumentDating,
    earlier: &HashMap<i64, Resolved>,
) -> Option<(Bound, Option<Bound>)> {
    match anchor {
        Anchor::Document => ctx.date.as_ref().and_then(parts_to_time).map(|t| (t, None)),
        Anchor::Mention { id } => {
            let r = earlier.get(id)?;
            let from = r.from.zip(r.from_p)?;
            Some((from, r.to.zip(r.to_p)))
        }
        Anchor::Period { name } => {
            let p = ctx
                .periods
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(name))?;
            let (from, to) = period_bounds(&p.from, &p.to)?;
            Some((from, Some(to)))
        }
    }
}

/// 一条解释 → 世界轴位置。`earlier` 是同一批里已经算出来的提及（锚点指着它们）。
fn resolve_one(
    interp: &Interpretation,
    ctx: &DocumentDating,
    earlier: &HashMap<i64, Resolved>,
) -> Resolved {
    let g = granularity_str(interp.granularity);
    // 「结束了，不知哪天」是字明说的（#393 的那一档）：没有日期可算，但结束本身是 A 级
    if interp.shape == Shape::EndedUnknown {
        return place(Shape::EndedUnknown, None, None, "A");
    }
    match &interp.reference {
        Reference::Absolute { from, to } => {
            let Some((t, p)) = parts_to_time(from) else {
                return UNRESOLVED;
            };
            let until = to.as_ref().and_then(parts_to_time);
            place(interp.shape, Some((t, coarser(p, g))), until, "A")
        }
        Reference::Anchored { anchor, offset } => {
            let Some(((t, p), until)) = anchor_time(anchor, ctx, earlier) else {
                return UNRESOLVED;
            };
            // 对着一个期间说「截至」「至」（「年末」对着报告年）：说的是期间的末，不是首。
            // 这是形状的语义，不是认词
            if offset.is_none()
                && matches!(anchor, Anchor::Period { .. })
                && matches!(interp.shape, Shape::AsOf | Shape::Until)
            {
                if let Some(end) = until {
                    return place(interp.shape, Some(end), None, "B");
                }
            }
            let (t, until) = match offset {
                Some(o) => {
                    let Some(moved) = shift(t, o) else {
                        return UNRESOLVED;
                    };
                    // 以「年」「月」为单位的偏移（本年、去年、上个月）说的是那一整段：
                    // 起止同一个桶，精度就是那个单位
                    if matches!(o.unit, Unit::Year | Unit::Month)
                        && matches!(interp.shape, Shape::Point | Shape::Interval)
                    {
                        let unit_p = if o.unit == Unit::Year {
                            "year"
                        } else {
                            "month"
                        };
                        let start = truncate_to(moved, Some(unit_p));
                        return place(Shape::Point, Some((start, unit_p)), None, "B");
                    }
                    // 以「季」为单位的偏移（本季度、上季度）说的是那一整个季度，不是某一天
                    if o.unit == Unit::Quarter {
                        let Some((from, to)) = quarter_around(moved, ctx.fiscal_year_end) else {
                            return UNRESOLVED;
                        };
                        let shape = match interp.shape {
                            Shape::Point | Shape::Since | Shape::AsOf | Shape::Duration => {
                                Shape::Interval
                            }
                            s => s,
                        };
                        return place(shape, Some(from), Some(to), "B");
                    }
                    (moved, until.and_then(|(u, _)| shift(u, o).map(|m| (m, g))))
                }
                None => (t, until),
            };
            // 锚点算出来的时刻，精度由字说的粒度定，但不能比锚点自己更细
            place(interp.shape, Some((t, coarser(p, g))), until, "B")
        }
        Reference::Period { name, from, to } => {
            // 只写了一端的期间（「三个月截至 6 月 30 日」）：写明的那一端照写，另一端不猜
            match (from, to) {
                (None, Some(t)) => {
                    return place(Shape::Until, None, parts_to_time(t), "A");
                }
                (Some(f), None) => {
                    return place(Shape::Since, parts_to_time(f), None, "A");
                }
                _ => {}
            }
            let (bounds, grade) = match (from, to) {
                (Some(f), Some(t)) => (period_bounds(f, t), "A"),
                _ => (
                    ctx.periods
                        .iter()
                        .find(|p| p.name.eq_ignore_ascii_case(name))
                        .and_then(|p| period_bounds(&p.from, &p.to)),
                    "B",
                ),
            };
            let Some((from, to)) = bounds else {
                return UNRESOLVED;
            };
            // 一个期间本身是区间；「截至期末」这类形状照形状算
            let shape = match interp.shape {
                Shape::Point | Shape::Since | Shape::Duration => Shape::Interval,
                s => s,
            };
            place(shape, Some(from), Some(to), grade)
        }
        Reference::None => UNRESOLVED,
    }
}

/// 一条陈述的起与止：`when` 给起（点与区间也给止），`ended` 给止。
fn combine(when: Option<Resolved>, ended: Option<Resolved>) -> Resolved {
    let mut r = when.unwrap_or(UNRESOLVED);
    if let Some(e) = ended {
        let to = e.to.or(e.from);
        let to_p = e.to_p.or(e.from_p);
        if to.is_some() || to_p.is_some() {
            r.to = to;
            r.to_p = to_p;
        }
        if r.grade == "C" {
            r.grade = e.grade;
        }
    }
    r
}

/// 解析一篇文档的全部时间提及（任务 `resolve_time`）。幂等：再跑一次覆盖上一次的结果，
/// 所以锚点晚到（改了文档日期、后一块给了期间）时重排一次即可（决定 4 的第一步）
pub async fn resolve_document(state: &AppState, document_id: Uuid) -> anyhow::Result<()> {
    let pool = &state.pool;
    let doc = utopia_store::documents::get(pool, document_id).await?;
    if doc.deleted_at.is_some() {
        return Ok(());
    }
    let kb = utopia_store::kbs::get(pool, doc.kb_id).await?;
    let settings = utopia_store::settings::get(pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot resolve time"))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot resolve time"))?;

    // 1. 文档时间上下文：一篇只问一次
    let context: DocumentDating = match doc.time_context.clone() {
        Some(json) => serde_json::from_value(json)?,
        None => {
            let opening = utopia_store::documents::opening_chunk(pool, document_id)
                .await?
                .map(|(_, text)| text)
                .unwrap_or_default();
            let opening: String = opening.chars().take(OPENING_CHARS).collect();
            let messages = build_dating_messages(&doc.filename, &opening);
            let reply =
                chat_retrying_rate_limits_at(state, &settings, &client, &messages, Some(0.0))
                    .await?;
            let mut dating = parse_dating_response(&reply.text)?;
            // 日期词要在开头里核对得到（与名字、引文同一条规矩）；核不到的日期不算
            match dating.date_words.as_deref() {
                Some(words) if span_in_quote(words, &opening) => {}
                _ => dating.date = None,
            }
            utopia_store::documents::set_time_context(
                pool,
                document_id,
                &serde_json::to_value(&dating)?,
            )
            .await?;
            // 文档自己的日期（决定 3）：只在它还没有内容或来源给的日期时写
            if crate::extraction_open::dated_at(&doc).is_none() {
                if let Some((t, _)) = dating.date.as_ref().and_then(parts_to_time) {
                    utopia_store::documents::set_content_date(pool, document_id, t).await?;
                }
            }
            dating
        }
    };

    // 2. 提及：同一篇里同样的字在同样的句子里只解释一次
    let mentions = utopia_store::time_mentions::for_document(pool, document_id).await?;
    if mentions.is_empty() {
        return Ok(());
    }
    let mut distinct: Vec<(String, String)> = Vec::new();
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    for m in &mentions {
        let key = (m.text.clone(), m.sentence.clone());
        if !index.contains_key(&key) {
            index.insert(key.clone(), distinct.len());
            distinct.push(key);
        }
    }
    let ctx = TimeContext {
        date: context.date.as_ref(),
        date_words: context.date_words.as_deref(),
        periods: &context.periods,
        fiscal_year_end: context.fiscal_year_end,
    };
    let mut resolved: HashMap<usize, (Interpretation, Resolved)> = HashMap::new();
    let mut malformed = 0usize;
    for (batch_no, batch) in distinct.chunks(BATCH).enumerate() {
        let base = (batch_no * BATCH) as i64;
        let inputs: Vec<MentionInput<'_>> = batch
            .iter()
            .enumerate()
            .map(|(i, (text, sentence))| MentionInput {
                id: base + i as i64,
                text,
                sentence,
            })
            .collect();
        let ids: Vec<i64> = inputs.iter().map(|m| m.id).collect();
        let messages = build_interpretation_messages(&ctx, &inputs);
        let reply =
            match chat_retrying_rate_limits_at(state, &settings, &client, &messages, Some(0.0))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(%document_id, error = %e, "时间解释调用失败，这一批留作未解析");
                    continue;
                }
            };
        let (interps, skipped) = match parse_interpretation_response(&reply.text, &ids) {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(%document_id, error = %e, "时间解释回复解析失败，这一批留作未解析");
                continue;
            }
        };
        malformed += skipped;
        // 两遍：先算不靠别的提及的，再算指着别的提及的
        let mut earlier: HashMap<i64, Resolved> = HashMap::new();
        let anchored_to_mention = |i: &Interpretation| {
            matches!(
                &i.reference,
                Reference::Anchored {
                    anchor: Anchor::Mention { .. },
                    ..
                }
            )
        };
        for pass in 0..2 {
            for interp in interps
                .iter()
                .filter(|i| anchored_to_mention(i) == (pass == 1))
            {
                let r = resolve_one(interp, &context, &earlier);
                earlier.insert(interp.id, r);
                if let Ok(i) = usize::try_from(interp.id) {
                    resolved.insert(i, (interp.clone(), r));
                }
            }
        }
    }

    // 3. 写回提及，再按陈述合起止
    let mut per_fact: HashMap<Uuid, (Option<Resolved>, Option<Resolved>)> = HashMap::new();
    let (mut a, mut b, mut c) = (0usize, 0usize, 0usize);
    for m in &mentions {
        let Some(&i) = index.get(&(m.text.clone(), m.sentence.clone())) else {
            continue;
        };
        let Some((interp, r)) = resolved.get(&i) else {
            // 这一批没答到：留着字，不写等级（下次重跑再问）
            continue;
        };
        utopia_store::time_mentions::set_interpretation(
            pool,
            m.id,
            serde_json::to_value(interp.shape)?
                .as_str()
                .unwrap_or("point"),
            &serde_json::to_value(&interp.reference)?,
            granularity_str(interp.granularity),
        )
        .await?;
        utopia_store::time_mentions::set_resolution(
            pool, m.id, r.grade, r.from, r.from_p, r.to, r.to_p,
        )
        .await?;
        match r.grade {
            "A" => a += 1,
            "B" => b += 1,
            _ => c += 1,
        }
        let slot = per_fact.entry(m.fact_id).or_insert((None, None));
        if m.role == "ended" {
            slot.1 = Some(*r);
        } else {
            slot.0 = Some(*r);
        }
    }
    let mut dated = 0usize;
    for (fact_id, (when, ended)) in per_fact {
        let r = combine(when, ended);
        // 起点的等级只看起点那条提及。combine 在起点锚不到时借结束端的等级来报数，
        // 这里不借：引擎问的是「这一行的起点是怎么来的」（0045 第 3 刀）
        let grade = when.map(|w| w.grade);
        let placed = r.from.is_some() || r.to.is_some() || r.to_p.is_some();
        // 什么都没算出来也要写等级 C：只有这一列说得出「这句话有时间词、我们没能把它
        // 放到轴上」，否则它和「整句没有时间词」在库里长得一模一样
        if !placed && grade != Some("C") {
            continue;
        }
        utopia_store::graph::set_open_validity(
            pool, fact_id, r.from, r.from_p, r.to, r.to_p, grade,
        )
        .await?;
        if placed {
            dated += 1;
        }
    }
    tracing::info!(
        %document_id,
        mentions = mentions.len(),
        grade_a = a,
        grade_b = b,
        grade_c = c,
        malformed,
        statements_dated = dated,
        "时间提及解析完成"
    );
    if dated > 0 {
        state.emit_graph(doc.kb_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use utopia_extract::time::NamedPeriod;

    fn parts(y: i32, m: Option<u32>, d: Option<u32>) -> DateParts {
        DateParts {
            year: y,
            month: m,
            day: d,
            hour: None,
            minute: None,
            second: None,
        }
    }
    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }
    fn ctx_dated(y: i32, m: u32, d: u32) -> DocumentDating {
        DocumentDating {
            date: Some(parts(y, Some(m), Some(d))),
            date_words: Some("x".into()),
            periods: vec![NamedPeriod {
                name: "fiscal 2027".into(),
                from: parts(2026, Some(1), Some(26)),
                to: parts(2027, Some(1), Some(25)),
            }],
            fiscal_year_end: Some((1, 25)),
            skipped: 0,
        }
    }

    #[test]
    fn parts_give_the_precision_the_words_state() {
        assert_eq!(parts_to_time(&parts(2024, None, None)).unwrap().1, "year");
        assert_eq!(
            parts_to_time(&parts(2024, Some(3), None)).unwrap().1,
            "month"
        );
        let (t, p) = parts_to_time(&parts(2011, Some(3), Some(4))).unwrap();
        assert_eq!((t, p), (at("2011-03-04T00:00:00Z"), "day"));
        assert!(parts_to_time(&parts(2024, Some(13), None)).is_none());
    }

    #[test]
    fn an_absolute_date_is_grade_a_and_a_point_has_two_equal_ends() {
        let i = Interpretation {
            id: 0,
            shape: Shape::Point,
            reference: Reference::Absolute {
                from: parts(2011, Some(3), Some(4)),
                to: None,
            },
            granularity: Granularity::Day,
        };
        let r = resolve_one(&i, &ctx_dated(2026, 9, 2), &HashMap::new());
        assert_eq!(r.grade, "A");
        assert_eq!(r.from, Some(at("2011-03-04T00:00:00Z")));
        assert_eq!(r.to, r.from);
        assert_eq!((r.from_p, r.to_p), (Some("day"), Some("day")));
    }

    #[test]
    fn a_bare_year_stays_a_year_even_if_the_model_says_day() {
        let i = Interpretation {
            id: 0,
            shape: Shape::Since,
            reference: Reference::Absolute {
                from: parts(1987, None, None),
                to: None,
            },
            granularity: Granularity::Day,
        };
        let r = resolve_one(&i, &ctx_dated(2026, 9, 2), &HashMap::new());
        assert_eq!(
            (r.from, r.from_p, r.to),
            (Some(at("1987-01-01T00:00:00Z")), Some("year"), None)
        );
    }

    #[test]
    fn today_and_last_year_anchor_to_the_document_as_grade_b() {
        let today = Interpretation {
            id: 0,
            shape: Shape::Point,
            reference: Reference::Anchored {
                anchor: Anchor::Document,
                offset: None,
            },
            granularity: Granularity::Day,
        };
        let r = resolve_one(&today, &ctx_dated(2026, 9, 2), &HashMap::new());
        assert_eq!((r.grade, r.from), ("B", Some(at("2026-09-02T00:00:00Z"))));
        let last_year = Interpretation {
            id: 1,
            shape: Shape::Interval,
            reference: Reference::Anchored {
                anchor: Anchor::Document,
                offset: Some(Offset {
                    count: 1,
                    unit: Unit::Year,
                    direction: Direction::Before,
                }),
            },
            granularity: Granularity::Year,
        };
        let r = resolve_one(&last_year, &ctx_dated(2026, 9, 2), &HashMap::new());
        assert_eq!(
            (r.grade, r.from, r.from_p),
            ("B", Some(at("2025-01-01T00:00:00Z")), Some("year"))
        );
        // 没有文档日期就锚不到
        let undated = DocumentDating::default();
        assert_eq!(resolve_one(&today, &undated, &HashMap::new()), UNRESOLVED);
    }

    #[test]
    fn a_named_period_is_an_interval_from_the_context() {
        let i = Interpretation {
            id: 0,
            shape: Shape::Point,
            reference: Reference::Period {
                name: "Fiscal 2027".into(),
                from: None,
                to: None,
            },
            granularity: Granularity::Day,
        };
        let r = resolve_one(&i, &ctx_dated(2026, 9, 2), &HashMap::new());
        assert_eq!(r.grade, "B");
        assert_eq!(r.from, Some(at("2026-01-26T00:00:00Z")));
        assert_eq!(r.to, Some(at("2027-01-25T00:00:00Z")));
        // 文中自己写了起止的期间是 A 级
        let stated = Interpretation {
            reference: Reference::Period {
                name: "three months ended July 26, 2026".into(),
                from: Some(parts(2026, Some(4), Some(27))),
                to: Some(parts(2026, Some(7), Some(26))),
            },
            ..i
        };
        assert_eq!(
            resolve_one(&stated, &DocumentDating::default(), &HashMap::new()).grade,
            "A"
        );
    }

    #[test]
    fn two_years_later_follows_the_mention_it_names() {
        let mut earlier = HashMap::new();
        earlier.insert(
            7,
            Resolved {
                from: Some(at("2019-01-01T00:00:00Z")),
                from_p: Some("year"),
                to: Some(at("2019-01-01T00:00:00Z")),
                to_p: Some("year"),
                grade: "A",
            },
        );
        let i = Interpretation {
            id: 8,
            shape: Shape::Point,
            reference: Reference::Anchored {
                anchor: Anchor::Mention { id: 7 },
                offset: Some(Offset {
                    count: 2,
                    unit: Unit::Year,
                    direction: Direction::After,
                }),
            },
            granularity: Granularity::Year,
        };
        let r = resolve_one(&i, &DocumentDating::default(), &earlier);
        assert_eq!(
            (r.grade, r.from, r.from_p),
            ("B", Some(at("2021-01-01T00:00:00Z")), Some("year"))
        );
    }

    #[test]
    fn an_ending_without_a_date_and_a_when_plus_ended_combine() {
        let ended = Interpretation {
            id: 0,
            shape: Shape::EndedUnknown,
            reference: Reference::None,
            granularity: Granularity::Day,
        };
        let e = resolve_one(&ended, &DocumentDating::default(), &HashMap::new());
        assert_eq!((e.to, e.to_p, e.grade), (None, Some(ENDED_UNKNOWN), "A"));
        let since = Resolved {
            from: Some(at("2020-01-01T00:00:00Z")),
            from_p: Some("year"),
            to: None,
            to_p: None,
            grade: "A",
        };
        let until = Resolved {
            from: Some(at("2024-06-01T00:00:00Z")),
            from_p: Some("month"),
            to: None,
            to_p: None,
            grade: "A",
        };
        let r = combine(Some(since), Some(until));
        assert_eq!(
            (r.from, r.to, r.to_p),
            (since.from, until.from, Some("month"))
        );
        assert_eq!(combine(None, Some(e)).to_p, Some(ENDED_UNKNOWN));
    }

    #[test]
    fn this_quarter_is_the_whole_quarter_and_the_fiscal_calendar_moves_it() {
        let this_quarter = Interpretation {
            id: 0,
            shape: Shape::Point,
            reference: Reference::Anchored {
                anchor: Anchor::Document,
                offset: Some(Offset {
                    count: 0,
                    unit: Unit::Quarter,
                    direction: Direction::After,
                }),
            },
            granularity: Granularity::Month,
        };
        // 日历季：9 月 2 日在第三季（7–9 月）
        let calendar = DocumentDating {
            fiscal_year_end: None,
            ..ctx_dated(2026, 9, 2)
        };
        let r = resolve_one(&this_quarter, &calendar, &HashMap::new());
        assert_eq!(
            (r.from, r.to),
            (
                Some(at("2026-07-01T00:00:00Z")),
                Some(at("2026-09-01T00:00:00Z"))
            )
        );
        assert_eq!(
            (r.from_p, r.to_p, r.grade),
            (Some("month"), Some("month"), "B")
        );
        // 财年结束于 1 月 25 日：季首月是 2、5、8、11 月，9 月 2 日在 8–10 月那一季
        let r = resolve_one(&this_quarter, &ctx_dated(2026, 9, 2), &HashMap::new());
        assert_eq!(
            (r.from, r.to),
            (
                Some(at("2026-08-01T00:00:00Z")),
                Some(at("2026-10-01T00:00:00Z"))
            )
        );
        // 上一季度
        let last_quarter = Interpretation {
            reference: Reference::Anchored {
                anchor: Anchor::Document,
                offset: Some(Offset {
                    count: 1,
                    unit: Unit::Quarter,
                    direction: Direction::Before,
                }),
            },
            ..this_quarter
        };
        let r = resolve_one(&last_quarter, &calendar, &HashMap::new());
        assert_eq!(r.from, Some(at("2026-04-01T00:00:00Z")));
    }

    #[test]
    fn as_of_a_period_means_its_end_and_a_whole_year_is_one_bucket() {
        let year_end = Interpretation {
            id: 0,
            shape: Shape::AsOf,
            reference: Reference::Anchored {
                anchor: Anchor::Period {
                    name: "fiscal 2027".into(),
                },
                offset: None,
            },
            granularity: Granularity::Day,
        };
        let r = resolve_one(&year_end, &ctx_dated(2026, 9, 2), &HashMap::new());
        assert_eq!((r.from, r.grade), (Some(at("2027-01-25T00:00:00Z")), "B"));
        let whole_year = Interpretation {
            id: 1,
            shape: Shape::Interval,
            reference: Reference::Anchored {
                anchor: Anchor::Document,
                offset: Some(Offset {
                    count: 0,
                    unit: Unit::Year,
                    direction: Direction::After,
                }),
            },
            granularity: Granularity::Year,
        };
        let r = resolve_one(&whole_year, &ctx_dated(2024, 1, 1), &HashMap::new());
        assert_eq!(
            (r.from, r.to, r.from_p, r.to_p),
            (
                Some(at("2024-01-01T00:00:00Z")),
                Some(at("2024-01-01T00:00:00Z")),
                Some("year"),
                Some("year")
            )
        );
    }

    #[test]
    fn a_period_with_one_stated_bound_writes_only_that_bound() {
        let ended = Interpretation {
            id: 0,
            shape: Shape::Interval,
            reference: Reference::Period {
                name: "three months ended June 30, 2019".into(),
                from: None,
                to: Some(parts(2019, Some(6), Some(30))),
            },
            granularity: Granularity::Day,
        };
        let r = resolve_one(&ended, &DocumentDating::default(), &HashMap::new());
        assert_eq!(
            (r.from, r.to, r.grade),
            (None, Some(at("2019-06-30T00:00:00Z")), "A")
        );
    }

    #[test]
    fn month_arithmetic_clamps_to_the_month_end() {
        let t = at("2024-03-31T00:00:00Z");
        let back = shift(
            t,
            &Offset {
                count: 1,
                unit: Unit::Month,
                direction: Direction::Before,
            },
        )
        .unwrap();
        assert_eq!(back, at("2024-02-29T00:00:00Z"));
        let q = shift(
            t,
            &Offset {
                count: 1,
                unit: Unit::Quarter,
                direction: Direction::After,
            },
        )
        .unwrap();
        assert_eq!(q, at("2024-06-30T00:00:00Z"));
    }
}
