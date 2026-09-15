//! 时态引擎（S3）：functional 状态关系的矛盾检测与自动闭合。
//!
//! 原则：
//! - 纯规则判定，零 LLM——模糊性已在上游（消解归并实体、本体标 functional）消化
//! - 闭合走"作废 + 改写"而非原地改：旧断言 invalidated_at 记下"何时被修正"，
//!   修正行闭合区间并以 supersedes 链回旧行——"以当时的认知回放当时"得以成立
//! - 闭合点只用世界时间：后任的 valid_from；后任没有起点时，前任写成「结束了，不知哪天」，
//!   锚在后任最早那份自带日期的证据上（0022 的形状，#681 §1）。文档日期从不写进日期列
//! - 拿不准（缺时间/同时开始/低置信）绝不硬闭合，进 fact_conflicts 由人裁决
//!
//! **一条时间线是一个整体。** 同一 (库, 持有者, 谓词, 唯一性方向) 的所有现存行按时间排成
//! 一列。一行的终点要么是原文或人写明的，从不重算；要么由引擎推出（`facts.end_derived`，
//! 0057）：开着的行、引擎关上的行，都止于它之后最近的另一个值开始时。事实落库或又被观察到、
//! 合并搬来或撤回、文档删除或恢复，都把这一列按当下有哪些行**一次**重算到这个形状——结果
//! 只取决于有哪些行，与它们到达的先后无关（#679）。
//!
//! 一条关系两侧都唯一（functional 且 inverse functional，「一个人同时只领导一个项目，一个
//! 项目同时只有一个领导」）时，一行同时在两条时间线上：它的终点是两侧各推一个、取早的那个。
//!
//! 重算在一个事务里、持着这条时间线的咨询锁做完。一次要动几条时间线的事务（撤回合并、删除
//! 文档）先按固定顺序把这些锁全拿到，再改任何一行：落库对账也是先拿锁再锁行，两边顺序一致，
//! 谁也不会拿着行锁去等咨询锁。两侧都唯一的关系一条谓词一把锁——重算一侧要读另一侧。

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use utopia_core::models::ConflictView;
use utopia_core::AppResult;
use uuid::Uuid;

/// 低于此置信度的后任不允许自动改写前任的历史（进审）。
const AUTO_CLOSE_MIN_CONFIDENCE: f32 = 0.75;

/// 证据文件**自带**的最早日期，按 `facts` 的别名 `f` 投影。只认正文（`content`）与来源
/// （`source`）给的日期：上传时刻、文件修改时间不是文档自己的日期——拿它们排序，
/// 每条没起点的旧行都会被读成「此刻还在」。删掉的文档不再作证
const DATED_AT: &str = "(SELECT min(d.doc_time) FROM fact_evidence fe
                         JOIN documents d ON d.id = fe.document_id
                         WHERE fe.fact_id = f.id AND d.doc_time IS NOT NULL
                           AND d.deleted_at IS NULL
                           AND d.doc_time_source IN ('content', 'source'))";

/// 唯一性方向：functional = 主语侧（张三同时只 reports_to 一人）；
/// inverse functional = 宾语侧（一个项目同时只有一个 leads 它的人）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Uniqueness {
    SubjectSide,
    ObjectSide,
}

impl Uniqueness {
    fn other(self) -> Self {
        match self {
            Self::SubjectSide => Self::ObjectSide,
            Self::ObjectSide => Self::SubjectSide,
        }
    }
}

/// 一条唯一性时间线：谁（持有者）在哪个谓词、哪个方向上同时只能有一个值
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Timeline {
    pub holder: Uuid,
    pub predicate_id: Uuid,
    pub side: Uniqueness,
    /// 谓词两侧都唯一：这条线上的每一行也在另一侧的一条线上
    pub both_sides: bool,
}

impl Timeline {
    fn lock_key(&self, kb_id: Uuid) -> String {
        if self.both_sides {
            format!("timeline:{kb_id}:{}:both", self.predicate_id)
        } else {
            format!(
                "timeline:{kb_id}:{}:{}:{:?}",
                self.holder, self.predicate_id, self.side
            )
        }
    }
}

/// 对账结果：自动闭合产生的修正行 id（调用方按需记账）与这次新进人审的冲突数。
#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub corrected: Vec<Uuid>,
    pub conflicts: u32,
}

/// 时间线上的一行
#[derive(Debug, Clone, sqlx::FromRow)]
struct Row {
    id: Uuid,
    subject_id: Uuid,
    object_id: Option<Uuid>,
    object_value: Option<serde_json::Value>,
    valid_from: Option<DateTime<Utc>>,
    valid_from_precision: Option<String>,
    valid_to: Option<DateTime<Utc>>,
    valid_to_precision: Option<String>,
    attested_to: Option<DateTime<Utc>>,
    confidence: f32,
    end_derived: bool,
    dated_at: Option<DateTime<Utc>>,
}

/// 一行的终点
#[derive(Debug, Clone, PartialEq)]
enum End {
    /// 仍在持续
    Open,
    /// 在这一刻结束，带精度
    At(DateTime<Utc>, String),
    /// 结束了，不知哪天；读出侧读到锚点为止
    Unknown(Option<DateTime<Utc>>),
}

impl End {
    /// 读出来止于哪一刻；开着的没有
    fn instant(&self) -> Option<DateTime<Utc>> {
        match self {
            End::Open => None,
            End::At(at, _) => Some(*at),
            End::Unknown(anchor) => *anchor,
        }
    }

    /// 两个终点里早的那个；同一刻时写着日期的胜过锚点
    fn earlier(self, other: End) -> End {
        match (self.instant(), other.instant()) {
            (None, _) => other,
            (_, None) => self,
            (Some(a), Some(b)) if b < a => other,
            (Some(a), Some(b)) if a == b && matches!(other, End::At(..)) => other,
            _ => self,
        }
    }
}

impl Row {
    /// 排序用的时刻：起点。没有起点时，最早那份自带日期的证据——但只对还开着、或者由引擎
    /// 关上的行成立：它们的证据说那天还成立。原文说已经结束的行，证据的日期只说明「那天
    /// 之前结束了」，拿它排序会让一个早就结束的值去关上当下的值（#679 第三轮评审）
    fn key(&self) -> Option<DateTime<Utc>> {
        match self.valid_from {
            Some(start) => Some(start),
            None if self.is_open() || self.end_derived => self.dated_at,
            None => None,
        }
    }
    fn is_open(&self) -> bool {
        self.valid_to.is_none() && self.valid_to_precision.is_none()
    }
    /// 终点由引擎按时间线定：开着的，或引擎关上的
    fn recomputable(&self) -> bool {
        self.is_open() || self.end_derived
    }
    /// 终点的时刻：日期；「结束了，不知哪天」的锚点（读出侧同样读到它为止）
    fn end(&self) -> Option<DateTime<Utc>> {
        match (self.valid_to, self.valid_to_precision.as_deref()) {
            (Some(to), _) => Some(to),
            (None, Some(crate::graph::ENDED_UNKNOWN)) => self.attested_to,
            _ => None,
        }
    }
    /// 眼下写着的终点
    fn current_end(&self) -> End {
        match (self.valid_to, self.valid_to_precision.as_deref()) {
            (Some(to), p) => End::At(to, p.unwrap_or("day").to_string()),
            (None, Some(crate::graph::ENDED_UNKNOWN)) => End::Unknown(self.attested_to),
            _ => End::Open,
        }
    }
    /// 前一段止于这一行开始时，终点写成什么：有起点就是那一刻；没有起点，是「结束了，
    /// 不知哪天」，锚在这一行自带日期的证据上
    fn end_before(&self) -> End {
        match self.valid_from {
            Some(start) => {
                let precision = self.valid_from_precision.as_deref().unwrap_or("day");
                End::At(
                    crate::graph::truncate_to(start, Some(precision)),
                    precision.to_string(),
                )
            }
            None => End::Unknown(self.dated_at),
        }
    }
    /// 在 `at` 这一刻还成立
    fn holds_at(&self, at: DateTime<Utc>) -> bool {
        self.is_open() || self.end().is_some_and(|end| end > at)
    }
    /// 这一行在 `side` 那一侧的持有者
    fn holder(&self, side: Uniqueness) -> Option<Uuid> {
        match side {
            Uniqueness::SubjectSide => Some(self.subject_id),
            Uniqueness::ObjectSide => self.object_id,
        }
    }
}

/// 两行说的是不是同一个值（同一侧：主语侧比宾语，宾语侧比主语）
fn same_value(side: Uniqueness, a: &Row, b: &Row) -> bool {
    match side {
        Uniqueness::SubjectSide => a.object_id == b.object_id && a.object_value == b.object_value,
        Uniqueness::ObjectSide => a.subject_id == b.subject_id,
    }
}

async fn lock_timeline(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    timeline: Timeline,
) -> AppResult<()> {
    lock_timelines(tx, kb_id, &[timeline]).await
}

/// 一次锁的时间线超过这么多条，改拿谓词一级的排他锁（见 [`lock_timelines`]）
const BULK_TIMELINES: usize = 256;

/// 拿下几条时间线的咨询锁，**按固定顺序**。一次要动多条时间线的事务都走这里：
/// 大家按同一个顺序排队，谁也不会拿着一条去等另一条。同一事务里重复拿同一把锁无妨。
///
/// **锁分两级。** 平常先拿谓词一级的共享锁，再拿时间线一级的排他锁——同一个谓词上
/// 不同持有者的时间线互不相干，照旧并行。一次要锁的时间线太多时（删一篇给几千个实体
/// 各记了一个属性的表格），改拿这些谓词的排他锁、不再逐条锁：逐条锁时 8000 条要拿
/// 8000 把锁，两万条时 Postgres 的锁表（`max_locks_per_transaction`）直接装不下，
/// 文档就删不掉了（#679 第四轮评审）。谓词锁在前、时间线锁在后，各自排序，
/// 两级之间不会反着等
pub async fn lock_timelines(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    timelines: &[Timeline],
) -> AppResult<()> {
    if timelines.is_empty() {
        return Ok(());
    }
    let mut predicates: Vec<String> = timelines
        .iter()
        .map(|t| format!("predicate:{kb_id}:{}", t.predicate_id))
        .collect();
    predicates.sort();
    predicates.dedup();
    let bulk = timelines.len() > BULK_TIMELINES;
    let predicate_lock = if bulk {
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))"
    } else {
        "SELECT pg_advisory_xact_lock_shared(hashtextextended($1, 0))"
    };
    for key in predicates {
        sqlx::query(predicate_lock)
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    if bulk {
        return Ok(());
    }
    let mut keys: Vec<String> = timelines.iter().map(|t| t.lock_key(kb_id)).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn load_timeline(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    timeline: Timeline,
) -> AppResult<Vec<Row>> {
    let holder_column = match timeline.side {
        Uniqueness::SubjectSide => "f.subject_id",
        Uniqueness::ObjectSide => "f.object_id",
    };
    let rows = sqlx::query_as(&format!(
        "SELECT f.id, f.subject_id, f.object_id, f.object_value,
                f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision,
                f.attested_to, f.confidence, f.end_derived, {DATED_AT} AS dated_at
         FROM facts f
         WHERE f.kb_id = $1 AND {holder_column} = $2 AND f.predicate_id = $3
           AND f.invalidated_at IS NULL
         ORDER BY f.recorded_at
         FOR UPDATE OF f"
    ))
    .bind(kb_id)
    .bind(timeline.holder)
    .bind(timeline.predicate_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows)
}

/// 这些事实所在的全部唯一性时间线（只算 temporal = state、声明了唯一性的谓词）。
///
/// `swap = (from, to)`：主语或宾语是 `from` 的事实，换成 `to` 之后落在的那条也算上——
/// 撤回合并要在搬动任何一行之前，把搬走前、搬回后两边的锁都拿到
pub async fn timelines_of<'e, E>(
    executor: E,
    kb_id: Uuid,
    fact_ids: &[Uuid],
    // 这些持有者上的时间线，另记一份换到后面那个实体上（合并、撤回合并时两边都要锁）
    swap: Option<(&[Uuid], Uuid)>,
) -> AppResult<Vec<Timeline>>
where
    E: sqlx::PgExecutor<'e>,
{
    #[derive(sqlx::FromRow)]
    struct Placed {
        subject_id: Uuid,
        object_id: Option<Uuid>,
        predicate_id: Uuid,
        functional: bool,
        inverse_functional: bool,
    }
    if fact_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placed: Vec<Placed> = sqlx::query_as(
        "SELECT DISTINCT f.subject_id, f.object_id, f.predicate_id, r.functional, r.inverse_functional
         FROM facts f JOIN relation_types r ON r.id = f.predicate_id
         WHERE f.kb_id = $1 AND f.id = ANY($2)
           AND r.temporal = 'state' AND (r.functional OR r.inverse_functional)",
    )
    .bind(kb_id)
    .bind(fact_ids)
    .fetch_all(executor)
    .await?;
    let swapped = |id: Uuid| {
        swap.filter(|(from, _)| from.contains(&id))
            .map(|(_, to)| to)
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in placed {
        let both_sides = p.functional && p.inverse_functional;
        let mut holders = Vec::new();
        if p.functional {
            holders.push((p.subject_id, Uniqueness::SubjectSide));
            holders.extend(swapped(p.subject_id).map(|to| (to, Uniqueness::SubjectSide)));
        }
        // 宾语侧唯一性只对实体宾语有意义（字面值不"被占用"）
        if let (true, Some(object)) = (p.inverse_functional, p.object_id) {
            holders.push((object, Uniqueness::ObjectSide));
            holders.extend(swapped(object).map(|to| (to, Uniqueness::ObjectSide)));
        }
        for (holder, side) in holders {
            let timeline = Timeline {
                holder,
                predicate_id: p.predicate_id,
                side,
                both_sides,
            };
            if seen.insert(timeline) {
                out.push(timeline);
            }
        }
    }
    Ok(out)
}

/// 一条 state 事实**被观察到**之后、沿指定唯一性方向的对账。新落库的要调，**又被观察到**
/// 的也要调（同一断言多了一份证据，那份证据的日期可能更早，时间线的形状跟着变），写完
/// 证据再调。调用方负责判断关系确实带该方向的唯一性且 temporal = state。
///
/// 宾语可以是实体（object_id）或字面值（object_value，属性事实）——
/// "宾语不同"的判定是 (object_id, object_value) 组合比较：工资从 3 万变 3.5 万
/// 与"从张三换成李四"走同一条闭合路径。
///
/// 这条事实的时间、置信度从库里读，参数只用来决定对哪条时间线
#[allow(clippy::too_many_arguments)]
pub async fn reconcile_new_fact(
    pool: &PgPool,
    kb_id: Uuid,
    new_fact_id: Uuid,
    subject_id: Uuid,
    predicate_id: Uuid,
    object_id: Option<Uuid>,
    _object_value: Option<&serde_json::Value>,
    direction: Uniqueness,
    _new_validity: crate::graph::Validity<'_>,
    _new_confidence: f32,
) -> AppResult<ReconcileReport> {
    let holder = match direction {
        Uniqueness::SubjectSide => subject_id,
        Uniqueness::ObjectSide => match object_id {
            Some(o) => o,
            None => return Ok(ReconcileReport::default()),
        },
    };
    let both_sides: bool = sqlx::query_scalar(
        "SELECT functional AND inverse_functional FROM relation_types WHERE id = $1",
    )
    .bind(predicate_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false);
    let timeline = Timeline {
        holder,
        predicate_id,
        side: direction,
        both_sides,
    };
    let mut report = ReconcileReport::default();
    let mut tx = pool.begin().await?;
    lock_timeline(&mut tx, kb_id, timeline).await?;
    arrive(&mut tx, kb_id, timeline, new_fact_id, &[], &mut report).await?;
    tidy(&mut tx, kb_id, timeline, &mut report).await?;
    tx.commit().await?;
    Ok(report)
}

/// 一条事实来到它的时间线上，记下重算裁不了的，交给人（调用方已持锁）：
///
/// - 与别的值同一时刻开始、两边那一刻都还成立：谁接替谁说不清
/// - 两边都还开着，有一边说不出时间（没起点、也没有自带日期的证据）：谁先谁后无从谈起。
///   不论哪一边先到都是这一对冲突，不替人关上任何一边
///
/// `settled`：同一批里已经来过的事实，不再与它们重复成对
async fn arrive(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    timeline: Timeline,
    fact_id: Uuid,
    settled: &[Uuid],
    report: &mut ReconcileReport,
) -> AppResult<()> {
    let rows = load_timeline(tx, kb_id, timeline).await?;
    let Some(new) = rows.iter().find(|r| r.id == fact_id) else {
        return Ok(());
    };
    let others = rows
        .iter()
        .filter(|r| r.id != new.id && !same_value(timeline.side, r, new))
        .filter(|r| !settled.contains(&r.id));
    for other in others {
        let reason = match (new.key(), other.key()) {
            (Some(at), Some(theirs)) if at == theirs && new.holds_at(at) && other.holds_at(at) => {
                "simultaneous"
            }
            (None, _) | (_, None) if new.is_open() && other.is_open() => "no_time",
            _ => continue,
        };
        if record_conflict_tx(tx, kb_id, other.id, new.id, reason).await? {
            report.conflicts += 1;
        }
    }
    Ok(())
}

/// 重算一条时间线要改的地方（单元测试看的形状）
#[cfg(test)]
#[derive(Debug, Default, PartialEq)]
struct Plan {
    /// (行, 它该有的终点)
    ends: Vec<(Uuid, End)>,
    /// (行, 置信度不够、没让它关上的最近那个后任)：交给人
    held: Vec<(Uuid, Uuid)>,
}

/// 一条时间线上每一行该有的终点（纯函数，不碰库）：只含终点由引擎定的行。
///
/// 能排进时间线的行（有起点，或有自带日期的证据）按时刻排好；终点是写明的行不动。其余
/// 每一行——开着的、引擎关上的——止于它之后最近的、值不同的那一行开始时；后面没有这样
/// 的行就开着。那一行置信度不够时不许它改写历史：跳过它、交给人，再看下一行。
///
/// 每一行的终点只取决于各行的时刻、值和置信度，改写终点不改这三样，所以一次算完就是
/// 最终的样子，不必一轮一轮地来；行怎么排进来的也不影响结果
fn desired_ends(side: Uniqueness, rows: &[Row]) -> (HashMap<Uuid, End>, Vec<(Uuid, Uuid)>) {
    let mut keyed: Vec<&Row> = rows.iter().filter(|r| r.key().is_some()).collect();
    // 同一刻开始的几行，写着起点的排前面：后任取它，前任就止于一个日期而不是一个锚点
    keyed.sort_by_key(|r| (r.key(), r.valid_from.is_none(), r.id));
    let mut ends = HashMap::new();
    let mut held = Vec::new();
    for (i, row) in keyed.iter().enumerate() {
        if !row.recomputable() {
            continue;
        }
        let mut end = End::Open;
        let mut doubtful = None;
        for later in keyed[i + 1..]
            .iter()
            .filter(|later| later.key() > row.key() && !same_value(side, row, later))
        {
            if later.confidence < AUTO_CLOSE_MIN_CONFIDENCE {
                doubtful.get_or_insert(later.id);
                continue;
            }
            end = later.end_before();
            break;
        }
        if let Some(later) = doubtful {
            held.push((row.id, later));
        }
        ends.insert(row.id, end);
    }
    (ends, held)
}

/// 单侧时间线的重算计划：该有的终点与眼下写着的不同的那些行
#[cfg(test)]
fn plan_timeline(side: Uniqueness, rows: &[Row]) -> Plan {
    let (ends, held) = desired_ends(side, rows);
    Plan {
        ends: diff(rows, ends),
        held,
    }
}

/// 该有的终点里与眼下不同的，按行在时间线上的顺序
fn diff(rows: &[Row], mut ends: HashMap<Uuid, End>) -> Vec<(Uuid, End)> {
    rows.iter()
        .filter_map(|r| {
            let end = ends.remove(&r.id)?;
            (r.current_end() != end).then_some((r.id, end))
        })
        .collect()
}

/// 把一条时间线重算到 [`desired_ends`] 说的样子（调用方已持锁）。两侧都唯一的关系，
/// 每一行再按它另一侧的时间线推一次，取早的那个终点
async fn tidy(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    timeline: Timeline,
    report: &mut ReconcileReport,
) -> AppResult<()> {
    let rows = load_timeline(tx, kb_id, timeline).await?;
    let (mut ends, mut held) = desired_ends(timeline.side, &rows);
    if timeline.both_sides {
        let other = timeline.side.other();
        let holders: BTreeSet<Uuid> = rows.iter().filter_map(|r| r.holder(other)).collect();
        for holder in holders {
            let across = Timeline {
                holder,
                side: other,
                ..timeline
            };
            let across_rows = load_timeline(tx, kb_id, across).await?;
            let (across_ends, across_held) = desired_ends(other, &across_rows);
            for (id, end) in across_ends {
                if let Some(mine) = ends.remove(&id) {
                    ends.insert(id, mine.earlier(end));
                }
            }
            held.extend(
                across_held
                    .into_iter()
                    .filter(|(row, _)| ends.contains_key(row)),
            );
        }
    }
    let mut rewritten: HashMap<Uuid, Uuid> = HashMap::new();
    for (id, end) in diff(&rows, ends) {
        // 重新打开的行只是开着，终点不是谁推出来的
        let derived = end != End::Open;
        if let Some(corrected) = rewrite_end_tx(tx, id, &end, derived, false).await? {
            rewritten.insert(id, corrected);
            report.corrected.push(corrected);
        }
    }
    let mut still_held = HashSet::new();
    for (row, later) in held {
        let row = rewritten.get(&row).copied().unwrap_or(row);
        let later = rewritten.get(&later).copied().unwrap_or(later);
        still_held.insert((row, later));
        if record_conflict_tx(tx, kb_id, row, later, "low_confidence").await? {
            report.conflicts += 1;
        }
    }
    // 置信度不够、交给人的那一对，后任后来被人或 agent 确认过（0043）：这道题没了，
    // 撤下——不撤的话它挂在队列里，问的是一件引擎已经排好的事
    let on_timeline: Vec<Uuid> = rows
        .iter()
        .map(|r| r.id)
        .chain(rewritten.values().copied())
        .collect();
    let open: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT c.id, c.old_fact_id, c.new_fact_id FROM fact_conflicts c
          WHERE c.kb_id = $1 AND c.status = 'open' AND c.reason = 'low_confidence'
            AND c.new_fact_id = ANY($2)",
    )
    .bind(kb_id)
    .bind(&on_timeline)
    .fetch_all(&mut **tx)
    .await?;
    for (conflict, old, new) in open {
        if !still_held.contains(&(old, new)) {
            sqlx::query(
                "UPDATE fact_conflicts SET status = 'withdrawn', resolution = NULL,
                        resolved_at = now()
                  WHERE id = $1 AND status = 'open'",
            )
            .bind(conflict)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

/// 两个实体合成一个之后，哪些唯一性关系上**还会**有两个值同时成立（执行闸门，0027）。
///
/// 两边各有一个不同的值不等于矛盾：租约的房东先是 HPBB1、后来换成 BBHQ1，两个名字各挂
/// 一个，合起来是一条先后接替的时间线。这里把两边的行当成一条时间线，按引擎会怎么排
/// （[`desired_ends`]）推出每一行的终点，再看不同的值有没有哪一刻叠在一起——排不开的
/// （同一刻开始、说不出时间、置信度不够接替、两份原文写明的区间相交）才是合了之后一致性
/// 检查会报的矛盾。只看实体宾语的边，与一致性检查（`utopia_reason::check`）同一个口径。
/// 返回谓词标签，按标签排
pub async fn merge_would_overlap(
    pool: &PgPool,
    kb_id: Uuid,
    a: Uuid,
    b: Uuid,
) -> AppResult<Vec<String>> {
    #[derive(sqlx::FromRow)]
    struct Shared {
        predicate_id: Uuid,
        label: String,
        functional: bool,
        inverse_functional: bool,
        temporal: String,
    }
    let shared: Vec<Shared> = sqlx::query_as(
        "SELECT DISTINCT r.id AS predicate_id, r.label, r.functional, r.inverse_functional,
                r.temporal
         FROM facts x
         JOIN facts y ON y.predicate_id = x.predicate_id AND y.kb_id = x.kb_id
         JOIN relation_types r ON r.id = x.predicate_id
         WHERE x.kb_id = $1
           AND x.invalidated_at IS NULL AND y.invalidated_at IS NULL
           AND x.object_id IS NOT NULL AND y.object_id IS NOT NULL
           AND ((r.functional AND x.subject_id = $2 AND y.subject_id = $3
                 AND x.object_id <> y.object_id)
             OR (r.inverse_functional AND x.object_id = $2 AND y.object_id = $3
                 AND x.subject_id <> y.subject_id))",
    )
    .bind(kb_id)
    .bind(a)
    .bind(b)
    .fetch_all(pool)
    .await?;

    let mut labels = Vec::new();
    for s in shared {
        let mut sides = Vec::new();
        if s.functional {
            sides.push(Uniqueness::SubjectSide);
        }
        if s.inverse_functional {
            sides.push(Uniqueness::ObjectSide);
        }
        for side in sides {
            let holder_column = match side {
                Uniqueness::SubjectSide => "f.subject_id",
                Uniqueness::ObjectSide => "f.object_id",
            };
            let mut rows: Vec<Row> = sqlx::query_as(&format!(
                "SELECT f.id, f.subject_id, f.object_id, f.object_value,
                        f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision,
                        f.attested_to, f.confidence, f.end_derived, {DATED_AT} AS dated_at
                 FROM facts f
                 WHERE f.kb_id = $1 AND {holder_column} = ANY($2) AND f.predicate_id = $3
                   AND f.invalidated_at IS NULL AND f.object_id IS NOT NULL"
            ))
            .bind(kb_id)
            .bind(vec![a, b])
            .bind(s.predicate_id)
            .fetch_all(pool)
            .await?;
            // **只算合并带进来的冲突。** 一边持有的值另一边全都已经持有，合起来还是那几个值：
            // 同一个宾语并成一行，冲突要有也是那一边原本就有的，合不合都在。租约链上实测：
            // 一份租约自己已经挂着两个没日期的房东，另一个写法只挂着其中一个，0.98 的合并被
            // 「两个房东」挡下，而那两个房东合并之前就在
            let values_of = |holder: Uuid| -> HashSet<Uuid> {
                rows.iter()
                    .filter_map(|r| match side {
                        Uniqueness::SubjectSide => (r.subject_id == holder).then_some(r.object_id?),
                        Uniqueness::ObjectSide => {
                            (r.object_id == Some(holder)).then_some(r.subject_id)
                        }
                    })
                    .collect()
            };
            let (held_by_a, held_by_b) = (values_of(a), values_of(b));
            if held_by_a.is_subset(&held_by_b) || held_by_b.is_subset(&held_by_a) {
                continue;
            }
            // 合成一个之后的样子：持有者那一侧都是同一个实体
            for row in &mut rows {
                match side {
                    Uniqueness::SubjectSide => row.subject_id = a,
                    Uniqueness::ObjectSide => row.object_id = Some(a),
                }
            }
            if still_overlaps(side, &rows, s.temporal == "state") {
                labels.push(s.label.clone());
                break;
            }
        }
    }
    labels.sort();
    labels.dedup();
    Ok(labels)
}

/// 一行排完之后的 [起, 止)，`None` 是那一侧无界
type Span = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

/// 一条时间线按引擎排完之后，不同的值有没有哪一刻同时成立（纯函数）。`placed`：谓词是
/// 状态，引擎会排它；事件与恒常不排，照行上写着的终点比
fn still_overlaps(side: Uniqueness, rows: &[Row], placed: bool) -> bool {
    let ends = if placed {
        desired_ends(side, rows).0
    } else {
        HashMap::new()
    };
    // 每一行排完之后的 [起, 止)：起点是排序用的时刻，说不出时间的行从无穷早起；
    // 终点是引擎推的，或原文写明的
    let spans: Vec<Span> = rows
        .iter()
        .map(|r| {
            let end = ends.get(&r.id).map(End::instant).unwrap_or_else(|| r.end());
            (r.key(), end)
        })
        .collect();
    for i in 0..rows.len() {
        for j in i + 1..rows.len() {
            if same_value(side, &rows[i], &rows[j]) {
                continue;
            }
            let ((si, ei), (sj, ej)) = (spans[i], spans[j]);
            let starts_before_end =
                |s: Option<DateTime<Utc>>, e: Option<DateTime<Utc>>| match (s, e) {
                    (Some(s), Some(e)) => s < e,
                    _ => true,
                };
            if starts_before_end(si, ej) && starts_before_end(sj, ei) {
                return true;
            }
        }
    }
    false
}

/// 把几条时间线各自重算一遍（调用方已按 [`lock_timelines`] 拿到锁）。撤回合并、删除或
/// 恢复文档在自己的事务里改完行之后调
pub async fn tidy_timelines_tx(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    timelines: &[Timeline],
) -> AppResult<ReconcileReport> {
    let mut report = ReconcileReport::default();
    for timeline in timelines {
        tidy(tx, kb_id, *timeline, &mut report).await?;
    }
    Ok(report)
}

/// 实体合并搬移事实后的对账：换了主/宾的事实等价于"新落库的观察"——
/// 两个对象折成一个之后，唯一性不变量才第一次看得到它们相撞。人改过区间之后也走这里。
/// 返回的修正行 id 由调用方记入合并账本，供审计
pub async fn reconcile_moved_facts(
    pool: &PgPool,
    kb_id: Uuid,
    fact_ids: &[Uuid],
) -> AppResult<ReconcileReport> {
    reconcile_facts(pool, kb_id, fact_ids).await
}

/// 一批事实所在的每条时间线：批里的事实逐条「来到」时间线上（记下该交给人的），整条
/// 重算一遍。一条时间线一个事务
async fn reconcile_facts(
    pool: &PgPool,
    kb_id: Uuid,
    fact_ids: &[Uuid],
) -> AppResult<ReconcileReport> {
    let mut report = ReconcileReport::default();
    let batch: HashSet<Uuid> = fact_ids.iter().copied().collect();
    for timeline in timelines_of(pool, kb_id, fact_ids, None).await? {
        let mut tx = pool.begin().await?;
        lock_timeline(&mut tx, kb_id, timeline).await?;
        let rows = load_timeline(&mut tx, kb_id, timeline).await?;
        let mut arriving: Vec<&Row> = rows.iter().filter(|r| batch.contains(&r.id)).collect();
        arriving.sort_by_key(|r| (r.key().is_none(), r.key(), r.id));
        let arriving: Vec<Uuid> = arriving.iter().map(|r| r.id).collect();
        for (i, id) in arriving.iter().enumerate() {
            arrive(&mut tx, kb_id, timeline, *id, &arriving[..i], &mut report).await?;
        }
        tidy(&mut tx, kb_id, timeline, &mut report).await?;
        tx.commit().await?;
    }
    Ok(report)
}

/// 声明来晚了：一条谓词上所有现存事实所在的时间线重算一遍（#341）。
///
/// 本体自己长出来的库里没人声明过唯一性，接任不会闭合前任——三个人同时在管一个
/// 项目。人补上声明之后，这里把已经躺在账上的行对一遍。走的是与落库时同一条
/// 路（作废 + 改写，supersedes 链回旧行），所以记录轴倒回声明之前仍看得见三条
/// 开放的行；拿不准的（缺时间 / 同时开始 / 低置信）照旧进人审，不硬闭合。
///
/// 没有声明的谓词拒绝：引擎不替人推断（bootstrap_ontology.rs 写了为什么）。
pub async fn reconcile_predicate(
    pool: &PgPool,
    kb_id: Uuid,
    predicate_id: Uuid,
) -> AppResult<ReconcileReport> {
    let declared: Option<(bool, bool, String)> = sqlx::query_as(
        "SELECT functional, inverse_functional, temporal FROM relation_types
         WHERE kb_id = $1 AND id = $2",
    )
    .bind(kb_id)
    .bind(predicate_id)
    .fetch_optional(pool)
    .await?;
    let Some((functional, inverse_functional, temporal)) = declared else {
        return Err(utopia_core::AppError::NotFound);
    };
    if temporal != "state" {
        return Err(utopia_core::AppError::invalid(
            "not_a_state",
            "only a state relation has intervals to close",
        ));
    }
    if !functional && !inverse_functional {
        return Err(utopia_core::AppError::invalid(
            "not_unique",
            "declare the relation functional or inverse-functional first; the engine does not infer it",
        ));
    }
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM facts
         WHERE kb_id = $1 AND predicate_id = $2 AND invalidated_at IS NULL
           AND (object_id IS NOT NULL OR object_value IS NOT NULL)",
    )
    .bind(kb_id)
    .bind(predicate_id)
    .fetch_all(pool)
    .await?;
    reconcile_facts(pool, kb_id, &ids).await
}

/// 改一条事实的置信度，时间线跟着重算：置信度决定它能不能接替前任（0022），人或 agent
/// 确认了一条低置信的值，前任就该关在它开始时。先锁时间线再改（见模块头）。
/// 返回原来的置信度；行不在或已作废返回 `None`
pub async fn set_confidence(
    pool: &PgPool,
    kb_id: Uuid,
    fact_id: Uuid,
    confidence: f32,
) -> AppResult<Option<f32>> {
    let mut tx = pool.begin().await?;
    let timelines = timelines_of(&mut *tx, kb_id, &[fact_id], None).await?;
    lock_timelines(&mut tx, kb_id, &timelines).await?;
    let prior: Option<f32> = sqlx::query_scalar(
        "SELECT confidence FROM facts
         WHERE id = $1 AND kb_id = $2 AND invalidated_at IS NULL FOR UPDATE",
    )
    .bind(fact_id)
    .bind(kb_id)
    .fetch_optional(&mut *tx)
    .await?;
    if prior.is_some() {
        sqlx::query("UPDATE facts SET confidence = $2 WHERE id = $1")
            .bind(fact_id)
            .bind(confidence)
            .execute(&mut *tx)
            .await?;
        tidy_timelines_tx(&mut tx, kb_id, &timelines).await?;
    }
    tx.commit().await?;
    Ok(prior)
}

/// 撤回一次撤掉：作废了的事实回来，时间线按它回来之后的样子重算。agent 驳回一条事实之后，
/// 人撤回那个决定走这里。返回有没有救回一行
pub async fn restore(pool: &PgPool, kb_id: Uuid, fact_id: Uuid) -> AppResult<bool> {
    let mut tx = pool.begin().await?;
    let timelines = timelines_of(&mut *tx, kb_id, &[fact_id], None).await?;
    lock_timelines(&mut tx, kb_id, &timelines).await?;
    let restored = sqlx::query(
        "UPDATE facts SET invalidated_at = NULL
         WHERE id = $1 AND kb_id = $2 AND invalidated_at IS NOT NULL",
    )
    .bind(fact_id)
    .bind(kb_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if restored {
        tidy_timelines_tx(&mut tx, kb_id, &timelines).await?;
    }
    tx.commit().await?;
    Ok(restored)
}

/// 撤回一次改写：改写出来的那一行作废，被它取代的原行回来，时间线重算。agent 关上一条
/// 事实、改过一条事实的起点，人撤回时走这里。改写出来的行后来又被改写过的（换过终点、
/// 被人改过），不再撤：那之后的变化有它自己的依据。返回有没有撤成
pub async fn undo_rewrite(
    pool: &PgPool,
    kb_id: Uuid,
    corrected: Uuid,
    original: Uuid,
) -> AppResult<bool> {
    let mut tx = pool.begin().await?;
    let timelines = timelines_of(&mut *tx, kb_id, &[corrected, original], None).await?;
    lock_timelines(&mut tx, kb_id, &timelines).await?;
    let still: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM facts
         WHERE id = $1 AND kb_id = $2 AND supersedes = $3 AND invalidated_at IS NULL
         FOR UPDATE",
    )
    .bind(corrected)
    .bind(kb_id)
    .bind(original)
    .fetch_optional(&mut *tx)
    .await?;
    if still.is_none() {
        tx.commit().await?;
        return Ok(false);
    }
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(corrected)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE facts SET invalidated_at = NULL WHERE id = $1")
        .bind(original)
        .execute(&mut *tx)
        .await?;
    tidy_timelines_tx(&mut tx, kb_id, &timelines).await?;
    tx.commit().await?;
    Ok(true)
}

/// 一条裁过的冲突重新开着：撤回裁决时用。两边都还活着才开得回来
pub async fn reopen_conflict(pool: &PgPool, kb_id: Uuid, conflict_id: Uuid) -> AppResult<bool> {
    let n = sqlx::query(
        "UPDATE fact_conflicts c SET status = 'open', resolution = NULL, resolved_at = NULL
          WHERE c.id = $1 AND c.kb_id = $2 AND c.status <> 'open'
            AND NOT EXISTS (SELECT 1 FROM facts f
                             WHERE f.id IN (c.old_fact_id, c.new_fact_id)
                               AND f.invalidated_at IS NOT NULL)",
    )
    .bind(conflict_id)
    .bind(kb_id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// 撤掉一条事实（人判它是抽取错误）：它从来不在，时间线按剩下的行重算——关在它开始时的
/// 前任重新接上。先锁时间线再作废（见模块头）。返回有没有撤掉一行；已经作废的不算
pub async fn retract(pool: &PgPool, kb_id: Uuid, fact_id: Uuid) -> AppResult<bool> {
    let mut tx = pool.begin().await?;
    let timelines = timelines_of(&mut *tx, kb_id, &[fact_id], None).await?;
    lock_timelines(&mut tx, kb_id, &timelines).await?;
    let retracted = sqlx::query(
        "UPDATE facts SET invalidated_at = now()
         WHERE id = $1 AND kb_id = $2 AND invalidated_at IS NULL",
    )
    .bind(fact_id)
    .bind(kb_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if retracted {
        tidy_timelines_tx(&mut tx, kb_id, &timelines).await?;
    }
    tx.commit().await?;
    Ok(retracted)
}

/// 作废 + 改写成**写明的**终点：旧行记 invalidated_at（认知轴），插入闭合区间的修正行
/// （世界轴），证据引用随行复制。原文说它在哪天结束、人裁决把它关上，都走这里——
/// 这个终点此后不再由引擎重算。返回修正行 id；`None` = 这条已被作废，没动。
pub async fn close_superseded(
    pool: &PgPool,
    fact_id: Uuid,
    valid_to: DateTime<Utc>,
    valid_to_precision: &str,
) -> AppResult<Option<Uuid>> {
    // 闭合点截到它的精度（0024）：月精度的闭合就是那个月的 1 日 0 点
    let end = End::At(
        crate::graph::truncate_to(valid_to, Some(valid_to_precision)),
        valid_to_precision.to_string(),
    );
    let mut tx = pool.begin().await?;
    let corrected = rewrite_end_tx(&mut tx, fact_id, &end, false, false).await?;
    tx.commit().await?;
    Ok(corrected)
}

/// 作废 + 改写成「结束了，不知哪天」（0022 / #393）：旧行记 invalidated_at，修正行终点仍是
/// NULL、精度 'unknown'，`attested_to` 锚在**说出结束的那份文档**——读出来就是「到它为止」；
/// `attested_from` 从旧行继承——没起点的裸行靠它记着第一份证据，读出来是「从那时起」。
/// 这是原文写明的结束，引擎不重算。证据引用随行复制。返回修正行 id；`None` = 这条已不是
/// 开放行，没动。
pub async fn close_with_unknown_end(
    pool: &PgPool,
    fact_id: Uuid,
    attested_at: Option<DateTime<Utc>>,
) -> AppResult<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    let corrected =
        rewrite_end_tx(&mut tx, fact_id, &End::Unknown(attested_at), false, true).await?;
    tx.commit().await?;
    Ok(corrected)
}

/// 原文说出了一条**引擎关上**的行的终点：改写成原文说的，此后不再重算（#679 第三轮评审）。
/// `valid_to` 为 `None` 是「结束了，不知哪天」，锚在 `attested_at`。返回修正行 id；
/// 行已作废，或它的终点本来就是写明的，返回 `None`
pub async fn state_derived_end(
    pool: &PgPool,
    fact_id: Uuid,
    valid_to: Option<(DateTime<Utc>, &str)>,
    attested_at: Option<DateTime<Utc>>,
) -> AppResult<Option<Uuid>> {
    let end = match valid_to {
        Some((at, precision)) => End::At(
            crate::graph::truncate_to(at, Some(precision)),
            precision.to_string(),
        ),
        None => End::Unknown(attested_at),
    };
    let mut tx = pool.begin().await?;
    let derived: Option<bool> = sqlx::query_scalar(
        "SELECT end_derived FROM facts WHERE id = $1 AND invalidated_at IS NULL FOR UPDATE",
    )
    .bind(fact_id)
    .fetch_optional(&mut *tx)
    .await?;
    let corrected = match derived {
        Some(true) => rewrite_end_tx(&mut tx, fact_id, &end, false, false).await?,
        _ => None,
    };
    tx.commit().await?;
    Ok(corrected)
}

/// 作废 + 改写一行的终点。`derived`：这个终点是引擎按时间线推出来的（0057），之后随
/// 时间线重算；`require_open`：只改还开着的行。
///
/// 先 `FOR UPDATE` 锁住旧行：并行的两次改写只有一次看得见它还活着，另一次拿到 `None`，
/// 不会各插一条修正行；往这一行上添证据的也要等它（`graph::add_evidence`），证据不会落在
/// 刚被作废的旧行上。还没人裁的冲突跟着换到修正行上：问题还在，只是这一行换了终点
async fn rewrite_end_tx(
    tx: &mut Transaction<'_, Postgres>,
    fact_id: Uuid,
    end: &End,
    derived: bool,
    require_open: bool,
) -> AppResult<Option<Uuid>> {
    let alive: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM facts
         WHERE id = $1 AND invalidated_at IS NULL
           AND (NOT $2 OR (valid_to IS NULL AND valid_to_precision IS NULL))
         FOR UPDATE",
    )
    .bind(fact_id)
    .bind(require_open)
    .fetch_optional(&mut **tx)
    .await?;
    if alive.is_none() {
        return Ok(None);
    }
    let (valid_to, precision, anchor) = match end {
        End::Open => (None, None, None),
        End::At(at, precision) => (Some(*at), Some(precision.as_str()), None),
        End::Unknown(anchor) => (None, Some(crate::graph::ENDED_UNKNOWN), *anchor),
    };
    let corrected = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                            valid_from, valid_from_precision,
                            valid_to, valid_to_precision, confidence, supersedes,
                            attested_from, attested_to, end_derived)
         SELECT $1, kb_id, subject_id, predicate_id, object_id, object_value,
                valid_from, valid_from_precision, $3, $4, confidence, id,
                attested_from, CASE WHEN $4::text = 'unknown' THEN COALESCE($5, now()) END, $6
         FROM facts WHERE id = $2",
    )
    .bind(corrected)
    .bind(fact_id)
    .bind(valid_to)
    .bind(precision)
    .bind(anchor)
    .bind(derived)
    .execute(&mut **tx)
    .await?;
    carry_open_conflicts(tx, fact_id, corrected).await?;
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(fact_id)
        .execute(&mut **tx)
        .await?;
    copy_evidence(tx, fact_id, corrected).await?;
    copy_qualifiers(tx, fact_id, corrected).await?;
    Ok(Some(corrected))
}

/// 作废 + 改写一行的持有者：主语或宾语换成另一个实体，其余照旧（证据、边上的属性随行）。
/// 撤回合并把合并之后才改写出来的行送回源实体时用——原地改主语，记录轴回放合并窗口时
/// 就找不到它当时挂在哪（0027 只认合并账本上的行）。返回新行 id；`None` = 已作废
pub async fn rehome_tx(
    tx: &mut Transaction<'_, Postgres>,
    fact_id: Uuid,
    subject_id: Option<Uuid>,
    object_id: Option<Uuid>,
) -> AppResult<Option<Uuid>> {
    let alive: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM facts WHERE id = $1 AND invalidated_at IS NULL FOR UPDATE")
            .bind(fact_id)
            .fetch_optional(&mut **tx)
            .await?;
    if alive.is_none() {
        return Ok(None);
    }
    let moved = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                            valid_from, valid_from_precision, valid_to, valid_to_precision,
                            confidence, derived_by_rule, supersedes,
                            attested_from, attested_to, end_derived)
         SELECT $1, kb_id, COALESCE($3, subject_id), predicate_id, COALESCE($4, object_id),
                object_value, valid_from, valid_from_precision, valid_to, valid_to_precision,
                confidence, derived_by_rule, id, attested_from, attested_to, end_derived
         FROM facts WHERE id = $2",
    )
    .bind(moved)
    .bind(fact_id)
    .bind(subject_id)
    .bind(object_id)
    .execute(&mut **tx)
    .await?;
    // 开着的冲突先换到新行上：旧行一作废，0051 的触发器就把它们撤下，之后重算也不会
    // 再记同一天开始的那一对——撤回完两行叠在一起，审核队列却是空的（#679 第四轮评审）
    carry_open_conflicts(tx, fact_id, moved).await?;
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(fact_id)
        .execute(&mut **tx)
        .await?;
    copy_evidence(tx, fact_id, moved).await?;
    copy_qualifiers(tx, fact_id, moved).await?;
    Ok(Some(moved))
}

/// 还开着的冲突从旧行换到修正行上（旧行一作废，0051 的触发器就会把它们撤下）。
/// 修正行上已经有同一对的，留着旧的那条随作废撤下
async fn carry_open_conflicts(
    tx: &mut Transaction<'_, Postgres>,
    from: Uuid,
    to: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE fact_conflicts c SET old_fact_id = $2
          WHERE c.status = 'open' AND c.old_fact_id = $1
            AND NOT EXISTS (SELECT 1 FROM fact_conflicts d
                             WHERE d.old_fact_id = $2 AND d.new_fact_id = c.new_fact_id)",
    )
    .bind(from)
    .bind(to)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE fact_conflicts c SET new_fact_id = $2
          WHERE c.status = 'open' AND c.new_fact_id = $1
            AND NOT EXISTS (SELECT 1 FROM fact_conflicts d
                             WHERE d.new_fact_id = $2 AND d.old_fact_id = c.old_fact_id)",
    )
    .bind(from)
    .bind(to)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 边上的属性随修正行复制（0037）：纠正的是时间区间，边上的金额、职务照旧——
/// 不搬的话，闭合一段任职就丢了它的职务
async fn copy_qualifiers(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    from: Uuid,
    to: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO fact_qualifiers (fact_id, qualifier_type_id, value, entity_id)
         SELECT $1, qualifier_type_id, value, entity_id
         FROM fact_qualifiers WHERE fact_id = $2
         ON CONFLICT DO NOTHING",
    )
    .bind(to)
    .bind(from)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 证据引用随修正行复制。表层谓词一起搬：纠正的是时间区间，不是原文说了什么。
async fn copy_evidence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    from: Uuid,
    to: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, proposed_predicate, document_id, doc_version)
         SELECT $1, chunk_id, quote, proposed_predicate, document_id, doc_version
         FROM fact_evidence WHERE fact_id = $2
         ON CONFLICT DO NOTHING",
    )
    .bind(to)
    .bind(from)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 人工修正一条事实的有效区间。与自动闭合同一机制（作废 + 改写），区别只在
/// 两端都来自参数，而不是继承旧行的起点。
///
/// 抽取把「2023 年上半年」读成 1 月 1 日，在此之前只能删掉文档重抽一遍：
/// 名字判错有 Review 可改，时间判错没有入口，而整条时间线都歪在那一个值上。
///
/// **不原地 UPDATE。** 原地改会把修正本身抹掉，而那正是记录轴要回放的东西
/// （0019）——改过之后，问三月与问九月应当得到不同的区间。这也是这个函数
/// 与一条 `UPDATE facts SET valid_from = …` 的全部差别。
///
/// 返回修正行 id；`None` 表示这条已被并发改写或作废，本次没有动手。
pub async fn correct_interval(
    pool: &PgPool,
    fact_id: Uuid,
    validity: crate::graph::Validity<'_>,
) -> AppResult<Option<Uuid>> {
    // 人改区间也按谓词的时间语义归一（0031）：给一个事件填了一段，落下的仍是它的那一刻
    let predicate: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT predicate_id FROM facts WHERE id = $1")
            .bind(fact_id)
            .fetch_optional(pool)
            .await?;
    let temporal = crate::graph::predicate_temporal(pool, predicate.flatten()).await?;
    let validity = validity.under(temporal).truncated();
    let mut tx = pool.begin().await?;
    let corrected = Uuid::now_v7();
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                            valid_from, valid_from_precision,
                            valid_to, valid_to_precision, confidence, supersedes,
                            attested_from, attested_to)
         SELECT $1, kb_id, subject_id, predicate_id, object_id, object_value,
                $3, $4, $5, $6, confidence, id,
                attested_from,
                CASE WHEN $6::text = 'unknown' THEN COALESCE(attested_to, now()) END
         FROM facts WHERE id = $2 AND invalidated_at IS NULL
         RETURNING id",
    )
    .bind(corrected)
    .bind(fact_id)
    .bind(validity.from)
    .bind(validity.from_precision)
    .bind(validity.to)
    .bind(validity.to_precision)
    .fetch_optional(&mut *tx)
    .await?;
    // 已被并发修正过：不重复动手（与 close_superseded 同一防线）
    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;
    copy_evidence(&mut tx, fact_id, corrected).await?;
    copy_qualifiers(&mut tx, fact_id, corrected).await?;
    tx.commit().await?;
    Ok(Some(corrected))
}

/// 记一对冲突。返回这一对是不是第一次记：时间线每次重算都会再看见同一对，
/// 已经在队列里（或人已经裁过）的不再算作新冲突
async fn record_conflict_tx(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    old_fact_id: Uuid,
    new_fact_id: Uuid,
    reason: &str,
) -> AppResult<bool> {
    // 人已经裁过「两个都留着」的，不因为两行后来被改写（换了 id）就再问一遍：
    // 顺着 supersedes 往上找两行的前身，那一对裁过就不记（#679 第三轮评审）
    let inserted = sqlx::query(
        "INSERT INTO fact_conflicts (id, kb_id, old_fact_id, new_fact_id, reason)
         SELECT $1, $2, $3, $4, $5
         WHERE NOT EXISTS (
             WITH RECURSIVE a(id) AS (
                     SELECT $3::uuid
                     UNION SELECT f.supersedes FROM facts f JOIN a ON f.id = a.id
                      WHERE f.supersedes IS NOT NULL),
                 b(id) AS (
                     SELECT $4::uuid
                     UNION SELECT f.supersedes FROM facts f JOIN b ON f.id = b.id
                      WHERE f.supersedes IS NOT NULL)
             SELECT 1 FROM fact_conflicts c
              WHERE c.status = 'resolved' AND c.resolution = 'kept_both'
                AND ((c.old_fact_id IN (SELECT id FROM a) AND c.new_fact_id IN (SELECT id FROM b))
                  OR (c.old_fact_id IN (SELECT id FROM b) AND c.new_fact_id IN (SELECT id FROM a))))
           -- 同一对反过来记过的也算记过：重算按时间线顺序看，谁是「旧」谁是「新」
           -- 随到达顺序变，不去重的话一次全量重算会翻出一堆方向相反的重复
           AND NOT EXISTS (SELECT 1 FROM fact_conflicts r
                            WHERE r.old_fact_id = $4 AND r.new_fact_id = $3 AND r.status = 'open')
         ON CONFLICT (old_fact_id, new_fact_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(old_fact_id)
    .bind(new_fact_id)
    .bind(reason)
    .execute(&mut **tx)
    .await?;
    Ok(inserted.rows_affected() > 0)
}

/// Review 页的冲突列表（双方事实带名字与区间）。
/// 惰性清理：任一方已被作废（被驳回/被别的闭合改写）的冲突已无意义，
/// 自动出队标 stale——防止在僵尸冲突上误裁（如把 Eve 闭合在已驳回的 Ivan 上）。
pub async fn list_conflicts(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<ConflictView>> {
    // **这里从前有一条 UPDATE。**读之前先把「有一边已经作废」的冲突改成
    // resolved / stale / now()——清理是对的，位置是错的：`resolved_at` 于是记的是
    // 有人打开这一页的时刻，而没人打开的库里陈冲突永远开着。退场现在钉在作废
    // 发生的地方（0051 的触发器），这里只读
    let rows: Vec<ConflictView> = sqlx::query_as(
        "SELECT c.id, c.reason, c.created_at, r.label AS predicate_label,
                c.old_fact_id, os.canonical_name AS old_subject,
                oo.canonical_name AS old_object, fo.valid_from AS old_valid_from,
                c.new_fact_id, ns.canonical_name AS new_subject,
                no_.canonical_name AS new_object, fn_.valid_from AS new_valid_from,
                fn_.confidence AS new_confidence
         FROM fact_conflicts c
         JOIN facts fo ON fo.id = c.old_fact_id
         JOIN facts fn_ ON fn_.id = c.new_fact_id
         JOIN entities os ON os.id = fo.subject_id
         JOIN entities ns ON ns.id = fn_.subject_id
         JOIN relation_types r ON r.id = fo.predicate_id
         LEFT JOIN entities oo ON oo.id = fo.object_id
         LEFT JOIN entities no_ ON no_.id = fn_.object_id
         WHERE c.kb_id = $1 AND c.status = 'open'
         ORDER BY c.created_at DESC, c.id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 人工裁决：close（旧事实闭合于 close_at 或新事实起点）/ keep（并存不矛盾）/
/// reject_new（新事实是抽取错误，作废）。回 close 改写出来的那一行（撤回要用）
/// 一条待裁决的冲突：旧事实、新事实、新事实的起点及其精度
type ConflictRow = (Uuid, Uuid, Option<DateTime<Utc>>, Option<String>);

pub async fn resolve_conflict(
    pool: &PgPool,
    kb_id: Uuid,
    conflict_id: Uuid,
    resolution: &str,
    close_at: Option<DateTime<Utc>>,
    close_at_precision: &str,
) -> AppResult<Option<Uuid>> {
    let row: Option<ConflictRow> = sqlx::query_as(
        "SELECT c.old_fact_id, c.new_fact_id, fn_.valid_from, fn_.valid_from_precision
         FROM fact_conflicts c JOIN facts fn_ ON fn_.id = c.new_fact_id
         WHERE c.id = $1 AND c.kb_id = $2 AND c.status = 'open'",
    )
    .bind(conflict_id)
    .bind(kb_id)
    .fetch_optional(pool)
    .await?;
    let Some((old_fact_id, new_fact_id, new_from, new_from_precision)) = row else {
        return Err(utopia_core::AppError::NotFound);
    };

    let mut corrected = None;
    let stored = match resolution {
        "close" => {
            // 闭合点带着它的精度走：人给了日期就用人给的精度，没给就闭合在新事实的
            // 起点——那个起点是几月还是几号，闭合点就是几月还是几号。从前一律写 day，
            // 「2023 年 6 月接任」把前任闭合成了 6 月 1 日
            let (at, precision) = match close_at {
                Some(at) => (at, close_at_precision),
                None => (
                    new_from.ok_or_else(|| {
                        utopia_core::AppError::invalid(
                            "close_at_required",
                            "close_at is required when the new fact has no start time",
                        )
                    })?,
                    new_from_precision.as_deref().unwrap_or("day"),
                ),
            };
            corrected = close_superseded(pool, old_fact_id, at, precision).await?;
            "closed"
        }
        "keep" => "kept_both",
        "reject_new" => {
            retract(pool, kb_id, new_fact_id).await?;
            // 波及：同一新事实撞出的其他 open 冲突一并出队（新事实已死，无从裁起）
            sqlx::query(
                "UPDATE fact_conflicts
                 SET status = 'resolved', resolution = 'rejected_new', resolved_at = now()
                 WHERE new_fact_id = $1 AND status = 'open' AND id <> $2",
            )
            .bind(new_fact_id)
            .bind(conflict_id)
            .execute(pool)
            .await?;
            "rejected_new"
        }
        other => {
            return Err(utopia_core::AppError::Validation(format!(
                "Unknown resolution: {other}"
            )))
        }
    };
    sqlx::query(
        "UPDATE fact_conflicts SET status = 'resolved', resolution = $3, resolved_at = now()
         WHERE id = $1 AND kb_id = $2",
    )
    .bind(conflict_id)
    .bind(kb_id)
    .bind(stored)
    .execute(pool)
    .await?;
    Ok(corrected)
}

/// 一条谓词的一端挂着**两个以上开放值**的持有者——唯一性没声明（或声明来晚了）
/// 时账本的样子（#341）。这是给人看的提议依据，不是判决：`declared` 为假时它说
/// "这里像是该声明的"，为真时它说"声明了，但这些行还没对过账"。
#[derive(Debug, Clone)]
pub struct UniquenessCandidate {
    pub predicate_id: Uuid,
    pub key: String,
    pub label: String,
    /// relation | attribute
    pub kind: String,
    /// "subject"（→ functional）或 "object"（→ inverse_functional）
    pub side: &'static str,
    /// 这一端的唯一性是否已经声明
    pub declared: bool,
    /// 挂着两个以上开放值的持有者数
    pub holders: usize,
    /// 那些持有者身上的开放事实数
    pub open_facts: usize,
    /// 对账会闭合的区间数（估算，按落库时的规则：起点更早者止于后任起点）
    pub would_close: usize,
    /// 对账会送进人审的对数（缺时间 / 同时开始 / 低置信）
    pub would_review: usize,
    /// 头几个持有者与它们的开放值，按年表排
    pub examples: Vec<HolderExample>,
}

#[derive(Debug, Clone)]
pub struct HolderExample {
    pub holder: String,
    pub values: Vec<OpenValue>,
}

#[derive(Debug, Clone)]
pub struct OpenValue {
    pub fact_id: Uuid,
    pub name: String,
    pub valid_from: Option<DateTime<Utc>>,
    pub confidence: f32,
}

/// 每个候选带几个例子。
const EXAMPLE_HOLDERS: usize = 3;

pub async fn uniqueness_candidates(
    pool: &PgPool,
    kb_id: Uuid,
) -> AppResult<Vec<UniquenessCandidate>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        predicate_id: Uuid,
        key: String,
        label: String,
        kind: String,
        declared: bool,
        holder_name: String,
        other_name: Option<String>,
        object_value: Option<serde_json::Value>,
        valid_from: Option<DateTime<Utc>>,
        confidence: f32,
    }
    // 两端各查一遍。`crowded` 先按 (谓词, 持有者) 数不同的值，再把那些持有者
    // 身上的开放事实整批取回来——估算要看每一对相邻值的起点与置信度，
    // 光有计数不够。开放 = 世界轴没有终点，与落库时对账用的是同一个不变量
    let subject_side: Vec<Row> = sqlx::query_as(
        "WITH open AS (
             SELECT f.id, f.predicate_id, f.subject_id AS holder, f.object_id, f.object_value,
                    COALESCE(f.object_id::text, f.object_value::text) AS value_key,
                    f.valid_from, f.confidence, f.recorded_at
             FROM facts f JOIN relation_types r ON r.id = f.predicate_id
             WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
               AND f.valid_to IS NULL AND f.valid_to_precision IS NULL
               AND r.temporal = 'state'
               -- 名字不算：一个实体有两个名字是常态，不是「这个属性该唯一」的证据（0041）
               AND NOT (r.builtin AND r.key = 'known_as')
               AND (f.object_id IS NOT NULL OR f.object_value IS NOT NULL)
         ),
         crowded AS (
             SELECT predicate_id, holder FROM open
             GROUP BY predicate_id, holder HAVING count(DISTINCT value_key) >= 2
         )
         SELECT o.id, o.predicate_id, r.key, r.label, r.kind, r.functional AS declared,
                h.canonical_name AS holder_name, e.canonical_name AS other_name,
                o.object_value, o.valid_from, o.confidence
         FROM open o
         JOIN crowded c ON c.predicate_id = o.predicate_id AND c.holder = o.holder
         JOIN relation_types r ON r.id = o.predicate_id
         JOIN entities h ON h.id = o.holder
         LEFT JOIN entities e ON e.id = o.object_id
         ORDER BY r.key, h.canonical_name, o.valid_from ASC NULLS LAST, o.recorded_at ASC",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    let object_side: Vec<Row> = sqlx::query_as(
        "WITH open AS (
             SELECT f.id, f.predicate_id, f.object_id AS holder, f.subject_id,
                    f.valid_from, f.confidence, f.recorded_at
             FROM facts f JOIN relation_types r ON r.id = f.predicate_id
             WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
               AND f.valid_to IS NULL AND f.valid_to_precision IS NULL
               AND r.temporal = 'state' AND r.kind = 'relation'
               AND f.object_id IS NOT NULL
         ),
         crowded AS (
             SELECT predicate_id, holder FROM open
             GROUP BY predicate_id, holder HAVING count(DISTINCT subject_id) >= 2
         )
         SELECT o.id, o.predicate_id, r.key, r.label, r.kind, r.inverse_functional AS declared,
                h.canonical_name AS holder_name, e.canonical_name AS other_name,
                NULL::jsonb AS object_value, o.valid_from, o.confidence
         FROM open o
         JOIN crowded c ON c.predicate_id = o.predicate_id AND c.holder = o.holder
         JOIN relation_types r ON r.id = o.predicate_id
         JOIN entities h ON h.id = o.holder
         JOIN entities e ON e.id = o.subject_id
         ORDER BY r.key, h.canonical_name, o.valid_from ASC NULLS LAST, o.recorded_at ASC",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::new();
    for (side, rows) in [("subject", subject_side), ("object", object_side)] {
        // 行已按 (谓词, 持有者, 年表) 排好，顺着切段就是分组
        let mut i = 0;
        while i < rows.len() {
            let pred = rows[i].predicate_id;
            let mut cand = UniquenessCandidate {
                predicate_id: pred,
                key: rows[i].key.clone(),
                label: rows[i].label.clone(),
                kind: rows[i].kind.clone(),
                side,
                declared: rows[i].declared,
                holders: 0,
                open_facts: 0,
                would_close: 0,
                would_review: 0,
                examples: Vec::new(),
            };
            while i < rows.len() && rows[i].predicate_id == pred {
                let holder = rows[i].holder_name.clone();
                let mut values = Vec::new();
                while i < rows.len()
                    && rows[i].predicate_id == pred
                    && rows[i].holder_name == holder
                {
                    let r = &rows[i];
                    values.push(OpenValue {
                        fact_id: r.id,
                        name: r
                            .other_name
                            .clone()
                            .or_else(|| r.object_value.as_ref().map(literal_name))
                            .unwrap_or_else(|| "?".to_string()),
                        valid_from: r.valid_from,
                        confidence: r.confidence,
                    });
                    i += 1;
                }
                let (close, review) = plan_closures(&values);
                cand.holders += 1;
                cand.open_facts += values.len();
                cand.would_close += close;
                cand.would_review += review;
                if cand.examples.len() < EXAMPLE_HOLDERS {
                    cand.examples.push(HolderExample { holder, values });
                }
            }
            out.push(cand);
        }
    }
    Ok(out)
}

/// 一个持有者的开放值按年表排好后，对账会怎么处置：(闭合数, 进人审数)。
///
/// 把 `reconcile_predicate` 的过程干跑一遍，不落库：逐条当"新落库的观察"，与
/// 后面还开着的每一条比——起点更早者止于后任起点（自己置信度够才许改写历史）；
/// 两条都没起点、或同一天开始，说不清谁接替谁，进人审，两条都还开着；后任没起点
/// 的，它止于前任的起点。三条都没起点的持有者会报三对冲突，与引擎一致。
/// 这是估算——真跑一遍的结果才作数
fn plan_closures(values: &[OpenValue]) -> (usize, usize) {
    let mut open = vec![true; values.len()];
    let mut close = 0;
    let mut review = 0;
    for i in 0..values.len() {
        if !open[i] {
            continue;
        }
        let new = &values[i];
        for j in (i + 1)..values.len() {
            if !open[j] {
                continue;
            }
            let old = &values[j];
            match (old.valid_from, new.valid_from) {
                (_, None) => review += 1,
                (Some(of), Some(nf)) if of == nf => review += 1,
                (Some(of), Some(nf)) if nf < of => {
                    if new.confidence < AUTO_CLOSE_MIN_CONFIDENCE {
                        review += 1;
                    } else {
                        close += 1;
                        open[i] = false;
                        break;
                    }
                }
                (_, Some(_)) => {
                    if new.confidence < AUTO_CLOSE_MIN_CONFIDENCE {
                        review += 1;
                    } else {
                        close += 1;
                        open[j] = false;
                    }
                }
            }
        }
    }
    (close, review)
}

/// 字面值给人看的样子：`{value, unit}` → "28000 CNY"，裸标量读成它自己。
fn literal_name(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) => {
            let value = match o.get("value") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => return v.to_string(),
            };
            match o
                .get("unit")
                .and_then(|u| u.as_str())
                .filter(|u| !u.is_empty())
            {
                Some(unit) => format!("{value} {unit}"),
                None => value,
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn v(from: Option<(i32, u32, u32)>, confidence: f32) -> OpenValue {
        OpenValue {
            fact_id: Uuid::now_v7(),
            name: String::new(),
            valid_from: from.map(|(y, m, d)| Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()),
            confidence,
        }
    }

    fn day(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    /// 时间线上的一行：值、起点、没起点时自带日期的证据
    fn row(value: &str, from: Option<DateTime<Utc>>, dated: Option<DateTime<Utc>>) -> Row {
        Row {
            id: Uuid::now_v7(),
            subject_id: Uuid::nil(),
            object_id: None,
            object_value: Some(serde_json::json!({ "value": value })),
            valid_from: from,
            valid_from_precision: from.map(|_| "day".to_string()),
            valid_to: None,
            valid_to_precision: None,
            attested_to: None,
            confidence: 0.9,
            end_derived: false,
            dated_at: dated,
        }
    }

    /// 把计划落到行上（改写成新 id 的事与此无关，原地换终点），直到计划为空
    fn settle(rows: &mut [Row]) {
        let plan = plan_timeline(Uniqueness::SubjectSide, rows);
        for (id, end) in plan.ends {
            let r = rows.iter_mut().find(|r| r.id == id).unwrap();
            r.end_derived = end != End::Open;
            (r.valid_to, r.valid_to_precision, r.attested_to) = match end {
                End::Open => (None, None, None),
                End::At(t, p) => (Some(t), Some(p), None),
                End::Unknown(a) => (None, Some("unknown".into()), a),
            };
        }
        assert_eq!(
            plan_timeline(Uniqueness::SubjectSide, rows).ends,
            vec![],
            "一次算完就是最终的样子"
        );
    }

    fn shape(rows: &[Row]) -> Vec<(String, End)> {
        let mut out: Vec<_> = rows
            .iter()
            .map(|r| {
                (
                    r.object_value.as_ref().unwrap()["value"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                    r.current_end(),
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// 同一批行，不论以什么顺序排进来，算出来的时间线一样
    #[test]
    fn a_timeline_does_not_depend_on_the_order_its_rows_arrive() {
        let base = vec![
            row("v1", Some(day(2020, 2, 18)), None),
            row("v2", Some(day(2020, 3, 17)), None),
            row("v3", None, Some(day(2020, 4, 14))),
            row("v4", Some(day(2020, 5, 26)), None),
            row("v5", None, Some(day(2020, 6, 8))),
        ];
        let mut first = base.clone();
        settle(&mut first);
        assert_eq!(
            shape(&first),
            vec![
                ("v1".into(), End::At(day(2020, 3, 17), "day".into())),
                ("v2".into(), End::Unknown(Some(day(2020, 4, 14)))),
                ("v3".into(), End::At(day(2020, 5, 26), "day".into())),
                ("v4".into(), End::Unknown(Some(day(2020, 6, 8)))),
                ("v5".into(), End::Open),
            ]
        );
        for rotation in 1..base.len() {
            let mut rows = base.clone();
            rows.rotate_left(rotation);
            rows.reverse();
            settle(&mut rows);
            assert_eq!(shape(&rows), shape(&first));
        }
    }

    /// 后面那一段挪了、走了，引擎画的终点跟着变；写明的终点不动
    #[test]
    fn a_derived_end_follows_what_comes_next_and_a_stated_one_stays() {
        let mut rows = vec![
            row("A", Some(day(2016, 5, 16)), None),
            row("B", None, Some(day(2020, 8, 13))),
        ];
        settle(&mut rows);
        assert_eq!(rows[0].current_end(), End::Unknown(Some(day(2020, 8, 13))));
        // 后来的文件说 B 从 7 月 31 日起：A 止于那一天
        rows[1].valid_from = Some(day(2020, 7, 31));
        rows[1].valid_from_precision = Some("day".into());
        settle(&mut rows);
        assert_eq!(
            rows[0].current_end(),
            End::At(day(2020, 7, 31), "day".into())
        );
        // B 被撤回：A 重新开着
        rows.pop();
        settle(&mut rows);
        assert_eq!(rows[0].current_end(), End::Open);
        assert!(!rows[0].end_derived);

        // 原文写明 C 到 2019 年底，后面的 D 从 2019 年 6 月起：两份原文说法相左，不替人挑
        let mut stated = vec![
            row("C", Some(day(2018, 1, 1)), None),
            row("D", Some(day(2019, 6, 1)), None),
        ];
        stated[0].valid_to = Some(day(2019, 12, 31));
        stated[0].valid_to_precision = Some("day".into());
        assert_eq!(
            plan_timeline(Uniqueness::SubjectSide, &stated),
            Plan::default()
        );
    }

    /// 合并前的闸门：两个值能排成先后接替就不算叠在一起；排不开的才算
    #[test]
    fn two_values_overlap_only_when_the_engine_cannot_order_them() {
        let succession = vec![
            row("HPBB1", Some(day(2016, 5, 16)), None),
            row("BBHQ1", None, Some(day(2020, 8, 13))),
        ];
        assert!(!still_overlaps(Uniqueness::SubjectSide, &succession, true));

        let same_day = vec![
            row("P", Some(day(2020, 1, 1)), None),
            row("Q", Some(day(2020, 1, 1)), None),
        ];
        assert!(still_overlaps(Uniqueness::SubjectSide, &same_day, true));

        let no_time = vec![row("K", None, None), row("N", Some(day(2020, 1, 1)), None)];
        assert!(still_overlaps(Uniqueness::SubjectSide, &no_time, true));

        // 两份原文写明的区间相交：引擎不替人挑
        let mut stated = vec![
            row("C", Some(day(2018, 1, 1)), None),
            row("D", Some(day(2019, 6, 1)), None),
        ];
        stated[0].valid_to = Some(day(2019, 12, 31));
        stated[0].valid_to_precision = Some("day".into());
        assert!(still_overlaps(Uniqueness::SubjectSide, &stated, true));

        // 事件与恒常不经引擎排：两个说不出时间的值照旧算撞
        let eternal = vec![row("X", None, None), row("Y", None, None)];
        assert!(still_overlaps(Uniqueness::SubjectSide, &eternal, false));
    }

    /// 置信度不够的后任不许关上前任，交给人；前任止于它之后第一个够格的后任
    #[test]
    fn a_doubtful_successor_is_held_and_the_next_sure_one_closes() {
        let mut rows = vec![
            row("A", Some(day(2020, 1, 1)), None),
            row("B", Some(day(2020, 2, 1)), None),
            row("C", Some(day(2020, 3, 1)), None),
        ];
        rows[1].confidence = 0.5;
        let plan = plan_timeline(Uniqueness::SubjectSide, &rows);
        assert_eq!(
            plan.ends,
            vec![
                (rows[0].id, End::At(day(2020, 3, 1), "day".into())),
                (rows[1].id, End::At(day(2020, 3, 1), "day".into())),
            ]
        );
        assert_eq!(plan.held, vec![(rows[0].id, rows[1].id)]);
    }

    #[test]
    fn a_chain_of_three_closes_twice() {
        let values = [
            v(Some((2023, 2, 1)), 0.9),
            v(Some((2024, 7, 5)), 0.9),
            v(Some((2025, 9, 1)), 0.9),
        ];
        assert_eq!(plan_closures(&values), (2, 0));
    }

    #[test]
    fn what_the_engine_would_not_close_goes_to_review() {
        // 同一天开始
        assert_eq!(
            plan_closures(&[v(Some((2024, 1, 1)), 0.9), v(Some((2024, 1, 1)), 0.9)]),
            (0, 1)
        );
        // 两条都没起点
        assert_eq!(plan_closures(&[v(None, 0.9), v(None, 0.9)]), (0, 1));
        // 起点更早的那条置信度不够，不许它改写历史
        assert_eq!(
            plan_closures(&[v(Some((2023, 1, 1)), 0.5), v(Some((2024, 1, 1)), 0.9)]),
            (0, 1)
        );
        // 后任没起点：它止于前任的起点（落库时的"旧事实无起点也适用"）
        assert_eq!(
            plan_closures(&[v(Some((2023, 1, 1)), 0.9), v(None, 0.9)]),
            (1, 0)
        );
        // 一条不成对
        assert_eq!(plan_closures(&[v(Some((2023, 1, 1)), 0.9)]), (0, 0));
        // 三条都没起点：每一对都说不清，三对冲突，与引擎一致
        assert_eq!(
            plan_closures(&[v(None, 0.9), v(None, 0.9), v(None, 0.9)]),
            (0, 3)
        );
        // 有起点的一条闭合两条没起点的：它们都止于它的起点
        assert_eq!(
            plan_closures(&[v(Some((2023, 1, 1)), 0.9), v(None, 0.9), v(None, 0.9)]),
            (2, 0)
        );
    }

    #[test]
    fn a_literal_reads_as_a_value_with_its_unit() {
        assert_eq!(
            literal_name(&serde_json::json!({ "value": 28000, "unit": "CNY" })),
            "28000 CNY"
        );
        assert_eq!(
            literal_name(&serde_json::json!({ "value": "Staff Engineer" })),
            "Staff Engineer"
        );
        assert_eq!(literal_name(&serde_json::json!(32000)), "32000");
        assert_eq!(literal_name(&serde_json::json!("plain")), "plain");
    }
}
