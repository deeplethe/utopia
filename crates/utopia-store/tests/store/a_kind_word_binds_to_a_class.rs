//! 一个类别词绑到一个类（0044 决定 3–4 的第一片，账本侧，见 0065）。
//!
//! 开放抽取只记文档自己的类别词，不选类。库要能数出每个类别词的签名（"Company" 与
//! "company" 是一个词：两个写法、两个实体、它们参与的关系短语）；绑定判了之后写到该
//! 类别词下每个实体的 `type_id`（`type_source = 'aligned'`），人定过的不动；绑定按类的
//! `updated_at` 与库里最新的类判过期；人的判定不被代理覆盖，反过来可以；解绑只动
//! `aligned` 的行；没有类对得上的按老流程提成「建议加类」。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use std::collections::HashSet;
use std::time::Duration;
use utopia_store::graph::{self, FactObject};
use utopia_store::{ontology, resolution, type_bindings};
use uuid::Uuid;

const ORG: &str = "kind-word-binding-test";
const PROPOSAL: &str = "the stockholder proposal on declassifying the board";

struct Fixture {
    kb: Uuid,
    organization: Uuid,
    person: Uuid,
    acme: Uuid,
    beta: Uuid,
    carol: Uuid,
    dave: Uuid,
    proposal: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (organization, person) = (Uuid::now_v7(), Uuid::now_v7());
    let (acme, beta, carol, dave, proposal) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
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
    for (id, key, label) in [
        (organization, "organization", "Organization"),
        (person, "person", "Person"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    // 有名字的：类别词照文档的写法，类空着
    for (id, name, kind) in [
        (acme, "Acme", "Company"),
        (beta, "Beta", "company"),
        (carol, "Carol", "person"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, canonical_name, specific_type)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(name)
        .bind(kind)
        .execute(pool)
        .await?;
    }
    // 人看过、说没有合适的类：type_id 空、type_source = human
    sqlx::query(
        "INSERT INTO entities (id, kb_id, canonical_name, specific_type, type_source)
         VALUES ($1, $2, 'Dave', 'person', 'human')",
    )
    .bind(dave)
    .bind(kb)
    .execute(pool)
    .await?;
    // 一个被描述、没有名字的东西
    sqlx::query(
        "INSERT INTO entities (id, kb_id, canonical_name, description, specific_type)
         VALUES ($1, $2, $3, $3, 'stockholder proposal')",
    )
    .bind(proposal)
    .bind(kb)
    .bind(PROPOSAL)
    .execute(pool)
    .await?;
    // 几条开放陈述：主语是公司的
    for (subject, phrase, object) in [
        (acme, "acquired", FactObject::Entity(beta)),
        (
            acme,
            "acquired",
            FactObject::Value(&serde_json::json!({ "value": "a rival" })),
        ),
        (
            beta,
            "is headquartered in",
            FactObject::Value(&serde_json::json!({ "value": "Austin" })),
        ),
    ] {
        graph::insert_open_statement(pool, kb, subject, phrase, object, None, 0.9).await?;
    }
    Ok(Fixture {
        kb,
        organization,
        person,
        acme,
        beta,
        carol,
        dave,
        proposal,
    })
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct Typed {
    type_id: Option<Uuid>,
    type_source: String,
    proposed_type: Option<String>,
}

async fn typed(pool: &PgPool, id: Uuid) -> anyhow::Result<Typed> {
    Ok(
        sqlx::query_as("SELECT type_id, type_source, proposed_type FROM entities WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

/// 两次 `now()` 要分得开：过期靠时间戳的先后
async fn tick() {
    tokio::time::sleep(Duration::from_millis(5)).await;
}

#[tokio::test]
async fn a_kind_word_is_counted_once_bound_once_and_applied_to_its_entities() -> anyhow::Result<()>
{
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
        // 1. 签名："Company" 与 "company" 是一个词
        let sigs = type_bindings::signatures(&pool, f.kb).await?;
        let words: Vec<&str> = sigs.iter().map(|s| s.kind_word.as_str()).collect();
        assert_eq!(
            words,
            ["company", "person", "stockholder proposal"],
            "按实体数再按词序：{sigs:?}"
        );
        let company = &sigs[0];
        assert_eq!(company.count, 2);
        assert_eq!(
            company.words.iter().map(String::as_str).collect::<HashSet<_>>(),
            HashSet::from(["Company", "company"]),
            "两种写法都留着"
        );
        assert_eq!(
            company.examples.iter().map(String::as_str).collect::<HashSet<_>>(),
            HashSet::from(["Acme", "Beta"])
        );
        assert_eq!(
            company.phrases,
            ["acquired", "is headquartered in"],
            "以公司为主语的开放陈述的短语，按次数"
        );
        let proposal = &sigs[2];
        assert_eq!(proposal.count, 1);
        assert_eq!(proposal.words, ["stockholder proposal"]);
        assert_eq!(proposal.examples, [PROPOSAL], "没有名字的以描述为例");
        assert!(proposal.phrases.is_empty());
        assert_eq!(sigs[1].count, 2, "Carol 与 Dave");

        // 2. 绑上并写到实体上：两个公司都成了 organization，人定过的不动
        assert!(
            type_bindings::decide(
                &pool,
                f.kb,
                "company",
                &company.words,
                Some(f.organization),
                "bound",
                &serde_json::json!({ "votes": ["organization", "organization"] }),
                "agent",
            )
            .await?
        );
        assert_eq!(
            type_bindings::apply(&pool, f.kb, "company", f.organization).await?,
            2
        );
        for id in [f.acme, f.beta] {
            assert_eq!(
                typed(&pool, id).await?,
                Typed {
                    type_id: Some(f.organization),
                    type_source: "aligned".into(),
                    proposed_type: None,
                }
            );
        }
        assert_eq!(
            type_bindings::apply(&pool, f.kb, "company", f.organization).await?,
            0,
            "再写一次没有改动"
        );
        assert!(
            type_bindings::decide(
                &pool,
                f.kb,
                "Person",
                &[],
                Some(f.person),
                "bound",
                &serde_json::json!({}),
                "agent",
            )
            .await?,
            "写法归一后是同一个词"
        );
        assert_eq!(
            type_bindings::apply(&pool, f.kb, "person", f.person).await?,
            1,
            "只有 Carol；Dave 是人定的"
        );
        assert_eq!(typed(&pool, f.carol).await?.type_id, Some(f.person));
        assert_eq!(
            typed(&pool, f.dave).await?,
            Typed {
                type_id: None,
                type_source: "human".into(),
                proposed_type: None,
            },
            "人说没有类，就没有类"
        );

        // 3. 抽取时用的表
        let bound = type_bindings::bound_map(&pool, f.kb).await?;
        assert_eq!(bound.get("company"), Some(&f.organization));
        assert_eq!(bound.get("person"), Some(&f.person));
        assert_eq!(bound.len(), 2);

        // 4. 过期：绑到的类改了
        assert!(type_bindings::stale(&pool, f.kb).await?.is_empty());
        tick().await;
        ontology::update_entity_type(
            &pool,
            f.kb,
            f.organization,
            "Organisation",
            None,
            "square",
            &[],
            "a company or any other body",
        )
        .await?;
        assert_eq!(type_bindings::stale(&pool, f.kb).await?, ["company"]);
        tick().await;
        assert!(
            type_bindings::decide(
                &pool,
                f.kb,
                "company",
                &company.words,
                Some(f.organization),
                "bound",
                &serde_json::json!({ "votes": ["organization", "organization"] }),
                "agent",
            )
            .await?,
            "代理可以改代理的"
        );
        assert!(type_bindings::stale(&pool, f.kb).await?.is_empty());

        // 5. 没有类对得上：记 none，按老流程提成建议加类
        assert!(
            type_bindings::decide(
                &pool,
                f.kb,
                "stockholder proposal",
                &proposal.words,
                None,
                "none",
                &serde_json::json!({ "votes": [null, null] }),
                "agent",
            )
            .await?
        );
        assert_eq!(
            type_bindings::propose(&pool, f.kb, "stockholder proposal", "stockholder proposal")
                .await?,
            1
        );
        assert_eq!(
            typed(&pool, f.proposal).await?.proposed_type.as_deref(),
            Some("stockholder proposal")
        );
        assert_eq!(
            type_bindings::propose(&pool, f.kb, "stockholder proposal", "Stockholder Proposal")
                .await?,
            0,
            "只写第一次，与 set_proposed_type 同一条"
        );
        let proposed = resolution::proposed_types(&pool, f.kb).await?;
        let p = proposed
            .iter()
            .find(|p| p.form == "stockholder proposal")
            .expect("the ontology page offers it");
        assert_eq!(p.entity_count, 1);
        assert_eq!(p.example.as_deref(), Some(PROPOSAL));
        // 绑定的形状：绑上了就得有类
        assert!(type_bindings::decide(
            &pool,
            f.kb,
            "thing",
            &[],
            None,
            "bound",
            &serde_json::json!({}),
            "agent",
        )
        .await
        .is_err());

        // 6. 过期：判成 none 之后库里长出了新类
        assert!(type_bindings::stale(&pool, f.kb).await?.is_empty());
        tick().await;
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'proposal', 'Proposal')",
        )
        .bind(Uuid::now_v7())
        .bind(f.kb)
        .execute(&pool)
        .await?;
        assert_eq!(
            type_bindings::stale(&pool, f.kb).await?,
            ["stockholder proposal"],
            "绑上的不受新类影响，none 的要重判"
        );
        tick().await;
        assert!(
            type_bindings::decide(
                &pool,
                f.kb,
                "stockholder proposal",
                &[],
                None,
                "none",
                &serde_json::json!({ "votes": [null, null] }),
                "agent",
            )
            .await?
        );
        assert!(type_bindings::stale(&pool, f.kb).await?.is_empty());

        // 7. 人的判定不被代理覆盖，反过来可以
        assert!(
            type_bindings::decide(
                &pool,
                f.kb,
                "company",
                &[],
                Some(f.organization),
                "bound",
                &serde_json::json!({ "by": "a person" }),
                "person",
            )
            .await?
        );
        assert!(
            !type_bindings::decide(
                &pool,
                f.kb,
                "company",
                &[],
                None,
                "none",
                &serde_json::json!({}),
                "agent",
            )
            .await?,
            "代理改不了人的"
        );
        let b = type_bindings::bindings(&pool, f.kb).await?;
        let company_b = b.iter().find(|b| b.kind_word == "company").unwrap();
        assert_eq!(
            (company_b.status.as_str(), company_b.type_id, company_b.decided_by.as_str()),
            ("bound", Some(f.organization), "person")
        );
        assert_eq!(
            b.iter().map(|b| b.kind_word.as_str()).collect::<Vec<_>>(),
            ["company", "person", "stockholder proposal"]
        );
        assert!(
            type_bindings::decide(
                &pool,
                f.kb,
                "company",
                &[],
                None,
                "none",
                &serde_json::json!({}),
                "person",
            )
            .await?,
            "人可以改人的"
        );
        let b = type_bindings::bindings(&pool, f.kb).await?;
        let company_b = b.iter().find(|b| b.kind_word == "company").unwrap();
        assert_eq!((company_b.status.as_str(), company_b.type_id), ("none", None));
        assert!(!type_bindings::bound_map(&pool, f.kb)
            .await?
            .contains_key("company"));

        // 8. 解绑只动 aligned 的行
        sqlx::query("UPDATE entities SET type_source = 'human' WHERE id = $1")
            .bind(f.beta)
            .execute(&pool)
            .await?;
        assert_eq!(
            type_bindings::unapply(&pool, f.kb, "company").await?,
            1,
            "Beta 已被人认下，留着"
        );
        assert_eq!(
            typed(&pool, f.acme).await?,
            Typed {
                type_id: None,
                type_source: "extracted".into(),
                proposed_type: None,
            }
        );
        assert_eq!(typed(&pool, f.beta).await?.type_id, Some(f.organization));
        assert_eq!(typed(&pool, f.carol).await?.type_id, Some(f.person));
        assert_eq!(type_bindings::unapply(&pool, f.kb, "company").await?, 0);
        // 绑上的类之后有了 → 对齐时把建议清掉
        assert_eq!(
            type_bindings::apply(&pool, f.kb, "stockholder proposal", f.organization).await?,
            1
        );
        assert_eq!(typed(&pool, f.proposal).await?.proposed_type, None);

        // 9. 类删了，绑定跟着走
        assert_eq!(type_bindings::unapply(&pool, f.kb, "person").await?, 1);
        sqlx::query("DELETE FROM entity_types WHERE id = $1")
            .bind(f.person)
            .execute(&pool)
            .await?;
        let b = type_bindings::bindings(&pool, f.kb).await?;
        assert!(b.iter().all(|b| b.kind_word != "person"));
        assert!(type_bindings::stale(&pool, f.kb).await?.is_empty());
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    run
}
