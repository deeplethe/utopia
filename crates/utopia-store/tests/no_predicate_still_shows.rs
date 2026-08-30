//! 没有谓词的事实**不能从读路径上消失**（迁移 0052）。
//!
//! 为什么非要连库：0052 把 `facts.predicate_id` 改成可空之后，二十条读查询里的
//! `JOIN relation_types` 全都变成了静默的过滤器——内连接丢掉 NULL 行不报错、
//! 不告警，`cargo check` 和 clippy 一个字都看不见。这与 0009 的
//! `NULL <> uuid` 是同一个陷阱的两副面孔：三值逻辑下"没有值"被当成"不匹配"。
//!
//! 这个测试守的就是那条线：**造一条没有谓词的事实，然后要求每条读路径都还能看见它**。
//! 把任何一条 `LEFT JOIN relation_types` 改回 `JOIN`，这里必须红。
//!
//! 顺带守住显示口径：`fact_surface_predicate` 取出现最多的那个说法，
//! 于是同一条事实在图上、实体面板、变更历史里叫同一个名字。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    kb: Uuid,
    subject: Uuid,
    doc: Uuid,
    /// 没有谓词、但证据里留了原文说法的事实
    surfaced: Uuid,
    /// 没有谓词、连原文说法也没有的事实（0052 之前的历史遗留长这样）
    mute: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let etype = Uuid::now_v7();
    let (subject, object) = (Uuid::now_v7(), Uuid::now_v7());
    let (src, doc, chunk_a, chunk_b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (surfaced, mute) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'no-predicate-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'no-predicate-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'no-predicate-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'org', '组织')")
        .bind(etype)
        .bind(kb)
        .execute(pool)
        .await?;
    for (id, name) in [(subject, "Acme"), (object, "Beta")] {
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

    sqlx::query("INSERT INTO sources (id, kb_id, name) VALUES ($1, $2, '临时来源')")
        .bind(src)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO documents (id, kb_id, source_id, filename, sha256, status)
         VALUES ($1, $2, $3, 'note.md', 'nopredicate', 'ready')",
    )
    .bind(doc)
    .bind(kb)
    .bind(src)
    .execute(pool)
    .await?;
    for (id, seq, text) in [
        (chunk_a, 0i32, "Acme acquired Beta."),
        (chunk_b, 1i32, "Acme acquired Beta again."),
    ] {
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(doc)
        .bind(seq)
        .bind(text)
        .execute(pool)
        .await?;
    }

    // 两条事实都没有谓词——本体里没有对应的关系，这正是 0052 要表达的状态
    for (id, from) in [
        (surfaced, "2020-01-01T00:00:00Z"),
        (mute, "2021-01-01T00:00:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                valid_from, valid_from_precision, confidence)
             VALUES ($1, $2, $3, NULL, $4, $5, 'day', 0.9)",
        )
        .bind(id)
        .bind(kb)
        .bind(subject)
        .bind(object)
        .bind(from.parse::<chrono::DateTime<chrono::Utc>>()?)
        .execute(pool)
        .await?;
    }

    // surfaced 有三条证据：acquired 两次、bought 一次。众数是 acquired——
    // 同时验了 fact_surface_predicate 的"取出现最多的那个"。
    // mute 只有一条不带原文说法的证据，模拟 add_evidence 无条件记录之前的老数据
    for (fact, chunk, pred) in [
        (surfaced, chunk_a, Some("acquired")),
        (surfaced, chunk_b, Some("acquired")),
        (surfaced, chunk_a, Some("bought")),
        (mute, chunk_a, None),
    ] {
        sqlx::query(
            "INSERT INTO fact_evidence (fact_id, chunk_id, document_id, proposed_predicate, quote)
             VALUES ($1, $2, $3, $4, 'Acme acquired Beta.')
             ON CONFLICT DO NOTHING",
        )
        .bind(fact)
        .bind(chunk)
        .bind(doc)
        .bind(pred)
        .execute(pool)
        .await?;
    }

    Ok(Fixture {
        kb,
        subject,
        doc,
        surfaced,
        mute,
    })
}

#[tokio::test]
async fn a_fact_without_a_predicate_is_still_visible_everywhere() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    // **开跑前先扫地。** 断言 panic 会跳过下面的 teardown（它只接住 Err 那条路），
    // 于是一次失败的跑会给下一次留下垃圾。清干净比把每条断言改成返回 Err 便宜
    sqlx::query("DELETE FROM organizations WHERE name = 'no-predicate-test'")
        .execute(&pool)
        .await?;
    let f = seed(&pool).await?;

    let run = async {
        // 1. 显示口径：众数胜出，且不是随机的那一个
        let word: Option<String> = sqlx::query_scalar("SELECT fact_surface_predicate($1)")
            .bind(f.surfaced)
            .fetch_one(&pool)
            .await?;
        assert_eq!(word.as_deref(), Some("acquired"), "该取出现最多的说法");

        let none: Option<String> = sqlx::query_scalar("SELECT fact_surface_predicate($1)")
            .bind(f.mute)
            .fetch_one(&pool)
            .await?;
        assert!(none.is_none(), "连原文说法都没有时该是空，而不是编一个词");

        // 2. 图的边：两条都要在，且都标成 inferred
        let (_, edges) = utopia_store::graph::neighborhood(&pool, f.kb, f.subject, 1, None).await?;
        let ours: Vec<_> = edges
            .iter()
            .filter(|e| e.id == f.surfaced || e.id == f.mute)
            .collect();
        assert_eq!(ours.len(), 2, "内连接会把没有谓词的边整条吞掉");
        let e = ours.iter().find(|e| e.id == f.surfaced).unwrap();
        assert_eq!(e.label.as_deref(), Some("acquired"));
        assert!(e.inferred, "本体没认下，界面要看得出这词来自原文");
        let m = ours.iter().find(|e| e.id == f.mute).unwrap();
        assert!(m.label.is_none(), "说不出就是说不出，不该回落成「有关联」");

        // 3. 实体面板
        let (_, facts) = utopia_store::graph::entity_detail(&pool, f.kb, f.subject).await?;
        let panel: Vec<_> = facts
            .iter()
            .filter(|x| x.id == f.surfaced || x.id == f.mute)
            .collect();
        assert_eq!(panel.len(), 2, "实体面板漏掉了没有谓词的事实");
        let s = panel.iter().find(|x| x.id == f.surfaced).unwrap();
        assert_eq!(s.predicate_label.as_deref(), Some("acquired"));
        assert!(s.inferred);
        assert!(s.temporal.is_none(), "没有谓词就谈不上时态类别");

        // 4. 文档产出（DocViewer 逐块回看）
        let chunk_facts = utopia_store::graph::document_extractions(&pool, f.doc).await?;
        assert!(
            chunk_facts.iter().any(|c| c.fact_id == f.surfaced),
            "文档页看不见没有谓词的事实"
        );
        assert!(
            chunk_facts.iter().any(|c| c.fact_id == f.mute),
            "文档页看不见没有说法的事实"
        );

        // 5. 实体历史（外层 FROM 是 CTE，别名写错会直接报 missing FROM-clause）
        let (hist, _) = utopia_store::graph::entity_history(&pool, f.kb, f.subject, 50, 0).await?;
        let h = hist.iter().find(|e| e.fact_id == Some(f.surfaced));
        assert!(h.is_some(), "实体历史漏掉了没有谓词的事实");
        assert_eq!(h.unwrap().predicate_label.as_deref(), Some("acquired"));

        // 6. 账本变更流
        let changes = utopia_store::graph::graph_changes(
            &pool,
            f.kb,
            "2000-01-01T00:00:00Z".parse()?,
            "2100-01-01T00:00:00Z".parse()?,
            None,
            None,
            50,
        )
        .await?;
        let c = changes.iter().find(|c| c.fact_id == f.surfaced);
        assert!(c.is_some(), "变更流漏掉了没有谓词的事实");
        assert_eq!(c.unwrap().predicate_label.as_deref(), Some("acquired"));

        // 7. **不再有任何机制能把它长回来。**
        //
        // 这一条的历史值得留着：第一版是空断言（只 `SELECT … WHERE builtin`，
        // 而夹具是裸 SQL 建的库、从没播种过，于是断言在一片空地上成立）；
        // 第二版补了对照组——先调 `ensure_default_ontology` 把种子种下去，
        // 再确认 `related_to` 不在其中。因为 0052 删完七分钟，代码就把行种回来了（0053）。
        //
        // 现在**连播种函数都没有了**：0009 删内置实体类、0010 与 `#125` 删种子关系、
        // 0011 把 `mapped_to` 搬去 `concept_mappings`，`ensure_default_ontology`
        // 随之退场。所以这里改守更强的那条性质：**建库路径上不存在任何
        // 代码硬塞的关系**，`builtin` 那一列在新库里恒为空。
        let seeded: Vec<String> =
            sqlx::query_scalar("SELECT key FROM relation_types WHERE kb_id = $1 AND builtin")
                .bind(f.kb)
                .fetch_all(&pool)
                .await?;
        assert!(
            seeded.is_empty(),
            "本体里不该有代码硬塞进去的关系，实得 {seeded:?}"
        );

        // 只按 kb 查，不查全库：全库计数会被并行跑的其它测试和上一轮的残留污染，
        // 那是条会无故变红的断言。这一条已经守住了要守的东西——
        // 种子表种下之后，本体里没有一个"什么都没说"的关系可供模型挑选

        Ok::<(), anyhow::Error>(())
    }
    .await;

    // 无论断言是否炸，都把临时数据拆干净（下层全是 ON DELETE CASCADE）
    sqlx::query("DELETE FROM organizations WHERE name = 'no-predicate-test'")
        .execute(&pool)
        .await?;
    run
}
