//! 一个被描述的东西是一个没有名字的实体（0044 第一刀，#729）。
//!
//! `create_described` 写 `description`、不写 `known_as`：名字事实是召回的桥（0041），
//! 一段描述不能做桥——两篇文档里碰巧描述得一样的两个东西不该因此接到一起。
//! 于是按名字召回找不到它，它的名字栏是空的，`facts` 里没有一条以它为主语的行。

use sqlx::PgPool;
use utopia_store::{names, resolution};
use uuid::Uuid;

const ORG: &str = "described-thing-test";

struct Fixture {
    org: Uuid,
    kb: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(ORG)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(ORG)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(ORG)
        .execute(pool)
        .await?;
    Ok(Fixture { org, kb })
}

#[tokio::test]
async fn a_described_thing_has_a_description_and_no_name() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    let f = seed(&pool).await?;

    let run = async {
        let description = "the buyer's Shanghai subsidiary";
        let id = resolution::create_described(&pool, f.kb, description, Some("subsidiary"))
            .await?;

        let (canonical, desc, type_id, specific): (
            String,
            Option<String>,
            Option<Uuid>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT canonical_name, description, type_id, specific_type FROM entities WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(desc.as_deref(), Some(description), "描述是它的身份");
        assert_eq!(canonical, description, "显示名就是描述本身");
        assert!(type_id.is_none(), "文档的类别词不是本体的类");
        assert_eq!(specific.as_deref(), Some("subsidiary"));

        // 一条迟到的别名也不能把描述变成召回桥：record 得拒绝它，而不是照单全收
        assert_eq!(
            names::record(&pool, f.kb, id, "the Shanghai buyer", None, None).await?,
            None
        );

        // 没有一条以它为主语的事实——尤其没有 known_as
        let facts: i64 = sqlx::query_scalar("SELECT count(*) FROM facts WHERE subject_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(facts, 0, "描述不写成名字事实");
        assert!(
            names::for_entity(&pool, f.kb, id, None).await?.is_empty(),
            "名字栏是空的"
        );

        // 按名字召回找不到它：描述不是桥
        assert_eq!(
            resolution::existing_by_name(&pool, f.kb, description).await?,
            None,
            "一段描述不能把后来的提及接到它身上"
        );
        // 名字事实那条召回也找不到它：库里有一条同样小写值的名字事实才算
        let recalled: Option<Uuid> = sqlx::query_scalar(&format!(
            "SELECT e.id FROM entities e
             WHERE e.kb_id = $1 AND e.merged_into IS NULL AND {}",
            names::has_name_in("e", 1, 2)
        ))
        .bind(f.kb)
        .bind(vec![description.to_lowercase()])
        .fetch_optional(&pool)
        .await?;
        assert_eq!(recalled, None);
        // 后来一条同样措辞的提及走消解：另建一个，不归到被描述的那个身上
        let later =
            resolution::resolve_mention(&pool, f.kb, None, description, None, None, &[]).await?;
        assert!(later.created, "描述不是桥，提及不归到它身上");
        assert_ne!(later.entity_id, id);

        // 同一段描述再建一次是另一个实体：库里不去重，同一篇里并成一个是调用方的事
        let twin = resolution::create_described(&pool, f.kb, description, None).await?;
        assert_ne!(twin, id);
        let specific: Option<String> =
            sqlx::query_scalar("SELECT specific_type FROM entities WHERE id = $1")
                .bind(twin)
                .fetch_one(&pool)
                .await?;
        assert!(specific.is_none());

        assert!(
            resolution::create_described(&pool, f.kb, "   ", None)
                .await
                .is_err(),
            "空描述不是一个东西"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
