//! 一条时间提及是字，不是日期（0044 第一刀，#729；0045）。
//!
//! 「去年冬天」落库的是这四个字和它在 `chunks.text` 里的**字符**偏移（不是字节——
//! 中文一个字三个字节，按字节记偏移界面就指错地方）。把字读成日期是后面几刀的事：
//! 这条陈述没有 `valid_from`，也没有 `valid_to`。

use sqlx::PgPool;
use utopia_store::graph::{self, FactObject};
use utopia_store::time_mentions;
use uuid::Uuid;

const ORG: &str = "time-mention-test";
const TEXT: &str = "星云科技去年冬天收购了北斗软件，作价十二亿元。";

struct Fixture {
    org: Uuid,
    kb: Uuid,
    nebula: Uuid,
    beidou: Uuid,
    chunk: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (nebula, beidou) = (Uuid::now_v7(), Uuid::now_v7());
    let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
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
    for (id, name) in [(nebula, "星云科技"), (beidou, "北斗软件")] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(kb)
            .bind(name)
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, status)
         VALUES ($1, $2, 'deal.txt', 'timemention', 'ready')",
    )
    .bind(doc)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, $4)",
    )
    .bind(chunk)
    .bind(kb)
    .bind(doc)
    .bind(TEXT)
    .execute(pool)
    .await?;
    Ok(Fixture {
        org,
        kb,
        nebula,
        beidou,
        chunk,
    })
}

#[tokio::test]
async fn a_time_mention_resolves_inside_the_chunk_and_sets_no_date() -> anyhow::Result<()> {
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
        let (fact, _) = graph::insert_open_statement(
            &pool,
            f.kb,
            f.nebula,
            "收购了",
            FactObject::Entity(f.beidou),
            None,
            0.9,
        )
        .await?;
        graph::add_evidence(&pool, fact, f.chunk, Some(TEXT), Some("收购了")).await?;

        // 服务端在块里搜出来的**字符**偏移
        let words = "去年冬天";
        let byte_start = TEXT.find(words).unwrap();
        let char_start = TEXT[..byte_start].chars().count() as i32;
        assert_eq!(char_start, 4, "「星云科技」四个字之后");
        assert_ne!(
            byte_start as i32, char_start,
            "中文里字节偏移与字符偏移不同"
        );
        let id =
            time_mentions::record(&pool, f.kb, fact, f.chunk, words, char_start, "when").await?;

        // 字与偏移在块里对得上：按字符数到那里，取同样多的字，得到的就是记下的字
        let (stored_text, stored_start): (String, i32) =
            sqlx::query_as("SELECT text, char_start FROM time_mentions WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await?;
        let chunk_text: String = sqlx::query_scalar("SELECT text FROM chunks WHERE id = $1")
            .bind(f.chunk)
            .fetch_one(&pool)
            .await?;
        let at: String = chunk_text
            .chars()
            .skip(stored_start as usize)
            .take(stored_text.chars().count())
            .collect();
        assert_eq!(at, words);
        assert_eq!(stored_text, words);
        // Postgres 的 substr 也按字符数：界面用它高亮时指的是同一处
        let by_sql: String = sqlx::query_scalar(
            "SELECT substr(c.text, m.char_start + 1, char_length(m.text))
             FROM time_mentions m JOIN chunks c ON c.id = m.chunk_id WHERE m.id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(by_sql, words);

        // 字不是日期：陈述在世界轴上没有位置
        #[derive(sqlx::FromRow)]
        struct Span {
            valid_from: Option<chrono::DateTime<chrono::Utc>>,
            valid_to: Option<chrono::DateTime<chrono::Utc>>,
            valid_from_precision: Option<String>,
            valid_to_precision: Option<String>,
        }
        let span: Span = sqlx::query_as(
            "SELECT valid_from, valid_to, valid_from_precision, valid_to_precision
             FROM facts WHERE id = $1",
        )
        .bind(fact)
        .fetch_one(&pool)
        .await?;
        assert!(span.valid_from.is_none() && span.valid_from_precision.is_none());
        assert!(span.valid_to.is_none() && span.valid_to_precision.is_none());

        let mentions = time_mentions::for_facts(&pool, &[fact]).await?;
        assert_eq!(mentions[&fact].len(), 1);
        assert_eq!(mentions[&fact][0].chunk_id, f.chunk);
        assert_eq!(mentions[&fact][0].role, "when");
        // 刚记下的字还没有读法：解释与解算都空着
        let m = &mentions[&fact][0];
        assert!(m.shape.is_none() && m.reference.is_none() && m.granularity.is_none());
        assert!(m.grade.is_none() && m.resolved_from.is_none() && m.resolved_to.is_none());

        // 起与止各是一条提及：同一处字换一个槽是另一行，同一个槽再记一次是同一行
        let ended =
            time_mentions::record(&pool, f.kb, fact, f.chunk, words, char_start, "ended").await?;
        assert_ne!(ended, id);
        assert_eq!(
            time_mentions::record(&pool, f.kb, fact, f.chunk, words, char_start, "when").await?,
            id
        );
        let mentions = time_mentions::for_facts(&pool, &[fact]).await?;
        assert_eq!(
            mentions[&fact]
                .iter()
                .map(|m| m.role.as_str())
                .collect::<Vec<_>>(),
            ["ended", "when"]
        );
        // 槽只有这两个
        assert!(
            time_mentions::record(&pool, f.kb, fact, f.chunk, words, char_start, "as_of")
                .await
                .is_err(),
            "the ledger takes only when / ended"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
