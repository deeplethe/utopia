//! 导出读的是**整本账**（0020 / #308），打在真库上。
//!
//! 界面那些读路径全都过滤 `invalidated_at`——它们答的是「现在是什么样」。
//! 导出如果照抄那套过滤，导出去的就是一张干净自信的图，而三月被撤回的那一半
//! 一个字都不剩。审计要的恰恰是那一半。
//!
//! 所以这里断言的都是「**没有**被过滤掉」：撤回的事实在、删掉的文档在、
//! 本体没接住的原话在、派生的前提在。合并掉的实体是唯一该消失的东西——
//! 它的事实早已搬到留下的那个身上，再导一遍就是把同一条事实说两遍。

use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    live: Uuid,
    retracted: Uuid,
    bare: Uuid,
    kept: Uuid,
    swallowed: Uuid,
    deleted_doc: Uuid,
    derived: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, company) = (Uuid::now_v7(), Uuid::now_v7());
    let (works_for, part_of) = (Uuid::now_v7(), Uuid::now_v7());
    let (kept, swallowed, acme, group) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (doc, deleted_doc, chunk) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (live, retracted, bare) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (rule, derived) = (Uuid::now_v7(), Uuid::now_v7());
    let tag = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'export-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'export-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'export-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label, iri) in [
        (
            person,
            "person",
            "Person",
            Some("https://schema.org/Person"),
        ),
        (company, "company", "Company", None),
    ] {
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label, iri) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .bind(label)
        .bind(iri)
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, iri, functional)
         VALUES ($1, $2, 'works_for', 'works for', 'https://schema.org/worksFor', TRUE)",
    )
    .bind(works_for)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, is_transitive)
         VALUES ($1, $2, 'part_of', 'part of', TRUE)",
    )
    .bind(part_of)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, type_id, name) in [
        (kept, person, "Lin Zhao"),
        (swallowed, person, "L. Zhao"),
        (acme, company, "Acme"),
        (group, company, "Acme Group"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(type_id)
        .bind(name)
        .execute(pool)
        .await?;
    }
    // 合并掉的那个：事实早已搬走，导出里不该再出现
    sqlx::query("UPDATE entities SET merged_into = $2 WHERE id = $1")
        .bind(swallowed)
        .bind(kept)
        .execute(pool)
        .await?;

    for (id, name, deleted) in [(doc, "handbook.md", false), (deleted_doc, "old.md", true)] {
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256, status, external_key, deleted_at)
             VALUES ($1, $2, $3, $4, 'ready', $5,
                     CASE WHEN $6 THEN now() ELSE NULL END)",
        )
        .bind(id)
        .bind(kb)
        .bind(name)
        .bind(format!("sha-{tag}-{name}"))
        .bind(format!("file:///{name}"))
        .bind(deleted)
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text)
         VALUES ($1, $2, $3, 0, 'Lin Zhao works for Acme.')",
    )
    .bind(chunk)
    .bind(kb)
    .bind(doc)
    .execute(pool)
    .await?;

    let fact = |id: Uuid, predicate: Option<Uuid>, object: Uuid, invalidated: bool| {
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence,
                                invalidated_at)
             VALUES ($1, $2, $3, $4, $5, 0.9,
                     CASE WHEN $6 THEN now() ELSE NULL END)",
        )
        .bind(id)
        .bind(kb)
        .bind(kept)
        .bind(predicate)
        .bind(object)
        .bind(invalidated)
    };
    fact(live, Some(works_for), acme, false)
        .execute(pool)
        .await?;
    fact(retracted, Some(works_for), group, true)
        .execute(pool)
        .await?;
    // 本体没接住的那条（0010）：谓词为空，原话在证据里
    fact(bare, None, group, false).execute(pool).await?;
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, proposed_predicate)
         VALUES ($1, $2, 'Lin Zhao works for Acme.', $3, NULL),
                ($4, $2, 'Lin Zhao advises Acme Group.', $3, 'advises')",
    )
    .bind(live)
    .bind(chunk)
    .bind(doc)
    .bind(bare)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, 'transitive')",
    )
    .bind(rule)
    .bind(kb)
    .bind(part_of)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, rule_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(derived)
    .bind(kb)
    .bind(acme)
    .bind(part_of)
    .bind(group)
    .bind(rule)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO fact_derivations (derived_fact_id, premise_fact_id, seq) VALUES ($1, $2, 0)",
    )
    .bind(derived)
    .bind(live)
    .execute(pool)
    .await?;

    Ok(Fixture {
        org,
        kb,
        live,
        retracted,
        bare,
        kept,
        swallowed,
        deleted_doc,
        derived,
    })
}

#[tokio::test]
async fn an_export_reads_the_whole_ledger_not_the_current_view() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    // 1. 事实：撤回的那条**在**。界面把它藏起来是对的，导出把它藏起来就是骗人
    let facts = utopia_store::export::facts_page(&pool, f.kb, None).await?;
    let ids: Vec<Uuid> = facts.iter().map(|x| x.id).collect();
    assert!(ids.contains(&f.live));
    assert!(
        ids.contains(&f.retracted),
        "撤回的事实必须导出——它带着 invalidated_at，读的人自己判断"
    );
    let retracted = facts.iter().find(|x| x.id == f.retracted).unwrap();
    assert!(retracted.invalidated_at.is_some());

    // 2. 证据跟着事实走：出处文档与原句都在同一行上
    let live = facts.iter().find(|x| x.id == f.live).unwrap();
    assert_eq!(live.documents.len(), 1);
    assert_eq!(live.quotes, vec!["Lin Zhao works for Acme."]);

    // 3. 本体没接住的那条：谓词为空，但模型的原话拿得到——导出里它不会
    //    被铸成一个假谓词，也不会消失
    let bare = facts.iter().find(|x| x.id == f.bare).unwrap();
    assert!(bare.predicate_id.is_none());
    assert_eq!(bare.surface_predicate.as_deref(), Some("advises"));

    // 4. 实体：合并掉的那个是唯一该消失的东西
    let entities = utopia_store::export::entities_page(&pool, f.kb, None).await?;
    let ids: Vec<Uuid> = entities.iter().map(|e| e.id).collect();
    assert!(ids.contains(&f.kept));
    assert!(
        !ids.contains(&f.swallowed),
        "合并掉的实体再导一遍，等于把同一条事实说两遍"
    );

    // 5. 文档：删掉的留着墓碑（#268）。抹掉出处等于抹掉证据链
    let docs = utopia_store::export::documents_page(&pool, f.kb, None).await?;
    let deleted = docs.iter().find(|d| d.id == f.deleted_doc).unwrap();
    assert!(deleted.deleted_at.is_some());

    // 6. 派生：带着规则和前提，审计顺着它走得到断言
    let derived = utopia_store::export::derived_page(&pool, f.kb, None).await?;
    let d = derived.iter().find(|d| d.id == f.derived).unwrap();
    assert_eq!(d.rule, "transitive");
    assert_eq!(d.premises, vec![f.live]);

    // 7. 词汇表：导入来的类留着原 IRI，公理位照抄
    let classes = utopia_store::export::classes(&pool, f.kb).await?;
    let person = classes.iter().find(|c| c.key == "person").unwrap();
    assert_eq!(person.iri.as_deref(), Some("https://schema.org/Person"));
    let relations = utopia_store::export::relations(&pool, f.kb).await?;
    assert!(relations
        .iter()
        .any(|r| r.key == "works_for" && r.functional));
    assert!(relations
        .iter()
        .any(|r| r.key == "part_of" && r.is_transitive));

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
