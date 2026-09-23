//! 口径检索的存储面——打在真库上（#574）。
//!
//! 三条性质：
//! 1. **该嵌的才嵌**：没向量的、模型换了的、文本改了的要重嵌；嵌过且没变的不要。
//! 2. **向量一路按距离排**，维度对不上的行跳过。
//! 3. **按 id 取回保持给定顺序**——顺序是检索排出来的。

use sqlx::PgPool;
use utopia_store::mappings as m;
use uuid::Uuid;

async fn base(pool: &PgPool) -> anyhow::Result<Uuid> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'pick-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'pick-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'pick-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    Ok(kb)
}

async fn definition(pool: &PgPool, kb: Uuid, name: &str, summary: &str) -> anyhow::Result<Uuid> {
    let ent = Uuid::now_v7();
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, $3)")
        .bind(ent)
        .bind(kb)
        .bind(name)
        .execute(pool)
        .await?;
    let (id, _) = m::propose(
        pool,
        kb,
        ent,
        "wide",
        Some("dw.t"),
        Some("sum(x)"),
        None,
        None,
        Some(summary),
        false,
    )
    .await?;
    sqlx::query("UPDATE concept_mappings SET status = 'confirmed' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(id)
}

#[tokio::test]
async fn only_what_changed_gets_embedded_and_neighbours_come_back_in_order() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let kb = base(&pool).await?;

    let run = async {
        let gmv = definition(&pool, kb, "GMV", "paid, non-test, yuan").await?;
        let net = definition(&pool, kb, "Net sales", "GMV minus refunds").await?;
        let freight = definition(&pool, kb, "Freight", "freight collected").await?;

        // 一开始三条都要嵌
        let stale = m::needing_embedding(&pool, kb, "bge-m3", 100).await?;
        assert_eq!(stale.len(), 3);
        let text_of = |id: Uuid| stale.iter().find(|t| t.id == id).unwrap().clone();
        assert_eq!(text_of(gmv).embed_text(), "GMV: paid, non-test, yuan");

        // 嵌进去（三维就够测排序）
        m::set_embeddings(
            &pool,
            "bge-m3",
            &[
                (text_of(gmv), vec![1.0, 0.0, 0.0]),
                (text_of(net), vec![0.9, 0.1, 0.0]),
                (text_of(freight), vec![0.0, 0.0, 1.0]),
            ],
        )
        .await?;
        assert!(
            m::needing_embedding(&pool, kb, "bge-m3", 100)
                .await?
                .is_empty(),
            "嵌过且没变的不该再嵌"
        );

        // 换模型：全部要重嵌；改文本：只有那一条要重嵌
        assert_eq!(
            m::needing_embedding(&pool, kb, "other-model", 100)
                .await?
                .len(),
            3
        );
        // 补嵌有上限：一问只嵌这么多，剩下的下一问接着补
        assert_eq!(
            m::needing_embedding(&pool, kb, "other-model", 2)
                .await?
                .len(),
            2
        );
        sqlx::query(
            "UPDATE concept_mappings SET summary = 'freight, paid orders only' WHERE id = $1",
        )
        .bind(freight)
        .execute(&pool)
        .await?;
        let again = m::needing_embedding(&pool, kb, "bge-m3", 100).await?;
        assert_eq!(
            again.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![freight],
            "改了说明的那条要重嵌"
        );

        // 向量一路：离 (1,0,0) 最近的是 GMV，其次 Net sales；Freight 最远
        let near = m::vector_search(&pool, kb, "bge-m3", &[1.0, 0.0, 0.0], 3).await?;
        assert_eq!(near, vec![gmv, net, freight]);
        // 维度对不上的查询一条都不回（换过模型还没重嵌的那种状态）
        assert!(m::vector_search(&pool, kb, "bge-m3", &[1.0, 0.0], 3)
            .await?
            .is_empty());
        // 换了模型、还没重嵌的行不参与：维度一样也不比
        assert!(
            m::vector_search(&pool, kb, "other-model", &[1.0, 0.0, 0.0], 3)
                .await?
                .is_empty()
        );

        // 按 id 取回保持给定顺序
        let got = m::by_ids(&pool, kb, &[freight, gmv]).await?;
        assert_eq!(
            got.iter().map(|x| x.id).collect::<Vec<_>>(),
            vec![freight, gmv]
        );
        assert_eq!(got[1].concept_name, "GMV");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // 只删知识库，不删 org——用户是软删除的，测试也不该造一个产品里不存在的动作
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
