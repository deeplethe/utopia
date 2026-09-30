//! 迁移 0101：这之前规则算出的行，见证跟着它读的陈述走。
//!
//! 照这之前的来路造：物化照常算出隐含行，再把它们的见证改回那时写下的样子——处理那一刻，
//! 没有日期的名字。那时规则算的行入库时陈述没有见证就填此刻，陈述后来作了证也不跟。
//!
//! - 没说时间的陈述算出的行：留空，任何时点都成立
//! - 陈述在物化之后才作了证的：跟到那天，连同那条日期的名字
//! - 类别词从实体算出的行：没有陈述可跟，留空
//! - 人写的行只被规则的结论并进来：见证还是人写下它的那一刻
//!
//! 再跑一遍迁移，一样不动。迁移的语句不按 kb 过滤，改的是全库的行，所以这个测试在 tests/
//! 顶层自成一个二进制（与 0096、0099 同理）。没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。
//! 自建自拆，绝不碰已有的库。

use chrono::{DateTime, TimeZone, Utc};
use sqlx::PgPool;
use utopia_store::graph::{self, FactObject, Validity};
use uuid::Uuid;

const MIGRATION: &str =
    include_str!("../../../migrations/0101_a_row_a_rule_implies_follows_its_statements.sql");

async fn anchor(
    pool: &PgPool,
    fact: Uuid,
) -> anyhow::Result<(Option<DateTime<Utc>>, Option<String>)> {
    Ok(
        sqlx::query_as("SELECT attested_from, attested_by FROM facts WHERE id = $1")
            .bind(fact)
            .fetch_one(pool)
            .await?,
    )
}

/// 主语到宾语之间活着的那一行类型化事实
async fn typed_row(pool: &PgPool, kb: Uuid, subject: Uuid, object: Uuid) -> anyhow::Result<Uuid> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM facts
          WHERE kb_id = $1 AND subject_id = $2 AND object_id = $3
            AND layer = 'typed' AND invalidated_at IS NULL",
    )
    .bind(kb)
    .bind(subject)
    .bind(object)
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn a_row_a_rule_implied_before_follows_its_statements() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let ids: Vec<Uuid> = (0..15).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, person, company, works_for, origin) =
        (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6]);
    let (lin, aster, brightway, meridian, film, uk) =
        (ids[7], ids[8], ids[9], ids[10], ids[11], ids[12]);
    let (signed_rule, kind_rule) = (ids[13], ids[14]);
    // Only locally generated UUIDs are interpolated into fixture SQL.
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','rule-rows-before-0101');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','rule-rows-before-0101');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','rule-rows-before-0101');
         INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES
             ('{person}','{kb}','person','person','#000','circle'),
             ('{company}','{kb}','organization','organization','#000','circle');
         INSERT INTO relation_types(id,kb_id,key,label,kind,temporal) VALUES
             ('{works_for}','{kb}','works_for','works for','relation','state'),
             ('{origin}','{kb}','country_of_origin','country of origin','relation','state');
         INSERT INTO entities(id,kb_id,canonical_name,type_id,specific_type) VALUES
             ('{lin}','{kb}','Lin Zhao','{person}',NULL),
             ('{aster}','{kb}','Aster Labs','{company}',NULL),
             ('{brightway}','{kb}','Brightway','{company}',NULL),
             ('{meridian}','{kb}','Meridian Systems','{company}',NULL),
             ('{film}','{kb}','Loud Tour',NULL,'British film'),
             ('{uk}','{kb}','United Kingdom',NULL,NULL);
         INSERT INTO implication_rules(id,kb_id,trigger,phrase,subject_type_id,object_type_id,
                                       conclude_property_id,reading,status,decided_by) VALUES
             ('{signed_rule}','{kb}','phrase','signed with','{person}','{company}','{works_for}',
              NULL,'approved','person'),
             ('{kind_rule}','{kb}','kind_word','british film',NULL,NULL,'{origin}',
              'country_of_nationality','approved','person');
         INSERT INTO phrase_readings(kb_id,reading,phrase,entity_id) VALUES
             ('{kb}','country_of_nationality','british film','{uk}');"
    ))
    .execute(&pool)
    .await?;

    let run = async {
        let processed = Utc.with_ymd_and_hms(2026, 9, 30, 3, 0, 0).unwrap();
        let signed_day = Utc.with_ymd_and_hms(2026, 9, 4, 0, 0, 0).unwrap();
        let written_day = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
        let signed = |object: Uuid| {
            let pool = pool.clone();
            async move {
                anyhow::Ok(
                    graph::insert_open_statement(
                        &pool,
                        kb,
                        lin,
                        "signed with",
                        FactObject::Entity(object),
                        None,
                        0.9,
                    )
                    .await?
                    .0,
                )
            }
        };
        signed(aster).await?;
        let later = signed(brightway).await?;
        let (by_hand, _) = graph::insert_fact(
            &pool,
            kb,
            lin,
            Some(works_for),
            meridian,
            Validity {
                attested_at: Some(written_day),
                ..Validity::default()
            },
            0.9,
        )
        .await?;
        signed(meridian).await?;
        utopia_store::materialize::materialize(&pool, kb).await?;
        // 物化之后陈述才作证：只动陈述，那时算出的行不跟
        assert!(graph::attest_statement(&pool, later, signed_day, "签约日 2026年9月4日").await?);
        // 这之前写下的样子：规则算的行都是处理那一刻，没有日期的名字
        sqlx::query(
            "UPDATE facts SET attested_from = $2, attested_by = NULL WHERE kb_id = $1 AND implied",
        )
        .bind(kb)
        .bind(processed)
        .execute(&pool)
        .await?;
        let undated = typed_row(&pool, kb, lin, aster).await?;
        let dated_later = typed_row(&pool, kb, lin, brightway).await?;
        let by_kind = typed_row(&pool, kb, film, uk).await?;
        assert_eq!(typed_row(&pool, kb, lin, meridian).await?, by_hand);
        for row in [undated, dated_later, by_kind] {
            assert_eq!(anchor(&pool, row).await?, (Some(processed), None));
        }

        // 跑两遍：第二遍一样不动
        for _ in 0..2 {
            sqlx::raw_sql(MIGRATION).execute(&pool).await?;
            assert_eq!(
                anchor(&pool, undated).await?,
                (None, None),
                "an undated statement"
            );
            assert_eq!(
                anchor(&pool, dated_later).await?,
                (Some(signed_day), Some("签约日 2026年9月4日".to_string())),
                "the statement attested after the row was computed"
            );
            assert_eq!(anchor(&pool, by_kind).await?, (None, None), "a kind word");
            assert_eq!(
                anchor(&pool, by_hand).await?,
                (Some(written_day), None),
                "a person's row a rule merged into"
            );
        }
        anyhow::Ok(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
