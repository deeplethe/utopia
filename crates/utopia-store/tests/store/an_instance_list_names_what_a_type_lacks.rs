//! 一个类的实例列表能只列缺某条关系的成员（#1269）。
//!
//! 「哪些人没写雇主」：连着的方式不止类型化的一行——开放陈述照文档的字写（大小写、
//! 空白不同也是同一个短语）、逆关系从另一头写（`Acme employs Cy`）、绑到这个属性的
//! 短语（`is employed by`）、规则推出来的一行也算。五种连上的成员都不列，只剩真的什么
//! 都没有的那个。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::ontology::{entity_instances, link_predicate, Lacking, LinkEnd};
use uuid::Uuid;

const ORG: &str = "instances-lacking-test";

async fn names_lacking(
    pool: &PgPool,
    kb: Uuid,
    person: Uuid,
    predicate: &str,
    end: LinkEnd,
) -> anyhow::Result<(Vec<String>, i64)> {
    let predicate = link_predicate(pool, kb, predicate).await?;
    let lacking = Lacking {
        predicate: &predicate,
        end,
    };
    let (rows, total) = entity_instances(pool, kb, person, Some(lacking), 50, 0).await?;
    Ok((rows.into_iter().map(|r| r.name).collect(), total))
}

#[tokio::test]
async fn a_member_linked_any_way_is_not_listed_as_lacking() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    let id = Uuid::now_v7;
    let (org, ws, kb, person, company) = (id(), id(), id(), id(), id());
    let (works_for, employs) = (id(), id());
    let (ann, bo, cy, dee, eve, fay, acme) = (id(), id(), id(), id(), id(), id(), id());
    sqlx::raw_sql(&format!(
        r#"
        INSERT INTO organizations (id, name) VALUES ('{org}', '{ORG}');
        INSERT INTO workspaces (id, org_id, name) VALUES ('{ws}', '{org}', '{ORG}');
        INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ('{kb}', '{ws}', '{ORG}');
        INSERT INTO entity_types (id, kb_id, key, label) VALUES
            ('{person}', '{kb}', 'person', 'Person'),
            ('{company}', '{kb}', 'company', 'Company');
        INSERT INTO relation_types (id, kb_id, key, label) VALUES
            ('{works_for}', '{kb}', 'works_for', 'worksFor');
        INSERT INTO relation_types (id, kb_id, key, label, inverse_of) VALUES
            ('{employs}', '{kb}', 'employs', 'employs', '{works_for}');
        INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES
            ('{ann}', '{kb}', '{person}', 'Ann'), ('{bo}', '{kb}', '{person}', 'Bo'),
            ('{cy}', '{kb}', '{person}', 'Cy'), ('{dee}', '{kb}', '{person}', 'Dee'),
            ('{eve}', '{kb}', '{person}', 'Eve'), ('{fay}', '{kb}', '{person}', 'Fay'),
            ('{acme}', '{kb}', '{company}', 'Acme');
        -- 类型化、逆关系从另一头写
        INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id) VALUES
            ('{f1}', '{kb}', '{ann}', '{works_for}', '{acme}'),
            ('{f2}', '{kb}', '{acme}', '{employs}', '{cy}');
        -- 开放陈述：照写的短语、绑到属性的短语；Dee 只有一条不相干的
        INSERT INTO facts (id, kb_id, subject_id, object_id, layer, phrase) VALUES
            ('{f3}', '{kb}', '{bo}', '{acme}', 'open', ' Works_For'),
            ('{f4}', '{kb}', '{eve}', '{acme}', 'open', 'is  employed by'),
            ('{f5}', '{kb}', '{dee}', '{ann}', 'open', 'mentors');
        INSERT INTO phrase_bindings (id, kb_id, phrase, subject_type_id, object_type_id,
                                     relation_type_id, direction, status) VALUES
            ('{b1}', '{kb}', 'is employed by', '{person}', '{company}',
             '{works_for}', 'forward', 'bound');
        INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES
            ('{rule}', '{kb}', '{works_for}', 'transitive');
        INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, rule_id) VALUES
            ('{d1}', '{kb}', '{fay}', '{works_for}', '{acme}', '{rule}');
        "#,
        f1 = id(),
        f2 = id(),
        f3 = id(),
        f4 = id(),
        f5 = id(),
        b1 = id(),
        rule = id(),
        d1 = id(),
    ))
    .execute(&pool)
    .await?;

    let lacking_employer = names_lacking(&pool, kb, person, "works_for", LinkEnd::Out).await?;
    assert_eq!(lacking_employer, (vec!["Dee".to_owned()], 1));
    let predicate = link_predicate(&pool, kb, "works_for").await?;
    assert_eq!(predicate.relation_type_id, Some(works_for));
    assert_eq!(predicate.phrases(), ["is employed by", "works_for"]);

    // 开放短语、看宾语那端：只有 Ann 被人 mentors 过
    let never_mentored = names_lacking(&pool, kb, person, "Mentors", LinkEnd::In).await?;
    assert_eq!(
        never_mentored,
        (
            ["Bo", "Cy", "Dee", "Eve", "Fay"]
                .map(str::to_owned)
                .to_vec(),
            5
        )
    );

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}
