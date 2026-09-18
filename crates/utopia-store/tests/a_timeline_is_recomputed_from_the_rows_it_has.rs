//! 一条时间线只取决于它有哪些行（#679 第二轮评审的复现，留作回归）。
//!
//! 第一版靠「终点正好等于另一行的起点」认出引擎画的界：后面那一段挪了（晚到的证据把
//! 日期往前挪、补上了起点）就认不出来，界留在原地、两段叠在一起；整理一轮一轮地来，
//! 轮数用完就静默停下；撤回合并先改行再拿锁，与落库对账互相等死，还会复活人驳回的行、
//! 丢掉人改的区间。现在引擎画的终点记在行上（0057），每次按当下的行一次重算。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use serde_json::json;
use sqlx::PgPool;
use utopia_store::graph::Validity;
use utopia_store::temporal::Uniqueness;
use uuid::Uuid;

struct Fixture {
    kb: Uuid,
    etype: Uuid,
    lease: Uuid,
    deadline: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (etype, deadline, lease) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'recompute-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'recompute-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'recompute-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'lease', 'Lease')",
    )
    .bind(etype)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype, temporal, functional)
         VALUES ($1, $2, 'option_deadline', 'option deadline', 'attribute', 'date', 'state', TRUE)",
    )
    .bind(deadline)
    .bind(kb)
    .execute(pool)
    .await?;
    let lease = entity(pool, kb, etype, "HQ Lease", lease).await?;
    Ok(Fixture {
        kb,
        etype,
        lease,
        deadline,
    })
}

async fn entity(
    pool: &PgPool,
    kb: Uuid,
    etype: Uuid,
    name: &str,
    id: Uuid,
) -> anyhow::Result<Uuid> {
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(kb)
    .bind(etype)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(id)
}

fn t(day: &str) -> chrono::DateTime<chrono::Utc> {
    format!("{day}T00:00:00Z").parse().unwrap()
}

/// 一次观察：值、起止、出自哪天的哪种文档、置信度、起点的来历
#[derive(Clone, Copy)]
struct Seen<'a> {
    subject: Uuid,
    value: &'a str,
    from: Option<&'a str>,
    to: Option<&'a str>,
    doc: Option<&'a str>,
    confidence: f32,
    /// `None` = 没经过时间解析（这些测试的默认），`Some("C")` = 有时间词却锚不到
    grade: Option<&'a str>,
}

fn seen<'a>(f: &Fixture, value: &'a str, from: &'a str) -> Seen<'a> {
    Seen {
        subject: f.lease,
        value,
        from: Some(from),
        to: None,
        doc: None,
        confidence: 0.9,
        grade: None,
    }
}

/// 抽取的写法：落库（或并进已有断言）、写证据、对账——**并进已有断言的也对账**
async fn observe(pool: &PgPool, f: &Fixture, x: Seen<'_>) -> anyhow::Result<Uuid> {
    let mut validity = Validity {
        from: x.from.map(t),
        from_precision: x.from.map(|_| "day"),
        to: x.to.map(t),
        to_precision: x.to.map(|_| "day"),
        attested_at: None,
        from_grade: x.grade,
    };
    let mut chunk = None;
    if let Some(doc_time) = x.doc {
        let (d, c) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256, doc_time, doc_time_source)
             VALUES ($1, $2, $3, $3, $4, 'content')",
        )
        .bind(d)
        .bind(f.kb)
        .bind(format!("doc-{d}.html"))
        .bind(t(doc_time))
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, $4)",
        )
        .bind(c)
        .bind(f.kb)
        .bind(d)
        .bind(x.value)
        .execute(pool)
        .await?;
        validity = validity.attested(Some(t(doc_time)));
        chunk = Some(c);
    }
    let object = json!({ "value": x.value });
    let (id, _created) = utopia_store::graph::insert_value_fact(
        pool,
        f.kb,
        x.subject,
        Some(f.deadline),
        &object,
        validity,
        x.confidence,
    )
    .await?;
    if let Some(c) = chunk {
        utopia_store::graph::add_evidence(pool, id, c, Some(x.value), None).await?;
    }
    utopia_store::temporal::reconcile_new_fact(
        pool,
        f.kb,
        id,
        x.subject,
        f.deadline,
        None,
        Some(&object),
        Uniqueness::SubjectSide,
        validity,
        x.confidence,
    )
    .await?;
    Ok(id)
}

/// (值, 起点, 终点, 终点精度, 终点锚点, 读出的起点, 读出的终点)
type Seg = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn timeline(pool: &PgPool, f: &Fixture, subject: Uuid) -> anyhow::Result<Vec<Seg>> {
    let hf = utopia_store::world_axis::facts_holds_from("f");
    let ht = utopia_store::world_axis::facts_holds_to("f");
    Ok(sqlx::query_as(&format!(
        "SELECT object_value #>> '{{value}}', to_char(valid_from, 'YYYY-MM-DD'),
                to_char(valid_to, 'YYYY-MM-DD'), valid_to_precision,
                to_char(attested_to, 'YYYY-MM-DD'),
                to_char({hf}, 'YYYY-MM-DD'), to_char({ht}, 'YYYY-MM-DD')
         FROM facts f
         WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3 AND invalidated_at IS NULL
         ORDER BY {hf} NULLS FIRST, {ht} NULLS LAST, object_value #>> '{{value}}'"
    ))
    .bind(f.kb)
    .bind(subject)
    .bind(f.deadline)
    .fetch_all(pool)
    .await?)
}

/// 读出来的区间叠在一起、值又不同的行有几对
async fn overlaps(pool: &PgPool, f: &Fixture, subject: Uuid) -> anyhow::Result<i64> {
    let hf = utopia_store::world_axis::facts_holds_from("f");
    let ht = utopia_store::world_axis::facts_holds_to("f");
    Ok(sqlx::query_scalar(&format!(
        "WITH r AS (
            SELECT f.id, f.object_value, {hf} AS hf, {ht} AS ht FROM facts f
            WHERE f.kb_id = $1 AND f.subject_id = $2 AND f.predicate_id = $3
              AND f.invalidated_at IS NULL)
         SELECT count(*) FROM r a JOIN r b ON a.id < b.id
            AND a.object_value IS DISTINCT FROM b.object_value
         WHERE COALESCE(a.hf, '-infinity') < COALESCE(b.ht, 'infinity')
           AND COALESCE(b.hf, '-infinity') < COALESCE(a.ht, 'infinity')"
    ))
    .bind(f.kb)
    .bind(subject)
    .bind(f.deadline)
    .fetch_one(pool)
    .await?)
}

async fn open_rows(pool: &PgPool, f: &Fixture, subject: Uuid) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM facts WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3
           AND invalidated_at IS NULL AND valid_to IS NULL AND valid_to_precision IS NULL",
    )
    .bind(f.kb)
    .bind(subject)
    .bind(f.deadline)
    .fetch_one(pool)
    .await?)
}

async fn live_id(pool: &PgPool, f: &Fixture, subject: Uuid, value: &str) -> anyhow::Result<Uuid> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM facts WHERE kb_id = $1 AND subject_id = $2
           AND object_value #>> '{value}' = $3 AND invalidated_at IS NULL",
    )
    .bind(f.kb)
    .bind(subject)
    .bind(value)
    .fetch_one(pool)
    .await?)
}

async fn cleanup(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(pool)
        .await?;
    Ok(())
}

async fn pool() -> anyhow::Result<Option<PgPool>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    Ok(Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(12)
            .connect(&url)
            .await?,
    ))
}

/// 后面那一段晚到的文件补上了起点（第十一份补充协议之后，交割文件说 BBHQ1 从 7 月 31 日
/// 起就是房东）：HPBB1 止于那一天，不再叠到 8 月 13 日
#[tokio::test]
async fn a_start_that_arrives_later_moves_the_end_before_it() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(
        &pool,
        &f,
        Seen {
            doc: Some("2016-05-16"),
            ..seen(&f, "HPBB1", "2016-05-16")
        },
    )
    .await?;
    observe(
        &pool,
        &f,
        Seen {
            from: None,
            doc: Some("2020-08-13"),
            ..seen(&f, "BBHQ1", "")
        },
    )
    .await?;
    observe(
        &pool,
        &f,
        Seen {
            doc: Some("2020-09-01"),
            ..seen(&f, "BBHQ1", "2020-07-31")
        },
    )
    .await?;
    let rows = timeline(&pool, &f, f.lease).await?;
    let n = overlaps(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(n, 0, "{rows:?}");
    assert_eq!(rows[0].0, "HPBB1");
    assert_eq!(rows[0].2.as_deref(), Some("2020-07-31"), "{rows:?}");
    Ok(())
}

/// 没起点的后任又被一份更早的文件提到：前任的锚点跟着往前挪
#[tokio::test]
async fn earlier_evidence_for_a_value_with_no_start_moves_the_end_before_it() -> anyhow::Result<()>
{
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(
        &pool,
        &f,
        Seen {
            doc: Some("2019-01-01"),
            ..seen(&f, "P", "2019-01-01")
        },
    )
    .await?;
    let startless = |doc| Seen {
        from: None,
        doc: Some(doc),
        ..seen(&f, "S", "")
    };
    observe(&pool, &f, startless("2021-06-01")).await?;
    observe(&pool, &f, startless("2020-06-01")).await?;
    let rows = timeline(&pool, &f, f.lease).await?;
    let n = overlaps(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(n, 0, "{rows:?}");
    assert_eq!(rows[0].0, "P");
    assert_eq!(rows[0].4.as_deref(), Some("2020-06-01"), "{rows:?}");
    Ok(())
}

/// 一条没起点的值先到，有起点的后到：文档日期从不写进日期列，也不叠
#[tokio::test]
async fn a_value_with_no_start_that_arrives_first_still_takes_its_place() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(
        &pool,
        &f,
        Seen {
            from: None,
            doc: Some("2021-01-01"),
            ..seen(&f, "S", "")
        },
    )
    .await?;
    observe(&pool, &f, seen(&f, "P", "2019-01-01")).await?;
    observe(&pool, &f, seen(&f, "Q", "2020-01-01")).await?;
    let n = overlaps(&pool, &f, f.lease).await?;
    let in_date_columns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM facts
          WHERE kb_id = $1 AND (valid_from = '2021-01-01' OR valid_to = '2021-01-01')",
    )
    .bind(f.kb)
    .fetch_one(&pool)
    .await?;
    let open = open_rows(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!((n, in_date_columns, open), (0, 0, 1));
    Ok(())
}

/// 已经结束的新值与开着的旧值，谁先到都一样
#[tokio::test]
async fn a_closed_value_and_an_open_one_meet_the_same_way_in_either_order() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let other = entity(&pool, f.kb, f.etype, "Other Lease", Uuid::now_v7()).await?;
    let closed = |subject| Seen {
        subject,
        to: Some("2021-01-01"),
        ..seen(&f, "B", "2019-01-01")
    };
    observe(&pool, &f, seen(&f, "A", "2018-01-01")).await?;
    observe(&pool, &f, closed(f.lease)).await?;
    observe(&pool, &f, closed(other)).await?;
    observe(
        &pool,
        &f,
        Seen {
            subject: other,
            ..seen(&f, "A", "2018-01-01")
        },
    )
    .await?;
    let a_then_b = timeline(&pool, &f, f.lease).await?;
    let b_then_a = timeline(&pool, &f, other).await?;
    let n = overlaps(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(a_then_b, b_then_a);
    assert_eq!(n, 0);
    Ok(())
}

/// 七十个置信度不够的值排在前面，一个够格的值到了：每一个都关上，没有轮数用完这回事
#[tokio::test]
async fn unanchored_values_do_not_keep_a_placed_one_from_closing_them() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    for i in 0..70 {
        let (value, day) = (
            format!("w{i:02}"),
            format!("{}-{:02}-01", 2000 + i / 12, i % 12 + 1),
        );
        observe(
            &pool,
            &f,
            Seen {
                grade: Some("C"),
                ..seen(&f, &value, &day)
            },
        )
        .await?;
    }
    observe(&pool, &f, seen(&f, "Z", "2010-01-01")).await?;
    let open = open_rows(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(open, 1, "只有 Z 还开着");
    Ok(())
}

/// 起点锚不到的后任不关前任（0045 第 3 刀）：它进得了图、排得进时间线，可它在轴上的
/// 位置是文档日期给的，不是那句话给的，拿它改写历史就是拿一个猜的位置改写历史
#[tokio::test]
async fn an_unanchored_successor_closes_nothing() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "first", "2020-01-01")).await?;
    observe(
        &pool,
        &f,
        Seen {
            grade: Some("C"),
            ..seen(&f, "second", "2021-01-01")
        },
    )
    .await?;
    let open = open_rows(&pool, &f, f.lease).await?;
    // 再来一个锚得住的：这一个才关得了前面两个
    observe(&pool, &f, seen(&f, "third", "2022-01-01")).await?;
    let after = open_rows(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(open, 2, "锚不到的那一行关不了它前面的");
    assert_eq!(after, 1, "锚得住的一来，前面两行都关上");
    Ok(())
}

/// 两条各一百个值的时间线交错着合并：一次重算排好，没有轮数上限
#[tokio::test]
async fn a_merge_of_two_long_timelines_leaves_one_timeline() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
    for i in 0..200 {
        let (value, day) = (
            format!("v{i:03}"),
            format!("{}-{:02}-01", 2010 + i / 12, i % 12 + 1),
        );
        let subject = if i % 2 == 0 { f.lease } else { source };
        observe(
            &pool,
            &f,
            Seen {
                subject,
                ..seen(&f, &value, &day)
            },
        )
        .await?;
    }
    utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "test").await?;
    let n = overlaps(&pool, &f, f.lease).await?;
    let open = open_rows(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!((n, open), (0, 1));
    Ok(())
}

/// 合并两条交错的时间线再撤回：两边都回到合并之前的样子
#[tokio::test]
async fn a_revert_gives_both_timelines_back_as_they_were() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
    for (v, d) in [
        ("A", "2020-01-01"),
        ("C", "2020-05-01"),
        ("E", "2020-09-01"),
    ] {
        observe(&pool, &f, seen(&f, v, d)).await?;
    }
    for (v, d) in [
        ("B", "2020-03-01"),
        ("D", "2020-07-01"),
        ("F", "2020-11-01"),
    ] {
        observe(
            &pool,
            &f,
            Seen {
                subject: source,
                ..seen(&f, v, d)
            },
        )
        .await?;
    }
    let (target_before, source_before) = (
        timeline(&pool, &f, f.lease).await?,
        timeline(&pool, &f, source).await?,
    );
    let merge =
        utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "test")
            .await?;
    let merged = timeline(&pool, &f, f.lease).await?;
    let merged_overlaps = overlaps(&pool, &f, f.lease).await?;
    utopia_store::resolution::revert_merge(&pool, f.kb, merge).await?;
    let (target_after, source_after) = (
        timeline(&pool, &f, f.lease).await?,
        timeline(&pool, &f, source).await?,
    );
    cleanup(&pool, &f).await?;
    assert_eq!((merged.len(), merged_overlaps), (6, 0));
    assert_eq!(target_after, target_before);
    assert_eq!(source_after, source_before);
    Ok(())
}

/// 合并之后又到了一个值、关上了搬来的那一行：撤回时那一行（连同关上它的改写）回源实体
#[tokio::test]
async fn a_revert_takes_back_a_moved_value_closed_after_the_merge() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
    observe(&pool, &f, seen(&f, "A", "2019-01-01")).await?;
    observe(
        &pool,
        &f,
        Seen {
            subject: source,
            ..seen(&f, "D", "2020-04-01")
        },
    )
    .await?;
    let merge =
        utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "test")
            .await?;
    observe(&pool, &f, seen(&f, "N", "2020-06-01")).await?;
    utopia_store::resolution::revert_merge(&pool, f.kb, merge).await?;
    let target = timeline(&pool, &f, f.lease).await?;
    let back = timeline(&pool, &f, source).await?;
    cleanup(&pool, &f).await?;
    assert!(target.iter().all(|r| r.0 != "D"), "{target:?}");
    assert_eq!(back.len(), 1);
    assert_eq!((back[0].0.as_str(), back[0].2.as_deref()), ("D", None));
    assert_eq!(target[0].2.as_deref(), Some("2020-06-01"), "A 止于 N");
    Ok(())
}

/// 合并把 A 关在 D 开始时，人把 A 的起点改成了 2019-12-01：撤回之后人改的那一行还在
#[tokio::test]
async fn a_revert_keeps_the_interval_a_person_corrected() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
    observe(&pool, &f, seen(&f, "A", "2020-01-01")).await?;
    observe(
        &pool,
        &f,
        Seen {
            subject: source,
            ..seen(&f, "D", "2020-04-01")
        },
    )
    .await?;
    let merge =
        utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "test")
            .await?;
    let a = live_id(&pool, &f, f.lease, "A").await?;
    let fixed = utopia_store::temporal::correct_interval(
        &pool,
        a,
        Validity {
            from: Some(t("2019-12-01")),
            from_precision: Some("day"),
            to: Some(t("2020-04-01")),
            to_precision: Some("day"),
            attested_at: None,
            from_grade: None,
        },
    )
    .await?
    .unwrap();
    utopia_store::temporal::reconcile_moved_facts(&pool, f.kb, &[fixed]).await?;
    utopia_store::resolution::revert_merge(&pool, f.kb, merge).await?;
    let rows = timeline(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(
        rows,
        vec![(
            "A".to_string(),
            Some("2019-12-01".to_string()),
            Some("2020-04-01".to_string()),
            Some("day".to_string()),
            None,
            Some("2019-12-01".to_string()),
            Some("2020-04-01".to_string()),
        )]
    );
    Ok(())
}

/// 合并把 A 关上之后，人把 A 驳回了：撤回合并不让它回来
#[tokio::test]
async fn a_revert_does_not_bring_back_a_value_a_person_rejected() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
    observe(&pool, &f, seen(&f, "A", "2020-01-01")).await?;
    observe(
        &pool,
        &f,
        Seen {
            subject: source,
            ..seen(&f, "D", "2020-04-01")
        },
    )
    .await?;
    let merge =
        utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "test")
            .await?;
    let a = live_id(&pool, &f, f.lease, "A").await?;
    utopia_store::graph::reject_fact(&pool, f.kb, a).await?;
    utopia_store::resolution::revert_merge(&pool, f.kb, merge).await?;
    let rows = timeline(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert!(rows.iter().all(|r| r.0 != "A"), "{rows:?}");
    Ok(())
}

/// 人驳回了后任：关在它开始时的前任重新开着
#[tokio::test]
async fn rejecting_a_successor_reopens_the_value_it_closed() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "A", "2020-01-01")).await?;
    let b = observe(&pool, &f, seen(&f, "B", "2020-04-01")).await?;
    utopia_store::graph::reject_fact(&pool, f.kb, b).await?;
    let open = open_rows(&pool, &f, f.lease).await?;
    let rows = timeline(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(open, 1);
    assert_eq!((rows[0].0.as_str(), rows[0].2.as_deref()), ("A", None));
    Ok(())
}

/// 人写明的终点不随时间线重算：后任走了，它仍然关着
#[tokio::test]
async fn an_end_a_person_wrote_is_never_recomputed() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let a = observe(&pool, &f, seen(&f, "A", "2020-01-01")).await?;
    utopia_store::temporal::close_superseded(&pool, a, t("2020-03-01"), "day").await?;
    let b = observe(&pool, &f, seen(&f, "B", "2020-04-01")).await?;
    utopia_store::graph::reject_fact(&pool, f.kb, b).await?;
    let rows = timeline(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].2.as_deref(), Some("2020-03-01"));
    Ok(())
}

/// 删掉后任出自的文档，前任重新开着；恢复文档，前任又关上
#[tokio::test]
async fn deleting_and_restoring_a_document_moves_the_end_it_drew() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "A", "2020-01-01")).await?;
    let b = observe(
        &pool,
        &f,
        Seen {
            doc: Some("2020-04-01"),
            ..seen(&f, "B", "2020-04-01")
        },
    )
    .await?;
    let doc: Uuid = sqlx::query_scalar("SELECT document_id FROM fact_evidence WHERE fact_id = $1")
        .bind(b)
        .fetch_one(&pool)
        .await?;
    let a_end = || async {
        let (end,): (Option<String>,) = sqlx::query_as(
            "SELECT to_char(valid_to, 'YYYY-MM-DD') FROM facts
              WHERE kb_id = $1 AND object_value #>> '{value}' = 'A' AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        anyhow::Ok(end)
    };
    assert_eq!(a_end().await?.as_deref(), Some("2020-04-01"));
    utopia_store::documents::delete(&pool, f.kb, doc, None).await?;
    let deleted = a_end().await?;
    utopia_store::documents::restore(&pool, f.kb, doc).await?;
    let restored = a_end().await?;
    cleanup(&pool, &f).await?;
    assert_eq!(deleted, None);
    assert_eq!(restored.as_deref(), Some("2020-04-01"));
    Ok(())
}

/// 并行落库：三个、五个值同时到，最后都是一条时间线
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn values_that_arrive_together_leave_one_timeline() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    for _ in 0..10 {
        let f = seed(&pool).await?;
        observe(&pool, &f, seen(&f, "v1", "2020-02-18")).await?;
        observe(&pool, &f, seen(&f, "v5", "2020-06-08")).await?;
        let (x, y, z) = tokio::join!(
            observe(&pool, &f, seen(&f, "v4", "2020-05-10")),
            observe(&pool, &f, seen(&f, "v2", "2020-03-17")),
            observe(&pool, &f, seen(&f, "v3", "2020-04-14")),
        );
        x?;
        y?;
        z?;
        let three = (
            overlaps(&pool, &f, f.lease).await?,
            open_rows(&pool, &f, f.lease).await?,
        );
        cleanup(&pool, &f).await?;
        assert_eq!(three, (0, 1));

        let f = seed(&pool).await?;
        let (p, q, r, s, u) = tokio::join!(
            observe(&pool, &f, seen(&f, "v4", "2020-05-10")),
            observe(&pool, &f, seen(&f, "v2", "2020-03-17")),
            observe(&pool, &f, seen(&f, "v5", "2020-06-08")),
            observe(&pool, &f, seen(&f, "v1", "2020-02-18")),
            observe(&pool, &f, seen(&f, "v3", "2020-04-14")),
        );
        for result in [p, q, r, s, u] {
            result?;
        }
        let five = (
            timeline(&pool, &f, f.lease).await?.len(),
            overlaps(&pool, &f, f.lease).await?,
            open_rows(&pool, &f, f.lease).await?,
        );
        cleanup(&pool, &f).await?;
        assert_eq!(five, (5, 0, 1));
    }
    Ok(())
}

/// 撤回合并与落库同时进行：谁也不等死谁，最后没有叠在一起的区间
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revert_and_arriving_values_do_not_deadlock() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let mut errors = Vec::new();
    for i in 0..40 {
        let f = seed(&pool).await?;
        let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
        for (v, d) in [
            ("A", "2020-01-01"),
            ("C", "2020-05-01"),
            ("E", "2020-09-01"),
        ] {
            observe(&pool, &f, seen(&f, v, d)).await?;
        }
        for (v, d) in [("B", "2020-03-01"), ("D", "2020-07-01")] {
            observe(
                &pool,
                &f,
                Seen {
                    subject: source,
                    ..seen(&f, v, d)
                },
            )
            .await?;
        }
        let merge =
            utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "test")
                .await?;
        let delay = std::time::Duration::from_millis(i % 6);
        let (reverted, n, m) = tokio::join!(
            utopia_store::resolution::revert_merge(&pool, f.kb, merge),
            async {
                tokio::time::sleep(delay).await;
                observe(&pool, &f, seen(&f, "N", "2020-02-01")).await
            },
            observe(&pool, &f, seen(&f, "M", "2020-08-01")),
        );
        if let Err(e) = reverted {
            errors.push(format!("revert: {e}"));
        }
        for arrived in [n, m] {
            if let Err(e) = arrived {
                errors.push(format!("arrive: {e}"));
            }
        }
        let (target, back) = (
            overlaps(&pool, &f, f.lease).await?,
            overlaps(&pool, &f, source).await?,
        );
        cleanup(&pool, &f).await?;
        assert_eq!((target, back), (0, 0), "trial {i}");
    }
    assert!(errors.is_empty(), "{errors:?}");
    Ok(())
}
