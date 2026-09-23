//! 一条时间提及按它的文档来读（0045 第一、二刀的账本侧）。
//!
//! 一条开放陈述的 `when` 与 `ended` 各是一条提及；按文档取回时带着它在证据里的那句话，
//! 按分块顺序、块内位置排；模型的解释与代码的解算各自落在提及上、取得回来；解算结果由
//! `set_open_validity` 写到开放行的世界轴上——只写开放行，类型化的行一行不动；
//! 结束了不知哪天的解算存得下；值与精度不一致的由库挡下。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, TimeZone, Utc};
use sqlx::PgPool;
use utopia_store::graph::{self, FactObject};
use utopia_store::time_mentions;
use uuid::Uuid;

const ORG: &str = "time-resolution-test";
const TEXT_A: &str = "Acme acquired Beta on March 4, 2011 and sold it in 2019.";
const TEXT_B: &str = "Beta had been founded two years earlier by Carol.";

struct Fixture {
    kb: Uuid,
    acme: Uuid,
    beta: Uuid,
    carol: Uuid,
    doc: Uuid,
    chunk_a: Uuid,
    chunk_b: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (acme, beta, carol) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (src, doc, chunk_a, chunk_b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(ORG)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(ORG)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(ORG)
        .execute(pool)
        .await?;
    for (id, name) in [(acme, "Acme"), (beta, "Beta"), (carol, "Carol")] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(kb)
            .bind(name)
            .execute(pool)
            .await?;
    }
    sqlx::query("INSERT INTO sources (id, kb_id, name) VALUES ($1, $2, '临时来源')")
        .bind(src)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO documents (id, kb_id, source_id, filename, sha256, status)
         VALUES ($1, $2, $3, 'deal.md', 'timeresolution', 'ready')",
    )
    .bind(doc)
    .bind(kb)
    .bind(src)
    .execute(pool)
    .await?;
    for (id, seq, text) in [(chunk_a, 0i32, TEXT_A), (chunk_b, 1i32, TEXT_B)] {
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(doc)
        .bind(seq)
        .bind(text)
        .execute(pool)
        .await?;
    }
    Ok(Fixture {
        kb,
        acme,
        beta,
        carol,
        doc,
        chunk_a,
        chunk_b,
    })
}

/// 服务端在块里搜出来的字符起点（这里的文本是 ASCII，字节即字符）
fn start_of(text: &str, words: &str) -> i32 {
    let byte = text.find(words).expect("words are in the chunk");
    text[..byte].chars().count() as i32
}

fn day(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct Span {
    valid_from: Option<DateTime<Utc>>,
    valid_from_precision: Option<String>,
    valid_to: Option<DateTime<Utc>>,
    valid_to_precision: Option<String>,
}

async fn span_of(pool: &PgPool, fact: Uuid) -> anyhow::Result<Span> {
    Ok(sqlx::query_as(
        "SELECT valid_from, valid_from_precision, valid_to, valid_to_precision
         FROM facts WHERE id = $1",
    )
    .bind(fact)
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn a_mention_is_read_then_computed_then_written_to_the_open_row() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    let f = seed(&pool).await?;

    let run = async {
        // 一条开放陈述，证据带引文；起与止各一条提及
        let (fact, _) = graph::insert_open_statement(
            &pool,
            f.kb,
            f.acme,
            "acquired",
            FactObject::Entity(f.beta),
            None,
            0.9,
        )
        .await?;
        graph::add_evidence_located(
            &pool,
            fact,
            f.chunk_a,
            Some(TEXT_A),
            Some("acquired"),
            Some((0, TEXT_A.len() as i32)),
        )
        .await?;
        let when_start = start_of(TEXT_A, "March 4, 2011");
        let ended_start = start_of(TEXT_A, "2019");
        let when = time_mentions::record(
            &pool,
            f.kb,
            fact,
            f.chunk_a,
            "March 4, 2011",
            when_start,
            "when",
        )
        .await?;
        let ended =
            time_mentions::record(&pool, f.kb, fact, f.chunk_a, "2019", ended_start, "ended")
                .await?;

        // 1. 按文档取回：两条，带证据里的那句话，按位置排
        let todo = time_mentions::for_document(&pool, f.doc).await?;
        assert_eq!(todo.len(), 2, "{todo:?}");
        assert_eq!(
            (
                todo[0].id,
                todo[0].role.as_str(),
                todo[0].text.as_str(),
                todo[0].char_start
            ),
            (when, "when", "March 4, 2011", when_start)
        );
        assert_eq!(
            (
                todo[1].id,
                todo[1].role.as_str(),
                todo[1].text.as_str(),
                todo[1].char_start
            ),
            (ended, "ended", "2019", ended_start)
        );
        assert!(todo
            .iter()
            .all(|m| m.sentence == TEXT_A && m.fact_id == fact && m.chunk_id == f.chunk_a));

        // 第二块上的陈述排在后面；证据没留引文的退回整块的字
        let (founded, _) = graph::insert_open_statement(
            &pool,
            f.kb,
            f.beta,
            "founded by",
            FactObject::Entity(f.carol),
            None,
            0.9,
        )
        .await?;
        graph::add_evidence(&pool, founded, f.chunk_b, None, Some("founded by")).await?;
        let earlier_start = start_of(TEXT_B, "two years earlier");
        let earlier = time_mentions::record(
            &pool,
            f.kb,
            founded,
            f.chunk_b,
            "two years earlier",
            earlier_start,
            "when",
        )
        .await?;
        let todo = time_mentions::for_document(&pool, f.doc).await?;
        assert_eq!(
            todo.iter().map(|m| m.id).collect::<Vec<_>>(),
            [when, ended, earlier],
            "chunk order, then position"
        );
        assert_eq!(
            todo[2].sentence, TEXT_B,
            "no quote on the evidence: the chunk's text"
        );

        // 作废的陈述不在等着读的名单里
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(founded)
            .execute(&pool)
            .await?;
        let todo = time_mentions::for_document(&pool, f.doc).await?;
        assert_eq!(todo.iter().map(|m| m.id).collect::<Vec<_>>(), [when, ended]);

        // 2. 解释与解算各自落在提及上，取得回来
        time_mentions::set_interpretation(
            &pool,
            when,
            "point",
            &serde_json::json!({ "absolute": "2011-03-04" }),
            "day",
        )
        .await?;
        // 解算给的值截到精度：15 点落成那一天的 0 点
        time_mentions::set_resolution(
            &pool,
            when,
            "A",
            Some(Utc.with_ymd_and_hms(2011, 3, 4, 15, 0, 0).unwrap()),
            Some("day"),
            None,
            None,
        )
        .await?;
        time_mentions::set_interpretation(
            &pool,
            ended,
            "ended",
            &serde_json::json!({ "absolute": "2019" }),
            "year",
        )
        .await?;
        time_mentions::set_resolution(
            &pool,
            ended,
            "A",
            None,
            None,
            Some(day(2019, 6, 1)),
            Some("year"),
        )
        .await?;
        let mentions = time_mentions::for_facts(&pool, &[fact]).await?;
        let m = &mentions[&fact];
        assert_eq!(m.len(), 2);
        let w = m.iter().find(|x| x.id == when).unwrap();
        assert_eq!(
            (
                w.shape.as_deref(),
                w.granularity.as_deref(),
                w.grade.as_deref()
            ),
            (Some("point"), Some("day"), Some("A"))
        );
        assert_eq!(
            w.reference,
            Some(serde_json::json!({ "absolute": "2011-03-04" }))
        );
        assert_eq!(
            (w.resolved_from, w.resolved_from_precision.as_deref()),
            (Some(day(2011, 3, 4)), Some("day"))
        );
        assert!(w.resolved_to.is_none() && w.resolved_to_precision.is_none());
        let e = m.iter().find(|x| x.id == ended).unwrap();
        assert_eq!(
            (e.shape.as_deref(), e.grade.as_deref()),
            (Some("ended"), Some("A"))
        );
        assert_eq!(
            (e.resolved_to, e.resolved_to_precision.as_deref()),
            (Some(day(2019, 1, 1)), Some("year"))
        );
        let resolved_at: Vec<Option<DateTime<Utc>>> =
            sqlx::query_scalar("SELECT resolved_at FROM time_mentions WHERE fact_id = $1")
                .bind(fact)
                .fetch_all(&pool)
                .await?;
        assert!(resolved_at.iter().all(Option::is_some));

        // 结束了不知哪天：存得下
        time_mentions::set_resolution(&pool, ended, "B", None, None, None, Some("unknown")).await?;
        let e = &time_mentions::for_facts(&pool, &[fact]).await?[&fact]
            .iter()
            .find(|x| x.id == ended)
            .cloned()
            .unwrap();
        assert_eq!(
            (
                e.grade.as_deref(),
                e.resolved_to,
                e.resolved_to_precision.as_deref()
            ),
            (Some("B"), None, Some("unknown"))
        );
        // 没算出来：四列全空
        time_mentions::set_resolution(&pool, ended, "C", None, None, None, None).await?;
        let e = &time_mentions::for_facts(&pool, &[fact]).await?[&fact]
            .iter()
            .find(|x| x.id == ended)
            .cloned()
            .unwrap();
        assert_eq!(e.grade.as_deref(), Some("C"));
        assert!(
            e.resolved_from.is_none()
                && e.resolved_from_precision.is_none()
                && e.resolved_to.is_none()
                && e.resolved_to_precision.is_none()
        );
        // 值与精度不一致的由库挡下：有日期的结束端不能说 unknown，等级只有 A / B / C
        assert!(time_mentions::set_resolution(
            &pool,
            ended,
            "A",
            None,
            None,
            Some(day(2019, 1, 1)),
            Some("unknown"),
        )
        .await
        .is_err());
        assert!(
            time_mentions::set_resolution(&pool, ended, "D", None, None, None, None)
                .await
                .is_err()
        );

        // 3. 解算写到开放行的世界轴上，按精度截断
        graph::set_open_validity(
            &pool,
            fact,
            Some(Utc.with_ymd_and_hms(2011, 3, 4, 15, 0, 0).unwrap()),
            Some("day"),
            Some(day(2019, 6, 1)),
            Some("year"),
            Some("A"),
        )
        .await?;
        assert_eq!(
            span_of(&pool, fact).await?,
            Span {
                valid_from: Some(day(2011, 3, 4)),
                valid_from_precision: Some("day".into()),
                valid_to: Some(day(2019, 1, 1)),
                valid_to_precision: Some("year".into()),
            }
        );
        // 结束了不知哪天
        graph::set_open_validity(
            &pool,
            fact,
            Some(day(2011, 3, 4)),
            Some("day"),
            None,
            Some("unknown"),
            Some("A"),
        )
        .await?;
        assert_eq!(
            span_of(&pool, fact).await?,
            Span {
                valid_from: Some(day(2011, 3, 4)),
                valid_from_precision: Some("day".into()),
                valid_to: None,
                valid_to_precision: Some("unknown".into()),
            }
        );
        // 结束了不知哪天的有自己的锚点：说出结束的就是这条陈述自己的文档（#393）
        let anchored: (bool,) = sqlx::query_as(
            "SELECT attested_to IS NOT NULL AND attested_to = attested_from
             FROM facts WHERE id = $1",
        )
        .bind(fact)
        .fetch_one(&pool)
        .await?;
        assert!(
            anchored.0,
            "an unknown ending is anchored on its own document"
        );
        // 又算不出来了：清空，锚点也跟着走
        graph::set_open_validity(&pool, fact, None, None, None, None, None).await?;
        assert_eq!(
            span_of(&pool, fact).await?,
            Span {
                valid_from: None,
                valid_from_precision: None,
                valid_to: None,
                valid_to_precision: None,
            }
        );
        let anchored: (bool,) =
            sqlx::query_as("SELECT attested_to IS NOT NULL FROM facts WHERE id = $1")
                .bind(fact)
                .fetch_one(&pool)
                .await?;
        assert!(!anchored.0);

        // 4. 类型化的行不归它写：一行不改
        let typed = Uuid::now_v7();
        sqlx::query("INSERT INTO facts (id, kb_id, subject_id, object_id) VALUES ($1, $2, $3, $4)")
            .bind(typed)
            .bind(f.kb)
            .bind(f.acme)
            .bind(f.beta)
            .execute(&pool)
            .await?;
        let refused = graph::set_open_validity(
            &pool,
            typed,
            Some(day(2011, 3, 4)),
            Some("day"),
            None,
            None,
            None,
        )
        .await;
        assert!(refused.is_err(), "a typed row keeps to insert_fact_inner");
        assert_eq!(
            span_of(&pool, typed).await?,
            Span {
                valid_from: None,
                valid_from_precision: None,
                valid_to: None,
                valid_to_precision: None,
            }
        );
        // 作废的开放行是历史，也不改
        assert!(graph::set_open_validity(
            &pool,
            founded,
            Some(day(2009, 1, 1)),
            Some("year"),
            None,
            None,
            None
        )
        .await
        .is_err());
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    run
}
