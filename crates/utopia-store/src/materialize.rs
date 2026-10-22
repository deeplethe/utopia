//! 绑上的签名下的开放陈述算成类型化事实（0044 决定 3 的第三、四片，见 0067、0068）。
//!
//! 类型化图谱是视图：一条类型化行 = 一个三元组，来自一条或几条开放陈述与它们签名的绑定。
//! 谓词是绑定给的属性，主宾按绑定的方向（reverse 就是陈述的宾语当主语），字面值、世界轴
//! 时间、来源时间、置信度从陈述来；证据与限定各复制一份；来源记在 `typed_fact_sources`，
//! `from_statement_id` 是第一条。带 mood 限定的陈述不算。
//!
//! 写行走类型化图谱本来的门（[`crate::graph::insert_fact`] / [`crate::graph::insert_value_fact`]）：同断言
//! 同起点复用那一行，裸行被带时间的观察取代并链上，「结束了」关上开着的行——两份文档说
//! 同一件事，时间线上是一条边。
//!
//! 重算是集合运算，跑多少遍结果一样：先删「不再成立」的来源（陈述作废了、签名不再绑着、
//! 绑到了别的属性或反了方向、行本身作废了），再作废来源全空的类型化行，最后给「该有而
//! 没有」的（陈述, 绑定）对补上——有同断言的行就并进去，没有才新建。没有模型调用。
//!
//! 写下的行随后**对账**（#899）：写路径上抽取和点头写完一条 state 事实都会沿它的唯一性
//! 方向重算时间线（`temporal::reconcile_new_fact`），物化出来的行是同一种观察，不该
//! 少这一步——否则一个函数型属性的两个值各开着一段，后一次观察关不上前一次。重算
//! 提交之后再对账：对账按时间线各自开事务、拿自己的锁，不在物化的事务里做

use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

use crate::graph::{insert_fact_on, FactObject, Validity};
use serde_json::Value;

/// 陈述与绑定对得上的条件：短语归一后相等，两端的类相同（空也相同），宾语是不是字面值相同。
/// `s` 是开放陈述（facts），`se`/`oe` 是它两端的实体，`b` 是 phrase_bindings
const MATCH: &str = "b.kb_id = s.kb_id
       AND b.phrase = lower(btrim(regexp_replace(s.phrase, '\\s+', ' ', 'g')))
       AND b.subject_type_id IS NOT DISTINCT FROM se.type_id
       AND b.object_is_value = (s.object_id IS NULL)
       AND (s.object_id IS NULL OR b.object_type_id IS NOT DISTINCT FROM oe.type_id)";

/// 一条只说了一刻的陈述（起止同值）绑在一条状态属性上怎么读，见 [`span_of`]：绑定说
/// `start` 从那一刻起，说 `end` 到那一刻为止，说 `none` 或没说按那个日期命名的时段读
/// （0053 修订 2026-10-22）。从前 `none` 和没说的不算类型化行（`MOMENT_UNREAD`），四篇公司
/// 简介上每一条年度数字都这么丢了；照抄起止会写成一段任何时刻都不成立的状态，所以终点
/// 挪到桶的尽头、标上 `end_after_period`，读的人印得出原文写的 2023
/// 属性的 datatype 与单位，值按它们读（[`attribute_value`]）；`rt` 是绑到或蕴含的那条属性，
/// 是关系时 datatype 为 NULL，值原样写
const SHAPE: &str = "CASE WHEN rt.kind = 'attribute' THEN rt.datatype END AS datatype, rt.unit";

/// 一轮重算写了什么。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Outcome {
    /// 作废的类型化行（来源全空了）
    pub retired: u64,
    /// 新建的类型化行
    pub added: u64,
    /// 并进已有行的陈述数
    pub merged: u64,
    /// 规则算出来的隐含行（0044 决定 3 第五片），新建的
    pub implied: u64,
    /// 对账自动闭合而改写出来的修正行数（#899）
    pub corrected: u64,
    /// 对账裁不了、交给人的冲突数
    pub conflicts: u32,
}

/// 一条该物化的（陈述, 绑定）对，连陈述上要抄的东西。
#[derive(sqlx::FromRow)]
struct Due {
    statement: Uuid,
    property: Uuid,
    direction: String,
    subject_id: Uuid,
    object_id: Option<Uuid>,
    /// 宾语是实体时它的名字：绑到文本属性上，名字就是值（0044 修订 2026-10-22）
    object_name: Option<String>,
    object_value: Option<serde_json::Value>,
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_from_precision: Option<String>,
    valid_from_grade: Option<String>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    valid_to_precision: Option<String>,
    attested_from: Option<chrono::DateTime<chrono::Utc>>,
    confidence: f32,
    /// 绑定说一刻标的是哪一端（start / end / none，NULL 未知）
    marks: Option<String>,
    /// 绑到的属性的时间语义
    temporal: Option<String>,
    /// 绑到的是关系还是属性
    kind: String,
    /// 绑到的是属性时它的 datatype 与单位，值按它们读（[`attribute_value`]）
    datatype: Option<String>,
    unit: Option<String>,
}

/// 对一个库重算一遍。
///
/// Worker 走这一条：等多久都行，因为它没人在屏幕前面等。人的判定不走这条：
/// 见 `materialize_human`，那是另一条带预算的入口
pub async fn materialize(pool: &PgPool, kb_id: Uuid) -> AppResult<Outcome> {
    // A worker and a human review can recompute the same base concurrently.
    // Serialize before reading due statements, and use this connection for the
    // whole recompute so waiting runs cannot exhaust the pool with lock holders.
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('typed_materialize'), hashtext($1))")
        .bind(kb_id.to_string())
        .execute(&mut *tx)
        .await?;
    let (outcome, written) = materialize_in_tx(&mut tx, kb_id).await?;
    tx.commit().await?;
    reconcile_written(pool, kb_id, outcome, &written).await
}

/// 这一轮写下（新建或并入）的类型化行沿各自的唯一性时间线对账（#899）。只有 state 且
/// 声明了唯一性的谓词有时间线，`timelines_of` 自己筛；其余的行这里是空转
async fn reconcile_written(
    pool: &PgPool,
    kb_id: Uuid,
    mut outcome: Outcome,
    written: &[Uuid],
) -> AppResult<Outcome> {
    if !written.is_empty() {
        let report = crate::temporal::reconcile_facts(pool, kb_id, written).await?;
        outcome.corrected = report.corrected.len() as u64;
        outcome.conflicts = report.conflicts;
    }
    Ok(outcome)
}

/// 任务里的入口（0051）：**试锁，不等**。拿到 `typed_materialize` 就在这条连接上
/// 跑完整的重算并提交；拿不到就回滚、返回 `None`，由调用方挂成 `Deferred` 稍后再来。
///
/// 为什么不像 `materialize` 那样等锁：等锁的是一条池里的连接，几个决定连着点下来，
/// 每个 job 都抱着一条连接排队，池就空了（0051 §Alternatives）。也为什么不像旧的
/// 人工入口那样给等待设 2 秒预算：job 没人在屏幕前面等，超时只是把同一次重算推到
/// 下一次重试，不如一开始就不等。人的那一次点击只提交决定和这个 job（同一事务，
/// `phrase_bindings::decide_with_delivery`），屏幕上等的是事件，不是锁。
pub async fn try_materialize(pool: &PgPool, kb_id: Uuid) -> AppResult<Option<Outcome>> {
    let mut tx = pool.begin().await?;
    let acquired: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtext('typed_materialize'), hashtext($1))",
    )
    .bind(kb_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if !acquired {
        tx.rollback().await?;
        return Ok(None);
    }
    let (outcome, written) = materialize_in_tx(&mut tx, kb_id).await?;
    tx.commit().await?;
    Ok(Some(
        reconcile_written(pool, kb_id, outcome, &written).await?,
    ))
}

async fn materialize_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kb_id: Uuid,
) -> AppResult<(Outcome, Vec<Uuid>)> {
    // The statements that scan the whole base (steps 0 to 3, the implication scans and the
    // attestation sync) are not cached: a cached statement switches to a generic plan after a few
    // runs, and a generic plan cannot see how many rows `kb_id = $1` selects. The source cleanup's
    // generic plan ran for hours on a production base where the plan for its own id takes two
    // seconds (#1298). The per-row statements in the loops stay cached.
    // 这一轮写下的行（新建的和并入的），提交后对账
    let mut written: Vec<Uuid> = Vec::new();
    // 0. 起止同值的物化状态行（#966 之前照抄一刻写下的）任何时刻都不成立：作废，它的陈述
    //    在第 3 步按绑定说的读法重算——「那天加入」与「自那天起」重新落成一段从那天起的状态。
    //    只认照抄来的：它有一条来源陈述两端正是这一刻（类型化行看 typed_fact_sources，隐含行
    //    看 implied_fact_sources）。人改成起止相等的行也带着 from_statement_id、implied 和
    //    来源链接（#967、#911），看上去和算出来的一样，但它的来源陈述不是这一刻，不碰：
    //    不变量从此挡住新的，旧的由人自己改。排在第 1 步之前：那一刻的陈述在绑定说出读法
    //    之前不算来源，第 1 步会先删掉这条链接。只碰算出来的行（from_statement_id 或
    //    implied，与第 2 步同一条守卫）：人手写的行并进一条陈述之后也有来源链接，它不是算的
    let emptied = sqlx::query(
        "UPDATE facts t
            SET invalidated_at = now()
          WHERE t.kb_id = $1 AND t.layer = 'typed' AND t.invalidated_at IS NULL
            AND t.valid_from IS NOT NULL AND t.valid_to = t.valid_from
            AND (t.from_statement_id IS NOT NULL OR t.implied)
            AND EXISTS (SELECT 1 FROM relation_types r
                         WHERE r.id = t.predicate_id AND r.temporal = 'state')
            AND EXISTS (SELECT 1 FROM facts s
                         WHERE s.valid_from = t.valid_from AND s.valid_to = t.valid_from
                           AND (s.id IN (SELECT ts.statement_id FROM typed_fact_sources ts
                                          WHERE ts.fact_id = t.id)
                             OR s.id IN (SELECT i.statement_id FROM implied_fact_sources i
                                          WHERE i.fact_id = t.id)))",
    )
    .persistent(false)
    .bind(kb_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();

    // 1. 删不再成立的来源：陈述死了、行死了、签名没绑着、属性或方向变了、陈述带了 mood
    sqlx::query(&format!(
        "DELETE FROM typed_fact_sources src
          USING facts t
          WHERE src.fact_id = t.id AND t.kb_id = $1
            AND NOT EXISTS (
                SELECT 1
                  FROM facts s
                  JOIN entities se ON se.id = s.subject_id
             LEFT JOIN entities oe ON oe.id = s.object_id
                  JOIN phrase_bindings b ON {MATCH}
                 WHERE s.id = src.statement_id
                   AND s.kb_id = $1
                   AND s.invalidated_at IS NULL
                   AND t.invalidated_at IS NULL
                   AND b.status = 'bound'
                   AND b.relation_type_id = t.predicate_id
                   AND ((b.direction = 'forward' AND t.subject_id = s.subject_id)
                     OR (b.direction = 'reverse' AND t.subject_id = s.object_id))
                   AND NOT EXISTS (SELECT 1 FROM statement_qualifiers q
                                    WHERE q.fact_id = s.id AND q.role = 'mood'))"
    ))
    .persistent(false)
    .bind(kb_id)
    .execute(&mut **tx)
    .await?;

    // 1b. 隐含行的来源：规则不再批准、触发它的陈述死了或换了签名、实体没了或换了类别词，
    //     来源就删；与 1 同一条规矩，只是来源表是另一张（implied_fact_sources）
    sqlx::query(&format!(
        "DELETE FROM implied_fact_sources i
          USING implication_rules r
          WHERE i.rule_id = r.id AND r.kb_id = $1
            AND (r.status <> 'approved'
                 OR (i.statement_id IS NOT NULL AND NOT EXISTS (
                        SELECT 1 FROM facts s
                          JOIN entities se ON se.id = s.subject_id
                     LEFT JOIN entities oe ON oe.id = s.object_id
                         WHERE s.id = i.statement_id AND s.layer = 'open' AND s.invalidated_at IS NULL
                           AND r.trigger = 'phrase' AND {RULE_MATCH}))
                 OR (i.entity_id IS NOT NULL AND NOT EXISTS (
                        SELECT 1 FROM entities e
                         WHERE e.id = i.entity_id AND e.merged_into IS NULL
                           AND r.trigger = 'kind_word' AND {KIND} = r.phrase)))",
        RULE_MATCH = crate::implication_rules::RULE_MATCH,
        KIND = kind_word_sql("e.specific_type"),
    ))
    .persistent(false)
    .bind(kb_id)
    .execute(&mut **tx)
    .await?;

    // 2. 作废来源全空的类型化行：只动算出来的行（带 from_statement_id 的、或规则算的），人写的不碰
    let retired = sqlx::query(
        "UPDATE facts t
            SET invalidated_at = now()
          WHERE t.kb_id = $1 AND t.layer = 'typed' AND t.invalidated_at IS NULL
            AND (t.from_statement_id IS NOT NULL OR t.implied)
            AND NOT EXISTS (SELECT 1 FROM typed_fact_sources src WHERE src.fact_id = t.id)
            AND NOT EXISTS (SELECT 1 FROM implied_fact_sources i WHERE i.fact_id = t.id)",
    )
    .persistent(false)
    .bind(kb_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    let retired = retired + emptied;

    // 3. 该有而没有的（陈述, 绑定）对：陈述活着、签名绑着、没 mood、还没有活着的行以它为来源
    //    且谓词相同。reverse 只对两样东西之间的关系有意义（字面值当不了主语）
    let due: Vec<Due> = sqlx::query_as(&format!(
        "SELECT s.id AS statement, b.relation_type_id AS property, b.direction,
                s.subject_id, s.object_id, oe.canonical_name AS object_name, s.object_value,
                s.valid_from, s.valid_from_precision, s.valid_from_grade,
                s.valid_to, s.valid_to_precision,
                s.attested_from, s.confidence, b.marks,
                rt.temporal, rt.kind, {SHAPE}
           FROM facts s
           JOIN entities se ON se.id = s.subject_id
      LEFT JOIN entities oe ON oe.id = s.object_id
           JOIN phrase_bindings b ON {MATCH}
           JOIN relation_types rt ON rt.id = b.relation_type_id
          WHERE s.kb_id = $1 AND s.layer = 'open' AND s.invalidated_at IS NULL
            AND b.status = 'bound'
            AND (b.direction = 'forward' OR s.object_id IS NOT NULL)
            AND NOT EXISTS (SELECT 1 FROM statement_qualifiers q
                             WHERE q.fact_id = s.id AND q.role = 'mood')
            -- 勘误撤过的（陈述, 属性）不再算（0044 决定 7）：撤销要站得住
            AND NOT EXISTS (SELECT 1 FROM errata_actions ea
                             WHERE ea.statement_id = s.id AND ea.predicate_id = b.relation_type_id
                               AND ea.status = 'applied' AND ea.action IN ('retract', 'revise'))
            AND NOT EXISTS (SELECT 1 FROM typed_fact_sources src JOIN facts t ON t.id = src.fact_id
                             WHERE src.statement_id = s.id AND t.invalidated_at IS NULL
                               AND t.predicate_id = b.relation_type_id)
          ORDER BY s.id"
    ))
    .persistent(false)
    .bind(kb_id)
    .fetch_all(&mut **tx)
    .await?;

    let (mut added, mut merged) = (0u64, 0u64);
    // 绑上了却按 datatype 读不出的值：(陈述, 属性, 原文的字)，收尾时记成丢弃行
    let mut unreadable: Vec<(Uuid, Uuid, String)> = Vec::new();
    for d in &due {
        let reverse = d.direction == "reverse";
        // 一刻的陈述在状态属性下按绑定说的读（#966、0053 修订 2026-10-22）：开始写成从那一刻
        // 起，结束写成到那一刻为止（走 `insert_fact_on`「某天结束了」那条路，关上开放的那段），
        // 都不是或没说：属性按那个日期命名的时段读，关系不写（见 `span_of`）
        let Some(validity) = span_of(
            d.kind == "attribute",
            d.temporal.as_deref() == Some("state"),
            d.marks.as_deref(),
            d.valid_from,
            d.valid_from_precision.as_deref(),
            d.valid_from_grade.as_deref(),
            d.valid_to,
            d.valid_to_precision.as_deref(),
            d.attested_from,
        ) else {
            continue;
        };
        // 文本属性绑上了一条宾语是实体的陈述：宾语的名字就是值（0044 修订 2026-10-22）
        let named_value = (d.kind == "attribute")
            .then(|| {
                d.object_name
                    .as_deref()
                    .map(|n| serde_json::json!({ "value": n }))
            })
            .flatten();
        let object_value = named_value.as_ref().or(d.object_value.as_ref());
        let object_id = if d.kind == "attribute" {
            None
        } else {
            d.object_id
        };
        let (fact, new) = match (reverse, object_id, object_value) {
            (true, Some(object), _) => {
                insert_fact_on(
                    tx,
                    kb_id,
                    object,
                    Some(d.property),
                    FactObject::Entity(d.subject_id),
                    validity,
                    d.confidence,
                )
                .await?
            }
            (false, Some(object), _) => {
                insert_fact_on(
                    tx,
                    kb_id,
                    d.subject_id,
                    Some(d.property),
                    FactObject::Entity(object),
                    validity,
                    d.confidence,
                )
                .await?
            }
            (false, None, Some(value)) => {
                // 读不成属性的 datatype：陈述留在开放图谱里，这一对不写——但要记一行
                // （`attr_datatype`），从前这里什么都不说，图里少了一个数没人说得出为什么
                let Some(value) = attribute_value(value, d.datatype.as_deref(), d.unit.as_deref())
                else {
                    unreadable.push((d.statement, d.property, raw_text(value)));
                    continue;
                };
                insert_fact_on(
                    tx,
                    kb_id,
                    d.subject_id,
                    Some(d.property),
                    FactObject::Value(&value),
                    validity,
                    d.confidence,
                )
                .await?
            }
            _ => continue,
        };
        written.push(fact);
        if new {
            added += 1;
            sqlx::query("UPDATE facts SET from_statement_id = $2 WHERE id = $1 AND from_statement_id IS NULL")
                .bind(fact)
                .bind(d.statement)
                .execute(&mut **tx)
                .await?;
            // 新行取代了一条裸行（时间精化，supersedes 链上）：被取代那行的来源跟着搬过来，
            // 这一轮就收敛，不等下一轮把旧来源当「不成立」删掉再补
            sqlx::query(
                "INSERT INTO typed_fact_sources (fact_id, statement_id)
                 SELECT $1, src.statement_id
                   FROM facts n JOIN typed_fact_sources src ON src.fact_id = n.supersedes
                  WHERE n.id = $1
                 ON CONFLICT DO NOTHING",
            )
            .bind(fact)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                "DELETE FROM typed_fact_sources src
                  USING facts n
                  WHERE n.id = $1 AND src.fact_id = n.supersedes",
            )
            .bind(fact)
            .execute(&mut **tx)
            .await?;
        } else {
            merged += 1;
        }
        sqlx::query(
            "INSERT INTO typed_fact_sources (fact_id, statement_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(fact)
        .bind(d.statement)
        .execute(&mut **tx)
        .await?;
        // 证据与限定各抄一份：证据是同一段原文的同一处引文；限定照角色词原样带过去
        sqlx::query(
            "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version,
                                        proposed_predicate, quote_start, quote_end)
             SELECT $1, chunk_id, quote, document_id, doc_version, proposed_predicate,
                    quote_start, quote_end
               FROM fact_evidence WHERE fact_id = $2
             ON CONFLICT DO NOTHING",
        )
        .bind(fact)
        .bind(d.statement)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO statement_qualifiers (fact_id, role, value, entity_id)
             SELECT $1, role, value, entity_id FROM statement_qualifiers WHERE fact_id = $2
             ON CONFLICT DO NOTHING",
        )
        .bind(fact)
        .bind(d.statement)
        .execute(&mut **tx)
        .await?;
    }
    // 3a. 已批准的规则算隐含行（0044 决定 3 第五片）。读数只查缓存：缓存里没有的这一轮
    //     不算，`read_phrases` 填上之后再来。短语规则按陈述触发，类别词规则按实体触发
    let implied = imply_in_tx(tx, kb_id, &mut written, &mut unreadable).await?;
    record_unreadable(tx, kb_id, &unreadable).await?;
    // 3b. 见证跟着来源陈述走（0064 决定 3、5）：类型化的行自己没有出处。物化与算隐含行
    //     用的写入口在陈述没有见证时填的是此刻（那是给人写的事实用的缺省），这里改回陈述的。
    //     排在 3a 之后：规则算出的行也是这样写下的
    //     见证动了的已有行和新写下的行一起，提交后对账：见证是时间线排序用的时刻
    written.extend(sync_typed_attestation(&mut **tx, kb_id, None).await?);
    Ok((
        Outcome {
            retired,
            added,
            merged,
            implied,
            corrected: 0,
            conflicts: 0,
        },
        written,
    ))
}

/// 类型化行的见证 = 它的来源陈述里最早的那个见证，连同那条日期的名字；来源陈述一条见证
/// 都没有的，它也没有（任何时点都成立，0064 决定 5）。来源陈述有两处：物化进这一行的
/// （`typed_fact_sources`），和规则读出这一行的（`implied_fact_sources`）；类别词规则的
/// 来源是实体，不带见证。
///
/// 动的行：有陈述物化进来的，和规则算出来的（`implied`）。人写的行只被规则的结论并进来时
/// 不动——它的见证是人写下它的那一刻，并进来时 `attest_earlier` 已经往早挪过
///
/// 回见证动了的那些行。见证是唯一性时间线给没起点的行排序用的时刻（`temporal::DATED_AT`），
/// 所以调用方提交之后要拿这些行去对账（`temporal::reconcile_moved_facts`）——只对这些行
/// 所在的时间线，不是整个库
///
/// 作废了的来源陈述不作证：物化自己会先把它们从来源里清掉，删文档那条路
/// （`documents::reattest_tx`）却是在物化之前跑的，陈述刚随文档作废、来源表还没动。
/// `only`：只动这些行（删文档时只动引用了那篇文档的，它们的时间线已经锁上）；`None` 是整个库
pub async fn sync_typed_attestation<'e>(
    ex: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    kb_id: Uuid,
    only: Option<&[Uuid]>,
) -> AppResult<Vec<Uuid>> {
    let moved = sqlx::query_scalar(
        "UPDATE facts t
            SET attested_from = src.at, attested_by = src.by
           FROM (SELECT u.fact_id,
                        min(s.attested_from) AS at,
                        (array_agg(s.attested_by ORDER BY s.attested_from NULLS LAST, s.id))[1] AS by
                   FROM (SELECT ts.fact_id, ts.statement_id
                           FROM typed_fact_sources ts
                           JOIN facts k ON k.id = ts.statement_id AND k.kb_id = $1
                         UNION ALL
                         -- 规则的来源按规则的库筛：类别词规则的来源是实体，没有陈述
                         SELECT i.fact_id, i.statement_id
                           FROM implied_fact_sources i
                           JOIN implication_rules r ON r.id = i.rule_id AND r.kb_id = $1) u
              LEFT JOIN facts s ON s.id = u.statement_id AND s.invalidated_at IS NULL
                  GROUP BY u.fact_id) src
          WHERE t.id = src.fact_id AND t.kb_id = $1 AND t.layer = 'typed'
            AND t.invalidated_at IS NULL
            AND ($2::uuid[] IS NULL OR t.id = ANY($2))
            AND (t.implied
                 OR EXISTS (SELECT 1 FROM typed_fact_sources ts WHERE ts.fact_id = t.id))
            AND (t.attested_from IS DISTINCT FROM src.at
                 OR t.attested_by IS DISTINCT FROM src.by)
          RETURNING t.id",
    )
    .persistent(false)
    .bind(kb_id)
    .bind(only)
    .fetch_all(ex)
    .await?;
    Ok(moved)
}

#[derive(sqlx::FromRow)]
struct Implied {
    rule: Uuid,
    statement: Option<Uuid>,
    entity: Option<Uuid>,
    subject_id: Uuid,
    property: Uuid,
    object_id: Option<Uuid>,
    object_value: Option<serde_json::Value>,
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_from_precision: Option<String>,
    valid_from_grade: Option<String>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    valid_to_precision: Option<String>,
    attested_from: Option<chrono::DateTime<chrono::Utc>>,
    confidence: f32,
    /// 结论属性的时间语义：一刻的陈述落到状态属性上按时段读（规则没有 marks）
    temporal: String,
    /// 结论是属性还是关系：一刻的陈述只在属性上读成时段
    kind: String,
    datatype: Option<String>,
    unit: Option<String>,
}

async fn imply_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kb_id: Uuid,
    written: &mut Vec<Uuid>,
    unreadable: &mut Vec<(Uuid, Uuid, String)>,
) -> AppResult<u64> {
    // 短语规则：签名下活着的、没 mood 的陈述；宾语是读数的答案（缓存里的实体或值），
    // 没有读数时就是陈述的宾语。已经有活着的隐含行以这条陈述为来源的不再算
    let by_statement: Vec<Implied> = sqlx::query_as(&format!(
        "SELECT r.id AS rule, s.id AS statement, NULL::uuid AS entity,
                s.subject_id, r.conclude_property_id AS property,
                CASE WHEN r.reading IS NULL THEN s.object_id ELSE pr.entity_id END AS object_id,
                CASE WHEN r.reading IS NULL THEN s.object_value ELSE pr.value END AS object_value,
                s.valid_from, s.valid_from_precision, s.valid_from_grade, s.valid_to, s.valid_to_precision,
                s.attested_from, s.confidence, rt.temporal, rt.kind, {SHAPE}
           FROM implication_rules r
           JOIN relation_types rt ON rt.id = r.conclude_property_id
           JOIN facts s ON s.kb_id = r.kb_id AND s.layer = 'open' AND s.invalidated_at IS NULL
           JOIN entities se ON se.id = s.subject_id
      LEFT JOIN entities oe ON oe.id = s.object_id
      LEFT JOIN phrase_readings pr ON r.reading IS NOT NULL AND pr.kb_id = r.kb_id
                                   AND pr.reading = r.reading AND pr.phrase = {TEXT}
          WHERE r.kb_id = $1 AND r.status = 'approved' AND r.trigger = 'phrase'
            AND {RULE_MATCH}
            AND NOT EXISTS (SELECT 1 FROM statement_qualifiers q WHERE q.fact_id = s.id AND q.role = 'mood')
            AND (r.reading IS NULL OR pr.entity_id IS NOT NULL OR pr.value IS NOT NULL)
            AND NOT EXISTS (SELECT 1 FROM errata_actions ea
                             WHERE ea.statement_id = s.id AND ea.predicate_id = r.conclude_property_id
                               AND ea.status = 'applied' AND ea.action IN ('retract', 'revise'))
            AND NOT EXISTS (SELECT 1 FROM implied_fact_sources i JOIN facts t ON t.id = i.fact_id
                             WHERE i.rule_id = r.id AND i.statement_id = s.id AND t.invalidated_at IS NULL)
          ORDER BY s.id",
        TEXT = crate::implication_rules::object_text_sql(),
        RULE_MATCH = crate::implication_rules::RULE_MATCH,
    ))
    .persistent(false)
    .bind(kb_id)
    .fetch_all(&mut **tx)
    .await?;
    // 类别词规则：带这个类别词的活着的实体；宾语必须来自读数（类别词自己没有宾语）
    let by_entity: Vec<Implied> = sqlx::query_as(&format!(
        "SELECT r.id AS rule, NULL::uuid AS statement, e.id AS entity,
                e.id AS subject_id, r.conclude_property_id AS property,
                pr.entity_id AS object_id, pr.value AS object_value,
                NULL::timestamptz AS valid_from, NULL::text AS valid_from_precision, NULL::text AS valid_from_grade,
                NULL::timestamptz AS valid_to, NULL::text AS valid_to_precision,
                NULL::timestamptz AS attested_from, 0.9::real AS confidence, rt.temporal, rt.kind, {SHAPE}
           FROM implication_rules r
           JOIN relation_types rt ON rt.id = r.conclude_property_id
           JOIN entities e ON e.kb_id = r.kb_id AND e.merged_into IS NULL AND {KIND} = r.phrase
           JOIN phrase_readings pr ON pr.kb_id = r.kb_id AND pr.reading = r.reading AND pr.phrase = r.phrase
          WHERE r.kb_id = $1 AND r.status = 'approved' AND r.trigger = 'kind_word' AND r.reading IS NOT NULL
            AND (pr.entity_id IS NOT NULL OR pr.value IS NOT NULL)
            AND NOT EXISTS (SELECT 1 FROM implied_fact_sources i JOIN facts t ON t.id = i.fact_id
                             WHERE i.rule_id = r.id AND i.entity_id = e.id AND t.invalidated_at IS NULL)
          ORDER BY e.id",
        KIND = kind_word_sql("e.specific_type"),
    ))
    .persistent(false)
    .bind(kb_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut implied = 0u64;
    for d in by_statement.iter().chain(by_entity.iter()) {
        // 规则没有 marks（#966）：一刻的陈述蕴含一条状态属性时，结论是属性的按那个日期命名
        // 的时段读，是关系的不算隐含行（0053 修订 2026-10-22，同物化）
        let Some(validity) = span_of(
            d.kind == "attribute",
            d.temporal == "state",
            None,
            d.valid_from,
            d.valid_from_precision.as_deref(),
            d.valid_from_grade.as_deref(),
            d.valid_to,
            d.valid_to_precision.as_deref(),
            d.attested_from,
        ) else {
            continue;
        };
        let value;
        let object = match (d.object_id, &d.object_value) {
            (Some(o), _) => FactObject::Entity(o),
            (None, Some(v)) => {
                let Some(read) = attribute_value(v, d.datatype.as_deref(), d.unit.as_deref())
                else {
                    // 类别词规则的来源是实体，没有文档可记；短语规则的记在触发它的陈述上
                    if let Some(statement) = d.statement {
                        unreadable.push((statement, d.property, raw_text(v)));
                    }
                    continue;
                };
                value = read;
                FactObject::Value(&value)
            }
            _ => continue,
        };
        // 一个东西不蕴含自己：读数把「Virginia」读成「United States」是对的，把「France」
        // 读成「France」就是一条自环
        if matches!(object, FactObject::Entity(o) if o == d.subject_id) {
            continue;
        }
        let (fact, new) = insert_fact_on(
            tx,
            kb_id,
            d.subject_id,
            Some(d.property),
            object,
            validity,
            d.confidence,
        )
        .await?;
        written.push(fact);
        if new {
            implied += 1;
            sqlx::query("UPDATE facts SET implied = TRUE WHERE id = $1")
                .bind(fact)
                .execute(&mut **tx)
                .await?;
        }
        sqlx::query(
            "INSERT INTO implied_fact_sources (fact_id, rule_id, statement_id, entity_id)
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(fact)
        .bind(d.rule)
        .bind(d.statement)
        .bind(d.entity)
        .execute(&mut **tx)
        .await?;
        // 证据：短语规则抄触发它的那条陈述的引文——读的人从这句得出的结论，证据就是这句
        if let Some(statement) = d.statement {
            sqlx::query(
                "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version,
                                            proposed_predicate, quote_start, quote_end)
                 SELECT $1, chunk_id, quote, document_id, doc_version, proposed_predicate,
                        quote_start, quote_end
                   FROM fact_evidence WHERE fact_id = $2
                 ON CONFLICT DO NOTHING",
            )
            .bind(fact)
            .bind(statement)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(implied)
}

/// 一条陈述落成类型化行时的区间。起止不同的照抄；起止同值的一刻落到状态属性上按绑定说的读
/// （#966）：`start` 从那一刻起，`end` 到那一刻为止（走「某天结束了」那条路）。`none` 或没说：
/// 落到**属性**上按那个日期命名的时段读（0053 修订 2026-10-22）——「2023年营业收入58亿元」是
/// 2023 这一年的一个量，起点是它，终点是桶的尽头，标上 `end_after_period`，读的人印得出原文
/// 写的 2023；落到关系上照旧不算行——「joined on 2023-06-01」读成一天的任职会把同一天起的
/// 开放行关在次日（写入口「同起点、这次带了终点、那行还开着就关上」），那是一条错的结束。
/// 没有精度的一刻不是桶，读不出时段，也返回 None
#[allow(clippy::too_many_arguments)]
fn span_of<'a>(
    attribute: bool,
    state: bool,
    marks: Option<&str>,
    from: Option<chrono::DateTime<chrono::Utc>>,
    from_precision: Option<&'a str>,
    from_grade: Option<&'a str>,
    to: Option<chrono::DateTime<chrono::Utc>>,
    to_precision: Option<&'a str>,
    attested_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<Validity<'a>> {
    let moment = state && from.is_some() && from == to;
    Some(match (moment, marks) {
        (false, _) => Validity {
            from,
            from_precision,
            from_grade,
            to,
            to_precision,
            attested_at,
            end_after_period: false,
        },
        (true, Some("start")) => Validity {
            from,
            from_precision,
            from_grade,
            to: None,
            to_precision: None,
            attested_at,
            end_after_period: false,
        },
        (true, Some("end")) => Validity {
            from: None,
            from_precision: None,
            from_grade: None,
            to: from,
            to_precision: from_precision,
            attested_at,
            end_after_period: false,
        },
        (true, _) => {
            if !attribute {
                return None;
            }
            let start = from?;
            let end = crate::graph::bucket_end(start, from_precision);
            if end <= start {
                return None;
            }
            Validity {
                from,
                from_precision,
                from_grade,
                to: Some(end),
                to_precision: from_precision,
                attested_at,
                end_after_period: true,
            }
        }
    })
}

/// 值那一格原文的字，给丢弃行当样例：`{"value": "超过2000人"}` → `超过2000人`
fn raw_text(raw: &Value) -> String {
    match raw.get("value").unwrap_or(raw) {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 读不出的值记成 `attr_datatype` 丢弃行（0001：丢弃是一行，不是沉默）。按库整个重写：
/// 物化每轮重算，上一轮读不出、这一轮属性改成文本读出来了的，行跟着退掉。文档从陈述的
/// 证据取；没有证据的陈述（人写的）没有文档可挂，不记
async fn record_unreadable(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kb_id: Uuid,
    unreadable: &[(Uuid, Uuid, String)],
) -> AppResult<()> {
    let statements: Vec<Uuid> = unreadable.iter().map(|u| u.0).collect();
    let properties: Vec<Uuid> = unreadable.iter().map(|u| u.1).collect();
    let raws: Vec<String> = unreadable.iter().map(|u| u.2.clone()).collect();
    let rows: Vec<crate::extraction_drops::Unreadable> =
        sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT fe.document_id, rt.key, se.canonical_name || ' → ' || x.raw
           FROM unnest($2::uuid[], $3::uuid[], $4::text[]) AS x(statement, property, raw)
           JOIN relation_types rt ON rt.id = x.property
           JOIN facts s ON s.id = x.statement
           JOIN entities se ON se.id = s.subject_id
           JOIN LATERAL (SELECT fe.document_id FROM fact_evidence fe
                          WHERE fe.fact_id = x.statement AND fe.document_id IS NOT NULL
                          ORDER BY fe.quote_start NULLS LAST LIMIT 1) fe ON true
          WHERE rt.kb_id = $1
          ORDER BY x.statement",
        )
        .persistent(false)
        .bind(kb_id)
        .bind(&statements)
        .bind(&properties)
        .bind(&raws)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(
            |(document_id, detail, example)| crate::extraction_drops::Unreadable {
                document_id,
                detail,
                example,
            },
        )
        .collect();
    crate::extraction_drops::replace_reason_tx(
        tx,
        kb_id,
        crate::extraction_drops::reason::ATTR_DATATYPE,
        &rows,
    )
    .await
}

/// 开放陈述的值（`{"value": "2.3亿元"}`）落到属性上的样子：`value` 那一格按 datatype 与单位读成
/// 数、日期、真假（[`utopia_extract::attribute_value`]），其余键照抄；绑到关系上的值原样留着。
///
/// **读不出来就不写**（None）。从前照抄原文的字：声明了 number、单位万元的「注册资本」上躺着
/// `"2.3亿元"`，导出成 `"2.3亿元"^^xsd:decimal`，比较、对账、规则全都对不上。读不出来的陈述
/// 还在开放图谱里，原文的字一个没丢。只在这里读、不在 `insert_fact_on` 的门上读：人和勘误
/// 会在日期属性上写相对的日期（`relative`），那是另一种值
fn attribute_value(raw: &Value, datatype: Option<&str>, unit: Option<&str>) -> Option<Value> {
    let Some(datatype) = datatype else {
        return Some(raw.clone());
    };
    let read = utopia_extract::attribute_value(datatype, unit, raw.get("value").unwrap_or(raw))?;
    let mut out = if raw.is_object() {
        raw.clone()
    } else {
        serde_json::json!({})
    };
    out["value"] = read;
    Some(out)
}

/// 类别词的归一（同 type_bindings）：这里要在 SQL 里对上实体的 specific_type
fn kind_word_sql(col: &str) -> String {
    format!("lower(btrim(regexp_replace({col}, '\\s+', ' ', 'g')))")
}

/// 库里活着的、从陈述算出来的类型化行数。
pub async fn count(pool: &PgPool, kb_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM facts
          WHERE kb_id = $1 AND layer = 'typed' AND from_statement_id IS NOT NULL
            AND invalidated_at IS NULL",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
#[path = "materialize_delivery_tests.rs"]
mod delivery_tests;

#[cfg(test)]
#[path = "materialize_plan_tests.rs"]
mod plan_tests;

#[cfg(test)]
#[path = "materialize_unreadable_tests.rs"]
mod unreadable_tests;

#[cfg(test)]
#[path = "materialize_period_tests.rs"]
mod period_tests;
