//! 图谱仓储：本体、实体消解（P2 第一刀：同 KB 同类型同名合一）、事实账本、图查询。

use sqlx::PgPool;
use std::collections::HashMap;
use std::collections::HashSet;
use utopia_core::models::{
    ChunkFactView, EntityFact, EntityHistoryEvent, EntityType, EvidenceView, FactQualifier,
    FactReviewItem, GraphChange, GraphEdge, GraphNode, RelationType,
};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 同断言已有事实的行投影：(id, valid_from, valid_to)。
type FactSpanRow = (
    Uuid,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
);

// 建库不再播种任何关系，也不再有 `ensure_default_ontology`。
//
// 这里曾经有十条种子关系、一张中文措辞表、一个按语言取措辞的 `localized`，
// 以及一个在建库 / 首次读本体 / 每次抽取前都会跑一遍的播种函数。它们分三次退场：
//
// - `related_to`（0010）：代码层面的兜底，摆进提示词就成了逃生舱
// - 另外八条（`#125`）：零个带签名、装了本体包也不会被同名顶替
//   （`worksFor` 与 `works_at` 的 key 对不上，于是并存成两条边）、
//   而且公理位恒为 false，一致性检查在它们上面永远查不出矛盾
// - `mapped_to`（0011）：它是「这个数怎么算」不是「世界上有什么」，
//   已搬去 `concept_mappings`
//
// 剩下的那个函数于是只是在遍历一张空表。**本体从建库第一天起就只有
// 用户自己导入的词表**——与 0009 删掉内置实体类是同一件事的下半段。

/// 库里全部类。执行器取泛型：类别词对齐要在调模型之前的同一个快照事务里读它（#795）。
pub async fn entity_types<'e>(
    pool: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    kb_id: Uuid,
) -> AppResult<Vec<EntityType>> {
    Ok(
        // 又一次 SELECT *：parents 在关联表里，`*` 取不到。
        // 这是同一个陷阱的第三次——SQL 在字符串里，cargo check 全绿，
        // 第一个请求才报 no column found
        sqlx::query_as(
            "SELECT t.*,
                    ARRAY(SELECT p.parent_id FROM entity_type_parents p
                          WHERE p.child_id = t.id) AS parents,
                    (SELECT p.parent_id FROM entity_type_parents p
                      WHERE p.child_id = t.id AND p.is_primary) AS primary_parent
             FROM entity_types t WHERE t.kb_id = $1 ORDER BY t.created_at",
        )
        .bind(kb_id)
        .fetch_all(pool)
        .await?,
    )
}

pub async fn relation_types(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<RelationType>> {
    // 不用 SELECT *：domain/range 在关联表里，`*` 取不到，
    // 而且 sqlx 要到运行时才会说 "no column found" —— 编译器看不见 SQL 字符串
    Ok(sqlx::query_as(
        "SELECT r.*,
                ARRAY(SELECT d.entity_type_id FROM relation_type_domains d
                      WHERE d.relation_type_id = r.id) AS domains,
                ARRAY(SELECT g.entity_type_id FROM relation_type_ranges g
                      WHERE g.relation_type_id = r.id) AS ranges,
                ARRAY(SELECT q.qualifier_type_id FROM relation_type_qualifiers q
                      WHERE q.relation_type_id = r.id) AS qualifiers
         FROM relation_types r WHERE r.kb_id = $1 ORDER BY r.created_at",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 写入事实。返回 (事实 id, 是否新建)。
///
/// 同断言（同主谓宾）的多次观察不各立门户：
/// - 同 valid_from 的 live 行已存在 → 复用（证据累积到同一条）
/// - 新观察**没带时间**、同断言已有开放行 → 弱化陈述并入已有行（"隶属星云科技"
///   并进"2021-02 起隶属星云科技"，不再产生一条无时间的重复）
/// - 新观察**带了时间**、同断言已有的是无时无终的裸行 → 时间精化：新行落库后
///   把裸行作废并以 supersedes 链上（作废+改写，认知史完整），证据随行复制
/// - 双方都带时间但不同 → 保守并存（可能真是两段区间，如离职又回归）
///
/// 事实的宾语：实体（关系）或字面值（属性/问数映射）。同一套折并与时间精化逻辑。
#[derive(Debug, Clone, Copy)]
pub enum FactObject<'a> {
    Entity(Uuid),
    Value(&'a serde_json::Value),
}

/// 往一条边上写一个属性值的结果（0037）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualifierWrite {
    /// 这条边之前没有这个属性，写上了
    Set,
    /// 已经有了、值一样：再一次观察，什么都不用改
    Same,
    /// 已经有了、值**不一样**。不覆盖——先写者留着，调用方记一笔让人看见。
    /// 账本里两次观察不一致从来是两行 + 一条冲突，这里还没走到另立一行那一步
    Conflict,
}

/// 往一条边上写一个字面值属性。**属性不进事实的去重键**：同一条边再听到一次带了
/// 金额的，是同一条边补上金额，不是第二条边。
/// 两个属性值是不是同一个：数按数比（`65` 与 `65.0` 是同一个数——老库里存着整数，
/// 新写的是浮点），其余按结构比
fn qualifier_values_agree(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Number(x), serde_json::Value::Number(y)) => x.as_f64() == y.as_f64(),
        (serde_json::Value::Object(x), serde_json::Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| qualifier_values_agree(v, w)))
        }
        (serde_json::Value::Array(x), serde_json::Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| qualifier_values_agree(v, w))
        }
        _ => a == b,
    }
}

pub async fn upsert_fact_qualifier(
    pool: &PgPool,
    fact_id: Uuid,
    qualifier_type_id: Uuid,
    value: &serde_json::Value,
) -> AppResult<QualifierWrite> {
    let existing: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT value FROM fact_qualifiers WHERE fact_id = $1 AND qualifier_type_id = $2",
    )
    .bind(fact_id)
    .bind(qualifier_type_id)
    .fetch_optional(pool)
    .await?;
    match existing {
        Some((v,)) if qualifier_values_agree(&v, value) => Ok(QualifierWrite::Same),
        Some(_) => Ok(QualifierWrite::Conflict),
        None => {
            sqlx::query(
                "INSERT INTO fact_qualifiers (fact_id, qualifier_type_id, value)
                 VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            )
            .bind(fact_id)
            .bind(qualifier_type_id)
            .bind(value)
            .execute(pool)
            .await?;
            Ok(QualifierWrite::Set)
        }
    }
}

/// `fact_qualifiers` 连上定义与实体名之后的一行
#[derive(sqlx::FromRow)]
struct QualifierRow {
    fact_id: Uuid,
    qualifier_type_id: Uuid,
    key: String,
    label: String,
    value: Option<serde_json::Value>,
    entity_id: Option<Uuid>,
    entity_name: Option<String>,
}

/// 一批事实各自带的属性，按事实 id 取回。读边的两条路（面板、画布）加载完行后都过这里
pub async fn fact_qualifiers_for(
    pool: &PgPool,
    fact_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Vec<FactQualifier>>> {
    let mut out: HashMap<Uuid, Vec<FactQualifier>> = HashMap::new();
    if fact_ids.is_empty() {
        return Ok(out);
    }
    let rows: Vec<QualifierRow> = sqlx::query_as(
        "SELECT q.fact_id, q.qualifier_type_id, r.key, r.label, q.value, q.entity_id,
                    e.canonical_name AS entity_name
             FROM fact_qualifiers q
             JOIN relation_types r ON r.id = q.qualifier_type_id
             LEFT JOIN entities e ON e.id = q.entity_id
             WHERE q.fact_id = ANY($1)
             ORDER BY q.fact_id, r.key",
    )
    .bind(fact_ids)
    .fetch_all(pool)
    .await?;
    for r in rows {
        out.entry(r.fact_id).or_default().push(FactQualifier {
            qualifier_type_id: r.qualifier_type_id,
            key: r.key,
            label: r.label,
            value: r.value,
            entity_id: r.entity_id,
            entity_name: r.entity_name,
        });
    }
    Ok(out)
}

/// 开放陈述上的一个属性（0044 第一刀）：按文档的角色词记，不按本体属性。
/// 值或实体二选一；实体那一格带上它的显示名
#[derive(Debug, Clone, serde::Serialize)]
pub struct StatementQualifier {
    /// 文档自己的角色词（"amount"、"buyer"）
    pub role: String,
    pub value: Option<serde_json::Value>,
    pub entity_id: Option<Uuid>,
    pub entity_name: Option<String>,
}

/// 往一条开放陈述上写一个按角色词记的属性。**先写者留着**：同一条陈述同一个角色词
/// 再听到一次不覆盖（与 [`upsert_fact_qualifier`] 的 Conflict 一条规矩——两次观察
/// 不一致该另立一行加一条冲突，这一刀还没走到那一步）。值与实体必须恰好给一个
pub async fn add_statement_qualifier(
    pool: &PgPool,
    fact_id: Uuid,
    role: &str,
    value: Option<&serde_json::Value>,
    entity_id: Option<Uuid>,
) -> AppResult<()> {
    if value.is_some() == entity_id.is_some() {
        return Err(AppError::invalid(
            "qualifier_shape",
            "a statement qualifier carries exactly one of a value or an entity",
        ));
    }
    let role = role.trim();
    if role.is_empty() {
        return Err(AppError::invalid(
            "qualifier_role",
            "a statement qualifier needs the document's role word",
        ));
    }
    sqlx::query(
        "INSERT INTO statement_qualifiers (fact_id, role, value, entity_id)
         VALUES ($1, $2, $3, $4) ON CONFLICT (fact_id, role) DO NOTHING",
    )
    .bind(fact_id)
    .bind(role)
    .bind(value)
    .bind(entity_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 一批开放陈述各自的角色词属性，按事实 id 取回；实体那一格连上显示名
pub async fn statement_qualifiers_for(
    pool: &PgPool,
    fact_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Vec<StatementQualifier>>> {
    let mut out: HashMap<Uuid, Vec<StatementQualifier>> = HashMap::new();
    if fact_ids.is_empty() {
        return Ok(out);
    }
    #[derive(sqlx::FromRow)]
    struct Row {
        fact_id: Uuid,
        role: String,
        value: Option<serde_json::Value>,
        entity_id: Option<Uuid>,
        entity_name: Option<String>,
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT q.fact_id, q.role, q.value, q.entity_id, e.canonical_name AS entity_name
         FROM statement_qualifiers q
         LEFT JOIN entities e ON e.id = q.entity_id
         WHERE q.fact_id = ANY($1)
         ORDER BY q.fact_id, q.role",
    )
    .bind(fact_ids)
    .fetch_all(pool)
    .await?;
    for r in rows {
        out.entry(r.fact_id).or_default().push(StatementQualifier {
            role: r.role,
            value: r.value,
            entity_id: r.entity_id,
            entity_name: r.entity_name,
        });
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_fact(
    pool: &PgPool,
    kb_id: Uuid,
    subject_id: Uuid,
    // None = 本体里没有对应的关系。原意不丢——它在证据的 proposed_predicate 里，
    // 显示时由 fact_surface_predicate() 取回（见 `facts.predicate_id`）
    predicate_id: Option<Uuid>,
    object_id: Uuid,
    validity: Validity<'_>,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    insert_fact_inner(
        pool,
        kb_id,
        subject_id,
        predicate_id,
        FactObject::Entity(object_id),
        validity,
        confidence,
    )
    .await
}

/// 一条事实在**世界轴**上的位置：两端各自的时刻与粒度。
///
/// 打包成结构而不是四个平行参数：`Option<DateTime>` 和 `Option<&str>` 各有两个，
/// 相邻同型的参数写反了编译器一声不吭，而这里写反的后果是一条事实的起止颠倒。
///
/// 结束端的三种状态（数据库的 `facts_to_precision_matches_date` 约束在挡）：
///
/// | 语义 | `to` | `to_precision` |
/// |---|---|---|
/// | 仍在持续 | `None` | `None` |
/// | **结束了，不知哪天** | `None` | `Some("unknown")` |
/// | 某时结束 | `Some(t)` | `Some(WORLD_PRECISIONS 之一)` |
///
/// 第二行是后加的。在它之前 `to = None` 同时承载「还在持续」和「不知何时
/// 结束」，于是 "former CEO of Weta Digital" 这种**结束明确、日期缺失**的句子
/// 只能写成前者，图会断言一件原文说已经结束的事。
#[derive(Debug, Clone, Copy, Default)]
pub struct Validity<'a> {
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub from_precision: Option<&'a str>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    pub to_precision: Option<&'a str>,
    /// 这次观察的证据是哪一天的——文档的日期（0022）。`None` 即此刻：人此刻写下
    /// 的事实，人就是证据。落库成 `attested_from`，说结束了不知哪天的观察还落成
    /// `attested_to`（#393，两端各有各的锚点）；同一断言再被观察到时只往早挪。
    /// 没有起点的事实从它起成立，结束了不知哪天的到它为止——**它不是起点**，所以
    /// 不写进 `from`（0003 拒绝过把文档日期填进日期列）
    pub attested_at: Option<chrono::DateTime<chrono::Utc>>,
    /// 起点是怎么来的（0045 第 3 刀）：`A` 文档写明、`B` 按文档自己的锚点算出、
    /// `C` 有时间词但锚不到。`None` = 没经过时间解析（人写的、规则算的），按 A 算。
    /// 时态引擎读它决定这一行能不能关上前任
    pub from_grade: Option<&'a str>,
}

/// `valid_to_precision` 表示「结束了，但不知道是哪天」。
pub const ENDED_UNKNOWN: &str = "unknown";

/// 世界轴的精度梯子（0024）：从年到秒，不再往下——没有哪个源头陈述到亚秒；记录轴留
/// 微秒是因为那是我们自己的钟。结束端另有 `ENDED_UNKNOWN`。数据库的 CHECK 也是这一张表，
/// 抽取端 `parse_time`、给模型看的 `time_text`、导出的 `rdf::world_time` 都照它拼
pub const WORLD_PRECISIONS: [&str; 6] = ["year", "month", "day", "hour", "minute", "second"];

/// 把值截到它的精度：年精度是 1 月 1 日 0 点，秒精度是整秒。**存的值与精度说同一句话**
/// （0024 第 2 条，数据库有同样的 CHECK）；没有精度（锚点、派生的界）原样返回。
pub fn truncate_to(
    t: chrono::DateTime<chrono::Utc>,
    precision: Option<&str>,
) -> chrono::DateTime<chrono::Utc> {
    use chrono::{Datelike, NaiveDate, NaiveTime, TimeZone, Timelike};
    let n = t.naive_utc();
    let (d, time) = (n.date(), n.time());
    let (date, time) = match precision {
        Some("year") => (
            NaiveDate::from_ymd_opt(d.year(), 1, 1).unwrap_or(d),
            NaiveTime::MIN,
        ),
        Some("month") => (
            NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap_or(d),
            NaiveTime::MIN,
        ),
        Some("day") => (d, NaiveTime::MIN),
        Some("hour") => (
            d,
            NaiveTime::from_hms_opt(time.hour(), 0, 0).unwrap_or(time),
        ),
        Some("minute") => (
            d,
            NaiveTime::from_hms_opt(time.hour(), time.minute(), 0).unwrap_or(time),
        ),
        Some("second") => (
            d,
            NaiveTime::from_hms_opt(time.hour(), time.minute(), time.second()).unwrap_or(time),
        ),
        _ => return t,
    };
    chrono::Utc.from_utc_datetime(&date.and_time(time))
}

/// 一个桶的尽头：值加一个精度单位。`2024-03-15`（day）→ `2024-03-16`；`2024-03`（month）
/// → `2024-04`。事件在它命名的那个桶里成立（0031），读出来的终点就是这个；没有精度
/// （锚点）原样返回——锚点是一刻，不是一个桶
pub fn bucket_end(
    t: chrono::DateTime<chrono::Utc>,
    precision: Option<&str>,
) -> chrono::DateTime<chrono::Utc> {
    use chrono::{Duration, Months};
    match precision {
        Some("year") => t.checked_add_months(Months::new(12)).unwrap_or(t),
        Some("month") => t.checked_add_months(Months::new(1)).unwrap_or(t),
        Some("day") => t + Duration::days(1),
        Some("hour") => t + Duration::hours(1),
        Some("minute") => t + Duration::minutes(1),
        Some("second") => t + Duration::seconds(1),
        _ => t,
    }
}

/// 关系的时间语义（`relation_types.temporal`，0031）。状态有区间；事件是一刻——两端写
/// 同一个值，在它命名的那个桶里成立；恒常没有日期，每一刻都成立。
///
/// 从图谱层第一份迁移起这一列就在，界面也一直给选；但直到 0031 之前只有 state 驱动
/// 引擎，event 与 eternal 写进去、读出来都还是区间
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Temporal {
    #[default]
    State,
    Event,
    Eternal,
}

impl Temporal {
    /// 认不出的值当状态：数据库的 CHECK 只放这三个进来，这里不再报错
    pub fn parse(s: &str) -> Self {
        match s {
            "event" => Self::Event,
            "eternal" => Self::Eternal,
            _ => Self::State,
        }
    }
}

/// 谓词的时间语义。没有谓词（0010）按状态——三者里唯一不丢信息的那个，与导入本体时
/// 的判断一致
pub async fn predicate_temporal<'e>(
    pool: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    predicate_id: Option<Uuid>,
) -> AppResult<Temporal> {
    let Some(id) = predicate_id else {
        return Ok(Temporal::State);
    };
    let t: Option<String> = sqlx::query_scalar("SELECT temporal FROM relation_types WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(t.as_deref().map(Temporal::parse).unwrap_or_default())
}

impl<'a> Validity<'a> {
    /// 起始端已知、结束端未知或不适用。
    pub fn starting(
        from: Option<chrono::DateTime<chrono::Utc>>,
        from_precision: Option<&'a str>,
    ) -> Self {
        Self {
            from,
            from_precision,
            to: None,
            to_precision: None,
            attested_at: None,
            from_grade: None,
        }
    }

    /// 这次观察出自哪一天的文档。
    pub fn attested(mut self, at: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        self.attested_at = at;
        self
    }

    /// 两端截到各自的精度（0024）。写入路径进库前都走一遍，与数据库的 CHECK 同一句话
    pub fn truncated(mut self) -> Self {
        self.from = self.from.map(|t| truncate_to(t, self.from_precision));
        self.to = self.to.map(|t| truncate_to(t, self.to_precision));
        self
    }

    /// 按谓词的时间语义归一（0031）。写入路径进库前都走一遍，与读出侧 `world_axis`
    /// 说同一句话。
    ///
    /// - 事件：那一刻写在**两端**。原文给了起点用起点；只给了终点，那就是它发生的
    ///   时候；给了一段，取起点——收购不会持续到明年。没日期就两端都空；「结束了
    ///   不知哪天」对一刻没有意义，一并抹掉。两端同值是这一行自己就能说清的形状：
    ///   不看谓词的读者顶多把它读成一天的状态，读不成「从那天起一直如此」
    /// - 恒常：日期全抹。原文里的日期说的是别的事，不是这条关系何时成立
    /// - 状态：原样——**两端相等的不写**（#966）。世界轴按 `from <= T < to` 读状态，起止
    ///   同值的一段任何时刻都不成立，写下它只会把「那天加入」读成「从没在那儿过」。一刻的
    ///   陈述在状态属性下读成开始、结束还是什么都不算，由绑定说（`phrase_bindings.marks`），
    ///   不在这里猜；这里只守住不变量，所以没有哪条写入路径能存下一段不成立的状态
    ///
    /// 调用方先截断再归一：截到精度之后才相等的两端（同一天的两个钟点）同样拒绝。
    /// 没有谓词的行按状态读（0010）只是读法，不是声明：调用方不拿它过这一关
    pub fn under(mut self, temporal: Temporal) -> AppResult<Self> {
        match temporal {
            Temporal::State => {
                if self.from.is_some() && self.from == self.to {
                    return Err(empty_state_span());
                }
            }
            Temporal::Eternal => {
                self.from = None;
                self.from_precision = None;
                self.to = None;
                self.to_precision = None;
            }
            Temporal::Event => {
                let moment = self
                    .from
                    .map(|t| (t, self.from_precision))
                    .or_else(|| self.to.map(|t| (t, self.to_precision)));
                let (t, p) = match moment {
                    Some((t, p)) => (Some(t), p),
                    None => (None, None),
                };
                self.from = t;
                self.from_precision = p;
                self.to = t;
                self.to_precision = p;
            }
        }
        Ok(self)
    }

    /// 原文说它结束了，但没说哪天。
    pub fn ended_when_unknown(mut self) -> Self {
        self.to = None;
        self.to_precision = Some(ENDED_UNKNOWN);
        self
    }

    /// 这条断言是否已经不再成立——**两种结束都算**。
    ///
    /// 判据写在这里而不是散在各处的 `valid_to.is_some()`：那种写法会把
    /// 「结束了但不知哪天」漏成「仍在持续」，而那正是两端各记精度要修的东西。
    pub fn has_ended(&self) -> bool {
        self.to.is_some() || self.to_precision == Some(ENDED_UNKNOWN)
    }
}

/// 一段起止同值的状态（#966）：写入、改区间、关上都拒它，说的是同一句话
pub fn empty_state_span() -> AppError {
    AppError::invalid(
        "empty_state_span",
        "A state that ends where it starts holds at no moment: give an end after the start, \
         or leave it open.",
    )
}

#[allow(clippy::too_many_arguments)]
async fn insert_fact_inner(
    pool: &PgPool,
    kb_id: Uuid,
    subject_id: Uuid,
    // None = 本体里没有对应的关系。原意不丢——它在证据的 proposed_predicate 里，
    // 显示时由 fact_surface_predicate() 取回（见 `facts.predicate_id`）
    predicate_id: Option<Uuid>,
    object: FactObject<'_>,
    validity: Validity<'_>,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    let mut conn = pool.acquire().await?;
    insert_fact_on(
        &mut conn,
        kb_id,
        subject_id,
        predicate_id,
        object,
        validity,
        confidence,
    )
    .await
}

/// The same insertion semantics on a caller-owned connection/transaction.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_fact_on(
    conn: &mut sqlx::PgConnection,
    kb_id: Uuid,
    subject_id: Uuid,
    // None = 本体里没有对应的关系。原意不丢——它在证据的 proposed_predicate 里，
    // 显示时由 fact_surface_predicate() 取回（见 `facts.predicate_id`）
    predicate_id: Option<Uuid>,
    object: FactObject<'_>,
    validity: Validity<'_>,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    // 按谓词的时间语义归一（0031）：事件两端同一刻，恒常无日期。写在这里而不是各个
    // 写入者那儿——抽取、点头、人自己写的事实都经过这一个门
    let temporal = predicate_temporal(&mut *conn, predicate_id).await?;
    // 声明成状态的属性才守「两端不相等」（#966）；没有谓词的行按状态读只是读法，照旧写
    let declared_state = predicate_id.is_some() && temporal == Temporal::State;
    let validity = if predicate_id.is_some() {
        validity.truncated().under(temporal)?
    } else {
        validity.truncated()
    };
    let same_sql = match object {
        FactObject::Entity(_) => {
            "SELECT id, valid_from, valid_to, valid_to_precision FROM facts
             WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3 AND object_id = $4
               AND invalidated_at IS NULL"
        }
        FactObject::Value(_) => {
            "SELECT id, valid_from, valid_to, valid_to_precision FROM facts
             WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3 AND object_value = $4
               AND object_id IS NULL AND invalidated_at IS NULL"
        }
    };
    let mut q = sqlx::query_as(same_sql)
        .bind(kb_id)
        .bind(subject_id)
        .bind(predicate_id);
    q = match object {
        FactObject::Entity(id) => q.bind(id),
        FactObject::Value(v) => q.bind(v),
    };
    let mut same: Vec<FactSpanRow> = q.fetch_all(&mut *conn).await?;
    // 状态属性下起止同值的行（#966 之前写下的）任何时刻都不成立：不关它、也不并进它。
    // 不然「那天加入」与「自那天起」谁先落账，账本就是两个样子——一刻关上了开放的那段，
    // 或开放的那段并进了一刻。这样的行由物化作废重算，这里只是不让它牵动别的观察
    if declared_state {
        same.retain(|(_, vf, vt, _)| !(vf.is_some() && vf == vt));
    }
    // 「结束了，不知哪天」的观察撞上同断言的**开放行**（0022 / #393）：关上它。
    // 不并进去——并进去等于把「它结束了」这唯一带来的信息丢掉（同 valid_from 那条
    // 精确重复的路会这么干）；也不另立一行——另立一行让两条各说各话，开放的那条
    // 照旧被读成「至今仍是」（#345 的那道题正是这样挂的）。
    // 修正走 supersede：旧行作废，新行终点仍空、精度 'unknown'，`attested_to` 锚在说出
    // 结束的那份文档上；起点照旧——有日期的用日期，没日期的裸行留着它自己的
    // `attested_from`（第一份证据）。两个锚点，裸行也关得上
    if validity.to.is_none() && validity.to_precision == Some(ENDED_UNKNOWN) {
        let open = same
            .iter()
            .filter(|(_, vf, vt, vtp)| {
                vt.is_none() && vtp.is_none() && validity.from.is_none_or(|f| Some(f) == *vf)
            })
            .max_by_key(|(_, vf, _, _)| *vf);
        if let Some((open, _, _, _)) = open {
            if let Some(closed) =
                crate::temporal::close_with_unknown_end(&mut *conn, *open, validity.attested_at)
                    .await?
            {
                return Ok((closed, true));
            }
        }
        // 已经关上的（结束了不知哪天）再听到一次「结束了」：同一件事，复用那一行。
        // 锚点只往早挪——更早的文档说它结束了，它就结束得更早。那一行的终点若是引擎推的，
        // 现在原文说出来了：改成写明的，此后不随时间线重算（#679 第三轮评审）
        if let Some((ended, _, _, _)) = same.iter().find(|(_, vf, vt, vtp)| {
            vt.is_none()
                && vtp.as_deref() == Some(ENDED_UNKNOWN)
                && validity.from.is_none_or(|f| Some(f) == *vf)
        }) {
            if let Some(stated) =
                crate::temporal::state_derived_end(&mut *conn, *ended, None, validity.attested_at)
                    .await?
            {
                return Ok((stated, true));
            }
            attest_earlier(&mut *conn, *ended, validity.attested_at, true).await?;
            return Ok((*ended, false));
        }
    }
    /* **「某天结束了」的观察（没起点、有终点）撞上同断言的开放行：关上它，不另立一行。**
    另立一行让两条各说各话，开放的那条照旧被读成「至今仍是」——实测「移出失信名单」
    「辞去董事职务」各多出一条 `- → 日期`，而原来那条还开着。事件没有开放行
    （两端同一刻），所以只有状态走这里。修正走 supersede（作废 + 改写，证据和边上的
    属性随行），与 #393 关「不知哪天」同一条路；起点晚于终点所说时段的开放行不是这一段 */
    if temporal == Temporal::State && validity.from.is_none() {
        if let Some(to) = validity.to {
            let precision = validity.to_precision.unwrap_or("day");
            // 终点说的是一个时段，比的却是时刻：「2024 年离开」存成 2024-01-01。声明成状态的
            // 开放行若从这个时段里开始（2024-03-01 起任职），终点落在时段的尽头
            // （2025-01-01），不另立一行 `- → 2024`、让开放的那段一直开着；在时段之前开始的
            // 照旧关在时段的开头（0053 修订 2026-09-27）。同一天开始又结束的也是这样：关在
            // 那一天的尽头，不关在它自己的起点——那是一段不成立的状态（#966）
            let period_end = declared_state.then(|| bucket_end(to, Some(precision)));
            let closes_at = |from: Option<chrono::DateTime<chrono::Utc>>| match (from, period_end) {
                (Some(f), Some(end)) if to <= f && f < end => end,
                _ => to,
            };
            // 已经关在那一刻的：同一件事，复用那一行（引擎推的终点改成写明的，同上）
            if let Some((ended, from, _, _)) = same
                .iter()
                .find(|(_, vf, vt, _)| *vt == Some(closes_at(*vf)))
            {
                if let Some(stated) = crate::temporal::state_derived_end(
                    &mut *conn,
                    *ended,
                    Some((closes_at(*from), precision)),
                    validity.attested_at,
                )
                .await?
                {
                    return Ok((stated, true));
                }
                attest_earlier(&mut *conn, *ended, validity.attested_at, true).await?;
                return Ok((*ended, false));
            }
            let open = same
                .iter()
                .filter(|(_, vf, vt, vtp)| {
                    vt.is_none()
                        && vtp.is_none()
                        && vf.is_none_or(|f| match period_end {
                            Some(end) => f < end,
                            None => f <= to,
                        })
                })
                .max_by_key(|(_, vf, _, _)| *vf);
            if let Some((open, from, _, _)) = open {
                if let Some(closed) = crate::temporal::close_superseded(
                    &mut *conn,
                    *open,
                    closes_at(*from),
                    precision,
                )
                .await?
                {
                    return Ok((closed, true));
                }
            }
        }
    }
    // 精确重复：同 valid_from → 复用。同起点、**这次带了终点、那行还开着** → 关上它
    // （「自 2020-01-10 起任董事」之后读到「2020-01-10 至 2024-04-30 任董事」）
    if let Some((existing, _, vt, vtp)) = same.iter().find(|(_, vf, _, _)| *vf == validity.from) {
        if temporal == Temporal::State && vt.is_none() && vtp.is_none() {
            if let Some(to) = validity.to {
                if let Some(closed) = crate::temporal::close_superseded(
                    &mut *conn,
                    *existing,
                    to,
                    validity.to_precision.unwrap_or("day"),
                )
                .await?
                {
                    return Ok((closed, true));
                }
            }
        }
        // 那行已经关上，这次观察也说了终点：终点若是引擎推的，改成原文说的
        if temporal == Temporal::State && (vt.is_some() || vtp.is_some()) && validity.has_ended() {
            let stated_to = validity
                .to
                .map(|to| (to, validity.to_precision.unwrap_or("day")));
            if let Some(stated) = crate::temporal::state_derived_end(
                &mut *conn,
                *existing,
                stated_to,
                validity.attested_at,
            )
            .await?
            {
                return Ok((stated, true));
            }
        }
        attest_earlier(
            &mut *conn,
            *existing,
            validity.attested_at,
            validity.has_ended(),
        )
        .await?;
        return Ok((*existing, false));
    }
    // 弱化陈述：新观察无时间，同断言已有开放行 → 并入（取起点最新的开放行）。
    // 事件没有「开放」一说——它的两端总是同一刻——所以没日期的再观察并进已有的
    // 那一刻（0031）：说过一次「三月收购了」，再听到一句没日期的「收购了」，不是第二次收购
    if validity.from.is_none() && !validity.has_ended() {
        if let Some((existing, _, _, _)) = same
            .iter()
            .filter(|(_, _, vt, _)| vt.is_none() || temporal == Temporal::Event)
            .max_by_key(|(_, vf, _, _)| *vf)
        {
            attest_earlier(&mut *conn, *existing, validity.attested_at, false).await?;
            return Ok((*existing, false));
        }
        // 没有开放行，但这次观察的文档日期落在某条**已关上**的行里：说的是那一段，不是
        // 新的一段——处罚决定书里的「董事李文博」，日期在他的任期之内。另立一条裸行会被
        // 读成「至今仍是」，而任期明明已经关上了。文档日期在段之后的照旧另立：那可能真是
        // 新的一段（再次任职），拿不准时宁分勿合
        if let Some(at) = validity.attested_at {
            if let Some((existing, _, _, _)) = same
                .iter()
                .find(|(_, vf, vt, _)| vt.is_some_and(|t| at <= t) && vf.is_none_or(|f| f <= at))
            {
                attest_earlier(&mut *conn, *existing, validity.attested_at, false).await?;
                return Ok((*existing, false));
            }
        }
    }
    // 时间精化候选：已有无起点的行（裸行，或只知道终点的行——并行抽取时说结束的那份
    // 文档可能先到），本次观察带了起点 → 落库后作废那行并链上。只知道终点的行，
    // 终点跟着走：这次没说终点就沿用它的，说了就得是同一个
    let mut validity = validity;
    // 声明成状态的，沿用来的终点说的是一个时段（同上）：这次的起点落在时段里，终点取时段的
    // 尽头；落在时段之后，那个结束说的是更早的一段，不精化，各自一行（0053 修订 2026-09-27）
    let refine_target = match validity.from {
        Some(from) => same.iter().find_map(|(id, vf, vt, vtp)| {
            if vf.is_some() {
                return None;
            }
            let end = match (validity.to, *vt) {
                (None, Some(t)) if declared_state && t <= from => {
                    let end = bucket_end(t, Some(vtp.as_deref().unwrap_or("day")));
                    if from >= end {
                        return None;
                    }
                    Some(end)
                }
                (Some(to), Some(t)) if to != t => return None,
                (_, vt) => vt,
            };
            Some((*id, end, vtp.clone()))
        }),
        None => None,
    };
    if let Some((_, Some(vt), vtp)) = &refine_target {
        if validity.to.is_none() {
            validity.to = Some(*vt);
            validity.to_precision = vtp.as_deref().map(|p| match p {
                "year" => "year",
                "month" => "month",
                "day" => "day",
                "hour" => "hour",
                "minute" => "minute",
                "second" => "second",
                _ => ENDED_UNKNOWN,
            });
        }
    }
    let refine_target = refine_target.map(|(id, _, _)| id);

    let id = Uuid::now_v7();
    let insert_sql = match object {
        FactObject::Entity(_) => {
            // 说结束了不知哪天的观察，说出结束的就是它自己那份文档：attested_to 也落它
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                valid_from, valid_from_precision,
                                valid_to, valid_to_precision, confidence,
                                attested_from, attested_to, valid_from_grade)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, COALESCE($11, now()),
                     CASE WHEN $9::text = 'unknown' THEN COALESCE($11, now()) END, $12)"
        }
        FactObject::Value(_) => {
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value,
                                valid_from, valid_from_precision,
                                valid_to, valid_to_precision, confidence,
                                attested_from, attested_to, valid_from_grade)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, COALESCE($11, now()),
                     CASE WHEN $9::text = 'unknown' THEN COALESCE($11, now()) END, $12)"
        }
    };
    let mut ins = sqlx::query(insert_sql)
        .bind(id)
        .bind(kb_id)
        .bind(subject_id)
        .bind(predicate_id);
    ins = match object {
        FactObject::Entity(oid) => ins.bind(oid),
        FactObject::Value(v) => ins.bind(v),
    };
    ins.bind(validity.from)
        .bind(validity.from_precision)
        .bind(validity.to)
        .bind(validity.to_precision)
        .bind(confidence)
        .bind(validity.attested_at)
        .bind(validity.from_grade)
        .execute(&mut *conn)
        .await?;

    // 时间精化：裸行（无时无终的同断言）被本次带时间的观察取代——作废+链上，证据随行
    if let Some(old_id) = refine_target {
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(old_id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("UPDATE facts SET supersedes = $2 WHERE id = $1")
            .bind(id)
            .bind(old_id)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            // 表层谓词随证据一起搬：精化的是时间，不是原文说了什么。引文的偏移一起搬
            "INSERT INTO fact_evidence (fact_id, chunk_id, quote, proposed_predicate, document_id, doc_version,
                                        quote_start, quote_end)
             SELECT $1, chunk_id, quote, proposed_predicate, document_id, doc_version,
                    quote_start, quote_end
             FROM fact_evidence WHERE fact_id = $2
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(old_id)
        .execute(&mut *conn)
        .await?;
        // 边上的属性也随行（0037）：裸行上已有的金额、职务不因为精化了时间而丢
        sqlx::query(
            "INSERT INTO fact_qualifiers (fact_id, qualifier_type_id, value, entity_id)
             SELECT $1, qualifier_type_id, value, entity_id
             FROM fact_qualifiers WHERE fact_id = $2
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(old_id)
        .execute(&mut *conn)
        .await?;
    }
    Ok((id, true))
}

/// 同一断言又被观察到一次：锚点只往早挪（0022）。更早的文档是更早的证据；
/// 更晚的什么也不改——一条事实从有证据的那一刻起成立，之后再被提到不会把它
/// 往后推。`None`（此刻）也不动它：此刻不会早于任何已有的证据。
///
/// `ended`：这次观察说的是「它结束了」。只有它是结束得更早的证据，终点锚才跟着挪；
/// 说它成立的观察只挪起点锚。从前两个一起挪，一条晚到的、日期更早的「成立」并进一行
/// 「结束了，不知哪天」，终点锚就挪到了它自己身上，区间缩成空的（#875 的回放）
async fn attest_earlier<'e>(
    pool: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    fact_id: Uuid,
    at: Option<chrono::DateTime<chrono::Utc>>,
    ended: bool,
) -> AppResult<()> {
    if let Some(at) = at {
        // attested_to 只在结束未知的行上有，NULL 的留 NULL
        sqlx::query(
            // LEAST 会跳过 NULL——开放行的 attested_to 是 NULL，直接 least 会给它凭空长出一个
            // 终点锚，撞上 CHECK。NULL 的留 NULL
            "UPDATE facts SET attested_from = least(attested_from, $2),
                              attested_to = CASE WHEN attested_to IS NULL OR NOT $3 THEN attested_to
                                                 ELSE least(attested_to, $2) END
              WHERE id = $1",
        )
        .bind(fact_id)
        .bind(at)
        .bind(ended)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// 一条陈述的见证：它所在那一节里文本说话的那一刻，连同那条日期的名字（0064 决定 3）。
///
/// 只往早挪（同 [`attest_earlier`]）：同一句话在更早的文档里说过，见证就是更早的那份。
/// 名字跟着日期走——日期没动，名字也不动。物化出来的、规则算出来的类型化行自己没有出处，
/// 跟着陈述走（`materialize::sync_typed_attestation`，调用方在写完一篇文档的陈述之后叫一次）
pub async fn attest_statement(
    pool: &PgPool,
    fact_id: Uuid,
    at: chrono::DateTime<chrono::Utc>,
    by: &str,
) -> AppResult<bool> {
    let moved = sqlx::query(
        "UPDATE facts
            SET attested_from = $2, attested_by = $3
          WHERE id = $1 AND layer = 'open' AND invalidated_at IS NULL
            AND (attested_from IS NULL OR attested_from > $2
                 OR (attested_from = $2 AND attested_by IS DISTINCT FROM $3))",
    )
    .bind(fact_id)
    .bind(at)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(moved.rows_affected() > 0)
}

/// 一条证据记下它那段原文说话的那一刻，连同那条日期的名字；那一节没说日期的记成空。
///
/// [`attest_statement`] 只把事实的见证往早挪，哪份证据给的、别的证据各是哪天都不留。证据
/// 所在的文档删了、删除撤销了，事实的见证要照还在的证据重算（`documents::reattest_tx`），
/// 靠的就是这里记下的每一条
pub async fn witness_evidence(
    pool: &PgPool,
    fact_id: Uuid,
    chunk_id: Uuid,
    witness: Option<(chrono::DateTime<chrono::Utc>, &str)>,
) -> AppResult<()> {
    let (at, by) = witness.unzip();
    sqlx::query(
        "UPDATE fact_evidence SET attested_at = $3, attested_by = $4
          WHERE fact_id = $1 AND chunk_id = $2
            AND (attested_at IS DISTINCT FROM $3 OR attested_by IS DISTINCT FROM $4)",
    )
    .bind(fact_id)
    .bind(chunk_id)
    .bind(at)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(())
}

/// 字面值宾语的事实（object_value 通道，问数映射首个消费者）。
/// 去重：同 (S,P) 且 object_value 完全相等的 live 事实只存一条。
#[allow(clippy::too_many_arguments)]
pub async fn insert_value_fact(
    pool: &PgPool,
    kb_id: Uuid,
    subject_id: Uuid,
    // None = 本体里没有对应的关系。原意不丢——它在证据的 proposed_predicate 里，
    // 显示时由 fact_surface_predicate() 取回（见 `facts.predicate_id`）
    predicate_id: Option<Uuid>,
    object_value: &serde_json::Value,
    validity: Validity<'_>,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    insert_fact_inner(
        pool,
        kb_id,
        subject_id,
        predicate_id,
        FactObject::Value(object_value),
        validity,
        confidence,
    )
    .await
}

/// 一条开放陈述（0044 第一刀，#729）：文档说了什么，用它自己的话。
/// 返回 (事实 id, 是否新建)。
///
/// 落的是 `facts` 里 `layer = 'open'` 的一行：短语照写在 `phrase` 上、`predicate_id`
/// 为空（`facts_open_statement_shape` 在挡）。去重按 (库, 主语, 短语, 宾语) 在活着的
/// 开放行里找——今天空谓词的行按 (主, 谓, 宾) 去重、谓词是 NULL 就永远撞不上，每次
/// 观察都插一行；短语进了键之后同一句话再听到一次就是同一行，证据累积到它上面。
/// 撞上了只把 `attested_from` 往早挪（[`attest_earlier`]）。
///
/// **不走 [`insert_fact_inner`] 那道门**：不问 `predicate_temporal`，不过
/// `Validity::under`，不写任何 `valid_*`，也不碰时间线。一条开放陈述在世界轴上还
/// 没有位置——它提到的时间是照抄的字（`time_mentions`），把字读成日期是 0045
/// 后面那几刀的事。这里只有记录轴：`attested_from` 是来源系统给文档的日期（调用方只在
/// `doc_time_source IN ('content', 'source')` 时传，#714），没有就留空——**不填此刻**
/// （0064 决定 5）：处理文档的时刻不是文档说的日期。文档自己说的日期在抽完之后由
/// [`attest_statement`] 写上来。
///
/// 调用方随后要把 `proposed_predicate = phrase` 写到证据上（[`add_evidence_located`]），
/// 于是所有已经容得下空谓词的读路径（`fact_surface_predicate`）不改一字就按短语显示它
pub async fn insert_open_statement(
    pool: &PgPool,
    kb_id: Uuid,
    subject_id: Uuid,
    phrase: &str,
    object: FactObject<'_>,
    attested_at: Option<chrono::DateTime<chrono::Utc>>,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    let phrase = phrase.trim();
    if phrase.is_empty() {
        return Err(AppError::invalid(
            "phrase_missing",
            "an open statement keeps the document's relation phrase",
        ));
    }
    let same_sql = match object {
        FactObject::Entity(_) => {
            "SELECT id FROM facts
             WHERE layer = 'open' AND kb_id = $1 AND subject_id = $2 AND phrase = $3
               AND object_id = $4 AND invalidated_at IS NULL
             ORDER BY recorded_at LIMIT 1"
        }
        FactObject::Value(_) => {
            "SELECT id FROM facts
             WHERE layer = 'open' AND kb_id = $1 AND subject_id = $2 AND phrase = $3
               AND object_value = $4 AND object_id IS NULL AND invalidated_at IS NULL
             ORDER BY recorded_at LIMIT 1"
        }
    };
    let mut q = sqlx::query_scalar(same_sql)
        .bind(kb_id)
        .bind(subject_id)
        .bind(phrase);
    q = match object {
        FactObject::Entity(id) => q.bind(id),
        FactObject::Value(v) => q.bind(v),
    };
    let same: Option<Uuid> = q.fetch_optional(pool).await?;
    if let Some(existing) = same {
        // 开放陈述落库时还不知道这次提及说的是成立还是结束（时间词在 0045 的任务里才读），
        // 这里照旧两个锚点一起挪
        attest_earlier(pool, existing, attested_at, true).await?;
        return Ok((existing, false));
    }

    let id = Uuid::now_v7();
    let insert_sql = match object {
        FactObject::Entity(_) => {
            "INSERT INTO facts (id, kb_id, subject_id, layer, phrase, object_id,
                                confidence, attested_from)
             VALUES ($1, $2, $3, 'open', $4, $5, $6, $7)"
        }
        FactObject::Value(_) => {
            "INSERT INTO facts (id, kb_id, subject_id, layer, phrase, object_value,
                                confidence, attested_from)
             VALUES ($1, $2, $3, 'open', $4, $5, $6, $7)"
        }
    };
    let mut ins = sqlx::query(insert_sql)
        .bind(id)
        .bind(kb_id)
        .bind(subject_id)
        .bind(phrase);
    ins = match object {
        FactObject::Entity(oid) => ins.bind(oid),
        FactObject::Value(v) => ins.bind(v),
    };
    ins.bind(confidence).bind(attested_at).execute(pool).await?;
    Ok((id, true))
}

/// 把解算结果写到一条开放陈述的世界轴上（0045 决定 2：模型读，代码算）。
///
/// 只改 `layer = 'open'` 且活着的行——WHERE 里写死，不靠调用方检查；类型化的行照旧走
/// [`insert_fact_inner`] 那道门（谓词的时间语义、去重、时间线），作废的行是历史，不改。
/// 值先按精度截断（[`truncate_to`]），存的值与精度说同一句话，数据库的 CHECK 才放行。
/// 结束端的三种状态与 [`Validity`] 同一张表：`(None, None)` 仍在持续、
/// `(None, Some("unknown"))` 结束了不知哪天、`(Some(t), Some(精度))` 某时结束。
/// 结束了不知哪天的要有自己的锚点（`facts_ended_unknown_has_anchor`，#393）：说出结束的
/// 就是这条陈述自己的文档，锚点取 `attested_from`，文档没说自己是哪天的（0064 决定 5）
/// 就取账本记下它的那一刻；其余两种状态把 `attested_to` 清空
///
/// 没有这一行、它不是开放行、或它已作废：一行不改，返回 `not_an_open_statement`
pub async fn set_open_validity(
    pool: &PgPool,
    fact_id: Uuid,
    from: Option<chrono::DateTime<chrono::Utc>>,
    from_precision: Option<&str>,
    to: Option<chrono::DateTime<chrono::Utc>>,
    to_precision: Option<&str>,
    // 起点是怎么来的（0045 第 3 刀）。**锚不到（`C`）也要写**：那时两端都是空，
    // 只有这一列说得出「这句话有时间词、我们没能把它放到轴上」，物化把它带给
    // 类型化的行，时态引擎才不会让一个猜出来的位置改写前任的历史
    grade: Option<&str>,
) -> AppResult<()> {
    let from = from.map(|t| truncate_to(t, from_precision));
    let to = to.map(|t| truncate_to(t, to_precision));
    let done = sqlx::query(
        "UPDATE facts
            SET valid_from = $2, valid_from_precision = $3,
                valid_to = $4, valid_to_precision = $5, valid_from_grade = $6,
                attested_to = CASE WHEN $4 IS NULL AND $5 = 'unknown'
                                   THEN COALESCE(attested_to, attested_from, recorded_at) END
          WHERE id = $1 AND layer = 'open' AND invalidated_at IS NULL",
    )
    .bind(fact_id)
    .bind(from)
    .bind(from_precision)
    .bind(to)
    .bind(to_precision)
    .bind(grade)
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::invalid(
            "not_an_open_statement",
            "only a live open statement takes its valid time from resolution",
        ));
    }
    Ok(())
}

/// `proposed`：模型在这一块里实际提议的谓词。命中本体时它等于 key，
/// 本体外的谓词不落到关系上时，它是唯一还留着原意的东西——事实行上只剩
/// "有关联"，原文说的"runs on"就靠这里活下来。
pub async fn add_evidence(
    pool: &PgPool,
    fact_id: Uuid,
    chunk_id: Uuid,
    quote: Option<&str>,
    proposed: Option<&str>,
) -> AppResult<()> {
    add_evidence_located(pool, fact_id, chunk_id, quote, proposed, None).await
}

/// [`add_evidence`]，外加引文在 `chunks.text` 里的位置（0044 第一刀）。
///
/// `span` 是字符偏移（不是字节）的 `(start, end)`，**由服务端搜文本算出来，从不取
/// 模型报的数**；没定位到就传 `None`，两列留空。同一对 (事实, 分块) 再写一次只在
/// 原值为空时补上偏移，与表层谓词一条规矩：第一次记下的就是它的
pub async fn add_evidence_located(
    pool: &PgPool,
    fact_id: Uuid,
    chunk_id: Uuid,
    quote: Option<&str>,
    proposed: Option<&str>,
    span: Option<(i32, i32)>,
) -> AppResult<()> {
    // 证据落笔即记版本：出自哪份文档的第几版（S3 版本对账与"证据过期"判定的依据）
    // 冲突时补写表层谓词而非整行跳过：重抽命中的多是已有的 (事实, 分块) 对，
    // DO NOTHING 会让存量证据永远填不上这一列。只在原值为空时补，不覆盖——
    // 同一分块的同一条事实，第一次记下的说法就是它的说法
    //
    // **证据落在活着的那一行上**（#679 第三轮评审）。落库到写证据之间，时间线重算可能已经
    // 把这一行改写掉（换了终点、换了 id）：改写时复制的证据里没有这一条，写在旧行上就丢了。
    // 先 `FOR SHARE` 锁住这一行——正在改写它的事务持着 `FOR UPDATE`，这里等它提交；
    // 等到的若已作废，顺着 supersedes 走到它改写出来的那一行。被驳回、没有后继的，证据仍记在它身上
    let mut tx = pool.begin().await?;
    let mut target = fact_id;
    loop {
        let live: Option<bool> =
            sqlx::query_scalar("SELECT invalidated_at IS NULL FROM facts WHERE id = $1 FOR SHARE")
                .bind(target)
                .fetch_optional(&mut *tx)
                .await?;
        if live != Some(false) {
            break;
        }
        let next: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM facts WHERE supersedes = $1
              ORDER BY invalidated_at IS NULL DESC, recorded_at DESC LIMIT 1",
        )
        .bind(target)
        .fetch_optional(&mut *tx)
        .await?;
        match next {
            Some(next) => target = next,
            None => break,
        }
    }
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, proposed_predicate, document_id, doc_version,
                                    quote_start, quote_end)
         SELECT $1, $2, $3, left($4, 120), c.document_id, c.doc_version, $5, $6
         FROM chunks c WHERE c.id = $2
         ON CONFLICT (fact_id, chunk_id) DO UPDATE
           SET proposed_predicate = COALESCE(fact_evidence.proposed_predicate, EXCLUDED.proposed_predicate),
               quote_start = COALESCE(fact_evidence.quote_start, EXCLUDED.quote_start),
               quote_end = COALESCE(fact_evidence.quote_end, EXCLUDED.quote_end)",
    )
    .bind(target)
    .bind(chunk_id)
    .bind(quote)
    .bind(proposed)
    .bind(span.map(|(s, _)| s))
    .bind(span.map(|(_, e)| e))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 所有图查询共用的取节点语句。
///
/// **LEFT JOIN，不是 JOIN**（0009）。没判出类型的实体照样是图上的节点：它有名字、
/// 有事实、有证据，缺的只是一个标签。内连接会让它整个消失——事实还在库里，
/// 图上却查无此人，那是最难发现的一种数据丢失。
///
/// key 与 label 留 NULL，**颜色和形状给缺省值**：前者是身份，没有就该说没有；
/// 后者是画布必须拿到的东西，编不出来就没法渲染。灰色圆点正是「还没定」的样子
/// `as_of`：绑记录轴参数的位置（`None` = 只答现在，写路径和"当下"视图用这个）。
/// 度数跟着画布走——回放时数的是**当时**连在这个节点上的边，否则右上角的数
/// 和眼前的图对不上。
/// `owner`：**只在真的传了时刻时**才绑（#336）。`fact_owner_at` 包住列之后
/// `facts` 上按主宾的索引就用不上了，而「现在」是每次画图都要走的那条路
fn node_sql(as_of: Option<usize>, owner: Option<usize>) -> String {
    let held = crate::record_axis::facts_held_at("f", as_of);
    // 主宾也跟着倒：三月被合并掉的实体，在二月身上还挂着它自己的那些事实（#336）
    let subject = crate::record_axis::owner_at("f", "subject_id", owner, false);
    let object = crate::record_axis::owner_at("f", "object_id", owner, true);
    // 名字事实不算度数（0041）：每个实体至少有一个名字，数进去所有节点一起变大一号
    let not_name = crate::names::not_a_name("f");
    format!(
        "SELECT e.id, e.canonical_name AS name, t.key AS type_key,
        t.label AS type_label,
        coalesce(t.color, '#94a3b8') AS color,
        coalesce(t.shape, 'circle') AS shape,
        e.disambiguator,
        (SELECT count(*) FROM facts f
         WHERE f.kb_id = e.kb_id AND ({subject} = e.id OR {object} = e.id) AND {held} AND {not_name}) AS degree
     FROM entities e LEFT JOIN entity_types t ON t.id = e.type_id"
    )
}

/// 全图概览：按度数取 top N 实体及其间的边。
/// `at`：世界轴——只返回 T 时刻**有效**的边（起点不晚于 T 或未知，终点晚于 T 或开放）。
/// `as_of`：记录轴（0019）——只返回 T 时刻**我们持有**的边，三月被改掉的断言在
/// 三月之前的位置上应当还在。两个参数一路分开到 API：折成一个控件，就会拿
/// 「三月的世界，以今天的认知」去答「三月的世界，以三月的认知」，而两者在
/// 屏幕上都说得通。
/// 图谱总览：度数最高的 `limit` 个节点，以及它们之间的边。
///
/// **一并回总数。** 画多少个是渲染的事，库里有多少是知识库的事，两者从前
/// 在界面上被同一个数字表示——一个上万实体的库，右上角永远写着 150，而那
/// 是上限不是规模。渲染上限本身是合理的（画一万个点没人看得懂），骗人的是
/// 把它说成总数。
pub async fn overview(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<(Vec<GraphNode>, Vec<GraphEdge>, i64, i64)> {
    let nodes: Vec<GraphNode> = sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND {visible} ORDER BY degree DESC, e.created_at LIMIT $2",
        node_sql(as_of.map(|_| 3), as_of.map(|_| 3)),
        visible = crate::record_axis::entity_visible_at("e", as_of.map(|_| 3)),
    ))
    .bind(kb_id)
    .bind(limit)
    .bind(as_of)
    .fetch_all(pool)
    .await?;

    let ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();
    let edges = edges_among(pool, kb_id, &ids, at, as_of).await?;

    // 总数按与画布同一套口径数：合并掉的实体不算，作废的事实不算，
    // 属性事实（宾语是字面值）画不出边所以也不算。口径不同的话，
    // 「150 / 325」里那个 325 会跟用户在别处看到的数对不上——**回放时也一样**，
    // 边数跟着记录轴走，否则倒回三月的图上写着今天的边数。
    //
    // 节点数也跟着倒（#336）：实体的时刻在 `entity_merges` 上，不在实体行上——
    // 三月并掉的那个，在二月既该出现在画布上，也该数进这个总数里
    let total_nodes: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM entities e WHERE e.kb_id = $1 AND {visible}",
        visible = crate::record_axis::entity_visible_at("e", as_of.map(|_| 2)),
    ))
    .bind(kb_id)
    .bind(as_of)
    .fetch_one(pool)
    .await?;
    let total_edges: i64 = sqlx::query_scalar(&format!(
        // 两边都要 `object_id IS NOT NULL`：数的是**画得出来的边**。派生表拓宽
        // 之后（0021）字面值结论也住在这张表里，把它们数进来，状态栏报的边数
        // 就比画布上多——而多出来的那些永远找不到
        "SELECT (SELECT count(*) FROM facts f
                  WHERE f.kb_id = $1 AND {facts_held} AND f.object_id IS NOT NULL)
              + (SELECT count(*) FROM derived_facts d
                  WHERE d.kb_id = $1 AND {derived_held} AND d.object_id IS NOT NULL)",
        facts_held = crate::record_axis::facts_held_at("f", as_of.map(|_| 2)),
        derived_held = crate::record_axis::derived_held_at("d", as_of.map(|_| 2)),
    ))
    .bind(kb_id)
    .bind(as_of)
    .fetch_one(pool)
    .await?;
    Ok((nodes, edges, total_nodes, total_edges))
}

/// 这条开放陈述已经被一条活着的类型化行代表（0044 决定 1：类型化图谱是开放图谱按
/// 绑定算出来的视图）。画布与实体面板据此一条陈述只画一条边——有类型化行就画它，
/// 标签是属性的名字；没有才画原话。**账本两条都留着**，这里只管画面。
/// `$f` 换成调用处的表别名
const REPRESENTED_BY_TYPED: &str = "EXISTS (SELECT 1 FROM typed_fact_sources src
                    JOIN facts ty ON ty.id = src.fact_id
                   WHERE src.statement_id = $f.id AND ty.invalidated_at IS NULL)";

fn represented_by_typed(alias: &str) -> String {
    REPRESENTED_BY_TYPED.replace("$f", alias)
}

/// 一条类型化行背后那些陈述的原话，去重拼起来（几条陈述可能算出同一行，见 0068）。
fn said_as(alias: &str) -> String {
    format!(
        "(SELECT string_agg(DISTINCT st.phrase, ' · ')
            FROM typed_fact_sources src JOIN facts st ON st.id = src.statement_id
           WHERE src.fact_id = {alias}.id)"
    )
}

async fn edges_among(
    pool: &PgPool,
    kb_id: Uuid,
    ids: &[Uuid],
    at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Vec<GraphEdge>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // **派生边显式 UNION 进来。** 它们住在 `derived_facts`，不在 `facts` 里——
    // 所以每一个想看到推理结果的读路径都得像这里一样写出来。忘了写的后果是
    // 看不见派生，而不是把它们当成谁的断言（那正是分表买到的东西）。
    //
    // 图要它们，因为「这条边是推出来的」正是用户该看见的信息之一；`derived`
    // 那一位让界面画得出区别，也让人整体过滤掉。
    //
    // 第三段是**幽灵边**（0017 §3）：推出来却没落地的派生，住在 `axiom_violations`
    // 的 `detail` 里。它的 id 是违规的 id；`derived` 与 `blocked` 同时为 true，
    // 界面据此让它跟着派生开关走、画成争议色往背景混的那一档。
    //
    // 断言那一段多算一位 `contested`：有 open 的违规或时态冲突指着它。派生撞断言
    // 时被撞的是 left；right 只是最后一条前提，它本身没有争议
    let mut edges: Vec<GraphEdge> = sqlx::query_as(&format!(
        "SELECT f.id, {subject} AS source, {object} AS target,
                COALESCE(r.key, fact_surface_predicate(f.id)) AS predicate,
                COALESCE(r.label, fact_surface_predicate(f.id)) AS label,
                {said_as} AS said_as,
                r.id IS NULL AS inferred, FALSE AS derived, NULL::text AS rule,
                ARRAY[]::uuid[] AS premises,
                f.valid_from, f.valid_to,
                {holds_from} AS holds_from, {holds_to} AS holds_to, f.confidence,
                (EXISTS (SELECT 1 FROM axiom_violations v
                          WHERE {violation_open}
                            AND (v.left_fact = f.id
                                 OR (v.right_fact = f.id AND v.kind <> 'derived_contradiction')))
                 OR EXISTS (SELECT 1 FROM fact_conflicts c
                             WHERE {conflict_open}
                               AND (c.old_fact_id = f.id OR c.new_fact_id = f.id))
                ) AS contested,
                FALSE AS blocked
         FROM facts f LEFT JOIN relation_types r ON r.id = f.predicate_id
         WHERE f.kb_id = $1 AND {facts_held} AND f.object_id IS NOT NULL
           AND {subject} = ANY($2) AND {object} = ANY($2)
           AND {facts_hold}
           AND NOT {represented}
         UNION ALL
         SELECT d.id, d.subject_id AS source, d.object_id AS target,
                r.key AS predicate, r.label AS label, NULL::text AS said_as,
                FALSE AS inferred, TRUE AS derived, ru.kind AS rule,
                ARRAY(SELECT fd.premise_fact_id FROM fact_derivations fd
                       WHERE fd.derived_fact_id = d.id
                         AND fd.premise_fact_id IS NOT NULL
                       ORDER BY fd.seq) AS premises,
                d.valid_from, d.valid_to,
                d.valid_from AS holds_from, d.valid_to AS holds_to, d.confidence,
                FALSE AS contested, FALSE AS blocked
         FROM derived_facts d JOIN relation_types r ON r.id = d.predicate_id
                              JOIN rules ru ON ru.id = d.rule_id
         WHERE d.kb_id = $1 AND {derived_held}
           AND d.subject_id = ANY($2) AND d.object_id = ANY($2)
           AND {derived_hold}
         UNION ALL
         SELECT v.id,
                (v.detail->>'subject_id')::uuid AS source,
                (v.detail->>'object_id')::uuid AS target,
                v.detail->>'predicate' AS predicate, v.detail->>'predicate' AS label,
                NULL::text AS said_as,
                FALSE AS inferred, TRUE AS derived, v.detail->>'rule' AS rule,
                v.path AS premises,
                (v.detail->>'valid_from')::timestamptz AS valid_from,
                (v.detail->>'valid_to')::timestamptz AS valid_to,
                (v.detail->>'valid_from')::timestamptz AS holds_from,
                (v.detail->>'valid_to')::timestamptz AS holds_to,
                0::real AS confidence,
                TRUE AS contested, TRUE AS blocked
         FROM axiom_violations v
         WHERE v.kb_id = $1 AND v.kind = 'derived_contradiction' AND {violation_open}
           AND (v.detail->>'subject_id')::uuid = ANY($2)
           AND (v.detail->>'object_id')::uuid = ANY($2)
           AND {ghost_hold}",
        // 世界轴（0022）：三段都从 world_axis 拼，读点上不再手写 NULL 的含义
        facts_hold = crate::world_axis::facts_hold_at("f", 3),
        said_as = said_as("f"),
        represented = represented_by_typed("f"),
        derived_hold = crate::world_axis::derived_hold_at("d", 3),
        ghost_hold = crate::world_axis::interval_holds_at(
            "(v.detail->>'valid_from')::timestamptz",
            "(v.detail->>'valid_to')::timestamptz",
            3,
        ),
        holds_from = crate::world_axis::facts_holds_from("f"),
        holds_to = crate::world_axis::facts_holds_to("f"),
        facts_held = crate::record_axis::facts_held_at("f", as_of.map(|_| 4)),
        derived_held = crate::record_axis::derived_held_at("d", as_of.map(|_| 4)),
        violation_open = crate::record_axis::violation_open_at("v", as_of.map(|_| 4)),
        conflict_open = crate::record_axis::conflict_open_at("c", as_of.map(|_| 4)),
        // 派生边不跟着倒：它们由引擎按当时的断言推出，主宾从来没被合并改写过
        subject = crate::record_axis::owner_at("f", "subject_id", as_of.map(|_| 4), false),
        object = crate::record_axis::owner_at("f", "object_id", as_of.map(|_| 4), true),
    ))
    .bind(kb_id)
    .bind(ids)
    .bind(at)
    .bind(as_of)
    .fetch_all(pool)
    .await?;
    // 边上的属性另一张表（0037），按 id 一次取回补上
    {
        let ids: Vec<Uuid> = edges.iter().map(|x| x.id).collect();
        let mut by_fact = fact_qualifiers_for(pool, &ids).await?;
        for x in edges.iter_mut() {
            if let Some(q) = by_fact.remove(&x.id) {
                x.qualifiers = q;
            }
        }
    }
    Ok(edges)
}

/// 邻域扩展（BFS，最多 2 跳，节点数封顶）。
pub async fn neighborhood(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    hops: u8,
    at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<(Vec<GraphNode>, Vec<GraphEdge>)> {
    const MAX_NODES: usize = 300;
    let mut seen: HashSet<Uuid> = HashSet::from([entity_id]);
    let mut frontier: Vec<Uuid> = vec![entity_id];

    for _ in 0..hops.clamp(1, 2) {
        if frontier.is_empty() || seen.len() >= MAX_NODES {
            break;
        }
        // 铺开也走记录轴：邻居按**当时**的边找，否则回放的图上会长出
        // 只有今天才连得上的节点
        let touching: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(&format!(
            "SELECT {subject}, {object} FROM facts f
             WHERE f.kb_id = $1 AND {facts_held} AND f.object_id IS NOT NULL
               AND ({subject} = ANY($2) OR {object} = ANY($2))",
            facts_held = crate::record_axis::facts_held_at("f", as_of.map(|_| 3)),
            subject = crate::record_axis::owner_at("f", "subject_id", as_of.map(|_| 3), false),
            object = crate::record_axis::owner_at("f", "object_id", as_of.map(|_| 3), true),
        ))
        .bind(kb_id)
        .bind(&frontier)
        .bind(as_of)
        .fetch_all(pool)
        .await?;

        let mut next = Vec::new();
        for (s, o) in touching {
            for id in [Some(s), o].into_iter().flatten() {
                if seen.len() >= MAX_NODES {
                    break;
                }
                if seen.insert(id) {
                    next.push(id);
                }
            }
        }
        frontier = next;
    }

    let ids: Vec<Uuid> = seen.into_iter().collect();
    let nodes: Vec<GraphNode> = sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND e.id = ANY($2) AND {visible}",
        node_sql(as_of.map(|_| 3), as_of.map(|_| 3)),
        visible = crate::record_axis::entity_visible_at("e", as_of.map(|_| 3)),
    ))
    .bind(kb_id)
    .bind(&ids)
    .bind(as_of)
    .fetch_all(pool)
    .await?;
    let edges = edges_among(pool, kb_id, &ids, at, as_of).await?;
    Ok((nodes, edges))
}

/// 这批实体的上下文画像与一个向量的余弦距离；没有画像的不在结果里。
/// 图谱工具拿用户的问题来比：同名的几个里，谁的画像离问题近，问的多半是谁
pub async fn profile_distances(
    pool: &PgPool,
    kb_id: Uuid,
    ids: &[Uuid],
    embedding: &[f32],
) -> AppResult<Vec<(Uuid, f64)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(Uuid, f64)> = sqlx::query_as(
        "SELECT e.id, (e.profile_embedding <=> $3)::float8
           FROM entities e
          WHERE e.kb_id = $1 AND e.id = ANY($2) AND e.profile_embedding IS NOT NULL",
    )
    .bind(kb_id)
    .bind(ids)
    .bind(pgvector::Vector::from(embedding.to_vec()))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 按名字找实体。**一并回总数**——「宁分勿合」本来就会造出一堆同名，
/// 固定十条的时候，想找的那个可能根本不在这十条里而界面上看不出来。
///
/// **名字完全相同的排在最前**（规范名或现行的别名），再按度数。只按度数的话，
/// 问「Apple」而库里有九个事实更多的「Apple Store …」，叫 Apple 的那个排到第十，
/// 对话与 MCP 的工具只读前八条——它找不到，按名字读事实时读成了另一个
pub async fn search_entities(
    pool: &PgPool,
    kb_id: Uuid,
    text: &str,
    limit: i64,
    offset: i64,
    // 记录轴（0019）：给了就按**当时**回放——列出当时可见的实体（合并之前的被并者
    // 还在，之后才建的不在），度数按当时谁持有事实来数，与回放中的画布一致
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<(Vec<GraphNode>, i64)> {
    let pattern = format!("%{}%", text.trim());
    let named = crate::names::has_name_like("e", 2);
    // 同名的键与召回同一种写法（`resolution` 里 `has_name_in` 的用法）：规整之后小写
    let exact = vec![crate::resolution::normalize_name(text).to_lowercase()];
    // 不回放时 SQL 里没有时刻参数，与从前逐字相同；回放时才多绑一个
    let visible = |param: usize| crate::record_axis::entity_visible_at("e", as_of.map(|_| param));
    let rewind = as_of.map(|_| 6);
    let sql = format!(
        "{} WHERE e.kb_id = $1 AND {visible}
         AND (e.canonical_name ILIKE $2 OR {named})
         ORDER BY (lower(e.canonical_name) = ANY($5) OR {same_name}) DESC,
                  degree DESC, e.canonical_name, e.id LIMIT $3 OFFSET $4",
        node_sql(rewind, rewind),
        visible = visible(6),
        same_name = crate::names::has_name_in("e", 1, 5),
    );
    let mut nodes_query = sqlx::query_as::<_, GraphNode>(&sql)
        .bind(kb_id)
        .bind(&pattern)
        .bind(limit)
        .bind(offset)
        .bind(&exact);
    if let Some(t) = as_of {
        nodes_query = nodes_query.bind(t);
    }
    let nodes: Vec<GraphNode> = nodes_query.fetch_all(pool).await?;
    let count_sql = format!(
        "SELECT count(*) FROM entities e
          WHERE e.kb_id = $1 AND {visible}
            AND (e.canonical_name ILIKE $2 OR {named})",
        visible = visible(3),
    );
    let mut count_query = sqlx::query_as::<_, (i64,)>(&count_sql)
        .bind(kb_id)
        .bind(&pattern);
    if let Some(t) = as_of {
        count_query = count_query.bind(t);
    }
    let (total,) = count_query.fetch_one(pool).await?;
    Ok((nodes, total))
}

/// 一个实体节点本身，不带事实：只想知道它在不在这个库、叫什么的时候用。
/// `entity_detail` 会把全部事实一起读出来，一个枢纽实体就是几百行
pub async fn entity_node(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Option<GraphNode>> {
    Ok(sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND e.id = $2",
        node_sql(as_of.map(|_| 3), as_of.map(|_| 3))
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(as_of)
    .fetch_optional(pool)
    .await?)
}

/// 本库人改区间时写下的备注（`fact.time_corrected` 审计记在被改的那一行上，#970）：每个被改
/// 的行取最近那次的。查询开头取一次，只有带着人改标记（[`corrected_ends_sql`]）的行才顺着
/// 自己的 supersedes 链对它。取的这一次走部分索引 `audit_events_time_corrected_idx`（迁移
/// 0097）：只收这一种动作，不读这个库别的审计。`$1` 是库
pub(crate) const TIME_CORRECTED_TARGETS: &str = "time_corrected_targets AS MATERIALIZED (
         SELECT DISTINCT ON (target_id) target_id, detail->>'note' AS note FROM audit_events
          WHERE kb_id = $1 AND action = 'fact.time_corrected' AND target_id IS NOT NULL
          ORDER BY target_id, created_at DESC)";

/// 人改过这一行的哪一端：`start` / `end` / `both`，NULL 是没改过（`facts.corrected_ends`，
/// 随修正行在同一个事务里写下，关上、搬移时随行带着，#970）。终点是时间线推出来的
/// （`end_derived`）就不再说终点是人改的：那一端后来是时间线定的
pub(crate) fn corrected_ends_sql(alias: &str) -> String {
    format!(
        "(CASE WHEN {alias}.end_derived AND {alias}.corrected_ends = 'end' THEN NULL
               WHEN {alias}.end_derived AND {alias}.corrected_ends = 'both' THEN 'start'
               ELSE {alias}.corrected_ends END)"
    )
}

/// 人改区间时写下的备注（#970 第二步）：链上最近那一次修正的。那一次没写备注就是空的——
/// 更早一次的备注说的是被它取代的那个区间
pub(crate) fn correction_note_sql(alias: &str) -> String {
    format!(
        "(WITH RECURSIVE chain(id, depth) AS (
              SELECT {alias}.supersedes, 1 WHERE {alias}.supersedes IS NOT NULL
              UNION ALL
              SELECT p.supersedes, chain.depth + 1 FROM facts p JOIN chain ON p.id = chain.id
               WHERE p.supersedes IS NOT NULL)
          SELECT t.note FROM chain JOIN time_corrected_targets t ON t.target_id = chain.id
           ORDER BY chain.depth LIMIT 1)"
    )
}

/// 时间线把 `f` 关上时接的那一行（#970 第二步），作为 `closer` 接进查询。
///
/// 与引擎关它时同一判法（`temporal::desired_ends`）：同一谓词、同一条时间线——functional
/// 按主语，inverse functional 按宾语——起点正好是这一行的终点，值不同。起点锚不到的、看图
/// 描述出来的后任引擎不拿来关，这里也不认；几行都合时取 id 最小的，与引擎的排序一样。
/// 只找 `end_derived` 的行；后任没有起点（结束了不知哪天）的找不到。
///
/// 接任的那一端：同一宾语换了主语（一个项目换了 lead）是新的主语；同一主语换了宾语是新的
/// 宾语，属性事实是新的值。`subject` / `object` 是 `f` 按记录轴算的属主，`as_of` 是参数位
pub(crate) fn closed_by_join(subject: &str, object: &str, as_of: Option<usize>) -> String {
    let n_subject = crate::record_axis::owner_at("n", "subject_id", as_of, false);
    let n_object = crate::record_axis::owner_at("n", "object_id", as_of, true);
    let n_held = crate::record_axis::facts_held_at("n", as_of);
    let described = crate::temporal::described_sql("n");
    format!(
        "LEFT JOIN LATERAL (
             SELECT n.id AS closed_by_id,
                    CASE WHEN rn.inverse_functional AND {n_object} = {object}
                         THEN n_s.canonical_name ELSE n_o.canonical_name END AS closed_by,
                    CASE WHEN rn.inverse_functional AND {n_object} = {object}
                         THEN NULL ELSE n.object_value END AS closed_by_value
               FROM facts n
               JOIN relation_types rn ON rn.id = n.predicate_id
               LEFT JOIN entities n_s ON n_s.id = {n_subject}
               LEFT JOIN entities n_o ON n_o.id = {n_object}
              WHERE f.end_derived AND f.valid_to IS NOT NULL
                AND n.kb_id = f.kb_id AND n.predicate_id = f.predicate_id AND n.id <> f.id
                AND {n_held}
                AND n.valid_from = f.valid_to
                AND n.valid_from_grade IS DISTINCT FROM 'C'
                AND NOT {described}
                AND ((rn.functional AND {n_subject} = {subject}
                      AND ({n_object} IS DISTINCT FROM {object}
                           OR n.object_value IS DISTINCT FROM f.object_value))
                  OR (rn.inverse_functional AND {n_object} = {object}
                      AND {n_subject} <> {subject}))
              ORDER BY n.id LIMIT 1) closer ON true"
    )
}

/// 实体详情：节点信息 + 事实时间线。
pub async fn entity_detail(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<(GraphNode, Vec<EntityFact>)> {
    let node = entity_node(pool, kb_id, entity_id, as_of)
        .await?
        .ok_or(AppError::NotFound)?;

    let mut facts: Vec<EntityFact> = sqlx::query_as(&format!(
        "WITH {targets}
         SELECT f.id, {said_as} AS said_as, f.recorded_at, f.invalidated_at, f.supersedes,
                ARRAY(SELECT DISTINCT fe.document_id FROM fact_evidence fe
                      WHERE fe.fact_id = f.id AND fe.document_id IS NOT NULL
                      ORDER BY fe.document_id) AS document_ids,
                CASE WHEN {subject} = $2 THEN 'out' ELSE 'in' END AS direction,
                COALESCE(r.key, fact_surface_predicate(f.id)) AS predicate_key,
                COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate_label,
                r.id IS NULL AS inferred, r.temporal,
                CASE WHEN {subject} = $2 THEN {object} ELSE {subject} END AS other_id,
                o.canonical_name AS other_name, ot.label AS other_type, f.object_value,
                f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision,
                {holds_from} AS holds_from, {holds_to} AS holds_to, f.attested_by, f.confidence,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count,
                (EXISTS (SELECT 1 FROM fact_evidence fe WHERE fe.fact_id = f.id)
                 AND NOT EXISTS (SELECT 1 FROM fact_evidence fe
                                 JOIN chunks c ON c.id = fe.chunk_id
                                 WHERE fe.fact_id = f.id AND {chunk_live})
                ) AS stale,
                (f.supersedes IS NOT NULL) AS corrected,
                f.end_derived, {corrected_ends} AS corrected_ends,
                {corrected_ends} IS NOT NULL AS time_corrected,
                closer.closed_by_id, closer.closed_by, closer.closed_by_value,
                CASE WHEN {corrected_ends} IS NOT NULL THEN {correction_note} END
                    AS correction_note,
                (SELECT MAX(COALESCE(d.doc_time, d.created_at))
                 FROM fact_evidence fe JOIN documents d ON d.id = fe.document_id
                 WHERE fe.fact_id = f.id) AS last_evidence_time,
                COALESCE(
                    (SELECT jsonb_build_object(
                                'kind', v.kind, 'ref_id', v.id,
                                'derived', CASE WHEN v.kind = 'derived_contradiction'
                                    THEN (v.detail->>'subject') || ' · '
                                         || (v.detail->>'predicate') || ' · '
                                         || (v.detail->>'object') END)
                       FROM axiom_violations v
                      WHERE {violation_open}
                        AND (v.left_fact = f.id
                             OR (v.right_fact = f.id AND v.kind <> 'derived_contradiction'))
                      ORDER BY v.detected_at DESC LIMIT 1),
                    (SELECT jsonb_build_object('kind', 'temporal_conflict', 'ref_id', c.id)
                       FROM fact_conflicts c
                      WHERE {conflict_open}
                        AND (c.old_fact_id = f.id OR c.new_fact_id = f.id)
                      ORDER BY c.created_at DESC LIMIT 1)
                ) AS contested
         FROM facts f
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o
           ON o.id = CASE WHEN {subject} = $2 THEN {object} ELSE {subject} END
         LEFT JOIN entity_types ot ON ot.id = o.type_id
         {closer}
         WHERE f.kb_id = $1 AND {facts_held} AND {facts_hold}
           AND NOT {represented}
           AND ({subject} = $2 OR {object} = $2)
           AND {not_name}
         ORDER BY f.valid_from NULLS LAST, f.recorded_at",
        targets = TIME_CORRECTED_TARGETS,
        corrected_ends = corrected_ends_sql("f"),
        correction_note = correction_note_sql("f"),
        closer = closed_by_join(
            &crate::record_axis::owner_at("f", "subject_id", as_of.map(|_| 3), false),
            &crate::record_axis::owner_at("f", "object_id", as_of.map(|_| 3), true),
            as_of.map(|_| 3),
        ),
        not_name = crate::names::not_a_name("f"),
        said_as = said_as("f"),
        represented = represented_by_typed("f"),
        facts_held = crate::record_axis::facts_held_at("f", as_of.map(|_| 3)),
        facts_hold = crate::world_axis::facts_hold_at("f", 4),
        holds_from = crate::world_axis::facts_holds_from("f"),
        holds_to = crate::world_axis::facts_holds_to("f"),
        subject = crate::record_axis::owner_at("f", "subject_id", as_of.map(|_| 3), false),
        object = crate::record_axis::owner_at("f", "object_id", as_of.map(|_| 3), true),
        chunk_live = crate::record_axis::chunk_live_at("c", as_of.map(|_| 3)),
        violation_open = crate::record_axis::violation_open_at("v", as_of.map(|_| 3)),
        conflict_open = crate::record_axis::conflict_open_at("c", as_of.map(|_| 3)),
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(as_of)
    .bind(at)
    .fetch_all(pool)
    .await?;
    // 边上的属性另一张表（0037），按 id 一次取回补上
    {
        let ids: Vec<Uuid> = facts.iter().map(|x| x.id).collect();
        let mut by_fact = fact_qualifiers_for(pool, &ids).await?;
        for x in facts.iter_mut() {
            if let Some(q) = by_fact.remove(&x.id) {
                x.qualifiers = q;
            }
        }
    }

    Ok((node, facts))
}

/// 人工修正实体的类型或名字。返回 (改前快照, 改后状态)——调用方据此记审计台账。
///
/// 类型判错、名字抽歪，此前只能整库重抽这把大锤。抽取给的是初判，不是定论。
///
/// 同名不拦：同类同名的两个实体是"宁分勿合"的正当产物（两个张伟），
/// 拦下来就录不进第二个。碰撞由调用方查出后提示合并，见 `same_name_peers`。
pub async fn update_entity(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    type_id: Option<Uuid>,
    canonical_name: Option<&str>,
) -> AppResult<(GraphNode, GraphNode)> {
    let before: GraphNode = sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND e.id = $2 AND e.merged_into IS NULL",
        node_sql(None, None)
    ))
    .bind(kb_id)
    .bind(entity_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let new_name = match canonical_name {
        Some(raw) => {
            let n = raw.trim();
            if n.is_empty() {
                return Err(AppError::invalid(
                    "entity_name_required",
                    "Name cannot be empty",
                ));
            }
            // 与抽取侧同一上限：越过这条线的多半是整句被当成了名字
            if n.chars().count() > 100 {
                return Err(AppError::invalid(
                    "entity_name_too_long",
                    "Name is too long (max 100)",
                ));
            }
            Some(n)
        }
        None => None,
    };

    if let Some(t) = type_id {
        let exists: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM entity_types WHERE id = $1 AND kb_id = $2")
                .bind(t)
                .bind(kb_id)
                .fetch_optional(pool)
                .await?;
        if exists.is_none() {
            return Err(AppError::invalid(
                "unknown_entity_type",
                "No such entity type in this KB",
            ));
        }
    }

    sqlx::query(
        // 改了类型才标 human——这个端点也用来改名字，只改名不该顺手把类型
        // 的来源盖成人工。`$3 IS NULL` 在这个接口里表示「本次没提供类型」，
        // 不是「把类型清空」：路由层要求两个字段至少给一个，给不出三态。
        //
        // 于是有一件今天做不到的事：0009 之后「没有类型」可能是人的决定
        //（看过了，本体里没有合适的类），而这个接口表达不了它。要补得让
        // 请求体区分「未提供」与「显式置空」，那是另一件事
        "UPDATE entities
         SET type_id = COALESCE($3, type_id),
             canonical_name = COALESCE($4, canonical_name),
             type_source = CASE WHEN $3::uuid IS NULL THEN type_source ELSE 'human' END,
             updated_at = now()
         WHERE id = $1 AND kb_id = $2 AND merged_into IS NULL",
    )
    .bind(entity_id)
    .bind(kb_id)
    .bind(type_id)
    .bind(new_name)
    .execute(pool)
    .await?;

    // 人改的名字也是一条名字事实（0041）。旧名字不作废：之前的文档还管它叫旧名字，
    // 召回靠它认出来；它只是不再是界面上显示的那一个
    if let Some(n) = new_name {
        crate::names::record(pool, kb_id, entity_id, n, None, None).await?;
    }

    // 消歧后缀依赖名字分组与类型标签（类型标签是它的兜底值），两者都刚被改过。
    // 改名要刷两组：旧名那组可能掉到 1 个（后缀该清掉），新名那组可能涨到 2 个。
    if let Some(n) = new_name.filter(|n| !n.eq_ignore_ascii_case(&before.name)) {
        crate::resolution::refresh_disambiguators(pool, kb_id, &before.name).await?;
        crate::resolution::refresh_disambiguators(pool, kb_id, n).await?;
    } else if type_id.is_some() {
        crate::resolution::refresh_disambiguators(pool, kb_id, &before.name).await?;
    }

    let after: GraphNode = sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND e.id = $2",
        node_sql(None, None)
    ))
    .bind(kb_id)
    .bind(entity_id)
    .fetch_one(pool)
    .await?;
    Ok((before, after))
}

/// 与给定实体同名（不区分大小写）的其他存活实体——用于改名后提示"是否合并"。
/// 只报告，不阻断：判定它们是否真是同一个，是人的事。
/// 同名的那一栏要跟着面板上的滑杆走（0019 / #307）。
///
/// 不传时间时退回到今天：合并掉的实体不算、昨天及之前的边都数，与现状一致。
/// 传一个时间：把 `entity_visible_at` 挂上去，三月并掉的「张伟」在二月又会
/// 重新出现在同名列——而这正是面板想告诉人的事
pub async fn same_name_peers(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Vec<GraphNode>> {
    let visible = crate::record_axis::entity_visible_at("e", as_of.map(|_| 3));
    sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND {visible} AND e.id <> $2
           AND lower(e.canonical_name) = (SELECT lower(canonical_name) FROM entities WHERE id = $2)
         ORDER BY degree DESC LIMIT 10",
        // 度数也倒回当时谁持有事实：合并把事实搬到了目标身上，只按记录轴过滤、
        // 不倒回主宾，被并的那个在合并之前也显示 0（与画布、面板不一致）
        node_sql(as_of.map(|_| 3), as_of.map(|_| 3)),
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(as_of)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// 低置信 live 事实（审核页）。
pub async fn low_confidence_facts(
    pool: &PgPool,
    kb_id: Uuid,
    below: f32,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<FactReviewItem>> {
    let rows: Vec<FactReviewItem> = sqlx::query_as(
        "SELECT f.id, s.canonical_name AS subject_name, COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate_label,
                COALESCE(o.canonical_name, f.object_value->>'summary') AS object_name,
                f.valid_from, f.valid_to, f.confidence,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count,
                (SELECT fe.quote FROM fact_evidence fe
                 WHERE fe.fact_id = f.id AND fe.quote IS NOT NULL LIMIT 1) AS quote
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND f.confidence < $2
         ORDER BY f.confidence, f.recorded_at DESC, f.id DESC
         LIMIT $3 OFFSET $4",
    )
    .bind(kb_id)
    .bind(below)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// "证据全部停留在旧版"的现行事实（S3 第三刀：文档新版没再确认的知识）。
/// 判定纯派生自 chunk 存活性——认领机制保证未变段落的证据不被误伤；
/// 绝不自动删除（没再提 ≠ 不成立），删除/闭合权在 Review 的人手里。
///
/// **「现行」这一条不能少。** 闭合是作废+改写，修正行带着原证据（`temporal::close_superseded`），
/// 只看证据存活性的话，刚闭合的行立刻回到这一档——那个出路等于不存在。
/// WHERE 与 `review::UNCONFIRMED_FACT` 同一套（0022：「结束不知哪天」不是开放）。
pub async fn stale_facts(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<FactReviewItem>> {
    let rows: Vec<FactReviewItem> = sqlx::query_as(
        "SELECT f.id, s.canonical_name AS subject_name, COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate_label,
                COALESCE(o.canonical_name, f.object_value->>'summary') AS object_name,
                f.valid_from, f.valid_to, f.confidence,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count,
                (SELECT fe.quote FROM fact_evidence fe
                 WHERE fe.fact_id = f.id AND fe.quote IS NOT NULL LIMIT 1) AS quote
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
           AND f.valid_to IS NULL AND f.valid_to_precision IS NULL
           AND EXISTS (SELECT 1 FROM fact_evidence fe WHERE fe.fact_id = f.id)
           AND NOT EXISTS (SELECT 1 FROM fact_evidence fe
                           JOIN chunks c ON c.id = fe.chunk_id
                           WHERE fe.fact_id = f.id AND c.superseded_at IS NULL)
         ORDER BY f.recorded_at DESC, f.id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 人工确认低置信事实：置信度提到 1.0。
pub async fn confirm_fact(pool: &PgPool, kb_id: Uuid, fact_id: Uuid) -> AppResult<()> {
    let res = sqlx::query(
        "UPDATE facts SET confidence = 1.0 WHERE id = $1 AND kb_id = $2 AND invalidated_at IS NULL",
    )
    .bind(fact_id)
    .bind(kb_id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    // 从前这里还有一段：确认 `mapped_to` 事实时把同 (概念, 源) 的旧映射作废。
    // 映射已搬出账本（0011，`concept_mappings` 自己管唯一性），那段 SQL 恒匹配零行，删了。
    Ok(())
}

/// 人工否决事实：作废（账本 append-only，不 DELETE）。
pub async fn reject_fact(pool: &PgPool, kb_id: Uuid, fact_id: Uuid) -> AppResult<()> {
    if !crate::temporal::retract(pool, kb_id, fact_id).await? {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 反向证据链：该文档每个分块抽出了哪些 live 事实（文档查看器右栏）。
pub async fn document_extractions(
    pool: &PgPool,
    document_id: Uuid,
) -> AppResult<Vec<ChunkFactView>> {
    let rows: Vec<ChunkFactView> = sqlx::query_as(
        "SELECT fe.chunk_id, f.id AS fact_id,
                f.subject_id, s.canonical_name AS subject,
                COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate,
                r.id IS NULL AS inferred,
                -- 宾语是一样东西就用它的名字，是字面值（名字、金额、百分比）就用那个值。
                -- 只取 canonical_name 的话，每一条值宾语的陈述在阅读页右栏都是主语加短语、
                -- 后面空着一片——「Hugging Face, Inc. known as」后面什么都没有
                f.object_id, COALESCE(o.canonical_name, f.object_value #>> '{value}') AS object,
                f.valid_from, f.valid_to, f.confidence
         FROM fact_evidence fe
         JOIN chunks c ON c.id = fe.chunk_id AND c.document_id = $1
              AND c.superseded_at IS NULL
         JOIN facts f ON f.id = fe.fact_id AND f.invalidated_at IS NULL
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         ORDER BY c.seq, f.recorded_at",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 每条事实第一条仍然有效的证据块（#935）。对话把它登进这一轮的来源清单、在事实行上
/// 印它的 `[n]`，读的人点开就是那句原话。
///
/// 有效 = 分块是现行版本、文档没删；给了 `as_of` 就按那一刻判，与读事实的那一刻一致
/// （record_axis）。几条都有效时取最早说出它的那篇文档里的第一块。一条有效证据都没有
/// 的事实不在结果里——宁可不给号，也不给一个在读的那一刻已是旧版或已删文档的号
/// （给了 `as_of` 时引的就是那一刻还在的段落，哪怕它后来被替换或删了）
pub async fn first_live_evidence(
    pool: &PgPool,
    kb_id: Uuid,
    fact_ids: &[Uuid],
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Vec<(Uuid, utopia_core::models::ChunkView, Option<String>)>> {
    if fact_ids.is_empty() {
        return Ok(Vec::new());
    }
    #[derive(sqlx::FromRow)]
    struct Row {
        fact_id: Uuid,
        id: Uuid,
        document_id: Uuid,
        seq: i32,
        text: String,
        filename: String,
        quote: Option<String>,
    }
    // 连同那条证据的引文一起取（#968 的后续）：号打开的是那一块，引文是块里说出这条事实的
    // 那句话。同一块里几条证据时取块里最靠前的那一句，排序到此为止才是确定的
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT DISTINCT ON (fe.fact_id) fe.fact_id, c.id, c.document_id, c.seq, c.text, d.filename,
                NULLIF(btrim(fe.quote), '') AS quote
           FROM fact_evidence fe
           JOIN chunks c ON c.id = fe.chunk_id
           JOIN documents d ON d.id = c.document_id
          WHERE fe.fact_id = ANY($1) AND d.kb_id = $2 AND {chunk_live} AND {document_live}
          ORDER BY fe.fact_id, COALESCE(d.doc_time, d.created_at), d.id, c.seq, c.id",
        chunk_live = crate::record_axis::chunk_live_at("c", as_of.map(|_| 3)),
        document_live = crate::record_axis::document_live_at("d", as_of.map(|_| 3)),
    ))
    .bind(fact_ids)
    .bind(kb_id)
    .bind(as_of)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.fact_id,
                utopia_core::models::ChunkView {
                    id: r.id,
                    document_id: r.document_id,
                    seq: r.seq,
                    text: r.text,
                    filename: r.filename,
                },
                r.quote,
            )
        })
        .collect())
}

/// 证据回放路径：不过滤 superseded——它的职责就是能看旧版。
/// stale = 证据版本落后于文档当前版本（UI 标 "from v{n}"）。
pub async fn fact_evidence(pool: &PgPool, fact_id: Uuid) -> AppResult<Vec<EvidenceView>> {
    let rows: Vec<EvidenceView> = sqlx::query_as(
        "SELECT fe.quote, fe.proposed_predicate, fe.chunk_id, c.document_id, d.filename, c.seq,
                c.doc_version,
                c.doc_version < COALESCE(
                    (SELECT MAX(version) FROM document_versions dv
                     WHERE dv.document_id = c.document_id), 1) AS stale,
                d.deleted_at IS NOT NULL AS document_deleted,
                c.origin, c.origin_model, c.anchor
         FROM fact_evidence fe
         JOIN chunks c ON c.id = fe.chunk_id
         JOIN documents d ON d.id = c.document_id
         WHERE fe.fact_id = $1",
    )
    .bind(fact_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 清空 KB 的整个图层（Rebuild graph 的清算语义）：实体/事实/证据/待审/冲突/合并
/// 记录全删，本体（类与关系定义）与文档/分块/嵌入保留。
///
/// 刻意保留两样：决策台账（audit_events，快照自包含，图没了记录仍可读）与裁决
/// 缓存（resolution_verdicts，重建后同名对重现直接命中，省一批 LLM 调用）。
/// 返回 (删除实体数, 删除事实数)。
pub async fn purge_graph(pool: &PgPool, kb_id: Uuid) -> AppResult<(i64, i64)> {
    let mut tx = pool.begin().await?;
    let (entity_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM entities WHERE kb_id = $1")
        .bind(kb_id)
        .fetch_one(&mut *tx)
        .await?;
    let (fact_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM facts WHERE kb_id = $1")
        .bind(kb_id)
        .fetch_one(&mut *tx)
        .await?;

    // FK 多为 CASCADE，但两处自引用是 NO ACTION：先解引用再删，顺序显式写出
    // （这段本身就是"图层由什么构成"的定义）
    for sql in [
        "DELETE FROM fact_conflicts WHERE kb_id = $1",
        "DELETE FROM resolution_reviews WHERE kb_id = $1",
        "DELETE FROM entity_merges WHERE kb_id = $1",
        "UPDATE facts SET supersedes = NULL WHERE kb_id = $1",
        "DELETE FROM fact_evidence WHERE fact_id IN (SELECT id FROM facts WHERE kb_id = $1)",
        "DELETE FROM facts WHERE kb_id = $1",
        "UPDATE entities SET merged_into = NULL WHERE kb_id = $1",
        "DELETE FROM entities WHERE kb_id = $1",
        // 未匹配统计由抽取重新累积
        "DELETE FROM ontology_misses WHERE kb_id = $1",
    ] {
        sqlx::query(sql).bind(kb_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok((entity_count, fact_count))
}

/// 实体的认知变更历史（记录时间轴）。
///
/// 与 entity_detail 的根本差别：那里 `invalidated_at IS NULL`，只答"现在认为是什么"；
/// 这里不过滤，答"我们何时这么认为、又何时改了主意"。数据一直都在——账本
/// append-only，修正是插新行 + 标旧行作废，从不覆盖。
///
/// 一行事实最多产出两个事件：写入（asserted / corrected）与作废（rejected）。
/// 有后继修正行的作废不单独记——那次死亡已由后继那条 corrected 解释。
///
/// 归因：审计台账里 fact.close 的 target 是**被闭合的旧行**，而修正行是新插的另一行，
/// 所以按 COALESCE(supersedes, id) 回查；冲突裁决的 target 是 conflict 行，再绕一跳。
/// 查不到审计记录 = 引擎自动（抽取写入或时态对账），actor 为 NULL。
pub async fn entity_history(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<EntityHistoryEvent>, i64)> {
    const EVENTS: &str = "
        WITH ef AS (
            SELECT f.*,
                   CASE WHEN f.subject_id = $2 THEN 'out' ELSE 'in' END AS direction,
                   CASE WHEN f.subject_id = $2 THEN f.object_id ELSE f.subject_id END AS other_id
            FROM facts f
            WHERE f.kb_id = $1 AND (f.subject_id = $2 OR f.object_id = $2)
        ),
        ev AS (
            SELECT ef.*, ef.recorded_at AS at,
                   CASE WHEN ef.supersedes IS NULL THEN 'asserted' ELSE 'corrected' END AS kind
            FROM ef
            UNION ALL
            -- 作废且无后继 = 被推翻……除非它是被并进了另一条断言。那种情形下
            -- 内容一字未少，说成「撤回」就是界面在陈述一件没发生的事
            SELECT ef.*, ef.invalidated_at AS at,
                   CASE WHEN EXISTS (SELECT 1 FROM fact_adoptions fa
                                     WHERE fa.old_fact_id = ef.id AND fa.mode = 'merged'
                                       AND fa.reverted_at IS NULL)
                        THEN 'merged' ELSE 'rejected' END AS kind
            FROM ef
            WHERE ef.invalidated_at IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM facts s WHERE s.supersedes = ef.id)
        ),
        -- 改类不是事实：没有谓词、没有对方、没有方向。它来自 entity_retypes，
        -- 一行最多产出两个事件——改动本身，以及撤销。
        --
        -- **撤销过的照样显示。** 读成「改过、又撤了」，不是没发生过。同一类错
        -- 这仓库栽过两次（#37；并入被读成撤回），所以这不是防御性编程
        rt AS (
            SELECT r.created_at AS at, 'retyped' AS kind, r.actor_id,
                   tf.label AS from_type_label, tt.label AS to_type_label
            FROM entity_retypes r
            LEFT JOIN entity_types tf ON tf.id = r.from_type_id
            JOIN entity_types tt ON tt.id = r.to_type_id
            WHERE r.kb_id = $1 AND r.entity_id = $2
            UNION ALL
            SELECT r.reverted_at, 'retype_reverted', r.actor_id, tf.label, tt.label
            FROM entity_retypes r
            LEFT JOIN entity_types tf ON tf.id = r.from_type_id
            JOIN entity_types tt ON tt.id = r.to_type_id
            WHERE r.kb_id = $1 AND r.entity_id = $2 AND r.reverted_at IS NOT NULL
        ),
        /* 合并也是这个实体身上的一次认识改变，而且是最大的一次：从此它和另一个
           实体算同一个东西。**从前这条轴看不见它**——只看得见合并顺手作废的那些
           事实（`merged`），于是界面在说结果，不说发生了什么。#337 之后
           `entity_merges` 有了自己的时钟（created_at / reverted_at），接进来即可。

           两个方向分开记：`merged_in` = 别人并进了它（事实搬到它名下），
           `merged_away` = 它并进了别人（这个 id 从此不再单独存在）。同一句话
           说两件事会让「谁吸收了谁」读不出来，而回滚正是按方向做的。 */
        mg AS (
            SELECT m.created_at AS at,
                   CASE WHEN m.target_id = $2 THEN 'merged_in' ELSE 'merged_away' END AS kind,
                   m.merged_by AS actor_id,
                   CASE WHEN m.target_id = $2 THEN m.source_id ELSE m.target_id END AS other_id
            FROM entity_merges m
            WHERE m.kb_id = $1 AND (m.source_id = $2 OR m.target_id = $2)
            UNION ALL
            -- 撤销的归因**不能借合并那一行的 merged_by**：撤的人常常不是当初合的人，
            -- 而自动合并那一行本来就是 NULL。撤销写审计（`merge.revert`，target 是
            -- 这次合并），从那儿取才对得上人
            SELECT m.reverted_at, 'merge_reverted',
                   (SELECT a.actor_id FROM audit_events a
                     WHERE a.kb_id = $1 AND a.action = 'merge.revert'
                       AND a.target_id = m.id
                     ORDER BY a.created_at DESC LIMIT 1),
                   CASE WHEN m.target_id = $2 THEN m.source_id ELSE m.target_id END
            FROM entity_merges m
            WHERE m.kb_id = $1 AND (m.source_id = $2 OR m.target_id = $2)
              AND m.reverted_at IS NOT NULL
        )";
    let rows: Vec<EntityHistoryEvent> = sqlx::query_as(&format!(
        "{EVENTS}
         SELECT * FROM (
         SELECT ev.id AS fact_id, ev.at, ev.kind, ev.direction,
                COALESCE(r.label, fact_surface_predicate(ev.id)) AS predicate_label, o.canonical_name AS other_name,
                ev.object_value, ev.valid_from, ev.valid_from_precision,
                ev.valid_to, ev.valid_to_precision,
                ev.confidence, act.actor_name, act.action,
                src.document_id, src.filename, src.quote,
                NULL::text AS from_type_label, NULL::text AS to_type_label
         FROM ev
         LEFT JOIN relation_types r ON r.id = ev.predicate_id
         LEFT JOIN entities o ON o.id = ev.other_id
         LEFT JOIN LATERAL (
             SELECT u.display_name AS actor_name, a.action
             FROM audit_events a
             LEFT JOIN users u ON u.id = a.actor_id
             WHERE a.kb_id = $1
               -- 断言由抽取写入，从来不是人的决定：归因只问修正与推翻这两类事件，
               -- 否则后发生的人工裁决会被错安到当初那条断言头上
               AND ev.kind <> 'asserted'
               AND a.action = ANY(CASE ev.kind
                     WHEN 'corrected' THEN
                       ARRAY['fact.close', 'conflict.close_old', 'ontology.predicate_adopted',
                             -- 人工改区间（302）。少了这一条，人做的修正在
                             -- 这条轴上归给「engine」——一个决策账本把人的
                             -- 决定记成机器的，比不记还坏
                             'fact.time_corrected']
                     -- 并入只可能由采纳造成，不会是 Review 里的拒绝
                     WHEN 'merged' THEN ARRAY['ontology.predicate_adopted']
                     ELSE ARRAY['fact.reject', 'conflict.reject_new',
                                'ontology.adoption_reverted'] END)
               AND (a.target_id = COALESCE(ev.supersedes, ev.id)
                    OR a.target_id IN (SELECT c.id FROM fact_conflicts c
                                       WHERE c.old_fact_id = COALESCE(ev.supersedes, ev.id)
                                          OR c.new_fact_id = ev.id)
                    -- 采纳与撤销都记在关系类型上、一次动作改一批事实，
                    -- 靠 fact_adoptions 精确关联到具体哪几条（corrected 事件
                    -- 是新行、merged 是旧行，两头都认）
                    OR (a.action IN ('ontology.predicate_adopted',
                                     'ontology.adoption_reverted')
                        AND EXISTS (SELECT 1 FROM fact_adoptions fa
                                    WHERE fa.predicate_id = a.target_id
                                      AND (fa.new_fact_id = ev.id
                                           OR fa.old_fact_id = ev.id))))
             ORDER BY a.created_at DESC LIMIT 1
         ) act ON true
         LEFT JOIN LATERAL (
             SELECT d.id AS document_id, d.filename, fe.quote
             FROM fact_evidence fe
             JOIN chunks c ON c.id = fe.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE fe.fact_id = ev.id
             ORDER BY fe.doc_version DESC NULLS LAST LIMIT 1
         ) src ON true
         UNION ALL
         SELECT NULL::uuid, rt.at, rt.kind, NULL::text,
                NULL::text, NULL::text,
                NULL::jsonb, NULL::timestamptz, NULL::text,
                NULL::timestamptz, NULL::text,
                NULL::real, u.display_name, NULL::text,
                NULL::uuid, NULL::text, NULL::text,
                rt.from_type_label, rt.to_type_label
         FROM rt LEFT JOIN users u ON u.id = rt.actor_id
         UNION ALL
         -- 合并事件：没有谓词、没有区间、没有证据行，对方是另一个实体。
         -- 被合并掉的那一头仍留在 entities 里（revert_merge 要按原路搬回去），
         -- 所以这里拿得到名字；LEFT JOIN 只是防库被清理过
         SELECT NULL::uuid, mg.at, mg.kind, NULL::text,
                NULL::text, o.canonical_name,
                NULL::jsonb, NULL::timestamptz, NULL::text,
                NULL::timestamptz, NULL::text,
                NULL::real, u.display_name, NULL::text,
                NULL::uuid, NULL::text, NULL::text,
                NULL::text, NULL::text
         FROM mg
         LEFT JOIN entities o ON o.id = mg.other_id
         LEFT JOIN users u ON u.id = mg.actor_id
         ) x
         -- 改类型与合并那两支没有 fact_id，同一刻的几行只靠它排不出先后（#646）：
         -- 再按种类，最后按整行——两行连整行都一样，谁先谁后看不出差别
         ORDER BY x.at DESC, x.fact_id, x.kind, x::text
         LIMIT $3 OFFSET $4"
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    // 总数把改类、合并那两支也算上，否则分页会少一截
    let (total,): (i64,) = sqlx::query_as(&format!(
        "{EVENTS} SELECT (SELECT count(*) FROM ev) + (SELECT count(*) FROM rt)                        + (SELECT count(*) FROM mg)"
    ))
    .bind(kb_id)
    .bind(entity_id)
    .fetch_one(pool)
    .await?;
    Ok((rows, total))
}

/// 一段记录时间窗口里，全库的认知变更。
///
/// **窗口开在认知轴上**：`since`/`until` 比的是 recorded_at 与 invalidated_at，
/// 不是 valid_from/valid_to。这是与 `entity_facts(at)` 唯一也是全部的区别——
/// 那个问"某时刻世界什么样"，这个问"某段时间里我们改了什么主意"。两者查同一张表、
/// 用不同的列，混起来会安静地给出一个看着合理的错答案。
///
/// 事件推导与 `entity_history` 同源（见那里的注释）：一条事实行最多产出两个事件，
/// 且已被后继修正的死亡不重复记。
pub async fn graph_changes(
    pool: &PgPool,
    kb_id: Uuid,
    since: chrono::DateTime<chrono::Utc>,
    until: chrono::DateTime<chrono::Utc>,
    entity_id: Option<Uuid>,
    kinds: Option<&[String]>,
    limit: i64,
) -> AppResult<Vec<GraphChange>> {
    // 两个分支各自按**自己那根时间列**开窗，而不是先union再过滤：
    // 一条 2 月写入、8 月被推翻的事实，在"3–4 月"窗口里两个事件都不该出现
    const EVENTS: &str = "
        WITH ev AS (
            SELECT f.id, f.subject_id, f.predicate_id, f.object_id, f.object_value,
                   f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision, f.confidence,
                   f.recorded_at AS at,
                   CASE WHEN f.supersedes IS NULL THEN 'asserted' ELSE 'corrected' END AS kind
            FROM facts f
            WHERE f.kb_id = $1 AND f.recorded_at >= $2 AND f.recorded_at < $3
              AND ($4::uuid IS NULL OR f.subject_id = $4 OR f.object_id = $4)
            UNION ALL
            SELECT f.id, f.subject_id, f.predicate_id, f.object_id, f.object_value,
                   f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision, f.confidence,
                   f.invalidated_at AS at,
                   CASE WHEN EXISTS (SELECT 1 FROM fact_adoptions fa
                                     WHERE fa.old_fact_id = f.id AND fa.mode = 'merged'
                                       AND fa.reverted_at IS NULL)
                        THEN 'merged' ELSE 'rejected' END AS kind
            FROM facts f
            WHERE f.kb_id = $1 AND f.invalidated_at >= $2 AND f.invalidated_at < $3
              AND NOT EXISTS (SELECT 1 FROM facts s WHERE s.supersedes = f.id)
              AND ($4::uuid IS NULL OR f.subject_id = $4 OR f.object_id = $4)
        )";
    Ok(sqlx::query_as(&format!(
        "{EVENTS}
         SELECT ev.id AS fact_id, ev.at, ev.kind,
                ev.subject_id, s.canonical_name AS subject_name,
                COALESCE(r.label, fact_surface_predicate(ev.id)) AS predicate_label, o.canonical_name AS object_name,
                ev.object_value, ev.valid_from, ev.valid_from_precision,
                ev.valid_to, ev.valid_to_precision,
                ev.confidence, src.document_id, src.filename, src.quote, src.quote_origin
         FROM ev
         LEFT JOIN relation_types r ON r.id = ev.predicate_id
         JOIN entities s ON s.id = ev.subject_id
         LEFT JOIN entities o ON o.id = ev.object_id
         LEFT JOIN LATERAL (
             SELECT d.id AS document_id, d.filename, fe.quote, c.origin AS quote_origin
             FROM fact_evidence fe
             JOIN chunks c ON c.id = fe.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE fe.fact_id = ev.id
             ORDER BY fe.doc_version DESC NULLS LAST LIMIT 1
         ) src ON true
         WHERE ($5::text[] IS NULL OR ev.kind = ANY($5))
           -- 实体的本名那条名字事实不算一次变化（0041）：每建一个实体就多一行「X known as X」，
           -- 限量的变更清单会被它挤满。新读到的别名、改名照样列出来
           AND NOT (coalesce(r.builtin AND r.key = 'known_as', false)
                    AND lower(ev.object_value->>'value') = lower(s.canonical_name))
         ORDER BY ev.at DESC, ev.id
         LIMIT $6"
    ))
    .bind(kb_id)
    .bind(since)
    .bind(until)
    .bind(entity_id)
    .bind(kinds)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod temporal_shape_tests {
    use super::*;

    fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
        s.parse().unwrap()
    }

    /// 桶的尽头是加一个精度单位；没有精度（锚点）原样
    #[test]
    fn a_bucket_ends_one_unit_later() {
        let d = at("2024-03-15T00:00:00Z");
        assert_eq!(bucket_end(d, Some("day")), at("2024-03-16T00:00:00Z"));
        assert_eq!(
            bucket_end(at("2024-03-01T00:00:00Z"), Some("month")),
            at("2024-04-01T00:00:00Z")
        );
        assert_eq!(
            bucket_end(at("2024-12-01T00:00:00Z"), Some("month")),
            at("2025-01-01T00:00:00Z"),
            "跨年"
        );
        assert_eq!(
            bucket_end(at("2024-01-01T00:00:00Z"), Some("year")),
            at("2025-01-01T00:00:00Z")
        );
        assert_eq!(
            bucket_end(at("2024-03-15T14:32:00Z"), Some("minute")),
            at("2024-03-15T14:33:00Z")
        );
        assert_eq!(bucket_end(d, None), d);
    }

    /// 事件：起点优先，只有终点取终点，一段取起点，「结束了不知哪天」抹掉
    #[test]
    fn an_event_collapses_to_one_moment() {
        let span = Validity {
            from: Some(at("2024-03-15T00:00:00Z")),
            from_precision: Some("day"),
            to: Some(at("2025-01-01T00:00:00Z")),
            to_precision: Some("day"),
            attested_at: None,
            from_grade: None,
        }
        .under(Temporal::Event)
        .unwrap();
        assert_eq!(span.from, Some(at("2024-03-15T00:00:00Z")));
        assert_eq!(span.to, Some(at("2024-03-15T00:00:00Z")));
        assert_eq!(
            (span.from_precision, span.to_precision),
            (Some("day"), Some("day"))
        );

        let end_only = Validity {
            from: None,
            from_precision: None,
            to: Some(at("2024-05-01T00:00:00Z")),
            to_precision: Some("month"),
            attested_at: None,
            from_grade: None,
        }
        .under(Temporal::Event)
        .unwrap();
        assert_eq!(end_only.from, Some(at("2024-05-01T00:00:00Z")));
        assert_eq!(end_only.from_precision, Some("month"));

        let unknown = Validity::default()
            .ended_when_unknown()
            .under(Temporal::Event)
            .unwrap();
        assert_eq!(
            (unknown.from, unknown.to, unknown.to_precision),
            (None, None, None)
        );
        assert!(!unknown.has_ended(), "一刻没有「结束」可言");
    }

    /// 恒常抹掉日期；状态原样
    #[test]
    fn an_eternal_fact_keeps_no_dates_and_a_state_keeps_its_own() {
        let dated = Validity::starting(Some(at("1990-01-01T00:00:00Z")), Some("year"))
            .attested(Some(at("2024-04-01T00:00:00Z")));
        let eternal = dated.under(Temporal::Eternal).unwrap();
        assert_eq!((eternal.from, eternal.from_precision), (None, None));
        assert_eq!(
            eternal.attested_at,
            Some(at("2024-04-01T00:00:00Z")),
            "证据日期照记——读出侧不用它，账本仍知道"
        );
        let state = dated.under(Temporal::State).unwrap();
        assert_eq!(state.from, Some(at("1990-01-01T00:00:00Z")));
    }

    /// 状态两端相等就拒绝（#966）：起止同值的一段任何时刻都不成立。事件照旧写成一刻；
    /// 截到精度之后才相等的两端同样拒绝
    #[test]
    fn a_state_that_ends_where_it_starts_is_refused() {
        let moment = Validity {
            from: Some(at("2023-06-01T00:00:00Z")),
            from_precision: Some("day"),
            to: Some(at("2023-06-01T00:00:00Z")),
            to_precision: Some("day"),
            attested_at: None,
            from_grade: None,
        };
        let refused = moment
            .under(Temporal::State)
            .expect_err("an empty state span");
        assert!(
            format!("{refused:?}").contains("empty_state_span"),
            "{refused:?}"
        );
        assert!(moment.under(Temporal::Event).is_ok());
        let same_day = Validity {
            from: Some(at("2023-06-01T09:00:00Z")),
            to: Some(at("2023-06-01T17:00:00Z")),
            ..moment
        };
        assert!(same_day.truncated().under(Temporal::State).is_err());
        assert!(Validity {
            to: None,
            to_precision: None,
            ..moment
        }
        .under(Temporal::State)
        .is_ok());
    }
}
