//! 签名的陈述数和例句名额不随证据条数增长。
use sqlx::PgPool;
use utopia_store::{graph, phrase_bindings};
use uuid::Uuid;

#[tokio::test]
async fn multiple_evidence_does_not_multiply_statements_or_examples() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'signature-evidence')")
        .bind(org)
        .execute(&pool)
        .await?;
    let result = async {
        sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'signature-evidence')")
            .bind(ws)
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'signature-evidence')",
        )
        .bind(kb)
        .bind(ws)
        .execute(&pool)
        .await?;
        let mut statements = Vec::new();
        for name in ["甲", "乙", "丙", "丁"] {
            let subject = utopia_store::resolution::resolve_mention(
                &pool,
                kb,
                None,
                name,
                None,
                None,
                None,
                &[],
            )
            .await?
            .entity_id;
            let object = utopia_store::resolution::resolve_mention(
                &pool,
                kb,
                None,
                "买方",
                None,
                None,
                None,
                &[],
            )
            .await?
            .entity_id;
            statements.push(
                graph::insert_open_statement(
                    &pool,
                    kb,
                    subject,
                    "supplies",
                    graph::FactObject::Entity(object),
                    None,
                    1.0,
                )
                .await?
                .0,
            );
        }
        // 同一陈述有三条证据，也只能占一个例句名额。最早的证据没有引用位置，
        // 后来的完整位置必须优先；只按 chunk_id 排序会选错。
        for i in 0..3 {
            let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
            let text = if i == 2 {
                "前言。甲向买方供货。替代引文"
            } else {
                "前言。甲向买方供货。后记"
            };
            sqlx::query("INSERT INTO documents(id,kb_id,filename,sha256) VALUES($1,$2,$3,$3)")
                .bind(doc)
                .bind(kb)
                .bind(doc.to_string())
                .execute(&pool)
                .await?;
            sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES($1,$2,$3,0,$4)")
                .bind(chunk)
                .bind(kb)
                .bind(doc)
                .bind(text)
                .execute(&pool)
                .await?;
            graph::add_evidence_located(
                &pool,
                statements[0],
                chunk,
                Some(if i == 2 {
                    "替代引文"
                } else {
                    "甲向买方供货。"
                }),
                None,
                match i {
                    0 => None,
                    1 => Some((3, 10)),
                    _ => Some((10, 14)),
                },
            )
            .await?;
            let signatures = phrase_bindings::signatures(&pool, kb).await?;
            anyhow::ensure!(signatures.len() == 1);
            let s = &signatures[0];
            anyhow::ensure!(
                s.count == 4,
                "after {} evidence rows, four statements counted as {}",
                i + 1,
                s.count
            );
            anyhow::ensure!(s.examples.len() == 3);
            let distinct: std::collections::HashSet<_> = s.examples.iter().collect();
            anyhow::ensure!(
                distinct.len() == 3,
                "one statement filled multiple representative slots: {:?}",
                s.examples
            );
            anyhow::ensure!(
                s.quotes[0] == if i == 0 { "" } else { "甲向买方供货。" },
                "Unicode quote offsets must stay character-based: {:?}",
                s.quotes
            );
            anyhow::ensure!(
                s.quotes[1..].iter().all(String::is_empty),
                "statements without evidence remain eligible"
            );
        }
        sqlx::query("UPDATE facts SET invalidated_at=now() WHERE id=$1")
            .bind(statements[0])
            .execute(&pool)
            .await?;
        let s = phrase_bindings::signatures(&pool, kb).await?.remove(0);
        anyhow::ensure!(s.count == 3 && s.examples.len() == 3);
        anyhow::ensure!(s.quotes.iter().all(String::is_empty));
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    pool.close().await;
    result
}
