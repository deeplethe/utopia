//! 经 MCP 提的记忆，审核卡上要认得出是**哪一个 agent**（#304 / 0026）。
//!
//! 为什么非要连库：这条身份链整个由 SQL 承担——一列外键、一个 LEFT JOIN、
//! 一个 `AS` 别名。写错列名、join 错表、别名和 `FromRow` 的字段对不上，
//! `cargo check` 一个字都不会说，界面上只会安静地少显示半行字。
//!
//! 两条都要断言，因为它们坏的方式不同：
//! - 带令牌的提议，视图要给出令牌的名字（join 漏了 → 永远是空）
//! - 不带令牌的提议（网页端对话），那一位要是空（join 写成内连接 → 整条不见了）
//!
//! 自建自拆：一次性 org/workspace/kb，跑完连 org 一起删。

use sqlx::PgPool;
use utopia_store::graph::Validity;
use utopia_store::pending::{Outcome, Proposal};
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    /// 令牌记的那一条
    by_agent: Uuid,
    /// 网页端对话记的那一条
    by_person: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (user, token) = (Uuid::now_v7(), Uuid::now_v7());
    let (etype, doc, chunk) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (acme, zenith) = (Uuid::now_v7(), Uuid::now_v7());
    let tag = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'agent-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'agent-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'agent-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    // 邮箱唯一：带一次性后缀，别撞上库里留下的账号
    sqlx::query(
        "INSERT INTO users (id, org_id, email, password_hash, display_name)
         VALUES ($1, $2, $3, 'x', 'Zhang San')",
    )
    .bind(user)
    .bind(org)
    .bind(format!("agent-test-{tag}@utopia.test"))
    .execute(pool)
    .await?;
    // token_hash 也唯一
    sqlx::query(
        "INSERT INTO personal_tokens (id, user_id, name, token_hash, token_prefix, scope)
         VALUES ($1, $2, 'Meeting notes agent', $3, 'utp_pat_test', 'write')",
    )
    .bind(token)
    .bind(user)
    .bind(format!("hash-{tag}"))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'org', 'Organization')",
    )
    .bind(etype)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [(acme, "Acme"), (zenith, "Zenith")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(etype)
        .bind(name)
        .execute(pool)
        .await?;
    }
    // blob 按 sha 跨库共用，sha 也带一次性后缀
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, status)
         VALUES ($1, $2, 'memory-log.md', $3, 'ready')",
    )
    .bind(doc)
    .bind(kb)
    .bind(format!("sha-{tag}"))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text)
         VALUES ($1, $2, $3, 0, 'Acme partnered with Zenith on 2026-03-01.')",
    )
    .bind(chunk)
    .bind(kb)
    .bind(doc)
    .execute(pool)
    .await?;

    let propose = |object_id: Uuid, proposed_token: Option<Uuid>| {
        let p = Proposal {
            kb_id: kb,
            subject_id: acme,
            predicate_id: None,
            object_id: Some(object_id),
            object_value: None,
            proposed_predicate: Some("partnered with"),
            validity: Validity::default(),
            confidence: 0.6,
            chunk_id: chunk,
            proposed_by: Some(user),
            proposed_token,
        };
        async move {
            match utopia_store::pending::propose(pool, p).await? {
                Outcome::Proposed(id) => Ok::<Uuid, anyhow::Error>(id),
                other => anyhow::bail!("提议没有入队：{other:?}"),
            }
        }
    };
    let by_agent = propose(zenith, Some(token)).await?;
    // 同一个 (主语, 谓词, 宾语) 会被判成 AlreadyPending，所以第二条换个宾语
    let by_person = propose(acme, None).await?;

    Ok(Fixture {
        org,
        kb,
        by_agent,
        by_person,
    })
}

#[tokio::test]
async fn a_pending_fact_names_the_agent_that_proposed_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let queue = utopia_store::pending::list(&pool, f.kb, 50, 0).await?;
    let find = |id: Uuid| {
        queue
            .iter()
            .find(|v| v.id == id)
            .expect("待确认项不在队列里")
    };

    // 1. 令牌记的那条：人和 agent 都答得出。人是身份，agent 是「哪一个客户端」——
    //    同一个人挂三个 agent 时，卡片上只有后者分得开
    let agent = find(f.by_agent);
    assert_eq!(agent.proposed_by_name.as_deref(), Some("Zhang San"));
    assert_eq!(
        agent.proposed_token_name.as_deref(),
        Some("Meeting notes agent"),
        "令牌名没跟出来——多半是 VIEW_SELECT 少了那个 join"
    );

    // 2. 网页端对话记的那条：没有 agent，但**这一条本身不能消失**
    //    （join 写成内连接就会整条不见，而那是最难发现的一种丢失）
    let person = find(f.by_person);
    assert_eq!(person.proposed_by_name.as_deref(), Some("Zhang San"));
    assert_eq!(person.proposed_token_name, None);

    // **先删库再删 org。** 删 org 级联到 users，而 `pending_facts.proposed_by`
    // 是不带 ON DELETE 的外键（台账该拦住这种删除）；库不跟着 org 级联，
    // 所以顺序反了就会被外键顶回来
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    let gone = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    assert_eq!(gone.rows_affected(), 1, "一次性 org 没删掉");
    Ok(())
}
