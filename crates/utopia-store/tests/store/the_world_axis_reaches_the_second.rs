//! 世界轴到秒为止（0024），打在真库上。
//!
//! 钉三样，每一样都活在 SQL 或 SQL 约束里：
//! - 分精度的事实存得进去，存的值截到整分——写入路径截一遍，数据库的 CHECK 再守一遍
//! - 值与精度不一致的行（day 精度却带着 14 点）被 CHECK 挡在门外：三值逻辑不会放行
//! - 派生行的精度跟着**赢下这一端的那条前提**走，不再取所有前提里最粗的——year 标在
//!   一个 6 月 15 日的值上，正是这条不变量要消灭的矛盾
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::graph::Validity;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

struct Fixture {
    org: Uuid,
    kb: Uuid,
    part_of: Uuid,
    a: Uuid,
    b: Uuid,
    c: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (ty, part_of) = (Uuid::now_v7(), Uuid::now_v7());
    let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'second-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'second-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'second-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'org', 'Org')")
        .bind(ty)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, is_transitive)
         VALUES ($1, $2, 'part_of', 'part of', TRUE)",
    )
    .bind(part_of)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [(a, "Team A"), (b, "Group B"), (c, "Division C")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(ty)
        .bind(name)
        .execute(pool)
        .await?;
    }
    Ok(Fixture {
        org,
        kb,
        part_of,
        a,
        b,
        c,
    })
}

type Bounds = (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
);

async fn bounds(pool: &PgPool, table: &str, id: Uuid) -> anyhow::Result<Bounds> {
    Ok(sqlx::query_as(&format!(
        "SELECT valid_from, valid_from_precision, valid_to, valid_to_precision FROM {table} WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn a_stored_value_is_truncated_to_its_precision() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 记忆日志那一行：14:32 打的戳，精度到分。带着毫秒进来，存的是整分
        let (id, _) = utopia_store::graph::insert_fact(
            &pool,
            f.kb,
            f.a,
            Some(f.part_of),
            f.b,
            Validity::starting(Some(t("2026-06-01T14:32:07.382Z")), Some("minute"))
                .attested(Some(t("2026-06-01T14:32:07.382Z"))),
            0.9,
        )
        .await?;
        let (from, fp, _, _) = bounds(&pool, "facts", id).await?;
        assert_eq!(from, Some(t("2026-06-01T14:32:00Z")), "截到整分");
        assert_eq!(fp.as_deref(), Some("minute"));

        // 人把它闭合在某个小时：闭合点截到整点
        let closed =
            utopia_store::temporal::close_superseded(&pool, id, t("2026-06-01T18:45:00Z"), "hour")
                .await?
                .expect("开放行关得上");
        let (_, _, to, tp) = bounds(&pool, "facts", closed).await?;
        assert_eq!(to, Some(t("2026-06-01T18:00:00Z")), "小时精度就是整点");
        assert_eq!(tp.as_deref(), Some("hour"));

        // 绕过写入路径直接塞一行值与精度不一致的：CHECK 挡住。**这条要真跑**——
        // 三值逻辑里 NULL 会让 CHECK 静默放行，只有插一次才知道它没放
        let bad = sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                valid_from, valid_from_precision)
             VALUES ($1, $2, $3, $4, $5, '2026-06-01T14:00:00Z', 'day')",
        )
        .bind(Uuid::now_v7())
        .bind(f.kb)
        .bind(f.b)
        .bind(f.part_of)
        .bind(f.c)
        .execute(&pool)
        .await;
        let err = bad
            .expect_err("day 精度带着 14 点，不该存得进去")
            .to_string();
        assert!(err.contains("facts_from_precision_matches_date"), "{err}");
        // 精度不在梯子上的也挡
        let bad = sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                valid_from, valid_from_precision)
             VALUES ($1, $2, $3, $4, $5, '2026-06-01T14:00:00.500Z', 'millisecond')",
        )
        .bind(Uuid::now_v7())
        .bind(f.kb)
        .bind(f.b)
        .bind(f.part_of)
        .bind(f.c)
        .execute(&pool)
        .await;
        assert!(bad.is_err(), "账本到秒为止");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 派生的起点是前提里最晚的那个起点；精度就是那条前提的精度，不是所有前提里最粗的
#[tokio::test]
async fn a_derived_bound_carries_the_precision_of_the_premise_that_set_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // a ⊂ b 从 2020 年起（year）；b ⊂ c 从 2023-06-15 起（day）
        utopia_store::graph::insert_fact(
            &pool,
            f.kb,
            f.a,
            Some(f.part_of),
            f.b,
            Validity::starting(Some(t("2020-01-01T00:00:00Z")), Some("year"))
                .attested(Some(t("2020-01-01T00:00:00Z"))),
            0.9,
        )
        .await?;
        utopia_store::graph::insert_fact(
            &pool,
            f.kb,
            f.b,
            Some(f.part_of),
            f.c,
            Validity::starting(Some(t("2023-06-15T00:00:00Z")), Some("day"))
                .attested(Some(t("2023-06-15T00:00:00Z"))),
            0.9,
        )
        .await?;
        utopia_store::reasoning::materialize(&pool, f.kb).await?;

        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM derived_facts
              WHERE kb_id = $1 AND subject_id = $2 AND object_id = $3 AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .bind(f.a)
        .bind(f.c)
        .fetch_optional(&pool)
        .await?;
        let (id,) = row.expect("a ⊂ c 推得出");
        let (from, fp, _, _) = bounds(&pool, "derived_facts", id).await?;
        assert_eq!(
            from,
            Some(t("2023-06-15T00:00:00Z")),
            "起点是最晚的那条前提的起点"
        );
        assert_eq!(
            fp.as_deref(),
            Some("day"),
            "精度跟赢下这一端的前提走——从前取最粗会把 year 标在 6 月 15 日上"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
