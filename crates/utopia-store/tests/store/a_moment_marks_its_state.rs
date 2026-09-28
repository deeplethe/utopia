//! 只带一个日期的陈述绑在状态属性上（#966）。「Lin Zhao joined Meridian Systems on 2023-06-01」
//! 的日期，时间解析照 0031 写事件的办法写在两端；绑到 works_for（状态）之后照抄，就是一段
//! 2023-06-01 → 2023-06-01 的状态。世界轴按 `from <= T < to` 读状态，这一段任何时刻都不成立。
//! 绑定现在说那个日期标的是什么（`phrase_bindings.marks`）：
//!
//! - start：从那天起。「joined」在「works for … from 2023-06-01」之前、之后，或只有它
//!   （#966 的 A、B、C），都是一段从 2023-06-01 起、至今开着的状态；
//! - end：到那天为止，关上开着的那段。终点说的是一个时段（「2024 年离开」）而开着的那段从
//!   时段里开始时，关在时段的尽头；
//! - none 或还没说：不算类型化行，陈述带着日期留在开放图谱。
//!
//! 之前照抄写下的空段由物化作废重算，人写的、人改成一刻的不碰；声明成状态的属性下，两端
//! 相等的行写入、改区间、关在起点都被拒（同时开始的冲突也不能关在那一刻），没有属性的行
//! 照旧；已有的空段不并进后来的观察，也不被它关上；勘误改不了一段不成立的时间，也不能把
//! 一刻的事件改成状态；规则没有 marks，只说了一刻的陈述不蕴含状态，只有起点的照旧。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use utopia_store::errata::{self, ActionInput, Candidate, Proposed};
use utopia_store::graph::{self, Validity};
use utopia_store::materialize::{materialize, Outcome};
use uuid::Uuid;

/// 一行类型化事实：起、止，和它是不是规则算的
type ImpliedRow = (Option<DateTime<Utc>>, Option<DateTime<Utc>>, bool);

fn t(s: &str) -> DateTime<Utc> {
    s.parse().expect("fixed timestamp")
}

/// 2023-06-01，日精度
fn day() -> DateTime<Utc> {
    t("2023-06-01T00:00:00Z")
}

/// 两端都是那一天：时间解析写一刻的样子
fn moment() -> Validity<'static> {
    Validity {
        from: Some(day()),
        from_precision: Some("day"),
        to: Some(day()),
        to_precision: Some("day"),
        attested_at: None,
        from_grade: None,
    }
}

/// 只知道终点
fn ending(at: DateTime<Utc>) -> Validity<'static> {
    Validity {
        from: None,
        from_precision: None,
        to: Some(at),
        to_precision: Some("day"),
        attested_at: None,
        from_grade: None,
    }
}

/// 一个库：person、organization；works_for（状态）、acquired（事件）；Lin Zhao、
/// Meridian Systems、Aster Labs
struct Base {
    pool: PgPool,
    org: Uuid,
    kb: Uuid,
    person: Uuid,
    company: Uuid,
    works_for: Uuid,
    acquired: Uuid,
    lin: Uuid,
    meridian: Uuid,
    aster: Uuid,
}

impl Base {
    async fn new(pool: &PgPool) -> anyhow::Result<Self> {
        let ids: Vec<Uuid> = (0..10).map(|_| Uuid::now_v7()).collect();
        let (org, ws, kb, person, company) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
        let (works_for, acquired, lin, meridian, aster) = (ids[5], ids[6], ids[7], ids[8], ids[9]);
        // Only locally generated UUIDs are interpolated into fixture SQL.
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations (id, name) VALUES ('{org}', 'a-moment-marks-its-state');
             INSERT INTO workspaces (id, org_id, name) VALUES ('{ws}', '{org}', 'a-moment-marks-its-state');
             INSERT INTO knowledge_bases (id, workspace_id, name)
                  VALUES ('{kb}', '{ws}', 'a-moment-marks-its-state');
             INSERT INTO entity_types (id, kb_id, key, label, color, shape) VALUES
                 ('{person}', '{kb}', 'person', 'person', '#7fd0ff', 'circle'),
                 ('{company}', '{kb}', 'organization', 'organization', '#7fd0ff', 'circle');
             INSERT INTO relation_types (id, kb_id, key, label, temporal) VALUES
                 ('{works_for}', '{kb}', 'works_for', 'works for', 'state'),
                 ('{acquired}', '{kb}', 'acquired', 'acquired', 'event');
             INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES
                 ('{lin}', '{kb}', '{person}', 'Lin Zhao'),
                 ('{meridian}', '{kb}', '{company}', 'Meridian Systems'),
                 ('{aster}', '{kb}', '{company}', 'Aster Labs');"
        ))
        .execute(pool)
        .await?;
        Ok(Self {
            pool: pool.clone(),
            org,
            kb,
            person,
            company,
            works_for,
            acquired,
            lin,
            meridian,
            aster,
        })
    }

    /// 一条开放陈述 Lin Zhao —phrase→ Meridian Systems，两端按时间解析写的形状给：一刻两端
    /// 同值，「自」只有起点，「至」只有终点
    async fn statement(
        &self,
        phrase: &str,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> anyhow::Result<Uuid> {
        self.statement_in(phrase, from, to, "day").await
    }

    /// 同 [`Base::statement`]，两端是这个精度（「2024 年离开」是年精度的一刻）
    async fn statement_in(
        &self,
        phrase: &str,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        precision: &str,
    ) -> anyhow::Result<Uuid> {
        self.statement_to(phrase, self.meridian, from, to, precision)
            .await
    }

    /// 同 [`Base::statement_in`]，宾语是这一个
    async fn statement_to(
        &self,
        phrase: &str,
        object: Uuid,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        precision: &str,
    ) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, object_id, layer, phrase, confidence,
                                valid_from, valid_from_precision, valid_from_grade,
                                valid_to, valid_to_precision)
             VALUES ($1, $2, $3, $4, 'open', $5, 0.9,
                     $6, CASE WHEN $6 IS NULL THEN NULL ELSE $8 END,
                     CASE WHEN $6 IS NULL THEN NULL ELSE 'A' END,
                     $7, CASE WHEN $7 IS NULL THEN NULL ELSE $8 END)",
        )
        .bind(id)
        .bind(self.kb)
        .bind(self.lin)
        .bind(object)
        .bind(phrase)
        .bind(from)
        .bind(to)
        .bind(precision)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// 签名 (phrase, person, organization) 绑到 works_for，照原文方向，说那个日期标什么
    async fn bind(&self, phrase: &str, marks: Option<&str>) -> anyhow::Result<()> {
        self.bind_to(phrase, self.works_for, marks).await
    }

    /// 同 [`Base::bind`]，绑到这条属性
    async fn bind_to(
        &self,
        phrase: &str,
        property: Uuid,
        marks: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO phrase_bindings
                 (id, kb_id, phrase, subject_type_id, object_type_id, object_is_value,
                  relation_type_id, direction, status, marks)
             VALUES ($1, $2, $3, $4, $5, false, $6, 'forward', 'bound', $7)
             ON CONFLICT (kb_id, phrase, subject_type_id, object_type_id, object_is_value)
             DO UPDATE SET marks = EXCLUDED.marks",
        )
        .bind(Uuid::now_v7())
        .bind(self.kb)
        .bind(phrase)
        .bind(self.person)
        .bind(self.company)
        .bind(property)
        .bind(marks)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 一行类型化的 Lin Zhao —works_for→ object，两端都是那一天：#966 之前照抄写下的样子。
    /// `statement` 给了就是物化算的（`from_statement_id`），没给就是人写的
    async fn empty_row(&self, object: Uuid, statement: Option<Uuid>) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, layer,
                                valid_from, valid_from_precision, valid_to, valid_to_precision,
                                confidence, from_statement_id)
             VALUES ($1, $2, $3, $4, $5, 'typed', $6, 'day', $6, 'day', 0.9, $7)",
        )
        .bind(id)
        .bind(self.kb)
        .bind(self.lin)
        .bind(self.works_for)
        .bind(object)
        .bind(day())
        .bind(statement)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    /// Lin Zhao 与 Meridian 之间活着的类型化行：(起, 止, 来源数)，按起点排
    async fn rows(
        &self,
    ) -> anyhow::Result<Vec<(Option<DateTime<Utc>>, Option<DateTime<Utc>>, i64)>> {
        Ok(sqlx::query_as(
            "SELECT t.valid_from, t.valid_to,
                    (SELECT count(*) FROM typed_fact_sources s WHERE s.fact_id = t.id)
               FROM facts t
              WHERE t.kb_id = $1 AND t.layer = 'typed' AND t.invalidated_at IS NULL
                AND t.object_id = $2
              ORDER BY t.valid_from NULLS FIRST, t.valid_to NULLS LAST",
        )
        .bind(self.kb)
        .bind(self.meridian)
        .fetch_all(&self.pool)
        .await?)
    }

    /// 世界轴上某一刻，谁 works_for Meridian
    async fn at_meridian(&self, at: &str) -> anyhow::Result<Vec<String>> {
        let (nodes, edges) =
            graph::neighborhood(&self.pool, self.kb, self.meridian, 1, Some(t(at)), None).await?;
        Ok(edges
            .iter()
            .filter(|e| e.target == self.meridian && e.predicate.as_deref() == Some("works_for"))
            .filter_map(|e| nodes.iter().find(|n| n.id == e.source))
            .map(|n| n.name.clone())
            .collect())
    }

    /// 不看世界轴时，Meridian 身上的边：(id, 起, 止)
    async fn edges(
        &self,
    ) -> anyhow::Result<Vec<(Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>)>> {
        let (_, edges) =
            graph::neighborhood(&self.pool, self.kb, self.meridian, 1, None, None).await?;
        Ok(edges
            .iter()
            .filter(|e| e.target == self.meridian)
            .map(|e| (e.id, e.valid_from, e.valid_to))
            .collect())
    }

    async fn live(&self, fact: Uuid) -> anyhow::Result<(bool, Option<DateTime<Utc>>)> {
        Ok(
            sqlx::query_as("SELECT invalidated_at IS NULL, valid_to FROM facts WHERE id = $1")
                .bind(fact)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    async fn remove(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

async fn pool() -> anyhow::Result<Option<PgPool>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    Ok(Some(PgPool::connect(&url).await?))
}

fn refused_as_empty(e: &utopia_core::AppError) -> bool {
    format!("{e:?}").contains("empty_state_span")
}

/// #966 的三种顺序：「joined」（一刻）在「works for … from」（自）之前、之后，或只有它。
/// 绑定说「joined」标的是开始：三种都是一段从 2023-06-01 起、至今开着的状态
#[tokio::test]
async fn a_date_that_starts_a_state_opens_it_whatever_the_order() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let cases: [(&str, &[&str]); 3] = [
        ("A", &["joined", "works for"]),
        ("B", &["works for", "joined"]),
        ("C", &["joined"]),
    ];
    for (label, phrases) in cases {
        let base = Base::new(&pool).await?;
        let run = async {
            for phrase in phrases {
                let point = *phrase == "joined";
                base.statement(phrase, Some(day()), point.then(day)).await?;
                base.bind(phrase, point.then_some("start")).await?;
            }
            materialize(&pool, base.kb).await?;
            assert_eq!(
                base.rows().await?,
                vec![(Some(day()), None, phrases.len() as i64)],
                "case {label}: one row from 2023-06-01, every statement its source"
            );
            for at in [
                "2023-06-01T12:00:00Z",
                "2023-07-01T00:00:00Z",
                "2026-01-01T00:00:00Z",
            ] {
                assert_eq!(
                    base.at_meridian(at).await?,
                    vec!["Lin Zhao".to_string()],
                    "case {label} at {at}"
                );
            }
            assert_eq!(
                materialize(&pool, base.kb).await?,
                Outcome::default(),
                "case {label}: a second run changes nothing"
            );
            anyhow::Ok(())
        }
        .await;
        base.remove().await?;
        run?;
    }
    Ok(())
}

/// 标结束的短语：「left … on 2025-01-31」把开着的那段关在那一天。先读到哪一句都一样
#[tokio::test]
async fn a_date_that_ends_a_state_closes_it_there() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let left = t("2025-01-31T00:00:00Z");
    for order in [["works for", "left"], ["left", "works for"]] {
        let base = Base::new(&pool).await?;
        let run = async {
            for phrase in order {
                if phrase == "left" {
                    base.statement(phrase, Some(left), Some(left)).await?;
                } else {
                    base.statement(phrase, Some(day()), None).await?;
                }
            }
            base.bind("works for", None).await?;
            base.bind("left", Some("end")).await?;
            materialize(&pool, base.kb).await?;
            assert_eq!(
                base.rows().await?,
                vec![(Some(day()), Some(left), 2)],
                "{order:?}: one span, closed where he left"
            );
            assert_eq!(
                base.at_meridian("2024-06-01T00:00:00Z").await?,
                vec!["Lin Zhao".to_string()],
                "{order:?}"
            );
            assert!(
                base.at_meridian("2025-06-01T00:00:00Z").await?.is_empty(),
                "{order:?}: not after he left"
            );
            assert_eq!(materialize(&pool, base.kb).await?, Outcome::default());
            anyhow::Ok(())
        }
        .await;
        base.remove().await?;
        run?;
    }
    Ok(())
}

/// 标「都不是」的日期，和绑定还没说标什么的日期：不算类型化行。陈述带着它的日期
/// 留在开放图谱，画布上还是它
#[tokio::test]
async fn a_date_that_marks_neither_or_nothing_yet_computes_no_row() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    for marks in [Some("none"), None] {
        let base = Base::new(&pool).await?;
        let run = async {
            let seen = base
                .statement("was employed by", Some(day()), Some(day()))
                .await?;
            base.bind("was employed by", marks).await?;
            assert_eq!(
                materialize(&pool, base.kb).await?,
                Outcome::default(),
                "{marks:?}"
            );
            assert!(base.rows().await?.is_empty(), "{marks:?}: no typed row");
            assert_eq!(
                base.edges().await?,
                vec![(seen, Some(day()), Some(day()))],
                "{marks:?}: the statement stays in the open graph with its date"
            );
            anyhow::Ok(())
        }
        .await;
        base.remove().await?;
        run?;
    }
    Ok(())
}

/// 之前照抄写下的空段（#966 的 A）：物化作废它（还在表里），陈述按绑定重算——「works for
/// … from」落成一段从那天起的状态；「joined」的绑定还没说标什么，它留在开放图谱；绑定说了
/// start，它并进那一段。人写的空段不碰
#[tokio::test]
async fn a_row_that_held_at_no_moment_is_computed_again() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let base = Base::new(&pool).await?;
    let run = async {
        let joined = base.statement("joined", Some(day()), Some(day())).await?;
        let works = base.statement("works for", Some(day()), None).await?;
        base.bind("joined", None).await?;
        base.bind("works for", None).await?;
        let computed = base.empty_row(base.meridian, Some(joined)).await?;
        sqlx::query(
            "INSERT INTO typed_fact_sources (fact_id, statement_id) VALUES ($1, $2), ($1, $3)",
        )
        .bind(computed)
        .bind(joined)
        .bind(works)
        .execute(&pool)
        .await?;
        let by_hand = base.empty_row(base.aster, None).await?;
        assert!(
            base.at_meridian("2023-07-01T00:00:00Z").await?.is_empty(),
            "the ledger as #966 found it: nobody works for Meridian"
        );

        assert_eq!(
            materialize(&pool, base.kb).await?,
            Outcome {
                retired: 1,
                added: 1,
                ..Outcome::default()
            }
        );
        assert_eq!(
            base.live(computed).await?,
            (false, Some(day())),
            "retired, still in the table"
        );
        assert_eq!(base.rows().await?, vec![(Some(day()), None, 1)]);
        assert_eq!(
            base.at_meridian("2023-07-01T00:00:00Z").await?,
            vec!["Lin Zhao".to_string()]
        );
        let edges = base.edges().await?;
        assert!(
            edges.iter().any(|(id, _, _)| *id == joined),
            "joined has no typed row yet, so it shows as itself: {edges:?}"
        );
        assert!(!edges.iter().any(|(id, _, _)| *id == works), "{edges:?}");
        assert_eq!(
            base.live(by_hand).await?,
            (true, Some(day())),
            "a row a person wrote is not touched"
        );

        base.bind("joined", Some("start")).await?;
        assert_eq!(
            materialize(&pool, base.kb).await?,
            Outcome {
                merged: 1,
                ..Outcome::default()
            }
        );
        assert_eq!(base.rows().await?, vec![(Some(day()), None, 2)]);
        assert_eq!(materialize(&pool, base.kb).await?, Outcome::default());
        anyhow::Ok(())
    }
    .await;
    base.remove().await?;
    run
}

/// 不变量：声明成状态的属性下两端相等的行写不进去——写入（截到精度之后才相等的也算）、
/// 改区间、关在它自己的起点都拒，原来的行不动。事件照旧写成一刻；没有属性的行照旧带着它的日期
#[tokio::test]
async fn a_state_that_ends_where_it_starts_is_never_written() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let base = Base::new(&pool).await?;
    let run = async {
        let (kb, lin, meridian, aster) = (base.kb, base.lin, base.meridian, base.aster);
        let refused = graph::insert_fact(
            &pool,
            kb,
            lin,
            Some(base.works_for),
            meridian,
            moment(),
            0.9,
        )
        .await
        .expect_err("an empty state span");
        assert!(refused_as_empty(&refused), "{refused:?}");
        let same_day = Validity {
            from: Some(t("2023-06-01T09:00:00Z")),
            to: Some(t("2023-06-01T17:00:00Z")),
            ..moment()
        };
        let refused = graph::insert_fact(
            &pool,
            kb,
            lin,
            Some(base.works_for),
            meridian,
            same_day,
            0.9,
        )
        .await
        .expect_err("equal once truncated to the day");
        assert!(refused_as_empty(&refused), "{refused:?}");
        graph::insert_fact(
            &pool,
            kb,
            meridian,
            Some(base.acquired),
            aster,
            moment(),
            0.9,
        )
        .await?;
        let (bare, _) = graph::insert_fact(&pool, kb, lin, None, aster, moment(), 0.9).await?;
        let (_, to) = base.live(bare).await?;
        assert_eq!(to, Some(day()), "a row without a property keeps its date");

        let (open, _) = graph::insert_fact(
            &pool,
            kb,
            lin,
            Some(base.works_for),
            meridian,
            Validity::starting(Some(day()), Some("day")),
            0.9,
        )
        .await?;
        let refused = utopia_store::temporal::correct_interval(&pool, open, moment())
            .await
            .expect_err("corrected into an empty span");
        assert!(refused_as_empty(&refused), "{refused:?}");
        let refused = utopia_store::temporal::close_superseded(&pool, open, day(), "day")
            .await
            .expect_err("closed where it starts");
        assert!(refused_as_empty(&refused), "{refused:?}");
        assert_eq!(
            base.live(open).await?,
            (true, None),
            "the open row is untouched"
        );
        assert!(
            utopia_store::temporal::close_superseded(&pool, open, t("2024-01-01T00:00:00Z"), "day")
                .await?
                .is_some(),
            "closing after the start still works"
        );
        let (unnamed, _) = graph::insert_fact(
            &pool,
            kb,
            lin,
            None,
            meridian,
            Validity::starting(Some(day()), Some("day")),
            0.9,
        )
        .await?;
        assert!(
            utopia_store::temporal::correct_interval(&pool, unnamed, moment())
                .await?
                .is_some(),
            "a row without a property can be given a single date"
        );
        anyhow::Ok(())
    }
    .await;
    base.remove().await?;
    run
}

/// 护栏：之前写下的空段（这里是人写的，物化不作废它）不把后来「自那天起」的观察并进去，
/// 也不被「到哪天为止」的观察关上。同一天开始又结束的状态，不管先读到哪一句，都不写成空段，
/// 也不让开着的那段一直开着：关在那一天的尽头
#[tokio::test]
async fn a_row_that_holds_at_no_moment_neither_absorbs_nor_closes() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let base = Base::new(&pool).await?;
    let run = async {
        let (kb, lin, meridian) = (base.kb, base.lin, base.meridian);
        let empty = base.empty_row(meridian, None).await?;
        let since = Validity::starting(Some(day()), Some("day"));
        let (open, created) =
            graph::insert_fact(&pool, kb, lin, Some(base.works_for), meridian, since, 0.9).await?;
        assert!(
            created && open != empty,
            "a new row, not merged into the empty one"
        );
        let left = t("2025-01-31T00:00:00Z");
        graph::insert_fact(
            &pool,
            kb,
            lin,
            Some(base.works_for),
            meridian,
            ending(left),
            0.9,
        )
        .await?;
        assert_eq!(
            base.rows().await?,
            vec![(Some(day()), Some(day()), 0), (Some(day()), Some(left), 0)],
            "the open row is the one that closes"
        );
        assert_eq!(base.live(empty).await?, (true, Some(day())));
        anyhow::Ok(())
    }
    .await;
    base.remove().await?;
    run?;

    for start_first in [true, false] {
        let base = Base::new(&pool).await?;
        let run = async {
            let (kb, lin, meridian) = (base.kb, base.lin, base.meridian);
            let since = Validity::starting(Some(day()), Some("day"));
            let writes = if start_first {
                [since, ending(day())]
            } else {
                [ending(day()), since]
            };
            for v in writes {
                graph::insert_fact(&pool, kb, lin, Some(base.works_for), meridian, v, 0.9).await?;
            }
            assert_eq!(
                base.rows().await?,
                vec![(Some(day()), Some(t("2023-06-02T00:00:00Z")), 0)],
                "start first: {start_first}: one row, through that day"
            );
            anyhow::Ok(())
        }
        .await;
        base.remove().await?;
        run?;
    }
    Ok(())
}

/// 勘误改一条之前写下的空段：新行接不了一段不成立的时间。验证时就拒；升级前就留给人的
/// 那一笔，批了也不先撤旧行
#[tokio::test]
async fn a_revision_cannot_take_a_time_that_holds_at_no_moment() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let base = Base::new(&pool).await?;
    let run = async {
        let (kb, lin, meridian, aster) = (base.kb, base.lin, base.meridian, base.aster);
        let text = "Lin Zhao joined Aster Labs on 2023-06-01.";
        let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256) VALUES ($1, $2, 'offer.txt', 'x')",
        )
        .bind(doc)
        .bind(kb)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, $4)",
        )
        .bind(chunk)
        .bind(kb)
        .bind(doc)
        .bind(text)
        .execute(&pool)
        .await?;
        let empty = base.empty_row(meridian, None).await?;
        let run_id = errata::start_run(&pool, kb, doc, 1, 0).await?;
        let candidate = Candidate {
            fact_id: empty,
            statement_id: None,
            predicate_id: base.works_for,
            subject_id: lin,
            subject: "Lin Zhao".into(),
            subject_class: Some("person".into()),
            property: "works_for".into(),
            property_label: "works for".into(),
            object_id: Some(meridian),
            object: "Meridian Systems".into(),
            object_class: Some("organization".into()),
            flag: Some("name_absent".into()),
            quote: None,
        };
        let recorded = errata::record(
            &pool,
            kb,
            ActionInput {
                run_id,
                document_id: doc,
                candidate: Some(&candidate),
                proposed: Proposed::Revise {
                    property: None,
                    object: Some("Aster Labs".into()),
                },
                reason: "the document names Aster Labs",
                quote: Some("Lin Zhao joined Aster Labs"),
                document_text: text,
            },
        )
        .await?;
        assert_eq!(
            (recorded.status, recorded.detail.as_deref()),
            (
                "refused",
                Some("a state cannot take a time whose start and end are equal")
            )
        );
        assert_eq!(base.live(empty).await?, (true, Some(day())));

        let held = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO errata_actions (id, kb_id, run_id, document_id, fact_id, predicate_id,
                                         action, reason, quote, proposed, status)
             VALUES ($1, $2, $3, $4, $5, $6, 'revise', 'held before the upgrade',
                     'Lin Zhao joined Aster Labs', $7, 'held')",
        )
        .bind(held)
        .bind(kb)
        .bind(run_id)
        .bind(doc)
        .bind(empty)
        .bind(base.works_for)
        .bind(json!({
            "subject": "Lin Zhao", "property": "works_for", "object": "Aster Labs",
            "subject_id": lin, "predicate_id": base.works_for, "object_id": aster,
            "object_value": null,
        }))
        .execute(&pool)
        .await?;
        let refused = errata::decide_held(&pool, kb, held, true, Uuid::now_v7())
            .await
            .expect_err("the revision cannot be written");
        assert!(refused_as_empty(&refused), "{refused:?}");
        assert_eq!(
            base.live(empty).await?,
            (true, Some(day())),
            "the old row is not retracted first"
        );
        let status: String = sqlx::query_scalar("SELECT status FROM errata_actions WHERE id = $1")
            .bind(held)
            .fetch_one(&pool)
            .await?;
        assert_eq!(status, "held");

        // 一刻的事件改成状态属性：新行接过来的也是一段不成立的时间，同样拒
        let (acquisition, _) = graph::insert_fact(
            &pool,
            kb,
            meridian,
            Some(base.acquired),
            aster,
            moment(),
            0.9,
        )
        .await?;
        let event = Candidate {
            fact_id: acquisition,
            predicate_id: base.acquired,
            subject_id: meridian,
            subject: "Meridian Systems".into(),
            subject_class: Some("organization".into()),
            property: "acquired".into(),
            property_label: "acquired".into(),
            object_id: Some(aster),
            object: "Aster Labs".into(),
            ..candidate
        };
        let recorded = errata::record(
            &pool,
            kb,
            ActionInput {
                run_id,
                document_id: doc,
                candidate: Some(&event),
                proposed: Proposed::Revise {
                    property: Some("works_for".into()),
                    object: None,
                },
                reason: "the document says an employment",
                quote: Some("Lin Zhao joined Aster Labs"),
                document_text: text,
            },
        )
        .await?;
        assert_eq!(
            (recorded.status, recorded.detail.as_deref()),
            (
                "refused",
                Some("a state cannot take a time whose start and end are equal")
            )
        );
        assert_eq!(base.live(acquisition).await?, (true, Some(day())));
        anyhow::Ok(())
    }
    .await;
    base.remove().await?;
    run
}

/// 规则没有 marks：只有起点的陈述照旧蕴含一段从那天起的状态；只说了一刻的陈述蕴含状态时
/// 读法未知，不算隐含行
#[tokio::test]
async fn a_rule_reads_a_start_date_but_not_a_single_date() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let base = Base::new(&pool).await?;
    let run = async {
        base.statement("was hired by", Some(day()), None).await?;
        base.statement("signed with", Some(day()), Some(day()))
            .await?;
        for phrase in ["was hired by", "signed with"] {
            sqlx::query(
                "INSERT INTO implication_rules
                     (id, kb_id, trigger, phrase, subject_type_id, object_type_id, object_is_value,
                      conclude_property_id, status, decided_by)
                 VALUES ($1, $2, 'phrase', $3, $4, $5, false, $6, 'approved', 'person')",
            )
            .bind(Uuid::now_v7())
            .bind(base.kb)
            .bind(phrase)
            .bind(base.person)
            .bind(base.company)
            .bind(base.works_for)
            .execute(&pool)
            .await?;
        }
        let outcome = materialize(&pool, base.kb).await?;
        assert_eq!(outcome.implied, 1, "{outcome:?}");
        let rows: Vec<ImpliedRow> = sqlx::query_as(
            "SELECT valid_from, valid_to, implied FROM facts
              WHERE kb_id = $1 AND layer = 'typed' AND invalidated_at IS NULL",
        )
        .bind(base.kb)
        .fetch_all(&pool)
        .await?;
        assert_eq!(rows, vec![(Some(day()), None, true)]);
        anyhow::Ok(())
    }
    .await;
    base.remove().await?;
    run
}

/// 人改成起止相等的行（#975 评审）：带着 from_statement_id、implied 和来源链接，看上去和照抄
/// 一刻算出来的一样，可它的来源陈述不是那一刻，物化不作废它。照抄一刻写下的隐含行作废，
/// 人改过的隐含行不碰
#[tokio::test]
async fn a_row_a_person_set_to_a_single_date_is_not_retired() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let base = Base::new(&pool).await?;
    let run = async {
        // 「works for … from」算出一行；「was hired by … from」经规则蕴含一行，宾语另是一家，
        // 两行不并；「signed with」只说了一刻，#966 之前规则照抄出一行空段
        let works = base.statement("works for", Some(day()), None).await?;
        base.bind("works for", None).await?;
        let hired = base
            .statement_to("was hired by", base.aster, Some(day()), None, "day")
            .await?;
        let signed = base
            .statement("signed with", Some(day()), Some(day()))
            .await?;
        let mut rules = Vec::new();
        for phrase in ["was hired by", "signed with"] {
            let rule = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO implication_rules
                     (id, kb_id, trigger, phrase, subject_type_id, object_type_id, object_is_value,
                      conclude_property_id, status, decided_by)
                 VALUES ($1, $2, 'phrase', $3, $4, $5, false, $6, 'approved', 'person')",
            )
            .bind(rule)
            .bind(base.kb)
            .bind(phrase)
            .bind(base.person)
            .bind(base.company)
            .bind(base.works_for)
            .execute(&pool)
            .await?;
            rules.push(rule);
        }
        materialize(&pool, base.kb).await?;
        let row_of = |statement: Uuid, table: &'static str| {
            let pool = pool.clone();
            async move {
                let id: Uuid = sqlx::query_scalar(&format!(
                    "SELECT fact_id FROM {table} WHERE statement_id = $1"
                ))
                .bind(statement)
                .fetch_one(&pool)
                .await?;
                anyhow::Ok(id)
            }
        };
        let typed = row_of(works, "typed_fact_sources").await?;
        let implied = row_of(hired, "implied_fact_sources").await?;
        assert_ne!(typed, implied);
        // 人把两行都改成那一天：不变量出现之前 `correct_interval` 就这样写，行带着它的来源
        sqlx::query(
            "UPDATE facts SET valid_to = valid_from, valid_to_precision = valid_from_precision
              WHERE id = ANY($1)",
        )
        .bind(vec![typed, implied])
        .execute(&pool)
        .await?;
        let copied = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, layer,
                                valid_from, valid_from_precision, valid_to, valid_to_precision,
                                confidence, implied)
             VALUES ($1, $2, $3, $4, $5, 'typed', $6, 'day', $6, 'day', 0.9, true)",
        )
        .bind(copied)
        .bind(base.kb)
        .bind(base.lin)
        .bind(base.works_for)
        .bind(base.meridian)
        .bind(day())
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO implied_fact_sources (fact_id, rule_id, statement_id) VALUES ($1, $2, $3)",
        )
        .bind(copied)
        .bind(rules[1])
        .bind(signed)
        .execute(&pool)
        .await?;

        let outcome = materialize(&pool, base.kb).await?;
        assert_eq!(
            (outcome.retired, outcome.added, outcome.implied),
            (1, 0, 0),
            "{outcome:?}"
        );
        assert_eq!(
            base.live(copied).await?,
            (false, Some(day())),
            "copied: retired"
        );
        assert_eq!(
            base.live(typed).await?,
            (true, Some(day())),
            "a typed row a person set to a single date is kept"
        );
        assert_eq!(
            base.live(implied).await?,
            (true, Some(day())),
            "an implied row a person set to a single date is kept"
        );
        assert_eq!(materialize(&pool, base.kb).await?, Outcome::default());
        anyhow::Ok(())
    }
    .await;
    base.remove().await?;
    run
}

/// 终点说的是一个时段，比的却是时刻（#975 评审）：「left … in 2024」存成 2024-01-01，年精度。
/// 「works for … from 2024-03-01」从这个时段里开始，关在时段的尽头 2025-01-01，而不是另立一行
/// `- → 2024`、让它一直开着到 2025 年以后。先读到哪一句都一样
#[tokio::test]
async fn a_coarse_end_closes_a_row_that_starts_inside_its_period() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let (march, year, next_year) = (
        t("2024-03-01T00:00:00Z"),
        t("2024-01-01T00:00:00Z"),
        t("2025-01-01T00:00:00Z"),
    );
    for order in [["works for", "left"], ["left", "works for"]] {
        let base = Base::new(&pool).await?;
        let run = async {
            for phrase in order {
                if phrase == "left" {
                    base.statement_in(phrase, Some(year), Some(year), "year")
                        .await?;
                } else {
                    base.statement(phrase, Some(march), None).await?;
                }
            }
            base.bind("works for", None).await?;
            base.bind("left", Some("end")).await?;
            materialize(&pool, base.kb).await?;
            assert_eq!(
                base.rows().await?,
                vec![(Some(march), Some(next_year), 2)],
                "{order:?}: one span, closed at the end of 2024"
            );
            assert_eq!(
                base.at_meridian("2024-06-01T00:00:00Z").await?,
                vec!["Lin Zhao".to_string()],
                "{order:?}"
            );
            assert!(
                base.at_meridian("2025-06-01T00:00:00Z").await?.is_empty(),
                "{order:?}: not still working there in 2025"
            );
            assert_eq!(materialize(&pool, base.kb).await?, Outcome::default());
            anyhow::Ok(())
        }
        .await;
        base.remove().await?;
        run?;
    }
    Ok(())
}

/// 同一个时段的终点，对时段之前开始的行照旧关在时段的开头；对时段之后才开始的行不适用——
/// 那是更早的一段的结束，各自一行，开着的那段照旧开着。先写哪一句都一样
#[tokio::test]
async fn a_coarse_end_leaves_rows_outside_its_period_as_before() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let year = t("2024-01-01T00:00:00Z");
    let left_in_2024 = Validity {
        to_precision: Some("year"),
        ..ending(year)
    };
    let cases = [
        (
            t("2020-01-01T00:00:00Z"),
            vec![(Some(t("2020-01-01T00:00:00Z")), Some(year), 0)],
        ),
        (
            t("2025-06-01T00:00:00Z"),
            vec![
                (None, Some(year), 0),
                (Some(t("2025-06-01T00:00:00Z")), None, 0),
            ],
        ),
    ];
    for (start, expected) in cases {
        for start_first in [true, false] {
            let base = Base::new(&pool).await?;
            let run = async {
                let since = Validity::starting(Some(start), Some("day"));
                let writes = if start_first {
                    [since, left_in_2024]
                } else {
                    [left_in_2024, since]
                };
                for v in writes {
                    graph::insert_fact(
                        &pool,
                        base.kb,
                        base.lin,
                        Some(base.works_for),
                        base.meridian,
                        v,
                        0.9,
                    )
                    .await?;
                }
                assert_eq!(
                    base.rows().await?,
                    expected,
                    "from {start}, start first: {start_first}"
                );
                anyhow::Ok(())
            }
            .await;
            base.remove().await?;
            run?;
        }
    }
    Ok(())
}

/// 两个值同一刻开始的冲突（works_for 声明成函数性）：人选「关上旧的」却不给日期，旧的就关在
/// 新的起点——也是它自己的起点，一段不成立的状态，拒绝（`empty_state_span`）。冲突留着等人
/// 给一个日期
#[tokio::test]
async fn closing_a_simultaneous_conflict_at_its_own_start_is_refused() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let base = Base::new(&pool).await?;
    let run = async {
        sqlx::query("UPDATE relation_types SET functional = true WHERE id = $1")
            .bind(base.works_for)
            .execute(&pool)
            .await?;
        let since = Validity::starting(Some(day()), Some("day"));
        let mut facts = Vec::new();
        for object in [base.meridian, base.aster] {
            let (fact, _) = graph::insert_fact(
                &pool,
                base.kb,
                base.lin,
                Some(base.works_for),
                object,
                since,
                0.9,
            )
            .await?;
            facts.push(fact);
        }
        let report = utopia_store::temporal::reconcile_new_fact(
            &pool,
            base.kb,
            facts[1],
            base.lin,
            base.works_for,
            Some(base.aster),
            None,
            utopia_store::temporal::Uniqueness::SubjectSide,
            since,
            0.9,
        )
        .await?;
        assert_eq!(report.conflicts, 1);
        let conflicts = utopia_store::temporal::list_conflicts(&pool, base.kb, 10, 0).await?;
        let [conflict] = conflicts.as_slice() else {
            anyhow::bail!("one conflict: {conflicts:?}");
        };
        assert_eq!(conflict.reason, "simultaneous");
        let refused = utopia_store::temporal::resolve_conflict(
            &pool,
            base.kb,
            conflict.id,
            "close",
            None,
            "day",
        )
        .await
        .expect_err("closed where it starts");
        assert!(refused_as_empty(&refused), "{refused:?}");
        assert_eq!(
            base.live(facts[0]).await?,
            (true, None),
            "the old row is untouched"
        );
        assert_eq!(
            utopia_store::temporal::list_conflicts(&pool, base.kb, 10, 0)
                .await?
                .len(),
            1,
            "the conflict stays open"
        );
        utopia_store::temporal::resolve_conflict(
            &pool,
            base.kb,
            conflict.id,
            "close",
            Some(t("2024-01-01T00:00:00Z")),
            "day",
        )
        .await?;
        assert!(
            utopia_store::temporal::list_conflicts(&pool, base.kb, 10, 0)
                .await?
                .is_empty()
        );
        anyhow::Ok(())
    }
    .await;
    base.remove().await?;
    run
}

/// 人写的行不被第 0 步作废（#975 合并时补的守卫）。照它的来路造：属性原是事件，人写下一刻；
/// 一条只说了那一刻的陈述物化时并进这一行，留下来源链接，行却不因此变成算出来的
/// （`from_statement_id` 仍空）。后来本体页上把属性改成状态：这一行两端相等、有一条两端同值
/// 的来源陈述，可它是人写的，不作废。同一句话另算出的那一行（带 `from_statement_id`）照旧作废
#[tokio::test]
async fn a_row_a_person_wrote_is_kept_when_a_single_date_merged_into_it() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let base = Base::new(&pool).await?;
    let run = async {
        let (by_hand, _) = graph::insert_fact(
            &pool,
            base.kb,
            base.lin,
            Some(base.acquired),
            base.meridian,
            moment(),
            0.9,
        )
        .await?;
        base.statement("signed with", Some(day()), Some(day()))
            .await?;
        let to_aster = base
            .statement_to("signed with", base.aster, Some(day()), Some(day()), "day")
            .await?;
        base.bind_to("signed with", base.acquired, None).await?;
        let outcome = materialize(&pool, base.kb).await?;
        assert_eq!(
            (outcome.added, outcome.merged),
            (1, 1),
            "one merged into the person's row, one computed: {outcome:?}"
        );
        let computed: Uuid =
            sqlx::query_scalar("SELECT fact_id FROM typed_fact_sources WHERE statement_id = $1")
                .bind(to_aster)
                .fetch_one(&pool)
                .await?;
        let (from_statement, links): (Option<Uuid>, i64) = sqlx::query_as(
            "SELECT from_statement_id,
                    (SELECT count(*) FROM typed_fact_sources WHERE fact_id = f.id)
               FROM facts f WHERE id = $1",
        )
        .bind(by_hand)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            (from_statement, links),
            (None, 1),
            "the statement is a source of the person's row, which stays a person's"
        );

        // 本体页上把 acquired 改成状态
        sqlx::query("UPDATE relation_types SET temporal = 'state' WHERE id = $1")
            .bind(base.acquired)
            .execute(&pool)
            .await?;
        let outcome = materialize(&pool, base.kb).await?;
        assert_eq!(outcome.retired, 1, "only the computed row: {outcome:?}");
        assert_eq!(
            base.live(by_hand).await?,
            (true, Some(day())),
            "a row a person wrote is kept"
        );
        assert_eq!(
            base.live(computed).await?,
            (false, Some(day())),
            "the computed one is retired"
        );
        anyhow::Ok(())
    }
    .await;
    base.remove().await?;
    run
}
