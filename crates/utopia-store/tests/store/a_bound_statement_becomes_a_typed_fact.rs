//! 类型化图谱是视图（0044 决定 3，0067、0068）：绑上的签名下的开放陈述算成类型化行，
//! 几条陈述说同一件事就是一行，绑定变了行跟着变，跑多少遍结果一样。
//!
//! 一条陈述「Harbor Bakery —is based in→ Port Ellen」，签名 (is based in, organization,
//! place) 绑到 headquartered_in：算出一条类型化行，谓词是它，证据与限定抄过来，
//! `from_statement_id` 指回陈述；再跑一遍什么都不动；带 mood 的陈述不算；另一份文档
//! 的同一句并进同一行（两条来源、两条证据）；作废其中一条陈述，行还在、来源少一条；
//! 绑定翻成 reverse，旧行作废、新行主宾对调；绑定改成 none，行作废、不再补。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::materialize::{materialize, Outcome};
use uuid::Uuid;

#[tokio::test]
async fn a_bound_statement_becomes_a_typed_fact() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = 'materialize-test'")
        .execute(&pool)
        .await?;

    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'materialize-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'materialize-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'materialize-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let run = async {
        // 两个类、一条属性
        let (organization, place, hq) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        for (id, key) in [(organization, "organization"), (place, "place")] {
            sqlx::query(
                "INSERT INTO entity_types (id, kb_id, key, label, color, shape)
                 VALUES ($1, $2, $3, $3, '#7fd0ff', 'circle')",
            )
            .bind(id)
            .bind(kb)
            .bind(key)
            .execute(&pool)
            .await?;
        }
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal)
             VALUES ($1, $2, 'headquartered_in', 'headquartered in', 'state')",
        )
        .bind(hq)
        .bind(kb)
        .execute(&pool)
        .await?;

        // 两样东西；三条陈述：一条平常的，一条带 mood 的，一条另一份文档说的同一件事
        let (bakery, port) = (Uuid::now_v7(), Uuid::now_v7());
        for (id, type_id, name) in [(bakery, organization, "Harbor Bakery"), (port, place, "Port Ellen")] {
            sqlx::query(
                "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(kb)
            .bind(type_id)
            .bind(name)
            .execute(&pool)
            .await?;
        }
        // 先来一条没时间的裸陈述，再来两条带时间的：裸行会被带时间的行取代（supersedes），
        // 来源要跟着搬到新行上
        let bare = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, object_id, layer, phrase, confidence)
             VALUES ($1, $2, $3, $4, 'open', 'operates from', 0.9)",
        )
        .bind(bare)
        .bind(kb)
        .bind(bakery)
        .bind(port)
        .execute(&pool)
        .await?;
        let (stated, planned, again) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        for (id, phrase) in [(stated, "is based in"), (planned, "will move to"), (again, "is based in")] {
            sqlx::query(
                "INSERT INTO facts (id, kb_id, subject_id, object_id, layer, phrase, confidence,
                                    valid_from, valid_from_precision, valid_from_grade)
                 VALUES ($1, $2, $3, $4, 'open', $5, 0.9, '2019-01-01', 'year', 'B')",
            )
            .bind(id)
            .bind(kb)
            .bind(bakery)
            .bind(port)
            .bind(phrase)
            .execute(&pool)
            .await?;
        }
        let evidence_of = |statement: Uuid, name: &'static str| {
            let pool = pool.clone();
            async move {
                let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
                sqlx::query("INSERT INTO documents (id, kb_id, filename, sha256) VALUES ($1, $2, $3, $3)")
                    .bind(doc)
                    .bind(kb)
                    .bind(name)
                    .execute(&pool)
                    .await?;
                sqlx::query(
                    "INSERT INTO chunks (id, kb_id, document_id, seq, text)
                     VALUES ($1, $2, $3, 0, 'Harbor Bakery is based in Port Ellen.')",
                )
                .bind(chunk)
                .bind(kb)
                .bind(doc)
                .execute(&pool)
                .await?;
                sqlx::query(
                    "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, quote_start, quote_end)
                     VALUES ($1, $2, 'Harbor Bakery is based in Port Ellen.', $3, 0, 37)",
                )
                .bind(statement)
                .bind(chunk)
                .bind(doc)
                .execute(&pool)
                .await?;
                anyhow::Ok(())
            }
        };
        evidence_of(stated, "bakery.txt").await?;
        evidence_of(again, "bakery-2.txt").await?;
        sqlx::query(
            "INSERT INTO statement_qualifiers (fact_id, role, value) VALUES ($1, 'since', '\"2019\"'::jsonb)",
        )
        .bind(stated)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO statement_qualifiers (fact_id, role, value) VALUES ($1, 'mood', '\"will\"'::jsonb)",
        )
        .bind(planned)
        .execute(&pool)
        .await?;

        // 两条签名都绑到 headquartered_in
        let binding = |phrase: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO phrase_bindings
                         (id, kb_id, phrase, subject_type_id, object_type_id, object_is_value,
                          relation_type_id, direction, status)
                     VALUES ($1, $2, $3, $4, $5, false, $6, 'forward', 'bound')",
                )
                .bind(Uuid::now_v7())
                .bind(kb)
                .bind(phrase)
                .bind(organization)
                .bind(place)
                .bind(hq)
                .execute(&pool)
                .await
            }
        };
        binding("is based in").await?;
        binding("will move to").await?;
        binding("operates from").await?;

        // 三条平常的陈述说的是同一件事：裸的那条先成一行，带时间的取代它（added 2，旧行作废、
        // 来源搬过来），第三条并进去（merged 1）——最后一行，三条来源，两条证据；带 mood 的
        // 那条不算
        let first = materialize(&pool, kb).await?;
        assert_eq!(first, Outcome { retired: 0, added: 2, merged: 1, implied: 0 });
        let live = |pool: PgPool| async move {
            sqlx::query_as::<_, (Uuid, Uuid, Uuid, Uuid, Uuid, Option<chrono::DateTime<chrono::Utc>>, Option<String>)>(
                "SELECT id, subject_id, object_id, predicate_id, from_statement_id, valid_from,
                        valid_from_grade
                   FROM facts WHERE kb_id = $1 AND layer = 'typed' AND invalidated_at IS NULL",
            )
            .bind(kb)
            .fetch_all(&pool)
            .await
        };
        let rows = live(pool.clone()).await?;
        assert_eq!(rows.len(), 1, "同一个三元组只有一行");
        let (typed_id, subject, object, predicate, from, vf, grade) = rows[0].clone();
        assert_eq!((subject, object, predicate, from), (bakery, port, hq, stated));
        assert!(vf.is_some(), "世界轴时间抄过来");
        // 起点是怎么来的也抄过来：时态引擎在类型化的行上判，读不到等级就只能回去猜（0045 第 3 刀）
        assert_eq!(grade.as_deref(), Some("B"), "起点的来历随日期一起抄过来");
        let sources: i64 =
            sqlx::query_scalar("SELECT count(*) FROM typed_fact_sources WHERE fact_id = $1")
                .bind(typed_id)
                .fetch_one(&pool)
                .await?;
        let evidence: i64 =
            sqlx::query_scalar("SELECT count(*) FROM fact_evidence WHERE fact_id = $1")
                .bind(typed_id)
                .fetch_one(&pool)
                .await?;
        let qualifiers: Vec<String> = sqlx::query_scalar(
            "SELECT role FROM statement_qualifiers WHERE fact_id = $1 ORDER BY role",
        )
        .bind(typed_id)
        .fetch_all(&pool)
        .await?;
        assert_eq!((sources, evidence, qualifiers), (3, 2, vec!["since".to_string()]));

        // 再跑一遍：什么都不动
        assert_eq!(materialize(&pool, kb).await?, Outcome::default());

        // 一份文档撤了它那句：行还在，来源少一条
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(again)
            .execute(&pool)
            .await?;
        assert_eq!(materialize(&pool, kb).await?, Outcome::default());
        let sources: i64 =
            sqlx::query_scalar("SELECT count(*) FROM typed_fact_sources WHERE fact_id = $1")
                .bind(typed_id)
                .fetch_one(&pool)
                .await?;
        assert_eq!(sources, 2);
        assert_eq!(live(pool.clone()).await?.len(), 1, "还有来源，行留着");

        // 绑定翻成 reverse：旧行作废，新行主宾对调
        sqlx::query("UPDATE phrase_bindings SET direction = 'reverse' WHERE kb_id = $1 AND phrase IN ('is based in', 'operates from')")
            .bind(kb)
            .execute(&pool)
            .await?;
        // 旧行的来源全不成立了：作废 1；反向重算时裸的那条先成行、带时间的再取代它：新建 2
        assert_eq!(materialize(&pool, kb).await?, Outcome { retired: 1, added: 2, merged: 0, implied: 0 });
        let rows = live(pool.clone()).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].1, rows[0].2), (port, bakery), "方向反了主宾对调");
        let retired: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM facts WHERE from_statement_id = $1 AND invalidated_at IS NOT NULL",
        )
        .bind(stated)
        .fetch_one(&pool)
        .await?;
        assert_eq!(retired, 1, "旧行留着，只是作废了");

        // 绑定改成 none：行作废，不再补
        sqlx::query(
            "UPDATE phrase_bindings SET status = 'none', relation_type_id = NULL, direction = NULL
              WHERE kb_id = $1 AND phrase IN ('is based in', 'operates from')",
        )
        .bind(kb)
        .execute(&pool)
        .await?;
        assert_eq!(materialize(&pool, kb).await?, Outcome { retired: 1, added: 0, merged: 0, implied: 0 });
        assert_eq!(utopia_store::materialize::count(&pool, kb).await?, 0);
        anyhow::Ok(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
