//! 迁移 0096：#967 之前改写出来的行找回它由哪些陈述算出，之后物化重算出的重复行作废。
//!
//! 坏的状态照它当时的来路造出来。一条陈述「Zhang San —leads→ Aurora，自 2023-01-10」
//! 绑到 leads，一条已批准的短语规则从同一句蕴含 works_on。物化之后得到类型化行与隐含行。
//! 两行都经 `correct_interval` 把起点改成 2023-02-01，再把修正行的链接抹掉——那正是
//! #967 之前人改区间写出来的样子。再物化一次，旧代码的后果照样出现：陈述没有活着的行
//! 代表，照原区间又算出两行（D 与 D2）。
//!
//! 跑 0096 之后：两条修正行找回陈述与规则来源，D 与 D2 作废而仍在表里；人手写又改过的行、
//! #967 之后带着链接改写的行都不动；人点头的事实给一条裸行补上起点（时间精化，另一次观察）
//! 也不动——它链着一条物化行，却不是改写；再物化什么也不做，2023-01-20 那一刻没有人在管；
//! 再跑一遍迁移，一样不动。
//!
//! 迁移的语句不按 kb 过滤，改的是全库的行，所以这个测试在 tests/ 顶层自成一个二进制
//! （与 0057 的回填同理，见 `tests/store/main.rs`）。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_store::graph::{self, Validity};
use utopia_store::materialize::{materialize, Outcome};
use uuid::Uuid;

const MIGRATION: &str =
    include_str!("../../../migrations/0096_a_rewritten_row_gets_its_statements_back.sql");

fn t(s: &str) -> DateTime<Utc> {
    s.parse().expect("fixed timestamp")
}

/// 一行的链接：from_statement_id、implied、两张来源表里的陈述
async fn links(
    pool: &PgPool,
    fact: Uuid,
) -> anyhow::Result<(Option<Uuid>, bool, Vec<Uuid>, Vec<Uuid>)> {
    let (from, implied): (Option<Uuid>, bool) =
        sqlx::query_as("SELECT from_statement_id, implied FROM facts WHERE id = $1")
            .bind(fact)
            .fetch_one(pool)
            .await?;
    let typed: Vec<Uuid> = sqlx::query_scalar(
        "SELECT statement_id FROM typed_fact_sources WHERE fact_id = $1 ORDER BY statement_id",
    )
    .bind(fact)
    .fetch_all(pool)
    .await?;
    let by_rule: Vec<Uuid> = sqlx::query_scalar(
        "SELECT statement_id FROM implied_fact_sources WHERE fact_id = $1 ORDER BY statement_id",
    )
    .bind(fact)
    .fetch_all(pool)
    .await?;
    Ok((from, implied, typed, by_rule))
}

async fn live(pool: &PgPool, fact: Uuid) -> anyhow::Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT invalidated_at IS NULL FROM facts WHERE id = $1")
            .bind(fact)
            .fetch_one(pool)
            .await?,
    )
}

/// 修正行的链接抹掉：#967 之前的 `correct_interval` 写出来就是这样
async fn as_before_967(pool: &PgPool, fact: Uuid) -> anyhow::Result<()> {
    sqlx::query("UPDATE facts SET from_statement_id = NULL, implied = FALSE WHERE id = $1")
        .bind(fact)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM typed_fact_sources WHERE fact_id = $1")
        .bind(fact)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM implied_fact_sources WHERE fact_id = $1")
        .bind(fact)
        .execute(pool)
        .await?;
    Ok(())
}

async fn correct_start(pool: &PgPool, fact: Uuid, from: &str) -> anyhow::Result<Uuid> {
    Ok(utopia_store::temporal::correct_interval(
        pool,
        fact,
        Validity {
            from: Some(t(from)),
            from_precision: Some("day"),
            ..Default::default()
        },
    )
    .await?
    .expect("the row was live"))
}

/// 活着的某一谓词的类型化行（主语、宾语给定）
async fn rows(
    pool: &PgPool,
    kb: Uuid,
    subject: Uuid,
    predicate: Uuid,
) -> anyhow::Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM facts
          WHERE kb_id = $1 AND layer = 'typed' AND subject_id = $2 AND predicate_id = $3
            AND invalidated_at IS NULL
          ORDER BY id",
    )
    .bind(kb)
    .bind(subject)
    .bind(predicate)
    .fetch_all(pool)
    .await?)
}

#[tokio::test]
async fn a_rewritten_row_gets_its_statements_back() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'rewritten-row-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'rewritten-row-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'rewritten-row-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let run = async {
        let id = Uuid::now_v7;
        let (person, project, leads, works_on, rule) = (id(), id(), id(), id(), id());
        let (zhang, li, wang, zhou, aurora, helios, nova) =
            (id(), id(), id(), id(), id(), id(), id());
        let (charter, kickoff, bare, by_hand) = (id(), id(), id(), id());
        // 插进去的只有本地生成的 UUID 和写死的日期
        sqlx::raw_sql(&format!(
            r#"
            INSERT INTO entity_types (id, kb_id, key, label, color, shape) VALUES
                ('{person}','{kb}','person','person','#7fd0ff','circle'),
                ('{project}','{kb}','project','project','#7fd0ff','circle');
            INSERT INTO relation_types (id, kb_id, key, label, temporal) VALUES
                ('{leads}','{kb}','leads','leads','state'),
                ('{works_on}','{kb}','works_on','works on','state');
            INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES
                ('{zhang}','{kb}','{person}','Zhang San'),
                ('{li}','{kb}','{person}','Li Si'),
                ('{wang}','{kb}','{person}','Wang Wu'),
                ('{zhou}','{kb}','{person}','Zhou Qi'),
                ('{aurora}','{kb}','{project}','Aurora'),
                ('{helios}','{kb}','{project}','Helios'),
                ('{nova}','{kb}','{project}','Nova');
            -- 三条陈述：张三那条走 #967 之前的路，李四那条走 #967 之后的路，周七那条没有起点，
            -- 之后由人点头的事实补上
            INSERT INTO facts (id, kb_id, subject_id, object_id, layer, phrase, confidence,
                               valid_from, valid_from_precision, valid_from_grade) VALUES
                ('{charter}','{kb}','{zhang}','{aurora}','open','leads',0.9,'2023-01-10','day','A'),
                ('{kickoff}','{kb}','{li}','{helios}','open','leads',0.9,'2024-08-12','day','A'),
                ('{bare}','{kb}','{zhou}','{nova}','open','leads',0.9,NULL,NULL,NULL);
            INSERT INTO phrase_bindings (id, kb_id, phrase, subject_type_id, object_type_id,
                                         object_is_value, relation_type_id, direction, status) VALUES
                ('{binding}','{kb}','leads','{person}','{project}',false,'{leads}','forward','bound');
            INSERT INTO implication_rules (id, kb_id, trigger, phrase, subject_type_id, object_type_id,
                                           object_is_value, conclude_property_id, reading, status,
                                           decided_by) VALUES
                ('{rule}','{kb}','phrase','leads','{person}','{project}',false,'{works_on}',NULL,
                 'approved','person');
            -- 人手写的一行：链上没有物化出来的行
            INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, layer, confidence,
                               valid_from, valid_from_precision) VALUES
                ('{by_hand}','{kb}','{wang}','{leads}','{helios}','typed',1.0,'2022-01-01','day');
            "#,
            binding = id(),
        ))
        .execute(&pool)
        .await?;

        // 物化：三条陈述各一行 leads，每句另有一行规则蕴含的 works_on
        materialize(&pool, kb).await?;
        let typed = rows(&pool, kb, zhang, leads).await?;
        let by_rule = rows(&pool, kb, zhang, works_on).await?;
        assert_eq!((typed.len(), by_rule.len()), (1, 1));

        // #967 之前人改区间：修正行写出来没有链接
        let corrected = correct_start(&pool, typed[0], "2023-02-01T00:00:00Z").await?;
        let corrected_rule = correct_start(&pool, by_rule[0], "2023-02-01T00:00:00Z").await?;
        as_before_967(&pool, corrected).await?;
        as_before_967(&pool, corrected_rule).await?;
        // 对照：#967 之后改的一行带着链接；人手写的一行被改过
        let after = rows(&pool, kb, li, leads).await?;
        let after = correct_start(&pool, after[0], "2024-08-01T00:00:00Z").await?;
        let by_hand = correct_start(&pool, by_hand, "2022-02-01T00:00:00Z").await?;
        let after_links = links(&pool, after).await?;
        assert_eq!(after_links.0, Some(kickoff), "#967 之后改写带着链接");
        // 对照：人点头的事实给裸行补上起点——新行链着那条物化行，却是另一次观察
        let (refined, _) = graph::insert_fact(
            &pool,
            kb,
            zhou,
            Some(leads),
            nova,
            Validity {
                from: Some(t("2023-05-01T00:00:00Z")),
                from_precision: Some("day"),
                ..Default::default()
            },
            1.0,
        )
        .await?;
        let (supersedes,): (Option<Uuid>,) =
            sqlx::query_as("SELECT supersedes FROM facts WHERE id = $1")
                .bind(refined)
                .fetch_one(&pool)
                .await?;
        assert!(supersedes.is_some(), "the confirmed fact refines the bare row");

        // 旧代码的后果：再物化一次，照陈述又算出两行，与修正行并排
        let second = materialize(&pool, kb).await?;
        assert_eq!(
            (second.added, second.implied, second.merged),
            (1, 1, 1),
            "the two duplicates, and the bare statement merged into the refined row: {second:?}"
        );
        let refined_links = links(&pool, refined).await?;
        assert_eq!(refined_links, (None, false, vec![bare], vec![]));
        let duplicate = *rows(&pool, kb, zhang, leads)
            .await?
            .iter()
            .find(|f| **f != corrected)
            .expect("a duplicate of the corrected leads row");
        let duplicate_rule = *rows(&pool, kb, zhang, works_on)
            .await?
            .iter()
            .find(|f| **f != corrected_rule)
            .expect("a duplicate of the corrected works_on row");

        sqlx::raw_sql(MIGRATION).execute(&pool).await?;

        // 两条修正行找回陈述与规则来源
        assert_eq!(
            links(&pool, corrected).await?,
            (Some(charter), false, vec![charter], vec![])
        );
        assert_eq!(
            links(&pool, corrected_rule).await?,
            (None, true, vec![], vec![charter])
        );
        // 重复行作废，不删
        assert!(!live(&pool, duplicate).await? && !live(&pool, duplicate_rule).await?);
        // 对照不动
        assert_eq!(links(&pool, after).await?, after_links);
        assert_eq!(links(&pool, by_hand).await?, (None, false, vec![], vec![]));
        assert_eq!(links(&pool, refined).await?, refined_links, "a refinement is not a rewrite");
        assert!(
            live(&pool, by_hand).await? && live(&pool, after).await? && live(&pool, refined).await?
        );

        // 再物化什么也不做：陈述由修正行代表，规则的来源也在
        assert_eq!(materialize(&pool, kb).await?, Outcome::default());
        let (nodes, edges) =
            graph::neighborhood(&pool, kb, aurora, 1, Some(t("2023-01-20T00:00:00Z")), None)
                .await?;
        let names: Vec<&str> = edges
            .iter()
            .filter_map(|e| nodes.iter().find(|n| n.id == e.source))
            .map(|n| n.name.as_str())
            .collect();
        assert!(names.is_empty(), "nobody on 2023-01-20: {names:?}");

        // 再跑一遍，一样不动
        sqlx::raw_sql(MIGRATION).execute(&pool).await?;
        assert_eq!(
            links(&pool, corrected).await?,
            (Some(charter), false, vec![charter], vec![])
        );
        assert_eq!(
            rows(&pool, kb, zhang, leads).await?,
            vec![corrected],
            "one live leads row"
        );
        assert_eq!(rows(&pool, kb, zhang, works_on).await?, vec![corrected_rule]);
        anyhow::Ok(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
