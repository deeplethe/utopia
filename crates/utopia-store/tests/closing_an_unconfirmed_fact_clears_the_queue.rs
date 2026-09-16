//! 「闭合」这个出路要真的能清掉一条「已不再被陈述」的事实，打在真库上。
//!
//! 闭合走作废+改写（`temporal::close_superseded` / `rewrite_end_tx`），修正行把证据一起
//! 复制下来（`copy_evidence`）——于是它照样「证据全停在旧版分块上」。这一档的判据少了
//! 「至今仍成立」那一句，两个后果一起冒出来：
//!
//! 1. 闭合回来的那条还在列表里、计数也不减，看起来像闭合没生效；
//! 2. 再点一次闭合，修正行已经有终点，接口按「开放区间才可闭合」把它判成找不到而报错。
//!
//! 这里钉住：这一档只列**现行**的事实（0022：「结束不知哪天」不是开放），闭合过的行
//! 立刻离开，`counts` 与列表同一套 WHERE 一起归零。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

struct Fx {
    kb: Uuid,
    fact: Uuid,
    chunk: Uuid,
}

fn t(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (etype, rel) = (Uuid::now_v7(), Uuid::now_v7());
    let (subject, object) = (Uuid::now_v7(), Uuid::now_v7());
    let (doc, chunk, fact) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'stale-close-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'stale-close-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name)
         VALUES ($1, $2, 'stale-close-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'thing', 'Thing')",
    )
    .bind(etype)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal)
         VALUES ($1, $2, 'leads', 'leads', 'state')",
    )
    .bind(rel)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [(subject, "Akkaraju"), (object, "Weta")] {
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
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256) VALUES ($1, $2, 'a.md', 'sha-a')",
    )
    .bind(doc)
    .bind(kb)
    .execute(pool)
    .await?;
    // 第一版的分块：证据就落在这里，文档出新版后它会被取代
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text, doc_version)
         VALUES ($1, $2, $3, 0, 'Akkaraju leads Weta', 1)",
    )
    .bind(chunk)
    .bind(kb)
    .bind(doc)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, $5, 0.9)",
    )
    .bind(fact)
    .bind(kb)
    .bind(subject)
    .bind(rel)
    .bind(object)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote)
         VALUES ($1, $2, 'Akkaraju leads Weta')",
    )
    .bind(fact)
    .bind(chunk)
    .execute(pool)
    .await?;
    Ok(Fx { kb, fact, chunk })
}

async fn stale_ids(pool: &PgPool, kb: Uuid) -> anyhow::Result<Vec<Uuid>> {
    Ok(utopia_store::graph::stale_facts(pool, kb, 50, 0)
        .await?
        .into_iter()
        .map(|f| f.id)
        .collect())
}

#[tokio::test]
async fn a_fact_closed_from_that_queue_leaves_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 文档出了新版，旧分块被取代——证据只剩下停在旧版的那一条，这一档该列它
        sqlx::query("UPDATE chunks SET superseded_at = now() WHERE id = $1")
            .bind(f.chunk)
            .execute(&pool)
            .await?;
        assert_eq!(stale_ids(&pool, f.kb).await?, vec![f.fact]);
        assert_eq!(
            utopia_store::review::counts(&pool, f.kb).await?.unconfirmed,
            1
        );

        // 人裁决「它在 2021 年 1 月结束了」——月精度照写，服务端截到那个月的 1 日
        let corrected = utopia_store::temporal::close_superseded(
            &pool,
            f.fact,
            t("2021-01-01T00:00:00Z"),
            "month",
        )
        .await?
        .expect("闭合要改写出修正行");

        // **修正行带着同一批证据，但它已经有终点了，不再是「现行」。**
        // 少了这一条，它立刻回到列表里——用户看到的正是「点了闭合，条目还在」
        assert!(
            stale_ids(&pool, f.kb).await?.is_empty(),
            "闭合过的行要离开这一档"
        );
        assert_eq!(
            utopia_store::review::counts(&pool, f.kb).await?.unconfirmed,
            0,
            "左栏的计数与列表同一套 WHERE"
        );

        // 修正行自己：证据复制过来了，终点是写明的——判据靠的是上界那一句，不是证据没了
        let copied: i64 =
            sqlx::query_scalar("SELECT count(*) FROM fact_evidence WHERE fact_id = $1")
                .bind(corrected)
                .fetch_one(&pool)
                .await?;
        assert_eq!(copied, 1, "证据随修正行复制");
        let end: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT valid_to FROM facts WHERE id = $1")
                .bind(corrected)
                .fetch_one(&pool)
                .await?;
        assert!(end.is_some(), "修正行的终点是写明的");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}

/// 反面：「结束了，不知哪天」的行也不算现行（0022）。
/// 只看 `valid_to IS NULL` 会把这条留在队列里——它已经不成立了，问它没有意义。
#[tokio::test]
async fn an_ended_unknown_row_is_not_current_either() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        sqlx::query("UPDATE chunks SET superseded_at = now() WHERE id = $1")
            .bind(f.chunk)
            .execute(&pool)
            .await?;
        assert_eq!(stale_ids(&pool, f.kb).await?.len(), 1);

        // 锚点必须一起给：0022 的 CHECK 把「结束不知哪天」和说出结束的那份文档绑在一起
        sqlx::query(
            "UPDATE facts SET valid_to_precision = 'unknown', attested_to = now() WHERE id = $1",
        )
        .bind(f.fact)
        .execute(&pool)
        .await?;
        assert!(
            stale_ids(&pool, f.kb).await?.is_empty(),
            "结束不知哪天的行不是现行"
        );
        assert_eq!(
            utopia_store::review::counts(&pool, f.kb).await?.unconfirmed,
            0
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}
