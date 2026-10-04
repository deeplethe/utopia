//! 文档说了、本体还放不下的有多少：没绑到类的类别词、没绑到属性的短语形状。
//!
//! 工作台从前拿 0003 的 `ontology_misses` 说「本体覆盖了语料」，而开放图谱的抽取不往那张表
//! 里写：一个一个类都没有的库，二十二个类别词没着落，页面上写的是「覆盖了」。这两个数是
//! 本体代理读的那两样（0061 决定 2），别的库的不算。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::ontology::{uncovered, Uncovered};
use uuid::Uuid;

const ORG: &str = "empty-ontology-does-not-cover-test";

#[tokio::test]
async fn kind_words_and_phrases_with_no_place_are_counted_per_base() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    let (org, ws, kb, other, person, leads) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','{ORG}');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','{ORG}');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','a'), ('{other}','{ws}','b');"
    ))
    .execute(&pool)
    .await?;

    let run = async {
        // 什么都还没读过：没有要等的
        let none = Uncovered {
            kind_words: 0,
            phrases: 0,
        };
        assert_eq!(uncovered(&pool, kb).await?, none);

        // 读过文档、一个类都没有：每个类别词都没着落
        for word in ["person", "project", "city"] {
            sqlx::query(
                "INSERT INTO type_bindings (id, kb_id, kind_word, status) VALUES ($1, $2, $3, 'none')",
            )
            .bind(Uuid::now_v7())
            .bind(kb)
            .bind(word)
            .execute(&pool)
            .await?;
        }
        assert_eq!(uncovered(&pool, kb).await?.kind_words, 3);

        // 加了一个类、一条属性：绑上的不算，判成 none 的和两票不一致的都算
        sqlx::raw_sql(&format!(
            "INSERT INTO entity_types(id,kb_id,key,label) VALUES ('{person}','{kb}','person','person');
             INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description)
                 VALUES ('{leads}','{kb}','leads','leads','relation','state','');
             UPDATE type_bindings SET status='bound', type_id='{person}'
              WHERE kb_id='{kb}' AND kind_word='person';
             UPDATE type_bindings SET status='undecided' WHERE kb_id='{kb}' AND kind_word='city';
             INSERT INTO phrase_bindings (id, kb_id, phrase, subject_type_id, status, relation_type_id, direction)
                 VALUES ('{}','{kb}','负责','{person}','bound','{leads}','forward'),
                        ('{}','{kb}','汇报给','{person}','none',NULL,NULL),
                        ('{}','{kb}','调任','{person}','undecided',NULL,NULL);
             INSERT INTO type_bindings (id, kb_id, kind_word, status) VALUES ('{}','{other}','supplier','none');",
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        ))
        .execute(&pool)
        .await?;
        assert_eq!(
            uncovered(&pool, kb).await?,
            Uncovered {
                kind_words: 2,
                phrases: 2,
            }
        );
        assert_eq!(uncovered(&pool, other).await?.kind_words, 1, "each base counts its own");
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
