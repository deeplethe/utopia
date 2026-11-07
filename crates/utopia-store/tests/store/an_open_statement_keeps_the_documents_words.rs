//! 一条陈述留着文档自己的字（0044 第一刀，#729）。
//!
//! 开放陈述是 `facts` 里 `layer = 'open'` 的一行：短语照写、没有谓词。所有已经容得下
//! 空谓词的读路径（图、实体面板、邻域、路径、导出）要按短语显示它；同一句话再听到
//! 一次是同一行、证据累积；它的短语永远不出现在等着被采纳的说法里；角色词属性与
//! 时间词各自取得回来；证据带着引文在块里的偏移；库拒绝没有短语的开放行。
//! 角色词只在它那块活着时说话，没有活块时最后被取代的那块说话（0070）。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::graph::{self, FactObject};
use utopia_store::time_mentions;
use uuid::Uuid;

const ORG: &str = "open-statement-test";
const TEXT_A: &str = "Acme acquired Beta for $1.2 billion last winter.";
const TEXT_B: &str = "Regulators later cleared the deal: Acme acquired Beta.";

struct Fixture {
    kb: Uuid,
    acme: Uuid,
    beta: Uuid,
    chunk_a: Uuid,
    chunk_b: Uuid,
}

async fn seed(pool: &PgPool, org_name: &str) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let etype = Uuid::now_v7();
    let (acme, beta) = (Uuid::now_v7(), Uuid::now_v7());
    let (src, doc, chunk_a, chunk_b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(org_name)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(org_name)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(org_name)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'org', '组织')")
        .bind(etype)
        .bind(kb)
        .execute(pool)
        .await?;
    for (id, name) in [(acme, "Acme"), (beta, "Beta")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(etype)
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
         VALUES ($1, $2, $3, 'deal.md', 'openstatement', 'ready')",
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
        chunk_a,
        chunk_b,
    })
}

/// 引文在块里的字符偏移：服务端搜文本算出来的那个数（这里的文本是 ASCII，字节即字符）
fn span_of(text: &str, quote: &str) -> (i32, i32) {
    let start = text.find(quote).expect("quote is in the chunk");
    let chars_before = text[..start].chars().count() as i32;
    (chars_before, chars_before + quote.chars().count() as i32)
}

#[tokio::test]
async fn an_open_statement_shows_under_its_phrase_and_reuses_its_row() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    let f = seed(&pool, ORG).await?;

    let run = async {
        // 1. 落一条开放陈述：Acme —acquired→ Beta，短语照写、谓词为空、没有 valid_*
        let (fact, created) = graph::insert_open_statement(
            &pool,
            f.kb,
            f.acme,
            "acquired",
            FactObject::Entity(f.beta),
            None,
            0.9,
        )
        .await?;
        assert!(created);
        let quote = TEXT_A;
        graph::add_evidence_located(
            &pool,
            fact,
            f.chunk_a,
            Some(quote),
            Some("acquired"),
            Some(span_of(TEXT_A, quote)),
        )
        .await?;
        graph::add_statement_qualifier(
            &pool,
            fact,
            f.chunk_a,
            "amount",
            Some(&serde_json::json!({ "value": "$1.2 billion" })),
            None,
        )
        .await?;
        let (winter_start, _) = span_of(TEXT_A, "last winter");
        let mention = time_mentions::record(
            &pool,
            f.kb,
            fact,
            f.chunk_a,
            "last winter",
            winter_start,
            "when",
        )
        .await?;

        #[derive(sqlx::FromRow)]
        struct Row {
            layer: String,
            phrase: Option<String>,
            predicate_id: Option<Uuid>,
            valid_from: Option<chrono::DateTime<chrono::Utc>>,
            valid_to: Option<chrono::DateTime<chrono::Utc>>,
        }
        let row: Row = sqlx::query_as(
            "SELECT layer, phrase, predicate_id, valid_from, valid_to FROM facts WHERE id = $1",
        )
        .bind(fact)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row.layer, "open");
        assert_eq!(row.phrase.as_deref(), Some("acquired"));
        assert!(row.predicate_id.is_none(), "开放陈述没有谓词");
        assert!(
            row.valid_from.is_none() && row.valid_to.is_none(),
            "开放陈述在世界轴上还没有位置"
        );

        // 2. 每条读路径都按短语显示它
        let (_, edges, _, _) = graph::overview(&pool, f.kb, 50, None, None).await?;
        let e = edges
            .iter()
            .find(|e| e.id == fact)
            .expect("overview shows it");
        assert_eq!(e.label.as_deref(), Some("acquired"));
        assert!(e.inferred, "名字来自原文，不是本体认下的关系");
        // 角色词属性跟着边读回来（#1267），不混进按本体属性记的 qualifiers
        let amount = |q: &[utopia_core::models::StatementQualifier]| {
            serde_json::to_value(q).unwrap()
                == serde_json::json!([{ "role": "amount",
                    "value": { "value": "$1.2 billion" }, "entity_id": null, "entity_name": null,
                    "chunk_ids": [f.chunk_a] }])
        };
        assert!(
            amount(&e.statement_qualifiers),
            "{:?}",
            e.statement_qualifiers
        );
        assert!(e.qualifiers.is_empty());

        let (_, facts) = graph::entity_detail(&pool, f.kb, f.acme, None, None).await?;
        let d = facts
            .iter()
            .find(|x| x.id == fact)
            .expect("entity detail shows it");
        assert_eq!(d.predicate_label.as_deref(), Some("acquired"));
        assert_eq!(d.other_id, Some(f.beta));
        assert!(
            amount(&d.statement_qualifiers),
            "{:?}",
            d.statement_qualifiers
        );

        let (_, edges) = graph::neighborhood(&pool, f.kb, f.acme, 1, None, None).await?;
        let n = edges
            .iter()
            .find(|e| e.id == fact)
            .expect("neighborhood shows it");
        assert_eq!(n.label.as_deref(), Some("acquired"));

        let paths = utopia_store::paths::paths_between(
            &pool,
            f.kb,
            f.acme,
            f.beta,
            None,
            None,
            utopia_store::paths::Limits::default(),
        )
        .await?;
        let direct = paths
            .iter()
            .find(|p| p.edges.len() == 1 && p.edges[0].fact_id == fact)
            .expect("a path walks the open statement");
        assert_eq!(direct.edges[0].predicate.as_deref(), Some("acquired"));

        let exported =
            utopia_store::export::facts_page(&mut pool.begin().await?, f.kb, None).await?;
        let x = exported
            .iter()
            .find(|x| x.id == fact)
            .expect("export carries it");
        assert_eq!(x.surface_predicate.as_deref(), Some("acquired"));
        assert!(x.predicate_id.is_none());

        // 3. 同一句话再听到一次：同一行，证据累积
        let (again, created) = graph::insert_open_statement(
            &pool,
            f.kb,
            f.acme,
            "acquired",
            FactObject::Entity(f.beta),
            None,
            0.8,
        )
        .await?;
        assert_eq!(again, fact, "同 (主语, 短语, 宾语) 复用那一行");
        assert!(!created);
        let quote_b = "Acme acquired Beta.";
        graph::add_evidence_located(
            &pool,
            fact,
            f.chunk_b,
            Some(quote_b),
            Some("acquired"),
            Some(span_of(TEXT_B, quote_b)),
        )
        .await?;
        let evidence: Vec<(Uuid, Option<i32>, Option<i32>)> = sqlx::query_as(
            "SELECT chunk_id, quote_start, quote_end FROM fact_evidence WHERE fact_id = $1
             ORDER BY quote_start",
        )
        .bind(fact)
        .fetch_all(&pool)
        .await?;
        assert_eq!(evidence.len(), 2, "第二块的证据累积到同一行上");
        assert_eq!(evidence[0], (f.chunk_a, Some(0), Some(TEXT_A.len() as i32)));
        let (b_start, b_end) = span_of(TEXT_B, quote_b);
        assert_eq!(evidence[1], (f.chunk_b, Some(b_start), Some(b_end)));
        let live: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM facts
             WHERE kb_id = $1 AND layer = 'open' AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(live, 1, "两次观察一行");

        // 另一个宾语是另一行：去重键里有宾语
        let (other, created) = graph::insert_open_statement(
            &pool,
            f.kb,
            f.acme,
            "acquired",
            FactObject::Value(&serde_json::json!({ "value": "a rival" })),
            None,
            0.5,
        )
        .await?;
        assert!(created && other != fact);

        // 4. 角色词属性与时间词各自取得回来
        let qualifiers = graph::statement_qualifiers_for(&pool, &[fact], None).await?;
        let q = &qualifiers[&fact];
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].role, "amount");
        assert_eq!(
            q[0].value,
            Some(serde_json::json!({ "value": "$1.2 billion" }))
        );
        assert!(q[0].entity_id.is_none());
        // 角色词记在说它的那一块上：另一块的值另记一行，同一块再说一次不添行
        for (chunk, amount) in [(f.chunk_b, "$2 billion"), (f.chunk_a, "$1.2 billion")] {
            graph::add_statement_qualifier(
                &pool,
                fact,
                chunk,
                "amount",
                Some(&serde_json::json!({ "value": amount })),
                None,
            )
            .await?;
        }
        let amounts: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT chunk_id, value FROM statement_qualifiers
             WHERE fact_id = $1 AND role = 'amount' ORDER BY chunk_id",
        )
        .bind(fact)
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            amounts,
            vec![
                (f.chunk_a, serde_json::json!({ "value": "$1.2 billion" })),
                (f.chunk_b, serde_json::json!({ "value": "$2 billion" })),
            ]
        );
        // 值与实体必须恰好一个
        assert!(
            graph::add_statement_qualifier(&pool, fact, f.chunk_a, "buyer", None, None)
                .await
                .is_err()
        );
        graph::add_statement_qualifier(&pool, fact, f.chunk_a, "buyer", None, Some(f.acme)).await?;
        let q = &graph::statement_qualifiers_for(&pool, &[fact], None).await?[&fact];
        let buyer = q.iter().find(|x| x.role == "buyer").unwrap();
        assert_eq!(buyer.entity_id, Some(f.acme));
        assert_eq!(buyer.entity_name.as_deref(), Some("Acme"));

        let mentions = time_mentions::for_facts(&pool, &[fact]).await?;
        let m = &mentions[&fact];
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, mention);
        assert_eq!(
            (m[0].text.as_str(), m[0].char_start),
            ("last winter", winter_start)
        );
        assert_eq!(
            time_mentions::record(
                &pool,
                f.kb,
                fact,
                f.chunk_a,
                "last winter",
                winter_start,
                "when",
            )
            .await?,
            mention,
            "同一位置再记一次回的是同一行"
        );

        // 6. 库拒绝没有短语的开放行
        let bad = sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, layer, object_id)
             VALUES ($1, $2, $3, 'open', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(f.kb)
        .bind(f.acme)
        .bind(f.beta)
        .execute(&pool)
        .await;
        assert!(bad.is_err(), "开放行必须带短语");
        let bad = sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, layer, phrase, predicate_id, object_id)
             SELECT $1, $2, $3, 'open', 'acquired', r.id, $4
             FROM relation_types r WHERE r.kb_id = $2 LIMIT 1",
        )
        .bind(Uuid::now_v7())
        .bind(f.kb)
        .bind(f.acme)
        .bind(f.beta)
        .execute(&pool)
        .await;
        // 库里没有关系类型时这句什么也不插；有的话必须被约束挡下
        if let Ok(done) = &bad {
            assert_eq!(done.rows_affected(), 0);
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    run
}

/// 一条陈述，两块都是它的证据；每块记下自己说的角色词
async fn stated(pool: &PgPool, f: &Fixture, words: &[(Uuid, &str, &str)]) -> anyhow::Result<Uuid> {
    let (fact, _) = graph::insert_open_statement(
        pool,
        f.kb,
        f.acme,
        "covers",
        FactObject::Entity(f.beta),
        None,
        0.9,
    )
    .await?;
    for chunk in [f.chunk_a, f.chunk_b] {
        graph::add_evidence(pool, fact, chunk, None, None).await?;
    }
    for (chunk, role, value) in words {
        graph::add_statement_qualifier(
            pool,
            fact,
            *chunk,
            role,
            Some(&serde_json::json!(value)),
            None,
        )
        .await?;
    }
    Ok(fact)
}

async fn supersede(pool: &PgPool, chunk: Uuid, at: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE chunks SET superseded_at = $2::timestamptz WHERE id = $1")
        .bind(chunk)
        .bind(at)
        .execute(pool)
        .await?;
    Ok(())
}

/// 现在说话的角色词：(角色, 值, 说它的块)
async fn speaking(
    pool: &PgPool,
    fact: Uuid,
) -> anyhow::Result<Vec<(String, serde_json::Value, Vec<Uuid>)>> {
    let read = graph::statement_qualifiers_for(pool, &[fact], None).await?;
    Ok(read
        .get(&fact)
        .into_iter()
        .flatten()
        .map(|q| {
            (
                q.role.clone(),
                q.value.clone().unwrap_or_default(),
                q.chunk_ids.clone(),
            )
        })
        .collect())
}

/// 每个测试自己的组织：建库、跑完、拆掉
async fn with_fixture<F, Fut>(org_name: &str, run: F) -> anyhow::Result<()>
where
    F: FnOnce(PgPool, Fixture) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let clean = |pool: PgPool| async move {
        sqlx::query("DELETE FROM organizations WHERE name = $1")
            .bind(org_name)
            .execute(&pool)
            .await
    };
    clean(pool.clone()).await?;
    let f = seed(&pool, org_name).await?;
    let outcome = run(pool.clone(), f).await;
    clean(pool).await?;
    outcome
}

fn lines(value: &str, chunk: Uuid) -> (String, serde_json::Value, Vec<Uuid>) {
    ("lines".into(), serde_json::json!(value), vec![chunk])
}

#[tokio::test]
async fn a_superseded_chunk_stops_speaking_once_a_live_chunk_says_58_70() -> anyhow::Result<()> {
    with_fixture("role-word-superseded-test", |pool, f| async move {
        let fact = stated(
            &pool,
            &f,
            &[(f.chunk_a, "lines", "40-52"), (f.chunk_b, "lines", "58-70")],
        )
        .await?;
        supersede(&pool, f.chunk_a, "2026-03-01").await?;
        assert_eq!(
            speaking(&pool, fact).await?,
            vec![lines("58-70", f.chunk_b)]
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn two_live_chunks_that_disagree_both_speak_with_their_chunk() -> anyhow::Result<()> {
    with_fixture("role-word-disagree-test", |pool, f| async move {
        let fact = stated(
            &pool,
            &f,
            &[(f.chunk_a, "lines", "40-52"), (f.chunk_b, "lines", "58-70")],
        )
        .await?;
        assert_eq!(
            speaking(&pool, fact).await?,
            vec![lines("40-52", f.chunk_a), lines("58-70", f.chunk_b)]
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_status_the_live_chunk_does_not_repeat_is_gone() -> anyhow::Result<()> {
    with_fixture("role-word-dropped-test", |pool, f| async move {
        let fact = stated(
            &pool,
            &f,
            &[
                (f.chunk_a, "lines", "40-52"),
                (f.chunk_a, "status", "failing"),
                (f.chunk_b, "lines", "58-70"),
            ],
        )
        .await?;
        supersede(&pool, f.chunk_a, "2026-03-01").await?;
        assert_eq!(
            speaking(&pool, fact).await?,
            vec![lines("58-70", f.chunk_b)]
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_stale_statement_shows_58_70_from_its_last_superseded_chunk() -> anyhow::Result<()> {
    with_fixture("role-word-stale-test", |pool, f| async move {
        let fact = stated(
            &pool,
            &f,
            &[(f.chunk_a, "lines", "40-52"), (f.chunk_b, "lines", "58-70")],
        )
        .await?;
        supersede(&pool, f.chunk_a, "2026-03-01").await?;
        supersede(&pool, f.chunk_b, "2026-04-01").await?;
        assert_eq!(
            speaking(&pool, fact).await?,
            vec![lines("58-70", f.chunk_b)]
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_deleted_documents_chunk_is_dated_by_its_deleted_at() -> anyhow::Result<()> {
    with_fixture("role-word-deleted-test", |pool, f| async move {
        let fact = stated(
            &pool,
            &f,
            &[(f.chunk_a, "lines", "40-52"), (f.chunk_b, "lines", "58-70")],
        )
        .await?;
        supersede(&pool, f.chunk_a, "2026-03-01").await?;
        sqlx::query("UPDATE documents SET deleted_at = now() WHERE kb_id = $1")
            .bind(f.kb)
            .execute(&pool)
            .await?;
        assert_eq!(
            speaking(&pool, fact).await?,
            vec![lines("58-70", f.chunk_b)]
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_restored_document_lets_its_live_chunk_speak_58_70_alone_again() -> anyhow::Result<()> {
    with_fixture("role-word-restored-test", |pool, f| async move {
        let fact = stated(
            &pool,
            &f,
            &[(f.chunk_a, "lines", "40-52"), (f.chunk_b, "lines", "58-70")],
        )
        .await?;
        supersede(&pool, f.chunk_a, "2026-03-01").await?;
        let deleted_at = |at: Option<&'static str>| {
            sqlx::query("UPDATE documents SET deleted_at = $2::timestamptz WHERE kb_id = $1")
                .bind(f.kb)
                .bind(at)
                .execute(&pool)
        };
        // 删在取代之前：两块同一刻到头，都说话
        deleted_at(Some("2026-02-01")).await?;
        assert_eq!(
            speaking(&pool, fact).await?,
            vec![lines("40-52", f.chunk_a), lines("58-70", f.chunk_b)]
        );
        deleted_at(None).await?;
        assert_eq!(
            speaking(&pool, fact).await?,
            vec![lines("58-70", f.chunk_b)]
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_stale_read_as_of_february_shows_40_52_before_the_latest_chunk_existed(
) -> anyhow::Result<()> {
    with_fixture("role-word-stale-as-of-test", |pool, f| async move {
        let fact = stated(
            &pool,
            &f,
            &[(f.chunk_a, "lines", "40-52"), (f.chunk_b, "lines", "58-70")],
        )
        .await?;
        sqlx::raw_sql(&format!(
            "UPDATE documents SET created_at = '2026-01-01' WHERE kb_id = '{kb}';
             UPDATE chunks SET created_at = '2026-01-01', superseded_at = '2026-02-01'
              WHERE id = '{a}';
             UPDATE chunks SET created_at = '2026-03-01', superseded_at = '2026-04-01'
              WHERE id = '{b}';",
            kb = f.kb,
            a = f.chunk_a,
            b = f.chunk_b,
        ))
        .execute(&pool)
        .await?;
        let as_of = "2026-02-15T00:00:00Z".parse::<chrono::DateTime<chrono::Utc>>()?;
        let read = graph::statement_qualifiers_for(&pool, &[fact], Some(as_of)).await?;
        assert_eq!(
            read[&fact]
                .iter()
                .map(|q| q.value.clone())
                .collect::<Vec<_>>(),
            vec![Some(serde_json::json!("40-52"))]
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_read_as_of_february_shows_40_52_said_before_the_march_push() -> anyhow::Result<()> {
    with_fixture("role-word-as-of-test", |pool, f| async move {
        let fact = stated(
            &pool,
            &f,
            &[(f.chunk_a, "lines", "40-52"), (f.chunk_b, "lines", "58-70")],
        )
        .await?;
        sqlx::raw_sql(&format!(
            "UPDATE entities SET created_at = '2026-01-01' WHERE kb_id = '{kb}';
             UPDATE documents SET created_at = '2026-01-01' WHERE kb_id = '{kb}';
             UPDATE facts SET recorded_at = '2026-01-01' WHERE id = '{fact}';
             UPDATE chunks SET created_at = '2026-01-01', superseded_at = '2026-03-01'
              WHERE id = '{a}';
             UPDATE chunks SET created_at = '2026-03-01' WHERE id = '{b}';",
            kb = f.kb,
            a = f.chunk_a,
            b = f.chunk_b,
        ))
        .execute(&pool)
        .await?;
        let read_at = |as_of: Option<&str>| {
            let pool = pool.clone();
            let as_of = as_of.map(|t| t.parse::<chrono::DateTime<chrono::Utc>>().unwrap());
            async move {
                let (_, facts) = graph::entity_detail(&pool, f.kb, f.acme, None, as_of).await?;
                let row = facts
                    .into_iter()
                    .find(|x| x.id == fact)
                    .expect("the statement");
                anyhow::Ok(
                    row.statement_qualifiers
                        .into_iter()
                        .map(|q| q.value)
                        .collect::<Vec<_>>(),
                )
            }
        };
        assert_eq!(
            read_at(Some("2026-02-01T00:00:00Z")).await?,
            vec![Some(serde_json::json!("40-52"))]
        );
        assert_eq!(read_at(None).await?, vec![Some(serde_json::json!("58-70"))]);
        Ok(())
    })
    .await
}
