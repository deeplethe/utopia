//! 一个类被词汇表认领之后，**形状要跟着说实话**。
//!
//! 形状承载的是来历：方 = 词表声明的，圆 = 语料里长出来的。而 `adopt_iri_onto_key`
//! 是在形状还不表示来历的年代写的，它只写 IRI——于是往一个已经有 `person` /
//! `organization` 的库里导 schema.org，那几个类**拿到了 IRI 却仍然是圆的**。
//! 有 IRI 却是圆的，画面就在说谎。
//!
//! 这个缝隙只有真导入一次才撞得到：单看 `adopt_iri_onto_key` 的签名和调用点，
//! 没有任何东西提示"还有个字段该一起改"。实测撞上过（demo 库里 5 个类中招）。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn adopting_an_iri_turns_the_class_square() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    // 开跑前先扫地：断言 panic 会跳过 teardown，一次失败会给下一次留垃圾
    sqlx::query("DELETE FROM organizations WHERE name = 'adopt-shape-test'")
        .execute(&pool)
        .await?;

    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'adopt-shape-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'adopt-shape-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'adopt-shape-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let run = async {
        // 一个「语料里长出来的」类：没有 IRI，所以是圆的
        let grown = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label, color, shape)
             VALUES ($1, $2, 'person', 'Person', '#7fd0ff', 'circle')",
        )
        .bind(grown)
        .bind(kb)
        .execute(&pool)
        .await?;

        let shape: String = sqlx::query_scalar("SELECT shape FROM entity_types WHERE id = $1")
            .bind(grown)
            .fetch_one(&pool)
            .await?;
        assert_eq!(shape, "circle", "还没被认领时该是圆的");

        // 导入一份词汇表，把 schema.org 的 Person 认到这个 key 上
        let adopted = utopia_store::ontology::adopt_iri_onto_key(
            &pool,
            kb,
            "person",
            "https://schema.org/Person",
        )
        .await?;
        assert_eq!(adopted, Some(grown), "该认到那个已有的类上");

        let (iri, shape, color): (Option<String>, String, String) =
            sqlx::query_as("SELECT iri, shape, color FROM entity_types WHERE id = $1")
                .bind(grown)
                .fetch_one(&pool)
                .await?;
        assert_eq!(iri.as_deref(), Some("https://schema.org/Person"));
        assert_eq!(
            shape, "square",
            "认领之后是词表声明的类了，形状要跟着说实话"
        );
        // **颜色不该动**：颜色是身份（同一个 key 永远同一个色），
        // 认领改变的是来历不是身份
        assert_eq!(color, "#7fd0ff", "认领不该动用户看惯的颜色");

        // 幂等：再认一次不该改动任何东西（iri 已非空，UPDATE 匹配不上）
        let again = utopia_store::ontology::adopt_iri_onto_key(
            &pool,
            kb,
            "person",
            "https://example.org/OtherPerson",
        )
        .await?;
        assert_eq!(again, None, "已被认领的类不该被另一个词汇表抢走");
        let iri2: Option<String> = sqlx::query_scalar("SELECT iri FROM entity_types WHERE id = $1")
            .bind(grown)
            .fetch_one(&pool)
            .await?;
        assert_eq!(iri2.as_deref(), Some("https://schema.org/Person"));

        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = 'adopt-shape-test'")
        .execute(&pool)
        .await?;
    run
}
