//! 本体声明的公理要真的落到库里——打在真库上。
//!
//! 这条防线的由来是一个既存缺陷:`create_relation_types_bulk` 的文档写着
//! 「`functional` / `inverse_functional` 必须照词汇表写下去,不能默认 false」,
//! 而它的 SQL 里是硬编码的 `FALSE, FALSE`——两个数组绑定了却没进 `UNNEST`。
//! 实测装 FOAF(声明了 17 条 FunctionalProperty)之后,库里 functional 为真的
//! **一条都没有**。
//!
//! 那两位是时态引擎自动闭合事实的依据。写错的方向不同,后果也不同:标错成真
//! 会成批造假冲突(`part_of` 那次 59 条),而这次是反方向——该为真的全成了假,
//! 于是**该检测出的时态冲突一条都检测不到**,静悄悄地。
//!
//! `cargo check` 看不见这种错(类型全对),单元测试也看不见(它在 SQL 字符串里)。
//! 只有把一行真的写进去再读回来才行。

use sqlx::PgPool;
use uuid::Uuid;

/// 造一个最小的库:公理落库跟本体大小无关,一条就够。
async fn kb(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid)> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'ax-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'ax-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'ax-test')")
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    Ok((kb, org))
}

#[tokio::test]
async fn every_axiom_survives_the_bulk_insert() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (kb_id, org) = kb(&pool).await?;

    let run = async {
        let row = |key: &str, f: bool, i: bool, t: bool, s: bool, a: bool, r: bool| {
            utopia_store::ontology::BulkRelation {
                key: key.into(),
                label: key.into(),
                description: String::new(),
                iri: format!("http://ax.test/#{key}"),
                kind: "relation",
                datatype: None,
                functional: f,
                inverse_functional: i,
                transitive: t,
                symmetric: s,
                asymmetric: a,
                irreflexive: r,
            }
        };
        // 一条全真、一条全假：全假那条守的是"没声明的不该被写成真"
        utopia_store::ontology::create_relation_types_bulk(
            &pool,
            kb_id,
            &[
                row("all_true", true, true, true, true, true, true),
                row("all_false", false, false, false, false, false, false),
            ],
        )
        .await?;

        let got: Vec<(String, bool, bool, bool, bool, bool, bool)> = sqlx::query_as(
            "SELECT key, functional, inverse_functional,
                    is_transitive, is_symmetric, is_asymmetric, is_irreflexive
             FROM relation_types WHERE kb_id = $1 ORDER BY key",
        )
        .bind(kb_id)
        .fetch_all(&pool)
        .await?;

        let t = got
            .iter()
            .find(|r| r.0 == "all_true")
            .expect("all_true 落库");
        assert!(
            t.1 && t.2 && t.3 && t.4 && t.5 && t.6,
            "六个公理位都该照写下去，实得 {t:?}"
        );
        let f = got
            .iter()
            .find(|r| r.0 == "all_false")
            .expect("all_false 落库");
        assert!(
            !f.1 && !f.2 && !f.3 && !f.4 && !f.5 && !f.6,
            "没声明的公理不该凭空为真，实得 {f:?}"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
