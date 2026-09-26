//! 写入侧：导出会触碰的每条边都被 0070 的外键/触发器挡住；导出侧体检
//! 覆盖 0070 保护的**全部**结构引用边——别库/悬空的行宁可整份拒导，也不许
//! 伪造 IRI 或静默丢语义。落到别库的下场分两种：
//!   - 铸成本库 IRI 的引用（实体、事实、派生、文档、段落、规则）→ 伪造身份；
//!   - 进本库词汇表按 id 查的引用（谓词、属性类型、实体类型、父类）→ 静默消失。
//!
//! 两种都是坏行，触发器与导出侧一律拒。
//!
//! 坏行靠 `SET LOCAL session_replication_role='replica'` 造——只关本事务的
//! 触发器（含 FK 强制），提交即恢复；这同时允许造出「指着的行已不在」的
//! 悬空引用，正是从 0070 之前的备份恢复进来的形态。

use sqlx::{Acquire, PgPool};
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    a: Uuid,
    b: Uuid,
    doc_a: Uuid,
    doc_b: Uuid,
    chunk_a: Uuid,
    chunk_b: Uuid,
    ent_a: Uuid,
    ent_b: Uuid,
    fact_a: Uuid,
    fact_b: Uuid,
    rel_a: Uuid,
    rel_b: Uuid,
    cls_a: Uuid,
    cls_b: Uuid,
    rule_a: Uuid,
    rule_b: Uuid,
    arule_a: Uuid,
    arule_b: Uuid,
    der_a: Uuid,
    der_b: Uuid,
    irule_a: Uuid,
    irule_b: Uuid,
    arv_a: Uuid,
    arv_b: Uuid,
    erun_a: Uuid,
    erun_b: Uuid,
    eact_a: Uuid,
}

/// 两库，每库一套能被引用的零件：文档/段落/实体/事实/谓词/类/公理规则/业务规则/派生
async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, a, b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (doc_a, doc_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (chunk_a, chunk_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (ent_a, ent_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (fact_a, fact_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (rel_a, rel_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (cls_a, cls_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (rule_a, rule_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (arule_a, arule_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (der_a, der_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (irule_a, irule_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (arv_a, arv_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (erun_a, erun_b) = (Uuid::now_v7(), Uuid::now_v7());
    let eact_a = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'xkb-ref-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'xkb-ref-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for kb in [a, b] {
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'xkb-ref-test')",
        )
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, status, external_key)
         VALUES ($1, $2, 'a.md', 'sha-a', 'ready', 'file:///a.md')",
    )
    .bind(doc_a)
    .bind(a)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, status, external_key)
         VALUES ($1, $2, 'b.md', 'sha-b', 'ready', 'file:///b.md')",
    )
    .bind(doc_b)
    .bind(b)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, 'x')",
    )
    .bind(chunk_a)
    .bind(a)
    .bind(doc_a)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, 'x')",
    )
    .bind(chunk_b)
    .bind(b)
    .bind(doc_b)
    .execute(pool)
    .await?;
    for (id, kb) in [(ent_a, a), (ent_b, b)] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'e')")
            .bind(id)
            .bind(kb)
            .execute(pool)
            .await?;
    }
    for (id, kb, s, o) in [(fact_a, a, ent_a, ent_a), (fact_b, b, ent_b, ent_b)] {
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence)
             VALUES ($1, $2, $3, $4, 0.9)",
        )
        .bind(id)
        .bind(kb)
        .bind(s)
        .bind(o)
        .execute(pool)
        .await?;
    }
    for (id, kb) in [(rel_a, a), (rel_b, b)] {
        sqlx::query("INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, $3, 'r')")
            .bind(id)
            .bind(kb)
            .bind(format!("r-{id}"))
            .execute(pool)
            .await?;
    }
    for (id, kb) in [(cls_a, a), (cls_b, b)] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, 'c')")
            .bind(id)
            .bind(kb)
            .bind(format!("c-{id}"))
            .execute(pool)
            .await?;
    }
    for (id, kb, pred) in [(rule_a, a, rel_a), (rule_b, b, rel_b)] {
        sqlx::query(
            "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, 'transitive')",
        )
        .bind(id)
        .bind(kb)
        .bind(pred)
        .execute(pool)
        .await?;
    }
    for (id, kb, ty, pred) in [(arule_a, a, cls_a, rel_a), (arule_b, b, cls_b, rel_b)] {
        sqlx::query(
            "INSERT INTO attribute_rules (id, kb_id, name, subject_type_id, conclusion,
                                         conclude_predicate_id, conclude_value)
             VALUES ($1, $2, $3, $4, 'attribute', $5, '42'::jsonb)",
        )
        .bind(id)
        .bind(kb)
        .bind(format!("ar-{id}"))
        .bind(ty)
        .bind(pred)
        .execute(pool)
        .await?;
    }
    for (id, kb, s, p, o, r) in [
        (der_a, a, ent_a, rel_a, ent_a, rule_a),
        (der_b, b, ent_b, rel_b, ent_b, rule_b),
    ] {
        sqlx::query(
            "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, rule_id)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(kb)
        .bind(s)
        .bind(p)
        .bind(o)
        .bind(r)
        .execute(pool)
        .await?;
    }
    // 0073 之后进库的同库家族成员:蕴含规则、读数缓存、勘误账、定义史——
    // 每库一套,供越库断言取用
    for (id, kb, pred) in [(irule_a, a, rel_a), (irule_b, b, rel_b)] {
        sqlx::query(
            "INSERT INTO implication_rules (id, kb_id, trigger, phrase,
                                           conclude_property_id, status)
             VALUES ($1, $2, 'phrase', 'p', $3, 'approved')",
        )
        .bind(id)
        .bind(kb)
        .bind(pred)
        .execute(pool)
        .await?;
    }
    for (id, kb, rule) in [(arv_a, a, arule_a), (arv_b, b, arule_b)] {
        sqlx::query(
            "INSERT INTO attribute_rule_versions (id, kb_id, rule_id, seq, definition)
             VALUES ($1, $2, $3, 1, '{}'::jsonb)",
        )
        .bind(id)
        .bind(kb)
        .bind(rule)
        .execute(pool)
        .await?;
    }
    for (id, kb, doc) in [(erun_a, a, doc_a), (erun_b, b, doc_b)] {
        sqlx::query("INSERT INTO errata_runs (id, kb_id, document_id) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(kb)
            .bind(doc)
            .execute(pool)
            .await?;
    }
    for (id, kb, run, doc) in [(eact_a, a, erun_a, doc_a)] {
        sqlx::query(
            "INSERT INTO errata_actions (id, kb_id, run_id, document_id, action, status)
             VALUES ($1, $2, $3, $4, 'keep', 'held')",
        )
        .bind(id)
        .bind(kb)
        .bind(run)
        .bind(doc)
        .execute(pool)
        .await?;
    }
    // 读数缓存没有 id 列——键是 (kb_id, reading, phrase)
    for (kb, ent) in [(a, ent_a), (b, ent_b)] {
        sqlx::query(
            "INSERT INTO phrase_readings (kb_id, reading, phrase, entity_id)
             VALUES ($1, 'nationality', 'r', $2)",
        )
        .bind(kb)
        .bind(ent)
        .execute(pool)
        .await?;
    }
    Ok(Fixture {
        org,
        a,
        b,
        doc_a,
        doc_b,
        chunk_a,
        chunk_b,
        ent_a,
        ent_b,
        fact_a,
        fact_b,
        rel_a,
        rel_b,
        cls_a,
        cls_b,
        rule_a,
        rule_b,
        arule_a,
        arule_b,
        der_a,
        der_b,
        irule_a,
        irule_b,
        arv_a,
        arv_b,
        erun_a,
        erun_b,
        eact_a,
    })
}

async fn cleanup(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
    for kb in [f.a, f.b] {
        sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
            .bind(kb)
            .execute(pool)
            .await?;
    }
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

/// 新的跨库写：每条边都被它自己的触发器当场挡下
#[tokio::test]
async fn new_cross_kb_writes_are_rejected_on_every_exported_edge() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // —— 派生的前提：两种前提，两种越库形态
    for (premise_fact, premise_derived, what) in [
        (Some(f.fact_b), None, "foreign fact premise"),
        (None, Some(f.der_b), "foreign derived premise"),
    ] {
        let err = sqlx::query(
            "INSERT INTO fact_derivations (derived_fact_id, premise_fact_id, premise_derived_id, seq)
             VALUES ($1, $2, $3, 0)",
        )
        .bind(f.der_a)
        .bind(premise_fact)
        .bind(premise_derived)
        .execute(&pool)
        .await;
        assert!(err.is_err(), "derivation premise: {what} 必须被拒");
    }

    // —— 边上的属性：别库类型会被词汇表静默跳过，别库实体会被铸进本库 IRI
    let err = sqlx::query(
        "INSERT INTO fact_qualifiers (fact_id, qualifier_type_id, value)
         VALUES ($1, $2, '\"lit\"'::jsonb)",
    )
    .bind(f.fact_a)
    .bind(f.rel_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "qualifier.type→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO fact_qualifiers (fact_id, qualifier_type_id, entity_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact_a)
    .bind(f.rel_a)
    .bind(f.ent_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "qualifier.entity→foreign 必须被拒");

    // —— 事实本体的五个引用列（from_statement_id 是同表自指：immediate 端在
    //    BEFORE 触发器上，目标早已在场时当场拒）
    for (col, foreign_id, what) in [
        ("subject_id", f.ent_b, "fact.subject"),
        ("object_id", f.ent_b, "fact.object"),
        ("predicate_id", f.rel_b, "fact.predicate"),
        ("supersedes", f.fact_b, "fact.supersedes"),
        ("from_statement_id", f.fact_b, "fact.from_statement"),
    ] {
        let err = sqlx::query(&format!(
            "INSERT INTO facts (id, kb_id, subject_id, {col}) VALUES ($1, $2, $3, $4)"
        ))
        .bind(Uuid::now_v7())
        .bind(f.a)
        .bind(if col == "subject_id" {
            f.ent_b
        } else {
            f.ent_a
        })
        .bind(foreign_id)
        .execute(&pool)
        .await;
        assert!(err.is_err(), "{what}→foreign 必须被拒");
    }

    // —— 陈述来源（typed_fact_sources，0068）：行自己没有 kb 列，归属按所属
    //    fact 的库判——A 的事实吃 B 的陈述，序列化会铸出指着别库陈述的边
    let err = sqlx::query("INSERT INTO typed_fact_sources (fact_id, statement_id) VALUES ($1, $2)")
        .bind(f.fact_a)
        .bind(f.fact_b)
        .execute(&pool)
        .await;
    assert!(err.is_err(), "factsource.statement→foreign 必须被拒");

    // —— 开放陈述的属性（statement_qualifiers，0061）：行没有 kb 列，归属按
    //    所属 fact 的库判；别库的实体值会被铸进本库 IRI
    let err = sqlx::query(
        "INSERT INTO statement_qualifiers (fact_id, role, entity_id)
         VALUES ($1, 'as', $2)",
    )
    .bind(f.fact_a)
    .bind(f.ent_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "squalifier.entity→foreign 必须被拒");

    // —— 时间提及（0061/0064）：提及自己的 kb 必须与它指的事实、段落同属一库
    let err = sqlx::query(
        "INSERT INTO time_mentions (id, kb_id, fact_id, chunk_id, text, char_start)
         VALUES ($1, $2, $3, $4, '去年', 0)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.fact_b) // 事实在别库
    .bind(f.chunk_a)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "timemention.fact→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO time_mentions (id, kb_id, fact_id, chunk_id, text, char_start)
         VALUES ($1, $2, $3, $4, '去年', 0)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.fact_a)
    .bind(f.chunk_b) // 段落在别库
    .execute(&pool)
    .await;
    assert!(err.is_err(), "timemention.chunk→foreign 必须被拒");

    // —— 类型与短语绑定（0065/0066）：绑定的类/属性以绑定行自己的库为准
    let err = sqlx::query(
        "INSERT INTO type_bindings (id, kb_id, kind_word, status, type_id)
         VALUES ($1, $2, 'corp', 'bound', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "binding.type→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO phrase_bindings (id, kb_id, phrase, subject_type_id, status)
         VALUES ($1, $2, 'runs', $3, 'none')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "pbinding.subject_type→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO phrase_bindings (id, kb_id, phrase, relation_type_id,
                                     direction, status)
         VALUES ($1, $2, 'runs', $3, 'forward', 'bound')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.rel_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "pbinding.relation→foreign 必须被拒");

    // —— 派生事实本体的五个引用列
    for (col, foreign_id, what) in [
        ("subject_id", f.ent_b, "derived.subject"),
        ("object_id", f.ent_b, "derived.object"),
        ("predicate_id", f.rel_b, "derived.predicate"),
        ("rule_id", f.rule_b, "derived.rule"),
        ("attribute_rule_id", f.arule_b, "derived.attribute_rule"),
    ] {
        // attribute_rule 与 rule 互斥：测 attribute_rule 那行不带 rule_id
        let err = if col == "attribute_rule_id" {
            sqlx::query(
                "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, attribute_rule_id)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(Uuid::now_v7())
            .bind(f.a)
            .bind(f.ent_a)
            .bind(f.rel_a)
            .bind(foreign_id)
            .execute(&pool)
            .await
        } else {
            sqlx::query(&format!(
                "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, rule_id, {col})
                 VALUES ($1, $2, $3, $4, $5, $6)"
            ))
            .bind(Uuid::now_v7())
            .bind(f.a)
            .bind(if col == "subject_id" {
                f.ent_b
            } else {
                f.ent_a
            })
            .bind(if col == "predicate_id" {
                f.rel_b
            } else {
                f.rel_a
            })
            .bind(if col == "rule_id" { f.rule_b } else { f.rule_a })
            .bind(foreign_id)
            .execute(&pool)
            .await
        };
        assert!(err.is_err(), "{what}→foreign 必须被拒");
    }

    // —— 实体的类、类层级、互斥、domain/range
    let err = sqlx::query(
        "INSERT INTO entities (id, kb_id, canonical_name, type_id) VALUES ($1, $2, 'e', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "entity.type→foreign 必须被拒");

    let err = sqlx::query("INSERT INTO entity_type_parents (child_id, parent_id) VALUES ($1, $2)")
        .bind(f.cls_a)
        .bind(f.cls_b)
        .execute(&pool)
        .await;
    assert!(err.is_err(), "class.parent→foreign 必须被拒");

    let err =
        sqlx::query("INSERT INTO entity_type_disjoint (kb_id, a_id, b_id) VALUES ($1, $2, $3)")
            .bind(f.a)
            .bind(f.cls_a)
            .bind(f.cls_b)
            .execute(&pool)
            .await;
    assert!(err.is_err(), "class.disjoint→foreign 必须被拒");

    for (table, what) in [
        ("relation_type_domains", "relation.domain"),
        ("relation_type_ranges", "relation.range"),
    ] {
        let err = sqlx::query(&format!(
            "INSERT INTO {table} (relation_type_id, entity_type_id) VALUES ($1, $2)"
        ))
        .bind(f.rel_a)
        .bind(f.cls_b)
        .execute(&pool)
        .await;
        assert!(err.is_err(), "{what}→foreign 必须被拒");
    }

    // —— 关系的同表自指与边属性声明：owl:inverseOf /
    //    rdfs:subPropertyOf 与 qualifier 列表都是语义，别库的不许进来
    let err = sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, inverse_of) VALUES ($1, $2, 'inv', 'inv', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.rel_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "relation.inverse→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, sub_property_of) VALUES ($1, $2, 'sub', 'sub', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.rel_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "relation.sub_property→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO relation_type_qualifiers (relation_type_id, qualifier_type_id) VALUES ($1, $2)",
    )
    .bind(f.rel_a)
    .bind(f.rel_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "relation.qualifier→foreign 必须被拒");

    // —— 规则本体：公理编在哪个谓词上、业务规则看什么类得什么结论，
    //    现在都在导出里——它们的引用同样不许跨库
    let err = sqlx::query(
        "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, 'transitive')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.rel_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "rule.predicate→foreign 必须被拒");
    // 每条结论形状自己的必填列（CHECK 钉死了 typing/attribute/computed 的列组），
    // 一个引用列一种合法形状
    let err = sqlx::query(
        "INSERT INTO attribute_rules (id, kb_id, name, subject_type_id, conclusion,
                                      conclude_predicate_id, conclude_value)
         VALUES ($1, $2, 'ar', $3, 'attribute', $4, '42'::jsonb)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_b) // subject_type 别库
    .bind(f.rel_a)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "arule.subject_type→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO attribute_rules (id, kb_id, name, subject_type_id, conclusion,
                                      conclude_type_id)
         VALUES ($1, $2, 'ar', $3, 'typing', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_a)
    .bind(f.cls_b) // conclude_type 别库
    .execute(&pool)
    .await;
    assert!(err.is_err(), "arule.conclude_type→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO attribute_rules (id, kb_id, name, subject_type_id, conclusion,
                                      conclude_predicate_id, conclude_value)
         VALUES ($1, $2, 'ar', $3, 'attribute', $4, '42'::jsonb)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_a)
    .bind(f.rel_b) // conclude_predicate 别库
    .execute(&pool)
    .await;
    assert!(err.is_err(), "arule.conclude_predicate→foreign 必须被拒");

    // —— #901:0070 之后进库的表也归同一条不变量 ——
    // 蕴含规则本体的三个引用列（复合外键）
    for (col, foreign_id, what) in [
        ("subject_type_id", f.cls_b, "irule.subject_type"),
        ("object_type_id", f.cls_b, "irule.object_type"),
    ] {
        let err = sqlx::query(&format!(
            "INSERT INTO implication_rules (id, kb_id, trigger, phrase,
                                           conclude_property_id, status, {col})
             VALUES ($1, $2, 'phrase', 'p', $3, 'approved', $4)",
        ))
        .bind(Uuid::now_v7())
        .bind(f.a)
        .bind(f.rel_a)
        .bind(foreign_id)
        .execute(&pool)
        .await;
        assert!(err.is_err(), "{what}→foreign 必须被拒");
    }
    let err = sqlx::query(
        "INSERT INTO implication_rules (id, kb_id, trigger, phrase,
                                       conclude_property_id, status)
         VALUES ($1, $2, 'phrase', 'p', $3, 'approved')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.rel_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "irule.conclude_property→foreign 必须被拒");

    // 读数缓存答出的实体（复合外键）；UPDATE 路径同样拦
    let err = sqlx::query(
        "INSERT INTO phrase_readings (kb_id, reading, phrase, entity_id)
         VALUES ($1, 'nationality', 'x', $2)",
    )
    .bind(f.a)
    .bind(f.ent_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "reading.entity→foreign 必须被拒");
    let err =
        sqlx::query("UPDATE phrase_readings SET entity_id = $1 WHERE kb_id = $2 AND phrase = 'r'")
            .bind(f.ent_b)
            .bind(f.a)
            .execute(&pool)
            .await;
    assert!(err.is_err(), "reading.entity 改成别库也必须被拒");

    // 隐含事实的出处（owner-derived 触发器）：规则、触发的陈述、触发的实体
    // 都按所属 fact 的库判；CHECK 要求 statement/entity 恰好一个非空
    let err = sqlx::query(
        "INSERT INTO implied_fact_sources (fact_id, rule_id, statement_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact_a)
    .bind(f.irule_b)
    .bind(f.fact_a)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "implied.rule→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO implied_fact_sources (fact_id, rule_id, statement_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact_a)
    .bind(f.irule_a)
    .bind(f.fact_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "implied.statement→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO implied_fact_sources (fact_id, rule_id, entity_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact_a)
    .bind(f.irule_a)
    .bind(f.ent_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "implied.entity→foreign 必须被拒");
    // UPDATE 路径：先落合法行，再把 rule 改成别库的——UPDATE OF 盯得住
    sqlx::query(
        "INSERT INTO implied_fact_sources (fact_id, rule_id, entity_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact_b)
    .bind(f.irule_b)
    .bind(f.ent_b)
    .execute(&pool)
    .await?;
    let err = sqlx::query("UPDATE implied_fact_sources SET rule_id = $1 WHERE fact_id = $2")
        .bind(f.irule_a)
        .bind(f.fact_b)
        .execute(&pool)
        .await;
    assert!(err.is_err(), "implied.rule 改成别库也必须被拒");

    // 勘误账：run 盯的文档、action 上的六个引用列全是复合外键
    let err = sqlx::query("INSERT INTO errata_runs (id, kb_id, document_id) VALUES ($1, $2, $3)")
        .bind(Uuid::now_v7())
        .bind(f.a)
        .bind(f.doc_b)
        .execute(&pool)
        .await;
    assert!(err.is_err(), "erratarun.document→foreign 必须被拒");
    for (col, foreign_id, what) in [
        ("fact_id", f.fact_b, "errata.fact"),
        ("statement_id", f.fact_b, "errata.statement"),
        ("predicate_id", f.rel_b, "errata.predicate"),
        ("new_fact_id", f.fact_b, "errata.new_fact"),
    ] {
        let err = sqlx::query(&format!(
            "INSERT INTO errata_actions (id, kb_id, run_id, document_id, action, status, {col})
             VALUES ($1, $2, $3, $4, 'keep', 'held', $5)",
        ))
        .bind(Uuid::now_v7())
        .bind(f.a)
        .bind(f.erun_a)
        .bind(f.doc_a)
        .bind(foreign_id)
        .execute(&pool)
        .await;
        assert!(err.is_err(), "{what}→foreign 必须被拒");
    }
    // run/document 两列走 UPDATE 路径验:把已有 action 挪到别库的
    // run 或文档上,复合外键照样当场拒
    for (col, foreign_id, what) in [
        ("run_id", f.erun_b, "errata.run"),
        ("document_id", f.doc_b, "errata.document"),
    ] {
        let err = sqlx::query(&format!(
            "UPDATE errata_actions SET {col} = $1 WHERE id = $2",
        ))
        .bind(foreign_id)
        .bind(f.eact_a)
        .execute(&pool)
        .await;
        assert!(err.is_err(), "{what} 改成别库也必须被拒");
    }

    // 规则定义史与凭它推出的派生（#913 的表也在面内）
    // superseded 的版本不占「每规则一条 current」的唯一索引——让行只剩
    // 跨库这一个毛病,拒它的才是那条复合外键
    let err = sqlx::query(
        "INSERT INTO attribute_rule_versions (id, kb_id, rule_id, seq, definition,
                                              superseded_at)
         VALUES ($1, $2, $3, 9, '{}'::jsonb, now())",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.arule_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "arversion.rule→foreign 必须被拒");
    // 派生的「凭哪一版定义推出」不许指别库(identity 键不含规则列,
    // object_value 换个值躲开与已落派生的身份撞车)
    let err = sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_value,
                                    attribute_rule_id, attribute_rule_version_id)
         VALUES ($1, $2, $3, $4, '\"v\"'::jsonb, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.ent_a)
    .bind(f.rel_a)
    .bind(f.arule_a)
    .bind(f.arv_b)
    .execute(&pool)
    .await;
    assert!(
        err.is_err(),
        "derived.attribute_rule_version→foreign 必须被拒"
    );

    // 0071/0080 出生就带复合键、却一直不在体检里的两条边
    let err = sqlx::query(
        "INSERT INTO attribute_rules (id, kb_id, name, subject_type_id, conclusion,
                                      conclude_predicate_id, join_predicate_id)
         VALUES ($1, $2, 'ar-r', $3, 'relation', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_a)
    .bind(f.rel_a)
    .bind(f.rel_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "arule.join_predicate→foreign 必须被拒");
    let err = sqlx::query(
        "INSERT INTO name_vectors (fact_id, kb_id, entity_id, embedding)
         VALUES ($1, $2, $3, '[0]'::vector)",
    )
    .bind(f.fact_b)
    .bind(f.a)
    .bind(f.ent_a)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "namevector.fact→foreign 必须被拒");

    // —— kb 过户：把已被引用的行挪到别的库，等于把指着它的行一次全变坏行
    for (table, id, what) in [
        ("entities", f.ent_a, "entities.kb_id"),
        ("entity_types", f.cls_a, "entity_types.kb_id"),
        ("relation_types", f.rel_a, "relation_types.kb_id"),
        ("derived_facts", f.der_a, "derived_facts.kb_id"),
        ("rules", f.rule_a, "rules.kb_id"),
        ("attribute_rules", f.arule_a, "attribute_rules.kb_id"),
        ("implication_rules", f.irule_a, "implication_rules.kb_id"),
        ("errata_runs", f.erun_a, "errata_runs.kb_id"),
        ("errata_actions", f.eact_a, "errata_actions.kb_id"),
        (
            "attribute_rule_versions",
            f.arv_a,
            "attribute_rule_versions.kb_id",
        ),
    ] {
        let err = sqlx::query(&format!("UPDATE {table} SET kb_id = $2 WHERE id = $1"))
            .bind(id)
            .bind(f.b)
            .execute(&pool)
            .await;
        assert!(err.is_err(), "{what} 过户必须被拒");
    }
    // 读数缓存的键不是 id——按 (kb_id, reading, phrase) 挪库同样不许
    let err =
        sqlx::query("UPDATE phrase_readings SET kb_id = $1 WHERE kb_id = $2 AND phrase = 'r'")
            .bind(f.b)
            .bind(f.a)
            .execute(&pool)
            .await;
    assert!(err.is_err(), "phrase_readings.kb_id 过户必须被拒");

    // —— 防误伤：同库的合法写入照常
    sqlx::query(
        "INSERT INTO fact_derivations (derived_fact_id, premise_fact_id, seq)
         VALUES ($1, $2, 0)",
    )
    .bind(f.der_a)
    .bind(f.fact_a)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO fact_qualifiers (fact_id, qualifier_type_id, entity_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact_a)
    .bind(f.rel_a)
    .bind(f.ent_a)
    .execute(&pool)
    .await?;
    // 新家族同库写入也要照常：出处、读数、勘误、定义史、名字向量
    sqlx::query(
        "INSERT INTO implied_fact_sources (fact_id, rule_id, statement_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact_a)
    .bind(f.irule_a)
    .bind(f.fact_a)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO phrase_readings (kb_id, reading, phrase, entity_id)
         VALUES ($1, 'nationality', 'y', $2)",
    )
    .bind(f.a)
    .bind(f.ent_a)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO errata_actions (id, kb_id, run_id, document_id, fact_id,
                                     statement_id, predicate_id, new_fact_id,
                                     action, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'revise', 'held')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.erun_a)
    .bind(f.doc_a)
    .bind(f.fact_a)
    .bind(f.fact_a)
    .bind(f.rel_a)
    .bind(f.fact_a)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_value,
                                    attribute_rule_id, attribute_rule_version_id)
         VALUES ($1, $2, $3, $4, '\"ok\"'::jsonb, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.ent_a)
    .bind(f.rel_a)
    .bind(f.arule_a)
    .bind(f.arv_a)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO name_vectors (fact_id, kb_id, entity_id, embedding)
         VALUES ($1, $2, $3, '[0]'::vector)",
    )
    .bind(f.fact_a)
    .bind(f.a)
    .bind(f.ent_a)
    .execute(&pool)
    .await?;

    cleanup(&pool, &f).await
}

/// 存量坏行：体检报出正确的边，对应的页读取同样拒
#[tokio::test]
async fn malformed_rows_fail_every_exported_edge_closed() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    sqlx::query("SET LOCAL session_replication_role = 'replica'")
        .execute(&mut *tx)
        .await?;

    // 每条边造一行坏行——全部落在库 A 身上
    sqlx::query(
        "INSERT INTO fact_derivations (derived_fact_id, premise_fact_id, seq)
         VALUES ($1, $2, 0)",
    )
    .bind(f.der_a)
    .bind(f.fact_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO fact_derivations (derived_fact_id, premise_derived_id, seq)
         VALUES ($1, $2, 1)",
    )
    .bind(f.der_a)
    .bind(f.der_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO fact_qualifiers (fact_id, qualifier_type_id, value)
         VALUES ($1, $2, '\"lit\"'::jsonb)",
    )
    .bind(f.fact_a)
    .bind(f.rel_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO fact_qualifiers (fact_id, qualifier_type_id, entity_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact_a)
    .bind(f.rel_a)
    .bind(f.ent_b)
    .execute(&mut *tx)
    .await?;
    // 事实的 supersedes 指向别库事实
    sqlx::query("UPDATE facts SET supersedes = $2 WHERE id = $1")
        .bind(f.fact_a)
        .bind(f.fact_b)
        .execute(&mut *tx)
        .await?;
    // 陈述来源指着别库陈述（typed_fact_sources 与 from_statement_id 两条路）
    sqlx::query("INSERT INTO typed_fact_sources (fact_id, statement_id) VALUES ($1, $2)")
        .bind(f.fact_a)
        .bind(f.fact_b)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE facts SET from_statement_id = $2 WHERE id = $1")
        .bind(f.fact_a)
        .bind(f.fact_b)
        .execute(&mut *tx)
        .await?;
    // 开放陈述的属性值指着别库实体
    sqlx::query(
        "INSERT INTO statement_qualifiers (fact_id, role, entity_id)
         VALUES ($1, 'as', $2)",
    )
    .bind(f.fact_a)
    .bind(f.ent_b)
    .execute(&mut *tx)
    .await?;
    // 时间提及两种坏法：归属在 A 却指着 B 的段落（体检扫得见——行按自己的
    // kb 计数）；归属在 B 却挂在 A 的事实上（按事实取回时，页校验拦它）
    sqlx::query(
        "INSERT INTO time_mentions (id, kb_id, fact_id, chunk_id, text, char_start)
         VALUES ($1, $2, $3, $4, '去年', 0)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.fact_a)
    .bind(f.chunk_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO time_mentions (id, kb_id, fact_id, chunk_id, text, char_start)
         VALUES ($1, $2, $3, $4, '前年', 2)",
    )
    .bind(Uuid::now_v7())
    .bind(f.b)
    .bind(f.fact_a)
    .bind(f.chunk_a)
    .execute(&mut *tx)
    .await?;
    // 类型绑定指着别库的类
    sqlx::query(
        "INSERT INTO type_bindings (id, kb_id, kind_word, status, type_id)
         VALUES ($1, $2, 'corp', 'bound', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_b)
    .execute(&mut *tx)
    .await?;
    // 短语绑定指着别库的属性
    sqlx::query(
        "INSERT INTO phrase_bindings (id, kb_id, phrase, relation_type_id,
                                     direction, status)
         VALUES ($1, $2, 'runs', $3, 'forward', 'bound')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.rel_b)
    .execute(&mut *tx)
    .await?;
    // 派生的公理规则换库
    sqlx::query("UPDATE derived_facts SET rule_id = $2 WHERE id = $1")
        .bind(f.der_a)
        .bind(f.rule_b)
        .execute(&mut *tx)
        .await?;
    // 实体的类换库
    sqlx::query("UPDATE entities SET type_id = $2 WHERE id = $1")
        .bind(f.ent_a)
        .bind(f.cls_b)
        .execute(&mut *tx)
        .await?;
    // 类层级与 domain/range
    sqlx::query("INSERT INTO entity_type_parents (child_id, parent_id) VALUES ($1, $2)")
        .bind(f.cls_a)
        .bind(f.cls_b)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO relation_type_domains (relation_type_id, entity_type_id) VALUES ($1, $2)",
    )
    .bind(f.rel_a)
    .bind(f.cls_b)
    .execute(&mut *tx)
    .await?;

    // —— 体检现在覆盖 0070 保护的全部结构边:剩下每条边也各造一行坏行 ——
    // 证据的段落指针与冗余文档指针
    sqlx::query("INSERT INTO fact_evidence (fact_id, chunk_id) VALUES ($1, $2)")
        .bind(f.fact_a)
        .bind(f.chunk_b)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO fact_evidence (fact_id, chunk_id, document_id) VALUES ($1, $2, $3)")
        .bind(f.fact_a)
        .bind(f.chunk_a)
        .bind(f.doc_b)
        .execute(&mut *tx)
        .await?;
    // 段落挂在别库文档下
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 9, 'x')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.doc_b)
    .execute(&mut *tx)
    .await?;
    // 事实本体的主语/宾语/谓词
    sqlx::query(
        "UPDATE facts SET subject_id = $2, object_id = $2, predicate_id = $3 WHERE id = $1",
    )
    .bind(f.fact_a)
    .bind(f.ent_b)
    .bind(f.rel_b)
    .execute(&mut *tx)
    .await?;
    // 派生本体的主语/宾语/谓词;另一条派生走业务规则——attribute_rule 别库
    sqlx::query(
        "UPDATE derived_facts SET subject_id = $2, object_id = $2, predicate_id = $3 WHERE id = $1",
    )
    .bind(f.der_a)
    .bind(f.ent_b)
    .bind(f.rel_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, attribute_rule_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.ent_a)
    .bind(f.rel_a)
    .bind(f.ent_a)
    .bind(f.arule_b)
    .execute(&mut *tx)
    .await?;
    // 类互斥的两个引用列是两条结构边、共用一个报错 label——
    // a_id 别库一行、b_id 别库一行(CHECK 不许 a_id = b_id)
    sqlx::query("INSERT INTO entity_type_disjoint (kb_id, a_id, b_id) VALUES ($1, $2, $3)")
        .bind(f.a)
        .bind(f.cls_b)
        .bind(f.cls_a)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO entity_type_disjoint (kb_id, a_id, b_id) VALUES ($1, $2, $3)")
        .bind(f.a)
        .bind(f.cls_a)
        .bind(f.cls_b)
        .execute(&mut *tx)
        .await?;
    // range 与关系声明的边属性
    sqlx::query(
        "INSERT INTO relation_type_ranges (relation_type_id, entity_type_id) VALUES ($1, $2)",
    )
    .bind(f.rel_a)
    .bind(f.cls_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO relation_type_qualifiers (relation_type_id, qualifier_type_id) VALUES ($1, $2)",
    )
    .bind(f.rel_a)
    .bind(f.rel_b)
    .execute(&mut *tx)
    .await?;
    // 关系的同表自指
    sqlx::query("UPDATE relation_types SET inverse_of = $2, sub_property_of = $2 WHERE id = $1")
        .bind(f.rel_a)
        .bind(f.rel_b)
        .execute(&mut *tx)
        .await?;
    // 公理的谓词
    sqlx::query("UPDATE rules SET predicate_id = $2 WHERE id = $1")
        .bind(f.rule_a)
        .bind(f.rel_b)
        .execute(&mut *tx)
        .await?;
    // 业务规则的两个引用列、typing 结论的类、条件的谓词
    sqlx::query(
        "UPDATE attribute_rules SET subject_type_id = $2, conclude_predicate_id = $3 WHERE id = $1",
    )
    .bind(f.arule_a)
    .bind(f.cls_b)
    .bind(f.rel_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO attribute_rules (id, kb_id, name, subject_type_id, conclusion, conclude_type_id)
         VALUES ($1, $2, 'ar-t', $3, 'typing', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_a)
    .bind(f.cls_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO attribute_rule_conditions (id, rule_id, seq, predicate_id, op)
         VALUES ($1, $2, 0, $3, 'present')",
    )
    .bind(Uuid::now_v7())
    .bind(f.arule_a)
    .bind(f.rel_b)
    .execute(&mut *tx)
    .await?;
    // 时间提及指着别库事实(段落同库)——timemention.fact 这一条单独验
    sqlx::query(
        "INSERT INTO time_mentions (id, kb_id, fact_id, chunk_id, text, char_start)
         VALUES ($1, $2, $3, $4, '明年', 4)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.fact_b)
    .bind(f.chunk_a)
    .execute(&mut *tx)
    .await?;
    // 短语绑定的两个类型引用列
    sqlx::query(
        "INSERT INTO phrase_bindings (id, kb_id, phrase, subject_type_id, status)
         VALUES ($1, $2, 'runs-sub', $3, 'none')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO phrase_bindings (id, kb_id, phrase, object_type_id, status)
         VALUES ($1, $2, 'runs-obj', $3, 'none')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_b)
    .execute(&mut *tx)
    .await?;

    // —— #901 新家族的坏行:0091 保护的 15 条新边 + 两条早就带复合键、
    //    却一直没人登记体检的边(join_predicate、name_vectors) ——
    // 蕴含规则本体的三个引用列
    sqlx::query(
        "INSERT INTO implication_rules (id, kb_id, trigger, phrase, conclude_property_id,
                                        status, subject_type_id)
         VALUES ($1, $2, 'phrase', 'p', $3, 'approved', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.rel_a)
    .bind(f.cls_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO implication_rules (id, kb_id, trigger, phrase, conclude_property_id,
                                        status, object_type_id)
         VALUES ($1, $2, 'phrase', 'p', $3, 'approved', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.rel_a)
    .bind(f.cls_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO implication_rules (id, kb_id, trigger, phrase, conclude_property_id,
                                        status)
         VALUES ($1, $2, 'phrase', 'p', $3, 'approved')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.rel_b)
    .execute(&mut *tx)
    .await?;
    // 读数缓存答了别库的实体
    sqlx::query(
        "INSERT INTO phrase_readings (kb_id, reading, phrase, entity_id)
         VALUES ($1, 'nationality', 'bad', $2)",
    )
    .bind(f.a)
    .bind(f.ent_b)
    .execute(&mut *tx)
    .await?;
    // 隐含事实的出处三种坏法(CHECK:statement/entity 恰好一个;
    // 主键是 (fact_id, rule_id),再落一条同库事实把三行错开)
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence)
         VALUES ($1, $2, $3, $3, 0.9)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.ent_a)
    .execute(&mut *tx)
    .await?;
    let fact_a2: Uuid = sqlx::query_scalar(
        "SELECT id FROM facts WHERE kb_id = $1 AND id <> $2 ORDER BY id DESC LIMIT 1",
    )
    .bind(f.a)
    .bind(f.fact_a)
    .fetch_one(&mut *tx)
    .await?;
    // 再落一条本库规则:三行各占一个 (fact_id, rule_id) 主键
    let irule_a2 = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO implication_rules (id, kb_id, trigger, phrase,
                                        conclude_property_id, status)
         VALUES ($1, $2, 'phrase', 'q', $3, 'approved')",
    )
    .bind(irule_a2)
    .bind(f.a)
    .bind(f.rel_a)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO implied_fact_sources (fact_id, rule_id, statement_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact_a)
    .bind(f.irule_b)
    .bind(f.fact_a)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO implied_fact_sources (fact_id, rule_id, statement_id)
         VALUES ($1, $2, $3)",
    )
    .bind(fact_a2)
    .bind(f.irule_a)
    .bind(f.fact_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO implied_fact_sources (fact_id, rule_id, entity_id)
         VALUES ($1, $2, $3)",
    )
    .bind(fact_a2)
    .bind(irule_a2)
    .bind(f.ent_b)
    .execute(&mut *tx)
    .await?;
    // 定义史挂到别库规则;派生记下别库的版本(版本行标 superseded,
    // 不与种子里那条 current 撞「每规则一条」的唯一索引)
    sqlx::query(
        "INSERT INTO attribute_rule_versions (id, kb_id, rule_id, seq, definition,
                                              superseded_at)
         VALUES ($1, $2, $3, 9, '{}'::jsonb, now())",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.arule_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_value,
                                    attribute_rule_id, attribute_rule_version_id)
         VALUES ($1, $2, $3, $4, '\"bad\"'::jsonb, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.ent_a)
    .bind(f.rel_a)
    .bind(f.arule_a)
    .bind(f.arv_b)
    .execute(&mut *tx)
    .await?;
    // 勘误账:run 盯别库文档一行;action 上的 run/document 各坏一行,
    // 其余四列塞进同一行
    sqlx::query("INSERT INTO errata_runs (id, kb_id, document_id) VALUES ($1, $2, $3)")
        .bind(Uuid::now_v7())
        .bind(f.a)
        .bind(f.doc_b)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO errata_actions (id, kb_id, run_id, document_id, action, status)
         VALUES ($1, $2, $3, $4, 'keep', 'held')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.erun_b)
    .bind(f.doc_a)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO errata_actions (id, kb_id, run_id, document_id, action, status)
         VALUES ($1, $2, $3, $4, 'keep', 'held')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.erun_a)
    .bind(f.doc_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO errata_actions (id, kb_id, run_id, document_id, fact_id,
                                     statement_id, predicate_id, new_fact_id,
                                     action, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'revise', 'held')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.erun_a)
    .bind(f.doc_a)
    .bind(f.fact_b)
    .bind(f.fact_b)
    .bind(f.rel_b)
    .bind(f.fact_b)
    .execute(&mut *tx)
    .await?;
    // relation 形状的 join 谓词指别库;名字向量两头各坏一行
    sqlx::query(
        "INSERT INTO attribute_rules (id, kb_id, name, subject_type_id, conclusion,
                                      conclude_predicate_id, join_predicate_id)
         VALUES ($1, $2, 'ar-j', $3, 'relation', $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.cls_a)
    .bind(f.rel_a)
    .bind(f.rel_b)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO name_vectors (fact_id, kb_id, entity_id, embedding)
         VALUES ($1, $2, $3, '[0]'::vector)",
    )
    .bind(f.fact_b)
    .bind(f.a)
    .bind(f.ent_a)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO name_vectors (fact_id, kb_id, entity_id, embedding)
         VALUES ($1, $2, $3, '[0]'::vector)",
    )
    .bind(f.fact_a)
    .bind(f.a)
    .bind(f.ent_b)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    drop(conn);

    let err = utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.a).await;
    let msg = format!("{err:?}");
    assert!(err.is_err(), "库 A 的体检必须拒导");
    // 体检覆盖 0070/0091 保护的全部结构边——埋下去的每一类坏行都要被点名,
    // 导出还没序列化的边(规则、绑定、时间提及、条件)也一样:受保护的
    // 同库引用断在账本上,这份导出就不可信。57 个报错 label 对应 58 条
    // 结构边(class.disjoint 的 a_id/b_id 共用一个 label)
    for edge in [
        "evidence.chunk",
        "evidence.document",
        "chunk.document",
        "derivation.premise_fact",
        "derivation.premise_derived",
        "qualifier.type",
        "qualifier.entity",
        "fact.subject",
        "fact.object",
        "fact.predicate",
        "fact.supersedes",
        "fact.from_statement",
        "derived.subject",
        "derived.object",
        "derived.predicate",
        "derived.rule",
        "derived.attribute_rule",
        "derived.attribute_rule_version",
        "entity.type",
        "class.parent",
        "class.disjoint",
        "relation.domain",
        "relation.range",
        "relation.qualifier",
        "relation.inverse",
        "relation.sub_property",
        "rule.predicate",
        "arule.subject_type",
        "arule.conclude_type",
        "arule.conclude_predicate",
        "arule.join_predicate",
        "arversion.rule",
        "condition.predicate",
        "factsource.statement",
        "squalifier.entity",
        "timemention.fact",
        "timemention.chunk",
        "binding.type",
        "pbinding.subject_type",
        "pbinding.object_type",
        "pbinding.relation",
        "irule.subject_type",
        "irule.object_type",
        "irule.conclude_property",
        "reading.entity",
        "implied.rule",
        "implied.statement",
        "implied.entity",
        "erratarun.document",
        "errata.run",
        "errata.document",
        "errata.fact",
        "errata.statement",
        "errata.predicate",
        "errata.new_fact",
        "namevector.fact",
        "namevector.entity",
    ] {
        assert!(msg.contains(edge), "体检该报 {edge}: {msg}");
    }

    // 逐页校验：坏行落在哪页，哪页就拒——不能漏出伪造 IRI
    assert!(
        utopia_store::export::facts_page(&mut pool.begin().await?, f.a, None)
            .await
            .is_err()
    );
    assert!(
        utopia_store::export::derived_page(&mut pool.begin().await?, f.a, None)
            .await
            .is_err()
    );
    assert!(
        utopia_store::export::entities_page(&mut pool.begin().await?, f.a, None)
            .await
            .is_err()
    );
    assert!(utopia_store::export::classes(&mut pool.begin().await?, f.a)
        .await
        .is_err());
    assert!(
        utopia_store::export::relations(&mut pool.begin().await?, f.a)
            .await
            .is_err()
    );

    cleanup(&pool, &f).await
}

/// 留下的行才参与校验：被引的行在两次读之间消失，判违规的是
/// **随页原子选出的归属**，不是事后另一个时刻的 JOIN——悬空引用照样拒
#[tokio::test]
async fn retained_row_validation_survives_dangling_and_late_state() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // 合法证据先行
    sqlx::query("INSERT INTO fact_evidence (fact_id, chunk_id, document_id) VALUES ($1, $2, $3)")
        .bind(f.fact_a)
        .bind(f.chunk_a)
        .bind(f.doc_a)
        .execute(&pool)
        .await?;
    utopia_store::export::facts_page(&mut pool.begin().await?, f.a, None).await?;

    // 绕过触发器把文档删掉：evidence 行还指着它——悬空不是「不存在所以跳过」
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    sqlx::query("SET LOCAL session_replication_role = 'replica'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(f.doc_a)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    drop(conn);

    let err = utopia_store::export::facts_page(&mut pool.begin().await?, f.a, None).await;
    let msg = format!("{err:?}");
    assert!(err.is_err(), "悬空 document 指针必须拒");
    assert!(
        msg.contains("evidence.document"),
        "该报 evidence.document: {msg}"
    );

    // 同理：前提行被删，derived 的前提数组仍留着死指针——逐页校验要拦
    sqlx::query(
        "INSERT INTO fact_derivations (derived_fact_id, premise_fact_id, seq)
         VALUES ($1, $2, 0)",
    )
    .bind(f.der_a)
    .bind(f.fact_a)
    .execute(&pool)
    .await?;
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    sqlx::query("SET LOCAL session_replication_role = 'replica'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM facts WHERE id = $1")
        .bind(f.fact_a)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    drop(conn);

    let err = utopia_store::export::derived_page(&mut pool.begin().await?, f.a, None).await;
    let msg = format!("{err:?}");
    assert!(err.is_err(), "悬空前提必须拒");
    assert!(
        msg.contains("derivation.premise_fact"),
        "该报 derivation.premise_fact: {msg}"
    );

    cleanup(&pool, &f).await
}
