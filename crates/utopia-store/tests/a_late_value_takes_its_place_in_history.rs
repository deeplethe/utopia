//! 单值属性的新值乱序到达，年表仍然一段接一段。
//!
//! Blackbaud 总部租约的二期行权截止日在 2020 年上半年被补充协议改了四次。十几份补充协议
//! 并行抽取，谁先落库全看分块多少：第五份（2 月 18 日起）先到，第九份（6 月 8 日起）
//! 第二个到，第七份、第六份最后才到。时态引擎从前只拿新值去比「仍在成立」的那一条，
//! 于是晚到的两份都关在 6 月 8 日，而第五份那段 [2 月 18 日, 6 月 8 日) 没人去切——
//! 问 5 月 1 日的截止日，查出三个。
//!
//! 这里钉两件事：乱序到达最后排成正确的年表；原文自己写明的终点不被切。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use serde_json::json;
use sqlx::PgPool;
use utopia_store::graph::Validity;
use utopia_store::temporal::Uniqueness;
use uuid::Uuid;

struct Fixture {
    kb: Uuid,
    lease: Uuid,
    deadline: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (etype, deadline, lease) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'late-value-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'late-value-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'late-value-test')",
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
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, 'HQ Lease')",
    )
    .bind(lease)
    .bind(kb)
    .bind(etype)
    .execute(pool)
    .await?;
    Ok(Fixture {
        kb,
        lease,
        deadline,
    })
}

fn t(day: &str) -> chrono::DateTime<chrono::Utc> {
    format!("{day}T00:00:00Z").parse().unwrap()
}

/// 落一条值，然后照抽取的做法对账
async fn arrive(
    pool: &PgPool,
    f: &Fixture,
    value: &str,
    from: &str,
    to: Option<&str>,
) -> anyhow::Result<Uuid> {
    let mut validity = Validity::starting(Some(t(from)), Some("day"));
    if let Some(to) = to {
        validity.to = Some(t(to));
        validity.to_precision = Some("day");
    }
    let object = json!({ "value": value });
    let (id, _) = utopia_store::graph::insert_value_fact(
        pool,
        f.kb,
        f.lease,
        Some(f.deadline),
        &object,
        validity,
        0.9,
    )
    .await?;
    utopia_store::temporal::reconcile_new_fact(
        pool,
        f.kb,
        id,
        f.lease,
        f.deadline,
        None,
        Some(&object),
        Uniqueness::SubjectSide,
        validity,
        0.9,
    )
    .await?;
    Ok(id)
}

/// 现存的各段：(值, 起点, 终点)，按起点排
async fn timeline(
    pool: &PgPool,
    f: &Fixture,
) -> anyhow::Result<Vec<(String, String, Option<String>)>> {
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT object_value #>> '{value}', to_char(valid_from, 'YYYY-MM-DD'),
                to_char(valid_to, 'YYYY-MM-DD')
         FROM facts
         WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3 AND invalidated_at IS NULL
         ORDER BY valid_from",
    )
    .bind(f.kb)
    .bind(f.lease)
    .bind(f.deadline)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

fn seg(value: &str, from: &str, to: Option<&str>) -> (String, String, Option<String>) {
    (value.into(), from.into(), to.map(Into::into))
}

#[tokio::test]
async fn amendments_that_arrive_out_of_order_still_hand_over_one_to_the_next() -> anyhow::Result<()>
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 第五、第九、第七、第六份，按它们真实的落库顺序
        arrive(&pool, &f, "2020-03-17", "2020-02-18", None).await?;
        arrive(&pool, &f, "2020-06-23", "2020-06-08", None).await?;
        arrive(&pool, &f, "2020-05-26", "2020-04-14", None).await?;
        arrive(&pool, &f, "2020-04-14", "2020-03-17", None).await?;

        assert_eq!(
            timeline(&pool, &f).await?,
            vec![
                seg("2020-03-17", "2020-02-18", Some("2020-03-17")),
                seg("2020-04-14", "2020-03-17", Some("2020-04-14")),
                seg("2020-05-26", "2020-04-14", Some("2020-06-08")),
                seg("2020-06-23", "2020-06-08", None),
            ],
            "每一段都止于下一段的起点，任何一天只有一个截止日"
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

/// 原文写明「到 12 月 31 日为止」的一段，不是接替留下的边界：年中来的新值不切它。
/// 那种重叠是两份原文说法相左，不该由引擎替人挑一个
#[tokio::test]
async fn an_end_the_text_states_is_not_cut_by_a_later_value() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        arrive(&pool, &f, "2020-09-30", "2020-01-01", Some("2020-12-31")).await?;
        arrive(&pool, &f, "2020-10-31", "2020-06-01", None).await?;

        let rows = timeline(&pool, &f).await?;
        assert!(
            rows.contains(&seg("2020-09-30", "2020-01-01", Some("2020-12-31"))),
            "原文写的终点原样留着：{rows:?}"
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
