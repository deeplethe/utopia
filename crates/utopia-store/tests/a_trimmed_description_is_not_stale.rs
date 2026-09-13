//! 嵌过的本体行不因为描述首尾的换行就永远判成陈的（#672），打在真库上。
//!
//! 陈不陈看「当时嵌的原文」与「现在要嵌的原文」是否一致。从前后者在 SQL 里用 `btrim`
//! 重拼，而原文由 Rust 的 `trim` 生成：`btrim` 只去空格，`trim` 连换行、制表符一起去。
//! schema.org 的描述结尾是 "\n      "，这些行嵌完仍旧判陈——补齐任务一遍遍重嵌，抽取
//! 门控等一个永远补不齐的索引，文档到期判失败。钉三样：
//!
//! 1. **嵌过一轮就不陈**：描述结尾带换行和空格、label 首尾带制表符的类（整段与 label
//!    两份）和关系，`set_type_embeddings` 写回之后，`types_needing_embedding` 为空
//! 2. **改了描述照样重嵌**：判据没有因此变松
//! 3. **换了模型全部重嵌**
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::ontology::{self, EmbedField, TypeKind};
use uuid::Uuid;

#[tokio::test]
async fn an_embedded_row_with_surrounding_whitespace_stays_embedded() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (place, located) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'trim-stale-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'trim-stale-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'trim-stale-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let result = async {
        // schema.org 导进来的样子：描述结尾一个换行加一串空格；label 首尾再夹个制表符
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label, description)
             VALUES ($1, $2, 'place', E'\\tPlace\\t', E'Entities that have a somewhat fixed, physical extension.\\n      ')",
        )
        .bind(place)
        .bind(kb)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, description)
             VALUES ($1, $2, 'located_in', 'located in', E'\\n  The place something is in.\\n')",
        )
        .bind(located)
        .bind(kb)
        .execute(&pool)
        .await?;

        let embed_all = |model: &'static str| {
            let pool = pool.clone();
            async move {
                let stale = ontology::types_needing_embedding(&pool, kb, model, None).await?;
                let items: Vec<_> = stale.iter().cloned().map(|t| (t, vec![0.1f32, 0.2, 0.3])).collect();
                ontology::set_type_embeddings(&pool, model, &items).await?;
                anyhow::Ok(stale)
            }
        };

        // ---- 一、第一轮三份都要嵌：类的整段与 label，关系的整段
        let first = embed_all("model-a").await?;
        let mut shape: Vec<(Uuid, &str, &str)> = first
            .iter()
            .map(|t| {
                (
                    t.id,
                    match t.kind {
                        TypeKind::Entity => "entity",
                        TypeKind::Relation => "relation",
                    },
                    match t.field {
                        EmbedField::Full => "full",
                        EmbedField::Label => "label",
                    },
                )
            })
            .collect();
        shape.sort();
        let mut want = vec![
            (place, "entity", "full"),
            (place, "entity", "label"),
            (located, "relation", "full"),
        ];
        want.sort();
        assert_eq!(shape, want);
        let label = first
            .iter()
            .find(|t| t.field == EmbedField::Label)
            .map(|t| t.text.clone());
        assert_eq!(label.as_deref(), Some("Place"), "送去嵌的是去掉首尾空白的 label");

        // 嵌过就不陈：从前这里会把三份原样再端回来，一轮一轮永远补不齐
        let again = ontology::types_needing_embedding(&pool, kb, "model-a", None).await?;
        assert!(again.is_empty(), "嵌过的行又判成陈的：{again:?}");

        // ---- 二、改了描述，只那一份重嵌
        sqlx::query("UPDATE relation_types SET description = 'Where something is.' WHERE id = $1")
            .bind(located)
            .execute(&pool)
            .await?;
        let changed = ontology::types_needing_embedding(&pool, kb, "model-a", None).await?;
        assert_eq!(changed.len(), 1);
        assert_eq!(
            (changed[0].id, changed[0].text.as_str()),
            (located, "located in\nWhere something is.")
        );
        embed_all("model-a").await?;

        // ---- 三、换了模型，三份全部重嵌
        let switched = ontology::types_needing_embedding(&pool, kb, "model-b", None).await?;
        assert_eq!(switched.len(), 3);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
