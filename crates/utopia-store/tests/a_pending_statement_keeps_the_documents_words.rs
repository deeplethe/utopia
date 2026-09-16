//! 等人点头的一条陈述也留着文档自己的字（0044 第一刀的记忆侧，迁移 0062）——打在真库上。
//!
//! 对话里说「Acme 把总部搬到了深圳」，抽出来的不再是一条带本体谓词的三元组，而是一条
//! 开放陈述：短语照写、角色词属性、照抄的时间词、引文在块里的位置。它先在
//! `pending_facts` 里等着；人点头之后按 `insert_open_statement` 那条路进账本——
//! `layer = 'open'`、没有谓词、不写 `valid_*`、证据带偏移、属性与时间词各归各表；
//! 摇头之后挡的是**这句话**（短语进了键），同一对实体之间换一句话照样能提。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::pending::{self, Outcome, Proposal};
use uuid::Uuid;

const SENTENCE: &str = "Acme moved its headquarters to Shenzhen on 2026-03-15.";
const PHRASE: &str = "moved its headquarters to";
const TIME_WORDS: &str = "on 2026-03-15";

struct Fixture {
    /// 夹具建的 org，收尾时从它删起（级联）
    org: Uuid,
    kb: Uuid,
    acme: Uuid,
    shenzhen: Uuid,
    chunk: Uuid,
    /// 那句记忆入库后的全文——带 `[YYYY-MM-DD HH:MM] ` 前缀，偏移按它算
    text: String,
}

async fn fixture(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'pending-words-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'pending-words-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'pending-words-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    let (acme, shenzhen) = (Uuid::now_v7(), Uuid::now_v7());
    for (id, name) in [(acme, "Acme"), (shenzhen, "Shenzhen")] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(kb)
            .bind(name)
            .execute(pool)
            .await?;
    }
    // 那句记忆，走产品自己的路落成 chunk
    let (_doc, chunk) =
        utopia_store::memory::append_episode(pool, kb, SENTENCE, chrono::Utc::now()).await?;
    let text: String = sqlx::query_scalar("SELECT text FROM chunks WHERE id = $1")
        .bind(chunk)
        .fetch_one(pool)
        .await?;
    Ok(Fixture {
        org,
        kb,
        acme,
        shenzhen,
        chunk,
        text,
    })
}

/// 引文在块里的字符偏移：服务端搜文本算出来的那个数——从入库后的文本算，不从字面量算
fn span_of(text: &str, quote: &str) -> (i32, i32) {
    let start = text.find(quote).expect("quote is in the chunk");
    let chars_before = text[..start].chars().count() as i32;
    (chars_before, chars_before + quote.chars().count() as i32)
}

async fn live_facts(pool: &PgPool, kb: Uuid) -> anyhow::Result<i64> {
    let (n,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM facts WHERE kb_id = $1 AND invalidated_at IS NULL")
            .bind(kb)
            .fetch_one(pool)
            .await?;
    Ok(n)
}

#[tokio::test]
async fn a_pending_statement_keeps_the_documents_words() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = fixture(&pool).await?;

    let run = async {
        let (quote_start, quote_end) = span_of(&f.text, SENTENCE);
        let (time_start, _) = span_of(&f.text, TIME_WORDS);
        assert!(quote_start > 0, "入库的文本带时间戳前缀，偏移不能从 0 起");
        let qualifiers = serde_json::json!([{ "role": "on", "value": "2026-03-15" }]);
        let time_words = serde_json::json!([{ "text": TIME_WORDS, "char_start": time_start }]);
        let propose = |phrase: &'static str| {
            pending::propose(
                &pool,
                Proposal {
                    kb_id: f.kb,
                    subject_id: f.acme,
                    predicate_id: None,
                    object_id: Some(f.shenzhen),
                    object_value: None,
                    proposed_predicate: Some(phrase),
                    validity: utopia_store::graph::Validity::default(),
                    confidence: 0.9,
                    chunk_id: f.chunk,
                    proposed_by: None,
                    proposed_token: None,
                    phrase: Some(phrase),
                    qualifiers: Some(&qualifiers),
                    time_words: Some(&time_words),
                    quote_span: Some((quote_start, quote_end)),
                },
            )
        };

        // 1. 提议不上图；队列里的那一条带着文档自己的字
        let first = propose(PHRASE).await?;
        assert!(matches!(first, Outcome::Proposed(_)), "第一次该进队列");
        assert_eq!(live_facts(&pool, f.kb).await?, 0, "提议阶段图上不能有活边");
        let queued = pending::for_chunk(&pool, f.kb, f.chunk).await?;
        assert_eq!(queued.len(), 1);
        let q = &queued[0];
        assert_eq!(q.subject_name, "Acme");
        assert_eq!(q.object_name.as_deref(), Some("Shenzhen"));
        assert_eq!(q.phrase.as_deref(), Some(PHRASE));
        assert!(q.predicate_id.is_none(), "开放陈述没有谓词");
        assert_eq!(q.qualifiers.as_ref(), Some(&qualifiers));
        assert_eq!(q.time_words.as_ref(), Some(&time_words));
        assert_eq!(
            (q.quote_start, q.quote_end),
            (Some(quote_start), Some(quote_end))
        );
        assert_eq!(q.quote, f.text, "原句要跟着提议一起给人看");

        // 2. 同一句重抽不重复提
        assert_eq!(propose(PHRASE).await?, Outcome::AlreadyPending);

        // 3. 点头：按开放陈述进账本
        let Outcome::Proposed(id) = first else {
            unreachable!()
        };
        let done = pending::confirm(&pool, f.kb, id).await?;
        assert!(done.created, "确认该落一条新事实");
        assert_eq!(done.conflicts, 0, "开放陈述不做时态对账");
        assert_eq!(done.snapshot["phrase"], PHRASE);
        assert_eq!(done.snapshot["qualifiers"], qualifiers);
        assert_eq!(live_facts(&pool, f.kb).await?, 1);

        #[derive(sqlx::FromRow)]
        struct Row {
            layer: String,
            phrase: Option<String>,
            predicate_id: Option<Uuid>,
            object_id: Option<Uuid>,
            valid_from: Option<chrono::DateTime<chrono::Utc>>,
            valid_to: Option<chrono::DateTime<chrono::Utc>>,
            attested_from: Option<chrono::DateTime<chrono::Utc>>,
        }
        let row: Row = sqlx::query_as(
            "SELECT layer, phrase, predicate_id, object_id, valid_from, valid_to, attested_from
               FROM facts WHERE id = $1",
        )
        .bind(done.fact_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row.layer, "open");
        assert_eq!(row.phrase.as_deref(), Some(PHRASE));
        assert!(row.predicate_id.is_none(), "开放陈述没有谓词");
        assert_eq!(row.object_id, Some(f.shenzhen));
        assert!(
            row.valid_from.is_none() && row.valid_to.is_none(),
            "开放陈述在世界轴上还没有位置"
        );
        assert!(
            row.attested_from.is_some(),
            "记录轴总是有：记忆文档没有文档日期，落成此刻"
        );

        // 证据指回那句记忆，带着引文在块里的位置
        let (ev_chunk, ev_proposed, ev_start, ev_end): (
            Uuid,
            Option<String>,
            Option<i32>,
            Option<i32>,
        ) = sqlx::query_as(
            "SELECT chunk_id, proposed_predicate, quote_start, quote_end
               FROM fact_evidence WHERE fact_id = $1",
        )
        .bind(done.fact_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(ev_chunk, f.chunk, "证据要指回那句记忆");
        assert_eq!(ev_proposed.as_deref(), Some(PHRASE), "表层谓词就是短语");
        assert_eq!((ev_start, ev_end), (Some(quote_start), Some(quote_end)));

        // 角色词属性按文档的角色词落
        let qualifier_rows: Vec<(String, Option<serde_json::Value>, Option<Uuid>)> =
            sqlx::query_as(
                "SELECT role, value, entity_id FROM statement_qualifiers WHERE fact_id = $1",
            )
            .bind(done.fact_id)
            .fetch_all(&pool)
            .await?;
        assert_eq!(qualifier_rows.len(), 1);
        assert_eq!(qualifier_rows[0].0, "on");
        assert_eq!(qualifier_rows[0].1, Some(serde_json::json!("2026-03-15")));
        assert!(qualifier_rows[0].2.is_none());

        // 时间词照抄，偏移在块里能复原出那几个字
        let mentions: Vec<(String, i32, String)> = sqlx::query_as(
            "SELECT m.text, m.char_start, substr(c.text, m.char_start + 1, length(m.text))
               FROM time_mentions m JOIN chunks c ON c.id = m.chunk_id
              WHERE m.fact_id = $1",
        )
        .bind(done.fact_id)
        .fetch_all(&pool)
        .await?;
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].0, TIME_WORDS);
        assert_eq!(mentions[0].1, time_start);
        assert_eq!(mentions[0].2, TIME_WORDS, "偏移指回的就是块里的那几个字");

        assert!(
            pending::for_chunk(&pool, f.kb, f.chunk).await?.is_empty(),
            "点头后队列清空"
        );

        // 4. 图上已有的不再问
        assert_eq!(propose(PHRASE).await?, Outcome::AlreadyAsserted);

        // 5. 摇头挡的是这句话：短语进了拒绝记录；同一对实体之间换一句话照样能提
        let Outcome::Proposed(bad) = propose("closed its office in").await? else {
            panic!("换一个短语该是新提议");
        };
        pending::reject(&pool, f.kb, bad, None).await?;
        let rejected: Vec<Option<String>> =
            sqlx::query_scalar("SELECT phrase FROM rejected_facts WHERE kb_id = $1")
                .bind(f.kb)
                .fetch_all(&pool)
                .await?;
        assert_eq!(rejected, vec![Some("closed its office in".to_string())]);
        assert_eq!(propose("closed its office in").await?, Outcome::Rejected);
        assert!(
            matches!(propose("opened a lab in").await?, Outcome::Proposed(_)),
            "拒绝只挡那一句话，不挡这一对实体之间的一切"
        );
        assert_eq!(live_facts(&pool, f.kb).await?, 1, "拒绝不碰图");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // 删掉夹具自己建的 org，级联带走工作区、知识库与其余一切
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
