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

/// 自己的组织名：上面那个测试开跑时按名字清场，共用名字会互相删掉对方的数据
const BASIS_ORG: &str = "kind-word-basis-test";

/// 代理的判定只在它读到的输入还成立时才收（#795）；人的判定不带指纹，代理不覆盖；
/// 人定的判定与它的短语对齐任务同一事务提交
#[tokio::test]
async fn an_agent_decision_is_accepted_only_for_the_inputs_it_read() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(BASIS_ORG)
        .execute(&pool)
        .await?;
    let (org, ws, kb, class, acme) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(BASIS_ORG)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(BASIS_ORG)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(BASIS_ORG)
        .execute(&pool)
        .await?;

    let run = async {
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label, description)
             VALUES ($1, $2, 'organization', 'Organization', 'OLD definition')",
        )
        .bind(class)
        .bind(kb)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO entities (id, kb_id, canonical_name, specific_type)
             VALUES ($1, $2, 'Acme', 'company')",
        )
        .bind(acme)
        .bind(kb)
        .execute(&pool)
        .await?;
        let shown = [class];
        let before = type_bindings::class_snapshot(&pool, kb)
            .await?
            .basis(&shown);

        // 模型答题期间定义改了：这份回复答的是旧定义，不收，什么都不写
        sqlx::query(
            "UPDATE entity_types SET description = 'NEW definition', updated_at = clock_timestamp()
              WHERE id = $1",
        )
        .bind(class)
        .execute(&pool)
        .await?;
        let votes = serde_json::json!({ "first": "organization", "second": "organization" });
        assert_eq!(
            type_bindings::decide_and_apply_if_current(
                &pool,
                kb,
                "company",
                &[],
                Some(class),
                "bound",
                &votes,
                &before,
                &shown,
            )
            .await?,
            type_bindings::Acceptance::Moved
        );
        assert!(type_bindings::bindings(&pool, kb).await?.is_empty());
        assert_eq!(typed(&pool, acme).await?.type_id, None);

        // 按新输入问出来的：收下，指纹一起存
        let current = type_bindings::class_snapshot(&pool, kb)
            .await?
            .basis(&shown);
        assert_ne!(before, current);
        assert_eq!(
            type_bindings::decide_and_apply_if_current(
                &pool,
                kb,
                "company",
                &[],
                Some(class),
                "bound",
                &votes,
                &current,
                &shown,
            )
            .await?,
            type_bindings::Acceptance::Written
        );
        let b = type_bindings::bindings(&pool, kb).await?.remove(0);
        assert_eq!(b.basis.as_deref(), Some(current.as_str()));
        assert_eq!(typed(&pool, acme).await?.type_id, Some(class));

        // 人判的：不带指纹；它的短语对齐任务与判定一起提交
        assert!(type_bindings::decide_and_apply_human(&pool, kb, "company", None, &votes).await?);
        let b = type_bindings::bindings(&pool, kb).await?.remove(0);
        assert_eq!((b.decided_by.as_str(), b.basis), ("person", None));
        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs
              WHERE kind = 'align_phrases' AND payload->>'kb_id' = $1 AND status = 'queued'",
        )
        .bind(kb.to_string())
        .fetch_one(&pool)
        .await?;
        assert_eq!(queued, 1);

        // 代理晚到的回复不覆盖人
        assert_eq!(
            type_bindings::decide_and_apply_if_current(
                &pool,
                kb,
                "company",
                &[],
                Some(class),
                "bound",
                &votes,
                &current,
                &shown,
            )
            .await?,
            type_bindings::Acceptance::KeptPerson
        );
        assert_eq!(
            type_bindings::bindings(&pool, kb)
                .await?
                .remove(0)
                .decided_by,
            "person"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id' = $1")
        .bind(kb.to_string())
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(BASIS_ORG)
        .execute(&pool)
        .await?;
    run
}

/// 锁顺序的两个测试用：一个类、一个带类别词的实体、一行绑在这个类上的代理判定。
/// 判定不投影——`entities.type_id` 删类时是 RESTRICT，实体一指着这个类，删类本身就会失败，
/// 测的就不是锁了。返回 (库, 类)
async fn seed_lock_order(pool: &PgPool, name: &str) -> anyhow::Result<(Uuid, Uuid)> {
    let (org, ws, kb, class) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(name)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(name)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(name)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'organization', 'Organization')",
    )
    .bind(class)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entities (id, kb_id, canonical_name, specific_type)
         VALUES ($1, $2, 'Acme', 'company')",
    )
    .bind(Uuid::now_v7())
    .bind(kb)
    .execute(pool)
    .await?;
    type_bindings::decide(
        pool,
        kb,
        "company",
        &[],
        Some(class),
        "bound",
        &serde_json::json!({}),
        "agent",
    )
    .await?;
    Ok((kb, class))
}

/// 等到有人排在 `pid` 后面等锁。`chain` 时等的是更长的一串：有人排在一个正排在 `pid`
/// 后面的人后面
async fn wait_behind(pool: &PgPool, pid: i32, chain: bool) -> anyhow::Result<()> {
    let sql = if chain {
        "SELECT EXISTS (SELECT 1 FROM pg_stat_activity p
                         WHERE EXISTS (SELECT 1 FROM unnest(pg_blocking_pids(p.pid)) b(pid)
                                        WHERE $1 = ANY(pg_blocking_pids(b.pid))))"
    } else {
        "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))"
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(sql).bind(pid).fetch_one(pool).await?;
            if waiting {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

/// 代理收判定时先锁候选类、再写判定行，与删类的顺序一致（删类先拿类的行，再级联到判定行）。
/// 反过来就是死锁：代理拿着判定行等类，删类拿着类等判定行。这里让代理停在两步之间，
/// 删类排到它后面，再放行：两边都得走完，谁也不报 40P01
#[tokio::test]
async fn an_acceptance_and_a_class_delete_wait_for_each_other_instead_of_deadlocking(
) -> anyhow::Result<()> {
    const NAME: &str = "kind-word-lock-order-test";
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(NAME)
        .execute(&pool)
        .await?;
    let (kb, class) = seed_lock_order(&pool, NAME).await?;

    let run = async {
        let shown = [class];
        let basis = type_bindings::class_snapshot(&pool, kb)
            .await?
            .basis(&shown);
        // 闸门拿着判定行：代理锁完候选类、算完指纹，停在写判定这一步
        let mut gate = pool.begin().await?;
        let gate_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *gate)
            .await?;
        sqlx::query("SELECT id FROM type_bindings WHERE kb_id = $1 FOR UPDATE")
            .bind(kb)
            .fetch_one(&mut *gate)
            .await?;
        let agent = {
            let pool = pool.clone();
            tokio::spawn(async move {
                type_bindings::decide_and_apply_if_current(
                    &pool,
                    kb,
                    "company",
                    &[],
                    None,
                    "undecided",
                    &serde_json::json!({}),
                    &basis,
                    &shown,
                )
                .await
            })
        };
        wait_behind(&pool, gate_pid, false).await?;
        let deleter = {
            let pool = pool.clone();
            tokio::spawn(async move {
                sqlx::query("DELETE FROM entity_types WHERE id = $1")
                    .bind(class)
                    .execute(&pool)
                    .await
            })
        };
        // 删类排在代理后面（代理拿着类的共享锁），代理排在闸门后面
        wait_behind(&pool, gate_pid, true).await?;
        gate.rollback().await?;
        let accepted = tokio::time::timeout(Duration::from_secs(10), agent).await???;
        tokio::time::timeout(Duration::from_secs(10), deleter).await???;
        assert_eq!(accepted, type_bindings::Acceptance::Written);
        let b = type_bindings::bindings(&pool, kb).await?;
        assert_eq!(
            b.len(),
            1,
            "the undecided row no longer points at the class"
        );
        assert_eq!((b[0].status.as_str(), b[0].type_id), ("undecided", None));
        let classes: i64 = sqlx::query_scalar("SELECT count(*) FROM entity_types WHERE kb_id = $1")
            .bind(kb)
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            classes, 0,
            "the delete went through after the agent committed"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(NAME)
        .execute(&pool)
        .await?;
    run
}

/// 候选类在收判定之前被删了：代理等删类提交，读到的类已经不在，指纹对不上，这份回复
/// 不收；判定行随级联走了，代理也不把它写回来
#[tokio::test]
async fn a_candidate_deleted_while_the_reply_waits_moves_it() -> anyhow::Result<()> {
    const NAME: &str = "kind-word-deleted-candidate-test";
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(NAME)
        .execute(&pool)
        .await?;
    let (kb, class) = seed_lock_order(&pool, NAME).await?;

    let run = async {
        let shown = [class];
        let basis = type_bindings::class_snapshot(&pool, kb)
            .await?
            .basis(&shown);
        let mut deleter = pool.begin().await?;
        let deleter_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *deleter)
            .await?;
        sqlx::query("DELETE FROM entity_types WHERE id = $1")
            .bind(class)
            .execute(&mut *deleter)
            .await?;
        let agent = {
            let pool = pool.clone();
            tokio::spawn(async move {
                type_bindings::decide_and_apply_if_current(
                    &pool,
                    kb,
                    "company",
                    &[],
                    None,
                    "none",
                    &serde_json::json!({}),
                    &basis,
                    &shown,
                )
                .await
            })
        };
        wait_behind(&pool, deleter_pid, false).await?;
        deleter.commit().await?;
        let accepted = tokio::time::timeout(Duration::from_secs(10), agent).await???;
        assert_eq!(accepted, type_bindings::Acceptance::Moved);
        assert!(
            type_bindings::bindings(&pool, kb).await?.is_empty(),
            "the cascade took the row and the moved reply did not write it back"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(NAME)
        .execute(&pool)
        .await?;
    run
}
