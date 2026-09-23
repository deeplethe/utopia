//! 一致性检查打在真库上：取数、判断、落库、重跑。
//!
//! 纯逻辑那部分在 `utopia-reason` 里已经有 12 个用例,不起库就能跑。这里钉的是
//! 那一层看不见的四样,每一样都活在 SQL 或表约束里:
//!
//! - **取数的三个过滤**。被推翻的事实、没有谓词的事实、宾语是字面值的属性事实,
//!   都不该参与——公理谈的是实体之间的关系
//! - **没有公理就没有判据**。一个没导本体包的库跑出来是零,那是实情不是故障
//! - **重跑幂等**。同一处矛盾不重复入库,而人表过态的那一行重跑不会被抹掉
//!   （`ontology_proposals` 在这里踩过坑:重跑把被拒绝的提案刷回待看）
//! - **陈旧的会被清掉**。事实撤了,那条违规就不该还挂在 Review 页上
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆。

use sqlx::PgPool;
use utopia_store::reasoning;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    /// 声明了 asymmetric + irreflexive
    owns: Uuid,
    /// 一位公理都没声明
    mentions: Uuid,
    a: Uuid,
    b: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let etype = Uuid::now_v7();
    let (owns, mentions) = (Uuid::now_v7(), Uuid::now_v7());
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'axioms-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'axioms-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'axioms-test')",
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
        "INSERT INTO relation_types (id, kb_id, key, label, is_asymmetric, is_irreflexive)
         VALUES ($1, $2, 'owns', 'owns', TRUE, TRUE)",
    )
    .bind(owns)
    .bind(kb)
    .execute(pool)
    .await?;
    // 一位公理都没有——它的边永远不该被判成矛盾
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'mentions', 'mentions')",
    )
    .bind(mentions)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [(a, "Acme"), (b, "Beta")] {
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
    Ok(Fixture {
        org,
        kb,
        owns,
        mentions,
        a,
        b,
    })
}

/// 落一条关系事实,返回它的 id。
async fn fact(pool: &PgPool, kb: Uuid, s: Uuid, p: Uuid, o: Uuid) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(kb)
    .bind(s)
    .bind(p)
    .bind(o)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn open_kinds(pool: &PgPool, kb: Uuid) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT kind FROM axiom_violations WHERE kb_id = $1 AND status = 'open' ORDER BY kind",
    )
    .bind(kb)
    .fetch_all(pool)
    .await?)
}

#[tokio::test]
async fn the_ontology_is_the_only_judge() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // ---- 一、没有矛盾的库跑出来是零
        let quiet = fact(&pool, f.kb, f.a, f.owns, f.b).await?;
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.edges, 1);
        assert_eq!(r.predicates_with_axioms, 1, "mentions 一位都没声明,不该进来");
        assert_eq!(r.found, 0, "一条单向的 owns 不构成任何矛盾");

        // ---- 二、公理说了才算数
        // B owns A —— 与上面那条构成反对称违规
        let back = fact(&pool, f.kb, f.b, f.owns, f.a).await?;
        // A mentions B / B mentions A —— 双向,但 mentions 没有公理,不该报
        fact(&pool, f.kb, f.a, f.mentions, f.b).await?;
        fact(&pool, f.kb, f.b, f.mentions, f.a).await?;
        // A owns A —— 自反违规
        let loop_fact = fact(&pool, f.kb, f.a, f.owns, f.a).await?;

        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.edges, 5);
        assert_eq!(
            open_kinds(&pool, f.kb).await?,
            vec!["asymmetry", "self_loop"],
            "双向的 mentions 不该被报——它的谓词没有任何公理"
        );
        assert_eq!(r.inserted, 2);

        // 自反那一条:两列指的是同一条事实
        let (l, rr): (Uuid, Uuid) = sqlx::query_as(
            "SELECT left_fact, right_fact FROM axiom_violations
              WHERE kb_id = $1 AND kind = 'self_loop'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(l, loop_fact);
        assert_eq!(l, rr, "一条事实跟自己矛盾,不需要第二条");

        // ---- 三、重跑不重复入库
        let again = reasoning::run(&pool, f.kb).await?;
        assert_eq!(again.found, 2);
        assert_eq!(again.inserted, 0, "同一处矛盾第二次跑不该再插一行");
        assert_eq!(again.cleared, 0, "也不该把上一轮的清掉");

        // ---- 四、人表过态的,重跑不抹
        sqlx::query(
            "UPDATE axiom_violations SET status = 'resolved', resolution = 'accepted'
              WHERE kb_id = $1 AND kind = 'asymmetry'",
        )
        .bind(f.kb)
        .execute(&pool)
        .await?;
        let after = reasoning::run(&pool, f.kb).await?;
        assert_eq!(after.inserted, 0, "已经有人表态的那一行不该被重新插一条");
        let resolved: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM axiom_violations WHERE kb_id = $1 AND status = 'resolved'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(resolved, 1, "人的决定必须活过重跑");

        // ---- 五、事实撤了,陈旧的 open 行要清掉
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(loop_fact)
            .execute(&pool)
            .await?;
        let swept = reasoning::run(&pool, f.kb).await?;
        assert_eq!(swept.cleared, 1, "被推翻的事实不该还挂着一条违规");
        assert!(
            !open_kinds(&pool, f.kb).await?.contains(&"self_loop".to_string()),
            "撤掉那条事实之后自反违规就不成立了"
        );

        // ---- 六、属性事实不参与:宾语是字面值,公理谈的是实体之间的关系
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value)
             VALUES ($1, $2, $3, $4, '\"2015\"'::jsonb)",
        )
        .bind(Uuid::now_v7())
        .bind(f.kb)
        .bind(f.a)
        .bind(f.owns)
        .execute(&pool)
        .await?;
        let attrs = reasoning::run(&pool, f.kb).await?;
        assert_eq!(attrs.edges, 4, "字面值宾语的那条不该被取成边");

        // ---- 七、没有本体包的库:结论是「没有判据」,不是「没有矛盾」
        sqlx::query("UPDATE relation_types SET is_asymmetric = FALSE, is_irreflexive = FALSE WHERE kb_id = $1")
            .bind(f.kb)
            .execute(&pool)
            .await?;
        let blind = reasoning::run(&pool, f.kb).await?;
        assert_eq!(blind.predicates_with_axioms, 0);
        assert_eq!(blind.found, 0);
        assert!(
            open_kinds(&pool, f.kb).await?.is_empty(),
            "公理撤了,据它报出来的违规也该跟着走"
        );
        let _ = (quiet, back);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // 连 org 一起删——只删 kb 会把 organizations / workspaces 留在开发库里
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
