//! 人拍过板的类型，引擎不许改——打在真库上。
//!
//! 这一整条防线活在 SQL 的 `WHERE` 子句里：三处读侧各加一句
//! `type_source <> 'human'`，漏一处就是静默失效。`cargo check` 一个字看不见，
//! 而 0009 刚在同一片区域踩过 `NULL <> uuid` 的坑——类型系统数得清 Rust，
//! 数不到 SQL 里去。
//!
//! 四条断言对应四条路径：
//!
//! - 类型消解取材（`entities_for_type_resolution`）不该捞人拍过板的
//! - 本体长出新类后的认领（`adopt_proposed_types`）不该盖掉人拍过板的
//! - 抽取升格不该给「人说过就是没有类型」的实体安一个类型 ← 0009 × P4 的交叉
//! - `retype_entities` 用现成的 `actor` 参数区分 human / inferred
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

struct Fx {
    org: Uuid,
    kb: Uuid,
    org_type: Uuid,
    sub_type: Uuid,
}

/// 造一个刚好能触发「第三种取材条件」的本体：`organization` 有子类 `startup`。
/// 人把实体定成 `organization` 之后，正是这个子类让它够格被重判。
async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (org_type, sub_type) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'p4a-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'p4a-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'p4a-test')")
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    for (id, key) in [(org_type, "organization"), (sub_type, "startup")] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $3)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .execute(pool)
            .await?;
    }
    sqlx::query("INSERT INTO entity_type_parents (child_id, parent_id) VALUES ($1, $2)")
        .bind(sub_type)
        .bind(org_type)
        .execute(pool)
        .await?;
    Ok(Fx {
        org,
        kb,
        org_type,
        sub_type,
    })
}

async fn entity(
    pool: &PgPool,
    f: &Fx,
    name: &str,
    type_id: Option<Uuid>,
    source: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name, type_source)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(type_id)
    .bind(name)
    .bind(source)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn source_of(pool: &PgPool, id: Uuid) -> anyhow::Result<String> {
    Ok(
        sqlx::query_scalar("SELECT type_source FROM entities WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

/// 主战场：取材条件里的「现类还有子类就纳入」会把人拍过板的实体一并捞回来。
#[tokio::test]
async fn type_resolution_leaves_human_decisions_alone() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let by_human = entity(&pool, &f, "Acme", Some(f.org_type), "human").await?;
        let by_engine = entity(&pool, &f, "Globex", Some(f.org_type), "extracted").await?;

        let picked = utopia_store::resolution::entities_for_type_resolution(&pool, f.kb, 100)
            .await?
            .into_iter()
            .map(|c| c.id)
            .collect::<std::collections::HashSet<_>>();

        assert!(
            picked.contains(&by_engine),
            "抽取定的类型该被重判——organization 有子类 startup，这正是消解的用武之地"
        );
        assert!(!picked.contains(&by_human), "人拍过板的不该被拿去重判");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}

/// 本体长出新类之后的认领，同样不该盖掉人的决定。
///
/// 这一条顺带保证了 `unadopt_types` 的正确性：human 行永远不进采纳批次，
/// 撤销时也就不会遇到它们，不必额外还原 `type_source`。
#[tokio::test]
async fn adopting_a_new_class_does_not_claim_human_typed_entities() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 两个实体都被模型提议过 startup，但一个的类型是人定的
        for (name, source) in [("Acme", "human"), ("Globex", "extracted")] {
            let id = entity(&pool, &f, name, Some(f.org_type), source).await?;
            sqlx::query("UPDATE entities SET proposed_type = 'startup' WHERE id = $1")
                .bind(id)
                .execute(&pool)
                .await?;
        }

        let (_, moved) = utopia_store::resolution::adopt_proposed_types(
            &pool,
            f.kb,
            f.sub_type,
            &["startup".to_string()],
            None,
        )
        .await?;
        assert_eq!(moved, 1, "只该认领那个不是人定的");

        let human: Option<Uuid> = sqlx::query_scalar(
            "SELECT type_id FROM entities WHERE kb_id = $1 AND canonical_name = 'Acme'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(human, Some(f.org_type), "人定的类型原样未动");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}

/// **0009 × P4 的交叉，最容易漏的一条。**
///
/// 0009 之后「没有类型」可能是人的决定——他看过这个实体，认为本体里没有合适的类。
/// 而抽取升格的守卫原本只看 `type_key.is_none()`，分不出「还没判」和「人判了，
/// 就是没有」，于是下一次抽取会给它安一个类型。
#[tokio::test]
async fn extraction_does_not_fill_in_a_type_a_human_left_empty() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 人看过它，认为本体里没有合适的类 → 没有类型，且这是个决定
        let decided = entity(&pool, &f, "Ambiguous Thing", None, "human").await?;
        // 还没轮到判的那个
        let pending = entity(&pool, &f, "Other Thing", None, "extracted").await?;

        // **升格分支要画像相似度才走得到**：ctx 为空时 best 是 None，整段被跳过。
        // 第一版就是这么写的，撤掉守卫也没有测试失败——测试没测到它要测的东西
        let ctx: Vec<f32> = vec![1.0, 0.0, 0.0];
        for id in [decided, pending] {
            sqlx::query(
                "UPDATE entities SET profile_embedding = $2::vector, profile_n = 1 WHERE id = $1",
            )
            .bind(id)
            .bind("[1,0,0]")
            .execute(&pool)
            .await?;
        }

        // 抽取再次遇到同名 mention，判出 organization。余弦 = 1.0，远高于 SIM_ATTACH
        for id in [decided, pending] {
            let name: String =
                sqlx::query_scalar("SELECT canonical_name FROM entities WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await?;
            let _ = utopia_store::resolution::resolve_mention(
                &pool,
                f.kb,
                Some(f.org_type),
                &name,
                Some(&ctx),
            )
            .await?;
        }

        // 对照：没有人拍过板的那个**应该**被升格，否则这个测试证明不了守卫在起作用
        let after_pending: Option<Uuid> =
            sqlx::query_scalar("SELECT type_id FROM entities WHERE id = $1")
                .bind(pending)
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            after_pending,
            Some(f.org_type),
            "还没判过的该被抽取升格——不然下面那条断言是空的"
        );

        let after_decided: Option<Uuid> =
            sqlx::query_scalar("SELECT type_id FROM entities WHERE id = $1")
                .bind(decided)
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            after_decided, None,
            "人说过「就是没有类型」，抽取不该替他填一个"
        );
        assert_eq!(source_of(&pool, decided).await?, "human");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}

/// `retype_entities` 用现成的 `actor` 参数区分来源：人点的批准是背书，受保护；
/// 引擎自动裁决的不是。不必为此加新参数——#112 加 actor 时它就已经在那儿了。
#[tokio::test]
async fn who_approved_a_retype_decides_whether_it_is_protected() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let actor = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $1 || '@p4a.test', 'p4a', 'x')",
    )
    .bind(actor)
    .bind(f.org)
    .execute(&pool)
    .await?;

    let run = async {
        let a = entity(&pool, &f, "Approved", Some(f.org_type), "extracted").await?;
        let b = entity(&pool, &f, "Auto", Some(f.org_type), "extracted").await?;

        utopia_store::resolution::retype_entities(&pool, f.kb, &[(a, f.sub_type)], Some(actor))
            .await?;
        utopia_store::resolution::retype_entities(&pool, f.kb, &[(b, f.sub_type)], None).await?;

        assert_eq!(source_of(&pool, a).await?, "human", "人点的批准是背书");
        assert_eq!(source_of(&pool, b).await?, "inferred", "引擎自动裁决的不是");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(actor)
        .execute(&pool)
        .await?;
    run
}

/// **引擎改过的实体,下一轮还要捞得回来。**
///
/// 这一条守的是一个真出过的事故:类型消解落库时把「点运行的那个人」当成了
/// `retype_entities` 的 actor,而有 actor 就写 `type_source = 'human'`。
/// 于是**跑过一次消解的实体从此永远不再被消解**——一个没有任何人工 PATCH 记录
/// 的库,跑完一轮之后预览返回空列表,而且没有任何报错。
///
/// 「谁点的运行」与「谁判定这个实体是什么」是两件事。前者记在
/// `ontology.types_resolved` 审计里,后者才该决定 `type_source`。
#[tokio::test]
async fn an_engine_retype_does_not_lock_the_entity_out_of_the_next_round() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let e = entity(&pool, &f, "Initech", Some(f.org_type), "extracted").await?;
        // 留一个 specific_type:让它在「取材条件」上始终够格,这样落选与否
        // 只取决于 type_source——否则改成叶子类之后它本来就该落选,测不出东西
        sqlx::query("UPDATE entities SET specific_type = 'startup company' WHERE id = $1")
            .bind(e)
            .execute(&pool)
            .await?;
        // 引擎自动裁决:没有人为这一条背书
        utopia_store::resolution::retype_entities(&pool, f.kb, &[(e, f.sub_type)], None).await?;

        assert_eq!(
            source_of(&pool, e).await?,
            "inferred",
            "引擎裁决不是人的背书"
        );

        let picked = utopia_store::resolution::entities_for_type_resolution(&pool, f.kb, 100)
            .await?
            .into_iter()
            .map(|c| c.id)
            .collect::<std::collections::HashSet<_>>();
        assert!(
            picked.contains(&e),
            "引擎改过一次不该把实体锁死——本体还会长大,它还得能被重判"
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
