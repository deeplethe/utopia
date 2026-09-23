//! 蕴含规则（0044 决定 3 第五片）：提案落库、人批与后续任务同事务、待读的字、缓存、
//! 物化算隐含行、驳回退掉。没有 `UTOPIA_DATABASE_URL` 时跳过。
use super::*;
use crate::{materialize, phrase_bindings};
use serde_json::json;
use sqlx::PgPool;

struct Fx {
    org: Uuid,
    kb: Uuid,
    film: Uuid,
    place: Uuid,
    country_of_origin: Uuid,
    located_in: Uuid,
    country: Uuid,
    loud_tour: Uuid,
    piedmont: Uuid,
    statement: Uuid,
}

/// 一个库：类 film / place，属性 country_of_origin / located_in / country；
/// 「Loud Tour」是 british film；一条陈述 Loud Tour —located in→ Piedmont（place）
async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let ids: Vec<Uuid> = (0..11).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, film, place, coo, li, country, loud, piedmont, stmt) = (
        ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6], ids[7], ids[8], ids[9], ids[10],
    );
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','implication-test');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','implication-test');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','implication-test');
         INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES
             ('{film}','{kb}','film','Film','#000','circle'), ('{place}','{kb}','place','Place','#000','circle');
         INSERT INTO relation_types(id,kb_id,key,label,kind,temporal) VALUES
             ('{coo}','{kb}','country_of_origin','country of origin','relation','state'),
             ('{li}','{kb}','located_in','located in','relation','state'),
             ('{country}','{kb}','country','country','relation','state');
         INSERT INTO entities(id,kb_id,canonical_name,type_id,specific_type) VALUES
             ('{loud}','{kb}','Loud Tour','{film}','British film'),
             ('{piedmont}','{kb}','Piedmont','{place}','region');
         INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES
             ('{stmt}','{kb}','{loud}','{piedmont}','open','located in');"
    ))
    .execute(pool)
    .await?;
    Ok(Fx {
        org,
        kb,
        film,
        place,
        country_of_origin: coo,
        located_in: li,
        country,
        loud_tour: loud,
        piedmont,
        statement: stmt,
    })
}

async fn cleanup(pool: &PgPool, f: &Fx) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(f.kb.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

fn phrase_rule<'a>(f: &Fx, reading: Option<&'a str>, property: Uuid) -> Proposal<'a> {
    Proposal {
        trigger: "phrase",
        phrase: "located in",
        subject_type_id: Some(f.film),
        object_type_id: Some(f.place),
        object_is_value: false,
        conclude_property_id: property,
        reading,
        status: "proposed",
        votes: &serde_json::Value::Null,
        basis: "b1",
        statement_count: 1,
        examples: &[],
    }
}

async fn implied_rows(pool: &PgPool, kb: Uuid) -> anyhow::Result<Vec<(Uuid, Uuid, Option<Uuid>)>> {
    Ok(sqlx::query_as(
        "SELECT subject_id, predicate_id, object_id FROM facts
          WHERE kb_id=$1 AND layer='typed' AND implied AND invalidated_at IS NULL ORDER BY recorded_at",
    )
    .bind(kb)
    .fetch_all(pool)
    .await?)
}

#[tokio::test]
async fn a_proposal_lands_once_and_a_person_decides_it_with_its_job() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        let p = phrase_rule(&f, Some("country_of_place"), f.country);
        let id = propose(&pool, f.kb, &p).await?.expect("first proposal");
        // 同一条再提：代理的行被覆盖（返回 id），不是第二行
        assert_eq!(propose(&pool, f.kb, &p).await?, Some(id));
        assert_eq!(list(&pool, f.kb, Some("proposed")).await?.len(), 1);
        // 人批：状态 approved，同事务排了 read_phrases（要读数）
        let job = decide_with_delivery(&pool, f.kb, id, true, &json!({}))
            .await?
            .expect("decided");
        let (kind, status): (String, String) =
            sqlx::query_as("SELECT kind, status FROM jobs WHERE id=$1")
                .bind(job)
                .fetch_one(&pool)
                .await?;
        assert_eq!((kind.as_str(), status.as_str()), (READ_KIND, "queued"));
        let r = get(&pool, f.kb, id).await?.unwrap();
        assert_eq!(
            (r.status.as_str(), r.decided_by.as_str()),
            ("approved", "person")
        );
        // 人判过的，代理再提也盖不掉
        assert_eq!(propose(&pool, f.kb, &p).await?, None);
        // 不要读数的规则，批了直接排物化
        let plain = propose(&pool, f.kb, &phrase_rule(&f, None, f.country_of_origin))
            .await?
            .unwrap();
        let job2 = decide_with_delivery(&pool, f.kb, plain, true, &json!({}))
            .await?
            .unwrap();
        let kind2: String = sqlx::query_scalar("SELECT kind FROM jobs WHERE id=$1")
            .bind(job2)
            .fetch_one(&pool)
            .await?;
        assert_eq!(kind2, phrase_bindings::MATERIALIZE_KIND);
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn an_approved_rule_waits_for_its_reading_then_implies_a_fact_with_evidence(
) -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        let id = propose(
            &pool,
            f.kb,
            &phrase_rule(&f, Some("country_of_place"), f.country),
        )
        .await?
        .unwrap();
        decide_with_delivery(&pool, f.kb, id, true, &json!({})).await?;
        // 待读：陈述的宾语「piedmont」按 country_of_place
        let pending = pending_readings(&pool, f.kb).await?;
        assert_eq!(
            pending,
            vec![PendingReading {
                reading: "country_of_place".into(),
                phrase: "piedmont".into()
            }]
        );
        // 缓存没填：物化不算隐含行
        let o = materialize::materialize(&pool, f.kb).await?;
        assert_eq!(o.implied, 0);
        assert!(implied_rows(&pool, f.kb).await?.is_empty());
        // 读数读出「United States」：库里没有就建，名字事实照记
        let us = resolve_or_create_named(&pool, f.kb, "United States").await?;
        assert_eq!(
            resolve_or_create_named(&pool, f.kb, "united states").await?,
            us,
            "found by name the second time"
        );
        record_reading(&pool, f.kb, "country_of_place", "Piedmont", Some(us), None).await?;
        assert!(
            pending_readings(&pool, f.kb).await?.is_empty(),
            "cached now"
        );
        let o = materialize::materialize(&pool, f.kb).await?;
        assert_eq!(o.implied, 1);
        let rows = implied_rows(&pool, f.kb).await?;
        assert_eq!(rows, vec![(f.loud_tour, f.country, Some(us))]);
        // 证据从触发它的陈述抄来；来源记着规则与陈述
        let src: (Uuid, Option<Uuid>) =
            sqlx::query_as("SELECT rule_id, statement_id FROM implied_fact_sources")
                .fetch_one(&pool)
                .await?;
        assert_eq!(src, (id, Some(f.statement)));
        // 再跑一遍什么都不动
        let o = materialize::materialize(&pool, f.kb).await?;
        assert_eq!((o.implied, o.retired), (0, 0));
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn rejecting_the_rule_or_losing_the_statement_retires_the_implied_row() -> anyhow::Result<()>
{
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        // 不要读数的规则：located in 的陈述还蕴含 country_of_origin = 陈述的宾语（就当测试）
        let id = propose(&pool, f.kb, &phrase_rule(&f, None, f.country_of_origin))
            .await?
            .unwrap();
        decide_with_delivery(&pool, f.kb, id, true, &json!({})).await?;
        assert_eq!(materialize::materialize(&pool, f.kb).await?.implied, 1);
        assert_eq!(implied_rows(&pool, f.kb).await?.len(), 1);
        // 驳回：来源删、行作废
        decide_with_delivery(&pool, f.kb, id, false, &json!({})).await?;
        let o = materialize::materialize(&pool, f.kb).await?;
        assert_eq!(o.retired, 1);
        assert!(implied_rows(&pool, f.kb).await?.is_empty());
        // 再批回来，行回来；陈述作废，行再退
        decide_with_delivery(&pool, f.kb, id, true, &json!({})).await?;
        assert_eq!(materialize::materialize(&pool, f.kb).await?.implied, 1);
        sqlx::query("UPDATE facts SET invalidated_at=now() WHERE id=$1")
            .bind(f.statement)
            .execute(&pool)
            .await?;
        assert_eq!(materialize::materialize(&pool, f.kb).await?.retired, 1);
        assert!(implied_rows(&pool, f.kb).await?.is_empty());
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn a_kind_word_rule_implies_a_fact_for_every_thing_so_called() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        let votes = json!({});
        let id = propose(
            &pool,
            f.kb,
            &Proposal {
                trigger: "kind_word",
                phrase: "British  Film",
                subject_type_id: None,
                object_type_id: None,
                object_is_value: false,
                conclude_property_id: f.country_of_origin,
                reading: Some("country_of_nationality"),
                status: "proposed",
                votes: &votes,
                basis: "k1",
                statement_count: 1,
                examples: &[],
            },
        )
        .await?
        .unwrap();
        decide_with_delivery(&pool, f.kb, id, true, &votes).await?;
        assert_eq!(
            pending_readings(&pool, f.kb).await?,
            vec![PendingReading {
                reading: "country_of_nationality".into(),
                phrase: "british film".into()
            }]
        );
        let uk = resolve_or_create_named(&pool, f.kb, "United Kingdom").await?;
        record_reading(
            &pool,
            f.kb,
            "country_of_nationality",
            "british film",
            Some(uk),
            None,
        )
        .await?;
        assert_eq!(materialize::materialize(&pool, f.kb).await?.implied, 1);
        assert_eq!(
            implied_rows(&pool, f.kb).await?,
            vec![(f.loud_tour, f.country_of_origin, Some(uk))]
        );
        // 读不出来也缓存：不再待读，也不算行
        record_reading(&pool, f.kb, "country_of_nationality", "iberian", None, None).await?;
        assert!(pending_readings(&pool, f.kb).await?.is_empty());
        let _ = f.piedmont;
        let _ = f.located_in;
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}
