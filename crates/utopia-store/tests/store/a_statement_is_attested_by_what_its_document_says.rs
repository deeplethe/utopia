//! 陈述的见证来自文档自己说的日期，没有就是没有（0064 决定 3、5）。
//!
//! - 抽取写下的开放陈述不再拿处理文档的那一刻当见证：文档没说就留空；
//! - 留空的行在世界轴上任何时点都成立（下界开放）；没日期的事件除外，它照旧任何时点都不成立；
//! - `attest_statement` 写上见证和那条日期的名字，只往早挪；
//! - 物化出来的类型化行跟着它的来源陈述走；
//! - 规则算出来的行也跟着它读的陈述走，类别词规则从实体算的行没有见证；人写的行只被规则的
//!   结论并进来时，见证还是人写下它的那一刻。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, TimeZone, Utc};
use sqlx::PgPool;
use utopia_store::graph::{self, FactObject, Validity};
use uuid::Uuid;

const ORG: &str = "statement-attested-by-document-test";
const RULE_ORG: &str = "rule-row-attested-by-its-statement-test";

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

/// 这一行在 T 时刻成立吗，按读路径自己的谓词
async fn holds_at(pool: &PgPool, fact: Uuid, at: DateTime<Utc>) -> anyhow::Result<bool> {
    let sql = format!(
        "SELECT {} FROM facts f WHERE f.id = $1",
        utopia_store::world_axis::facts_hold_at("f", 2)
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(fact)
        .bind(at)
        .fetch_one(pool)
        .await?)
}

#[tokio::test]
async fn an_unattested_statement_holds_at_every_moment_and_an_attested_one_from_its_date(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    let (org, ws, kb, watch, market, launch) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','{ORG}');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','{ORG}');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','{ORG}');
         INSERT INTO entities(id,kb_id,canonical_name) VALUES
             ('{watch}','{kb}','码表'), ('{market}','{kb}','全球');
         INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description) VALUES
             ('{launch}','{kb}','launched_in','launched in','relation','event','');"
    ))
    .execute(&pool)
    .await?;

    let run = async {
        let long_ago = Utc.with_ymd_and_hms(2015, 1, 1, 0, 0, 0).unwrap();
        let report_day = Utc.with_ymd_and_hms(2026, 9, 4, 0, 0, 0).unwrap();
        let earlier_report = Utc.with_ymd_and_hms(2026, 8, 28, 0, 0, 0).unwrap();

        // 文档没说自己是哪天的：没有见证，任何时点都成立
        let (statement, _) = graph::insert_open_statement(
            &pool,
            kb,
            watch,
            "所属市场为",
            FactObject::Entity(market),
            None,
            0.9,
        )
        .await?;
        assert_eq!(anchor(&pool, statement).await?, (None, None));
        assert!(holds_at(&pool, statement, long_ago).await?, "no time at all: holds at every moment");
        assert!(holds_at(&pool, statement, Utc::now()).await?);

        // 它所在那一节的日期作证：从那天起读得到，之前读不到（0022 的下界）
        assert!(graph::attest_statement(&pool, statement, report_day, "提报日期 2026年9月4日").await?);
        assert_eq!(
            anchor(&pool, statement).await?,
            (Some(report_day), Some("提报日期 2026年9月4日".to_string()))
        );
        assert!(!holds_at(&pool, statement, long_ago).await?);
        assert!(holds_at(&pool, statement, report_day).await?);
        // 更早的文档也说过：见证往早挪，名字跟着；更晚的不动它
        assert!(graph::attest_statement(&pool, statement, earlier_report, "提报日期 2026年8月28日").await?);
        assert!(!graph::attest_statement(&pool, statement, report_day, "提报日期 2026年9月4日").await?);
        assert_eq!(
            anchor(&pool, statement).await?,
            (Some(earlier_report), Some("提报日期 2026年8月28日".to_string()))
        );

        // 物化出来的类型化行跟着来源陈述走；来源没有见证的，它也没有
        let (typed, bare_typed) = (Uuid::now_v7(), Uuid::now_v7());
        let (bare, _) = graph::insert_open_statement(
            &pool,
            kb,
            watch,
            "上市于",
            FactObject::Entity(market),
            None,
            0.9,
        )
        .await?;
        for (id, source) in [(typed, statement), (bare_typed, bare)] {
            sqlx::query(
                "INSERT INTO facts (id, kb_id, subject_id, object_id, predicate_id, layer, from_statement_id)
                 VALUES ($1, $2, $3, $4, $5, 'typed', $6)",
            )
            .bind(id)
            .bind(kb)
            .bind(watch)
            .bind(market)
            .bind(launch)
            .bind(source)
            .execute(&pool)
            .await?;
            sqlx::query("INSERT INTO typed_fact_sources (fact_id, statement_id) VALUES ($1, $2)")
                .bind(id)
                .bind(source)
                .execute(&pool)
                .await?;
        }
        utopia_store::materialize::sync_typed_attestation(&pool, kb).await?;
        assert_eq!(
            anchor(&pool, typed).await?,
            (Some(earlier_report), Some("提报日期 2026年8月28日".to_string()))
        );
        assert_eq!(anchor(&pool, bare_typed).await?, (None, None));
        // 没日期的事件任何时点都不算成立（0022），没有见证也一样
        assert!(!holds_at(&pool, bare_typed, long_ago).await?);
        assert!(!holds_at(&pool, bare_typed, Utc::now()).await?);
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    run
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

/// 规则算出来的行（0073 的隐含行）跟着它读的陈述走：陈述没说时间，它就没有见证、任何时点
/// 都成立；陈述后来作了证，它跟到那天。类别词规则从实体算的行没有陈述可跟，也没有见证。
/// 人写的行被规则的结论并进来，见证还是人写下它的那一刻
#[tokio::test]
async fn a_row_a_rule_implies_follows_the_statement_it_read() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(RULE_ORG)
        .execute(&pool)
        .await?;
    let ids: Vec<Uuid> = (0..14).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, person, company, works_for, origin) =
        (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6]);
    let (lin, aster, meridian, film, uk, signed_rule, kind_rule) =
        (ids[7], ids[8], ids[9], ids[10], ids[11], ids[12], ids[13]);
    // 两条批准了的规则：「signed with」（人 → 机构）蕴含 works_for；类别词「british film」
    // 蕴含 country_of_origin，宾语由读数缓存给出
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','{RULE_ORG}');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','{RULE_ORG}');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','{RULE_ORG}');
         INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES
             ('{person}','{kb}','person','person','#000','circle'),
             ('{company}','{kb}','organization','organization','#000','circle');
         INSERT INTO relation_types(id,kb_id,key,label,kind,temporal) VALUES
             ('{works_for}','{kb}','works_for','works for','relation','state'),
             ('{origin}','{kb}','country_of_origin','country of origin','relation','state');
         INSERT INTO entities(id,kb_id,canonical_name,type_id,specific_type) VALUES
             ('{lin}','{kb}','Lin Zhao','{person}',NULL),
             ('{aster}','{kb}','Aster Labs','{company}',NULL),
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
        let long_ago = Utc.with_ymd_and_hms(2015, 1, 1, 0, 0, 0).unwrap();
        let signed_day = Utc.with_ymd_and_hms(2026, 9, 4, 0, 0, 0).unwrap();
        let written_day = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();

        // 一条没说时间的陈述：规则从它算出的行没有见证，任何时点都成立——与同一条陈述物化
        // 出来的行一样，不是算出它的那一刻。类别词算出的行也是
        let (signed, _) = graph::insert_open_statement(
            &pool,
            kb,
            lin,
            "signed with",
            FactObject::Entity(aster),
            None,
            0.9,
        )
        .await?;
        let outcome = utopia_store::materialize::materialize(&pool, kb).await?;
        assert_eq!(outcome.implied, 2, "{outcome:?}");
        let by_rule = typed_row(&pool, kb, lin, aster).await?;
        let by_kind = typed_row(&pool, kb, film, uk).await?;
        assert_eq!(anchor(&pool, by_rule).await?, (None, None));
        assert_eq!(anchor(&pool, by_kind).await?, (None, None));
        for row in [by_rule, by_kind] {
            assert!(
                holds_at(&pool, row, long_ago).await?,
                "no time at all: holds at every moment"
            );
            assert!(holds_at(&pool, row, Utc::now()).await?);
        }

        // 陈述后来由它所在那一节的日期作证，时间解析那条路：先给陈述作证，再同步类型化的行。
        // 规则算出的行跟到那天，连同那条日期的名字；类别词那行没有陈述，照旧没有
        assert!(graph::attest_statement(&pool, signed, signed_day, "签约日 2026年9月4日").await?);
        utopia_store::materialize::sync_typed_attestation(&pool, kb).await?;
        let followed = (Some(signed_day), Some("签约日 2026年9月4日".to_string()));
        assert_eq!(anchor(&pool, by_rule).await?, followed);
        assert!(!holds_at(&pool, by_rule, long_ago).await?);
        assert!(holds_at(&pool, by_rule, signed_day).await?);
        assert_eq!(anchor(&pool, by_kind).await?, (None, None));
        // 再物化一轮，不动它们
        utopia_store::materialize::materialize(&pool, kb).await?;
        assert_eq!(anchor(&pool, by_rule).await?, followed);
        assert_eq!(anchor(&pool, by_kind).await?, (None, None));

        // 人写的行由人写下它的那一刻作证。规则从一条没说时间的陈述算出同一件事、并进这一行：
        // 它有了规则的来源，见证不变
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
        graph::insert_open_statement(
            &pool,
            kb,
            lin,
            "signed with",
            FactObject::Entity(meridian),
            None,
            0.9,
        )
        .await?;
        let outcome = utopia_store::materialize::materialize(&pool, kb).await?;
        assert_eq!(
            outcome.implied, 0,
            "merged into the person's row: {outcome:?}"
        );
        let rule_sources: i64 =
            sqlx::query_scalar("SELECT count(*) FROM implied_fact_sources WHERE fact_id = $1")
                .bind(by_hand)
                .fetch_one(&pool)
                .await?;
        assert_eq!(rule_sources, 1);
        assert_eq!(
            anchor(&pool, by_hand).await?,
            (Some(written_day), None),
            "a person's row keeps the moment it was written"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(RULE_ORG)
        .execute(&pool)
        .await?;
    run
}
