//! 引擎只重画自己画的终点（#679 第三轮评审的复现，留作回归）。
//!
//! 第二版把「引擎画的终点」记在行上、每次一次重算。评审又找出它碰坏的地方：两侧都唯一的
//! 关系一侧关上、另一侧又打开；原文说已经结束的旧值被当成后任，关掉了当下的值；撤回合并
//! 让记录轴回放合并窗口时答案变了；删除文档读名单早于上锁；改写把没裁的冲突撤下、把人裁过
//! 的冲突又问一遍；原文说出的终点并进引擎关上的行之后仍被重算；删掉的文档还在给行排序；
//! 证据写到刚被改写掉的旧行上；说不出时间的值谁先到结果不同；迁移之前关上的行永远不重算。
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
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'redraw-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'redraw-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'redraw-test')",
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

/// 一次观察：值、起止、出自哪天的文档、置信度
#[derive(Clone, Copy)]
struct Seen<'a> {
    subject: Uuid,
    value: &'a str,
    from: Option<&'a str>,
    to: Option<&'a str>,
    ended_unknown: bool,
    doc: Option<&'a str>,
    confidence: f32,
    /// 这次观察出自一块看图描述出来的文字（0040 决定 4）：它不许单独关掉一段正确的旧值
    described: bool,
}

fn seen<'a>(f: &Fixture, value: &'a str, from: &'a str) -> Seen<'a> {
    Seen {
        subject: f.lease,
        value,
        from: Some(from),
        to: None,
        ended_unknown: false,
        doc: None,
        confidence: 0.9,
        described: false,
    }
}

/// 一份自带日期的文档，一个分块
async fn document(pool: &PgPool, kb: Uuid, day: &str, text: &str) -> anyhow::Result<(Uuid, Uuid)> {
    let (d, c) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, doc_time, doc_time_source)
         VALUES ($1, $2, $3, $3, $4, 'content')",
    )
    .bind(d)
    .bind(kb)
    .bind(format!("doc-{d}.html"))
    .bind(t(day))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, $4)",
    )
    .bind(c)
    .bind(kb)
    .bind(d)
    .bind(text)
    .execute(pool)
    .await?;
    Ok((d, c))
}

/// 抽取的写法：落库（或并进已有断言）、写证据、对账
async fn observe(pool: &PgPool, f: &Fixture, x: Seen<'_>) -> anyhow::Result<Uuid> {
    let mut validity = Validity {
        from: x.from.map(t),
        from_precision: x.from.map(|_| "day"),
        from_grade: None,
        to: x.to.map(t),
        to_precision: if x.ended_unknown {
            Some("unknown")
        } else {
            x.to.map(|_| "day")
        },
        attested_at: None,
    };
    let mut chunk = None;
    if let Some(day) = x.doc {
        let (_, c) = document(pool, f.kb, day, x.value).await?;
        validity = validity.attested(Some(t(day)));
        chunk = Some(c);
    }
    let object = json!({ "value": x.value });
    let (id, _) = utopia_store::graph::insert_value_fact(
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
        if x.described {
            // 描述出来的块必须带锚（0058 的 CHECK）：图在文件里的哪一块
            sqlx::query(
                "UPDATE chunks SET origin = 'described', origin_model = 'test-vision',
                        anchor = jsonb_build_object('page', 1, 'image', 0) WHERE id = $1",
            )
            .bind(c)
            .execute(pool)
            .await?;
        }
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

/// (值, 起点, 终点, 终点精度, 终点锚点)
type Seg = (
    String,
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
                to_char(attested_to, 'YYYY-MM-DD')
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

async fn open_conflicts(pool: &PgPool, kb: Uuid) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT reason FROM fact_conflicts WHERE kb_id = $1 AND status = 'open' ORDER BY reason",
    )
    .bind(kb)
    .fetch_all(pool)
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
            .max_connections(16)
            .connect(&url)
            .await?,
    ))
}

/// 「一个人同时只领导一个项目，一个项目同时只有一个领导」：一行在两条时间线上。
/// Pat 先领导 A、后领导 B，A 关在 B 开始时；再读到一遍「Pat 领导 A」，什么都不变
#[tokio::test]
async fn a_relation_unique_on_both_sides_does_not_flip_between_them() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let leads = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind, temporal, functional, inverse_functional)
         VALUES ($1, $2, 'leads', 'leads', 'relation', 'state', TRUE, TRUE)",
    )
    .bind(leads)
    .bind(f.kb)
    .execute(&pool)
    .await?;
    let pat = entity(&pool, f.kb, f.etype, "Pat", Uuid::now_v7()).await?;
    let a = entity(&pool, f.kb, f.etype, "Project A", Uuid::now_v7()).await?;
    let b = entity(&pool, f.kb, f.etype, "Project B", Uuid::now_v7()).await?;
    let edge = |object: Uuid, from: &'static str| {
        let pool = pool.clone();
        let kb = f.kb;
        async move {
            let validity = Validity::starting(Some(t(from)), Some("day"));
            let (id, _) = utopia_store::graph::insert_fact(
                &pool,
                kb,
                pat,
                Some(leads),
                object,
                validity,
                0.9,
            )
            .await?;
            for side in [Uniqueness::SubjectSide, Uniqueness::ObjectSide] {
                utopia_store::temporal::reconcile_new_fact(
                    &pool,
                    kb,
                    id,
                    pat,
                    leads,
                    Some(object),
                    None,
                    side,
                    validity,
                    0.9,
                )
                .await?;
            }
            anyhow::Ok(())
        }
    };
    let state = || {
        let pool = pool.clone();
        let kb = f.kb;
        async move {
            let rows: Vec<(String, Option<String>)> = sqlx::query_as(
                "SELECT o.canonical_name, to_char(x.valid_to, 'YYYY-MM-DD')
                   FROM facts x JOIN entities o ON o.id = x.object_id
                  WHERE x.kb_id = $1 AND x.predicate_id = $2 AND x.invalidated_at IS NULL
                  ORDER BY x.valid_from",
            )
            .bind(kb)
            .bind(leads)
            .fetch_all(&pool)
            .await?;
            let total: i64 =
                sqlx::query_scalar("SELECT count(*) FROM facts WHERE predicate_id = $1")
                    .bind(leads)
                    .fetch_one(&pool)
                    .await?;
            anyhow::Ok((rows, total))
        }
    };
    edge(a, "2020-01-01").await?;
    edge(b, "2021-01-01").await?;
    let after_b = state().await?;
    edge(a, "2020-01-01").await?;
    edge(a, "2020-01-01").await?;
    let again = state().await?;
    cleanup(&pool, &f).await?;
    assert_eq!(
        after_b.0,
        vec![
            ("Project A".to_string(), Some("2021-01-01".to_string())),
            ("Project B".to_string(), None),
        ]
    );
    assert_eq!(
        again, after_b,
        "再读一遍没变的事实，时间线不该变，也不该多出行"
    );
    Ok(())
}

/// 原文说一个旧值已经结束（「HPBB1 不再是房东」出自 2021 年的文件）：那份文件的日期只说明
/// 那天之前它结束了，不能把它排在 2021 年、去关上当下的值
#[tokio::test]
async fn a_value_the_text_says_has_ended_does_not_close_the_current_one() -> anyhow::Result<()> {
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
            doc: Some("2020-09-01"),
            ..seen(&f, "BBHQ1", "2020-07-31")
        },
    )
    .await?;
    observe(
        &pool,
        &f,
        Seen {
            from: None,
            ended_unknown: true,
            doc: Some("2021-03-01"),
            ..seen(&f, "HPBB1", "")
        },
    )
    .await?;
    let recital = open_rows(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;

    // 结束在一个写明的日子，没有起点，出自一份更晚的文件
    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "NEW", "2020-01-01")).await?;
    observe(
        &pool,
        &f,
        Seen {
            from: None,
            to: Some("2019-12-31"),
            doc: Some("2021-03-01"),
            ..seen(&f, "OLD", "")
        },
    )
    .await?;
    let dated = open_rows(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!((recital, dated), (1, 1));
    Ok(())
}

/// 合并；合并之后到了一个值，关上了搬来的那一行；撤回。回放合并窗口里的某一刻，
/// 撤回之前问与之后问答案一样
#[tokio::test]
async fn a_revert_does_not_rewrite_what_the_merge_window_held() -> anyhow::Result<()> {
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
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mid: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await?;
    let replay = || {
        let pool = pool.clone();
        let (kb, pred, lease) = (f.kb, f.deadline, f.lease);
        async move {
            let held = utopia_store::record_axis::facts_held_at("f", Some(2));
            let owner = utopia_store::record_axis::owner_at("f", "subject_id", Some(2), false);
            let rows: Vec<(String, Option<String>, bool)> = sqlx::query_as(&format!(
                "SELECT object_value #>> '{{value}}', to_char(valid_to, 'YYYY-MM-DD'), {owner} = $3
                 FROM facts f WHERE f.kb_id = $1 AND f.predicate_id = $4 AND {held}
                 ORDER BY valid_from"
            ))
            .bind(kb)
            .bind(mid)
            .bind(lease)
            .bind(pred)
            .fetch_all(&pool)
            .await?;
            anyhow::Ok(rows)
        }
    };
    let before = replay().await?;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    utopia_store::resolution::revert_merge(&pool, f.kb, merge).await?;
    let after = replay().await?;
    let back = timeline(&pool, &f, source).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(after, before);
    assert_eq!(back.len(), 1, "D 回到源实体上");
    Ok(())
}

/// 删除文档等锁的时候，那一行被时间线重算改写成了新的一行：锁上之后重读名单，新行一并作废
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deletion_that_waited_for_the_lock_still_takes_the_rewritten_row() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let a = observe(
        &pool,
        &f,
        Seen {
            doc: Some("2020-01-01"),
            ..seen(&f, "A", "2020-01-01")
        },
    )
    .await?;
    let doc: Uuid = sqlx::query_scalar("SELECT document_id FROM fact_evidence WHERE fact_id = $1")
        .bind(a)
        .fetch_one(&pool)
        .await?;
    // 像一次时间线重算那样持着锁，把 A 改写成新的一行
    let key = format!("timeline:{}:{}:{}:SubjectSide", f.kb, f.lease, f.deadline);
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&key)
        .execute(&mut *tx)
        .await?;
    let deleting = {
        let pool = pool.clone();
        let kb = f.kb;
        tokio::spawn(async move { utopia_store::documents::delete(&pool, kb, doc, None).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let rewritten = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value, valid_from,
                            valid_from_precision, valid_to, valid_to_precision, confidence,
                            supersedes, attested_from, end_derived)
         SELECT $1, kb_id, subject_id, predicate_id, object_value, valid_from, valid_from_precision,
                '2020-06-01', 'day', confidence, id, attested_from, TRUE FROM facts WHERE id = $2",
    )
    .bind(rewritten)
    .bind(a)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
         SELECT $1, chunk_id, quote, document_id, doc_version FROM fact_evidence WHERE fact_id = $2",
    )
    .bind(rewritten)
    .bind(a)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(a)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    deleting.await??;
    let alive: bool = sqlx::query_scalar("SELECT invalidated_at IS NULL FROM facts WHERE id = $1")
        .bind(rewritten)
        .fetch_one(&pool)
        .await?;
    cleanup(&pool, &f).await?;
    assert!(!alive, "唯一的出处删掉了，改写出来的那一行也该作废");
    Ok(())
}

/// 并行落库：证据不会写到刚被改写掉的旧行上，每一条现存的行都有证据
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn evidence_follows_a_row_rewritten_while_it_was_written() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    for _ in 0..30 {
        let f = seed(&pool).await?;
        let dated = |value: &'static str, day: &'static str, with_start: bool| Seen {
            from: with_start.then_some(day),
            doc: Some(day),
            ..seen(&f, value, day)
        };
        let (p, q, r, s, u) = tokio::join!(
            observe(&pool, &f, dated("v4", "2020-05-10", true)),
            observe(&pool, &f, dated("v2", "2020-03-17", true)),
            observe(&pool, &f, dated("v5", "2020-06-08", false)),
            observe(&pool, &f, dated("v1", "2020-02-18", true)),
            observe(&pool, &f, dated("v3", "2020-04-14", false)),
        );
        for result in [p, q, r, s, u] {
            result?;
        }
        let bare: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM facts f WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM fact_evidence fe WHERE fe.fact_id = f.id)",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        let n = overlaps(&pool, &f, f.lease).await?;
        cleanup(&pool, &f).await?;
        assert_eq!((bare, n), (0, 0));
    }
    Ok(())
}

/// 人裁过「两个都留着」的一对，两行后来被改写，不再问第二遍
#[tokio::test]
async fn a_pair_a_person_kept_is_not_asked_again_after_a_rewrite() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "A", "2020-01-01")).await?;
    observe(
        &pool,
        &f,
        Seen {
            described: true,
            doc: Some("2020-03-01"),
            ..seen(&f, "B", "2020-03-01")
        },
    )
    .await?;
    let asked: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM fact_conflicts WHERE kb_id = $1 AND status = 'open'")
            .bind(f.kb)
            .fetch_all(&pool)
            .await?;
    for conflict in &asked {
        utopia_store::temporal::resolve_conflict(&pool, f.kb, *conflict, "keep", None, "day")
            .await?;
    }
    observe(&pool, &f, seen(&f, "C", "2020-06-01")).await?;
    let again = open_conflicts(&pool, f.kb).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(asked.len(), 1);
    assert_eq!(again, Vec::<String>::new());
    Ok(())
}

/// 同一天开始的两个值记了一对冲突；后来的值把两行都改写了，那对冲突还开着
#[tokio::test]
async fn an_unresolved_conflict_stays_open_when_its_rows_are_rewritten() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "P", "2020-01-01")).await?;
    observe(&pool, &f, seen(&f, "Q", "2020-01-01")).await?;
    let before = open_conflicts(&pool, f.kb).await?;
    observe(&pool, &f, seen(&f, "R", "2020-06-01")).await?;
    let after = open_conflicts(&pool, f.kb).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(before, vec!["simultaneous".to_string()]);
    assert_eq!(after, before);
    Ok(())
}

/// 引擎把 A 关在 B 开始时；一份文件写明 A 到那天为止。后来 B 被驳回，A 仍然到那天为止
#[tokio::test]
async fn an_end_the_text_states_on_a_row_the_engine_closed_is_kept() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "A", "2020-01-01")).await?;
    let b = observe(&pool, &f, seen(&f, "B", "2020-03-01")).await?;
    observe(
        &pool,
        &f,
        Seen {
            to: Some("2020-03-01"),
            doc: Some("2020-04-01"),
            ..seen(&f, "A", "2020-01-01")
        },
    )
    .await?;
    utopia_store::graph::reject_fact(&pool, f.kb, b).await?;
    let rows = timeline(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].2.as_deref(), Some("2020-03-01"));
    Ok(())
}

/// 删掉一个没起点的值最早那份文档：它不再在那天排进时间线，前任的锚点跟着往后挪
#[tokio::test]
async fn a_deleted_document_no_longer_dates_a_row() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "P", "2019-01-01")).await?;
    let s = observe(
        &pool,
        &f,
        Seen {
            from: None,
            doc: Some("2020-06-01"),
            ..seen(&f, "S", "")
        },
    )
    .await?;
    let early: Uuid =
        sqlx::query_scalar("SELECT document_id FROM fact_evidence WHERE fact_id = $1")
            .bind(s)
            .fetch_one(&pool)
            .await?;
    observe(
        &pool,
        &f,
        Seen {
            from: None,
            doc: Some("2021-06-01"),
            ..seen(&f, "S", "")
        },
    )
    .await?;
    utopia_store::documents::delete(&pool, f.kb, early, None).await?;
    let rows = timeline(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(rows[0].0, "P");
    assert_eq!(rows[0].4.as_deref(), Some("2021-06-01"), "{rows:?}");
    Ok(())
}

/// 一个说不出时间的值（没起点，也没有自带日期的证据）与一个有日期的值：谁先到都是同一对
/// 冲突交给人，两边都开着
#[tokio::test]
async fn a_value_with_no_time_meets_a_dated_one_the_same_way_in_either_order() -> anyhow::Result<()>
{
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let other = entity(&pool, f.kb, f.etype, "Other Lease", Uuid::now_v7()).await?;
    let keyless = |subject| Seen {
        subject,
        from: None,
        ..seen(&f, "K", "")
    };
    observe(&pool, &f, keyless(f.lease)).await?;
    observe(&pool, &f, seen(&f, "N", "2020-01-01")).await?;
    observe(
        &pool,
        &f,
        Seen {
            subject: other,
            ..seen(&f, "N", "2020-01-01")
        },
    )
    .await?;
    observe(&pool, &f, keyless(other)).await?;
    let shape = |rows: Vec<Seg>| {
        rows.into_iter()
            .map(|r| (r.0, r.2, r.3))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let k_first = shape(timeline(&pool, &f, f.lease).await?);
    let n_first = shape(timeline(&pool, &f, other).await?);
    let asked = open_conflicts(&pool, f.kb).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(k_first, n_first);
    assert_eq!(asked, vec!["no_time".to_string(), "no_time".to_string()]);
    Ok(())
}

/// 迁移之前引擎关上的行由 0057 回填成引擎画的：已有的库再来两份晚到的补充协议，
/// 时间线照样排得开。人关上的、原文说出终点的不回填
#[tokio::test]
async fn the_migration_marks_the_ends_the_old_engine_drew() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let backfill =
        include_str!("../../../migrations/0057_an_end_the_engine_drew_moves_with_what_follows.sql");
    let backfill = &backfill[backfill.find("UPDATE facts c").expect("0057 回填语句")..];

    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "v1", "2020-02-18")).await?;
    observe(&pool, &f, seen(&f, "v5", "2020-06-08")).await?;
    // 人手动关上的一段，与引擎关的形状一样
    let other = entity(&pool, f.kb, f.etype, "Other Lease", Uuid::now_v7()).await?;
    let by_hand = observe(
        &pool,
        &f,
        Seen {
            subject: other,
            ..seen(&f, "h1", "2020-01-01")
        },
    )
    .await?;
    observe(
        &pool,
        &f,
        Seen {
            subject: other,
            described: true,
            doc: Some("2020-03-01"),
            ..seen(&f, "h2", "2020-03-01")
        },
    )
    .await?;
    utopia_store::temporal::close_superseded(&pool, by_hand, t("2020-03-01"), "day").await?;
    sqlx::query(
        "INSERT INTO audit_events (id, kb_id, action, target_kind, target_id)
         VALUES ($1, $2, 'fact.close', 'fact', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(f.kb)
    .bind(by_hand)
    .execute(&pool)
    .await?;
    // 迁移之前的库：这一列全是假
    sqlx::query("UPDATE facts SET end_derived = FALSE WHERE kb_id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::raw_sql(backfill).execute(&pool).await?;
    let marked: Vec<String> = sqlx::query_scalar(
        "SELECT object_value #>> '{value}' FROM facts
          WHERE kb_id = $1 AND invalidated_at IS NULL AND end_derived ORDER BY 1",
    )
    .bind(f.kb)
    .fetch_all(&pool)
    .await?;
    observe(&pool, &f, seen(&f, "v2", "2020-03-17")).await?;
    observe(&pool, &f, seen(&f, "v3", "2020-04-14")).await?;
    let n = overlaps(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(marked, vec!["v1".to_string()], "只有引擎关的那一段");
    assert_eq!(n, 0);
    Ok(())
}

/// 连环合并里撤回头一环（#679 第四轮评审）：S 并进 T、T 再并进 C，撤回 S→T。
/// S 的事实此刻挂在 C 身上，也要跟着 S 回去，C 的时间线上不留它
#[tokio::test]
async fn undoing_the_first_merge_of_a_chain_brings_its_facts_home() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease S", Uuid::now_v7()).await?;
    let chain_end = entity(&pool, f.kb, f.etype, "Lease C", Uuid::now_v7()).await?;
    observe(
        &pool,
        &f,
        Seen {
            subject: source,
            ..seen(&f, "S1", "2020-01-01")
        },
    )
    .await?;
    let first =
        utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "test")
            .await?;
    utopia_store::resolution::merge_entities(&pool, f.kb, f.lease, chain_end, None, "test").await?;
    utopia_store::resolution::revert_merge(&pool, f.kb, first).await?;
    let home = timeline(&pool, &f, source).await?;
    let tail = timeline(&pool, &f, chain_end).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(home.len(), 1, "S1 回到 S 上：{home:?}");
    assert_eq!(home[0].0, "S1");
    assert!(tail.iter().all(|s| s.0 != "S1"), "C 上不留 S1：{tail:?}");
    Ok(())
}

/// 撤回合并把合并之后改写出来的行送回源实体时，它们身上开着的冲突跟着走（#679 第四轮评审）
#[tokio::test]
async fn a_revert_keeps_the_conflict_its_rows_carried() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
    for value in ["D", "E"] {
        observe(
            &pool,
            &f,
            Seen {
                subject: source,
                ..seen(&f, value, "2020-01-01")
            },
        )
        .await?;
    }
    let merge =
        utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "test")
            .await?;
    observe(&pool, &f, seen(&f, "N", "2020-06-01")).await?;
    utopia_store::resolution::revert_merge(&pool, f.kb, merge).await?;
    let conflicts = open_conflicts(&pool, f.kb).await?;
    let back = timeline(&pool, &f, source).await?;
    let overlap = overlaps(&pool, &f, source).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(back.len(), 2, "D 与 E 回到源实体：{back:?}");
    assert!(
        overlap == 0 || conflicts.contains(&"simultaneous".to_string()),
        "两行叠在一起就得有一条开着的冲突：{conflicts:?}"
    );
    Ok(())
}

/// 删一篇给几百个实体各记了一个属性的文档：改拿谓词一级的锁，删得掉，也都作废了。
/// 逐条锁的时候几千条就要几千把锁，两万条时锁表装不下（#679 第四轮评审）
#[tokio::test]
async fn deleting_a_document_that_dates_many_timelines_still_works() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    let (doc, chunk) = document(&pool, f.kb, "2020-01-01", "a table").await?;
    // 默认 300 条走谓词锁那条路； 可以复现评审时锁表装不下的规模
    let n: usize = std::env::var("REDRAW_BULK_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    for i in 0..n {
        let holder = entity(&pool, f.kb, f.etype, &format!("Lease {i}"), Uuid::now_v7()).await?;
        let (id, _) = utopia_store::graph::insert_value_fact(
            &pool,
            f.kb,
            holder,
            Some(f.deadline),
            &json!({ "value": format!("2021-01-{:02}", i % 28 + 1) }),
            Validity {
                from: Some(t("2020-01-01")),
                from_precision: Some("day"),
                ..Default::default()
            },
            0.9,
        )
        .await?;
        utopia_store::graph::add_evidence(&pool, id, chunk, Some("row"), None).await?;
    }
    let report = utopia_store::documents::delete(&pool, f.kb, doc, None).await?;
    let restored = utopia_store::documents::restore(&pool, f.kb, doc).await;
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM facts WHERE kb_id = $1 AND predicate_id = $2 AND invalidated_at IS NULL",
    )
    .bind(f.kb)
    .bind(f.deadline)
    .fetch_one(&pool)
    .await?;
    cleanup(&pool, &f).await?;
    assert_eq!(report.invalidated_facts, n);
    assert!(restored.is_ok(), "{:?}", restored.err());
    assert_eq!(live, n as i64, "撤销删除之后全都回来");
    Ok(())
}

/// 删掉没起点那一行最早的证据文档：引擎的锚点挪到第二份文档，读出来的起点也跟着挪，
/// 两段不叠；撤销删除又都回去（#679 第四轮评审）
#[tokio::test]
async fn deleting_the_first_document_moves_the_read_start_with_the_anchor() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let f = seed(&pool).await?;
    observe(&pool, &f, seen(&f, "P", "2019-01-01")).await?;
    for day in ["2020-06-01", "2021-06-01"] {
        observe(
            &pool,
            &f,
            Seen {
                from: None,
                doc: Some(day),
                ..seen(&f, "S", day)
            },
        )
        .await?;
    }
    let first: Uuid = sqlx::query_scalar(
        "SELECT d.id FROM fact_evidence fe JOIN chunks c ON c.id = fe.chunk_id
           JOIN documents d ON d.id = c.document_id
          WHERE d.kb_id = $1 ORDER BY d.doc_time LIMIT 1",
    )
    .bind(f.kb)
    .fetch_one(&pool)
    .await?;
    let before = overlaps(&pool, &f, f.lease).await?;
    utopia_store::documents::delete(&pool, f.kb, first, None).await?;
    let after_delete = overlaps(&pool, &f, f.lease).await?;
    utopia_store::documents::restore(&pool, f.kb, first).await?;
    let after_restore = overlaps(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!((before, after_delete, after_restore), (0, 0, 0));
    Ok(())
}
