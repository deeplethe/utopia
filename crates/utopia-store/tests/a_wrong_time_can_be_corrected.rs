//! 人工修正一条事实的有效区间（302），打在真库上。
//!
//! 抽取把「2023 年上半年」读成 1 月 1 日，在此之前只能删掉文档重抽一遍。
//! 这条路径的全部风险在于**它长得像一次 UPDATE**：改一个日期，图上就对了，
//! 谁也看不出账本少了什么。所以这里钉的四样，`cargo check` 一样都看不见：
//!
//! - 旧行留在账本里并记下作废时刻，修正行以 `supersedes` 链回它——原地改
//!   会让这次修改自己消失，而那正是记录轴要回放的东西（0019）
//! - 证据随修正行复制。少了这一步，改完时间的事实立刻变成"无人陈述"，
//!   会被 stale 判定扫成灰的
//! - 起点挪动之后要重新对账：挪过继任者的上任日，唯一性不变量才第一次
//!   看到这次相撞
//! - 已被作废的行改不动，返回 None 而不是凭空插一条挂在死行后面的修正
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::graph::Validity;
use uuid::Uuid;

struct Fixture {
    kb: Uuid,
    zhang: Uuid,
    li: Uuid,
    project: Uuid,
    leads: Uuid,
    chunk: Uuid,
    doc: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let etype = Uuid::now_v7();
    let leads = Uuid::now_v7();
    let (zhang, li, project) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (src, doc, chunk) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'time-edit-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'time-edit-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'time-edit-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'thing', 'Thing')",
    )
    .bind(etype)
    .bind(kb)
    .execute(pool)
    .await?;
    // inverse_functional：一个项目同时只有一个人 leads 它——宾语侧的唯一性，
    // 起点挪动要撞的正是这条不变量
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal, inverse_functional)
         VALUES ($1, $2, 'leads', 'leads', 'state', TRUE)",
    )
    .bind(leads)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [(zhang, "Zhang San"), (li, "Li Si"), (project, "Phoenix")] {
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
    sqlx::query("INSERT INTO sources (id, kb_id, name) VALUES ($1, $2, 'time-edit-test')")
        .bind(src)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO documents (id, kb_id, source_id, filename, sha256, status)
         VALUES ($1, $2, $3, 'memo.md', 'timeedit', 'ready')",
    )
    .bind(doc)
    .bind(kb)
    .bind(src)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text)
         VALUES ($1, $2, $3, 0, 'Zhang San led Phoenix in the first half of 2023.')",
    )
    .bind(chunk)
    .bind(kb)
    .bind(doc)
    .execute(pool)
    .await?;
    Ok(Fixture {
        kb,
        zhang,
        li,
        project,
        leads,
        chunk,
        doc,
    })
}

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

/// 一条带证据的开放事实。
async fn assert_fact(
    pool: &PgPool,
    f: &Fixture,
    subject: Uuid,
    from: &str,
    precision: &str,
) -> anyhow::Result<Uuid> {
    let (id, _) = utopia_store::graph::insert_fact(
        pool,
        f.kb,
        subject,
        Some(f.leads),
        f.project,
        Validity::starting(Some(t(from)), Some(precision)),
        0.9,
    )
    .await?;
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
         VALUES ($1, $2, 'led Phoenix', $3, 1)",
    )
    .bind(id)
    .bind(f.chunk)
    .bind(f.doc)
    .execute(pool)
    .await?;
    Ok(id)
}

#[derive(sqlx::FromRow)]
struct Row {
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_from_precision: Option<String>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    valid_to_precision: Option<String>,
    invalidated_at: Option<chrono::DateTime<chrono::Utc>>,
    supersedes: Option<Uuid>,
}

async fn row(pool: &PgPool, id: Uuid) -> anyhow::Result<Row> {
    Ok(sqlx::query_as(
        "SELECT valid_from, valid_from_precision, valid_to, valid_to_precision,
                invalidated_at, supersedes
         FROM facts WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

async fn evidence_count(pool: &PgPool, id: Uuid) -> anyhow::Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM fact_evidence WHERE fact_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// 改一个日期，账本要留下改过的痕迹：旧行作废但不删，修正行链回它，证据跟着走。
#[tokio::test]
async fn correcting_a_date_leaves_the_old_reading_in_the_ledger() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 抽取读成了 1 月 1 日，精确到日——原文其实只说了「上半年」
        let wrong = assert_fact(&pool, &f, f.zhang, "2023-01-01T00:00:00Z", "day").await?;

        let corrected = utopia_store::temporal::correct_interval(
            &pool,
            wrong,
            Validity::starting(Some(t("2023-06-01T00:00:00Z")), Some("month")),
        )
        .await?
        .expect("这条还活着，应当改得动");

        let old = row(&pool, wrong).await?;
        assert!(
            old.invalidated_at.is_some(),
            "旧行要记下何时被推翻——原地 UPDATE 会让这次修改自己消失"
        );
        assert_eq!(
            old.valid_from,
            Some(t("2023-01-01T00:00:00Z")),
            "旧行的世界区间一个字都不该动：它记录的是我们当时读成了什么"
        );

        let new = row(&pool, corrected).await?;
        assert_eq!(new.supersedes, Some(wrong), "修正行要链回被它取代的那条");
        assert!(new.invalidated_at.is_none(), "修正行是现行的");
        assert_eq!(new.valid_from, Some(t("2023-06-01T00:00:00Z")));
        assert_eq!(
            new.valid_from_precision.as_deref(),
            Some("month"),
            "精度跟着值一起改——写到月就是月，不该继承旧行那个「日」"
        );

        assert_eq!(
            evidence_count(&pool, corrected).await?,
            1,
            "证据要随修正行复制。少了它，这条事实立刻变成「无人陈述」被扫成灰的"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}

/// 结束端的三态都要改得进去，尤其是**把闭区间重新打开**——
/// 那是「当初误判它结束了」唯一的出路，而它长得像一次「什么都没填」。
#[tokio::test]
async fn an_end_that_was_never_there_can_be_taken_back() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let fact = assert_fact(&pool, &f, f.zhang, "2023-01-01T00:00:00Z", "day").await?;
        // 先闭合到 2024
        let closed = utopia_store::temporal::correct_interval(
            &pool,
            fact,
            Validity {
                from: Some(t("2023-01-01T00:00:00Z")),
                from_precision: Some("day"),
                to: Some(t("2024-01-01T00:00:00Z")),
                to_precision: Some("year"),
                attested_at: None,
            },
        )
        .await?
        .expect("改得动");
        assert_eq!(
            row(&pool, closed).await?.valid_to,
            Some(t("2024-01-01T00:00:00Z"))
        );

        // 判错了：它其实没结束。把结束端整个收回
        let reopened = utopia_store::temporal::correct_interval(
            &pool,
            closed,
            Validity::starting(Some(t("2023-01-01T00:00:00Z")), Some("day")),
        )
        .await?
        .expect("改得动");
        let r = row(&pool, reopened).await?;
        assert!(r.valid_to.is_none(), "结束端收回了");
        assert!(
            r.valid_to_precision.is_none(),
            "精度也要一起收回：留着 'year' 而日期为空，CHECK 会拦，而三值逻辑下这种组合曾经静默放行"
        );
        assert_eq!(r.supersedes, Some(closed), "两次修正串成链，不是各挂各的");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}

/// 改完起点要重新对账。**这是这一刀最容易漏掉的一半**：区间改对了，图上
/// 那条边看着也对了，可它与继任者的关系没人再算一遍。
///
/// 这里的两个人在修改前后交换了身份——改之前张三 2025 上任，是李四的继任者；
/// 改回 2023 之后他成了前任，该被闭合在李四的上任日上。唯一性不变量只在
/// 这次对账里才第一次看到这件事。
#[tokio::test]
async fn moving_a_start_earlier_makes_it_the_predecessor() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 两条都开放：李四 2024-07 接任，张三被抽取读成 2025-01（错的，
        // 他其实 2023 年就在任）。此刻不变量已经被违反，只是还没人算
        let li_fact = assert_fact(&pool, &f, f.li, "2024-07-01T00:00:00Z", "month").await?;
        let zhang_fact = assert_fact(&pool, &f, f.zhang, "2025-01-01T00:00:00Z", "month").await?;

        // 把张三改回真实的 2023
        let fixed = utopia_store::temporal::correct_interval(
            &pool,
            zhang_fact,
            Validity::starting(Some(t("2023-01-01T00:00:00Z")), Some("month")),
        )
        .await?
        .expect("改得动");
        let report = utopia_store::temporal::reconcile_moved_facts(&pool, f.kb, &[fixed]).await?;

        assert!(
            !report.corrected.is_empty(),
            "对账要动手：张三 2023 起、李四 2024-07 接任，两条不能都挂着开放区间"
        );
        // 新事实开始得更早 = 它是前任，闭合在旧事实的开始（引擎的判据）
        assert!(
            row(&pool, fixed).await?.invalidated_at.is_some(),
            "被闭合的是张三自己那条，不是李四——早的那个是前任"
        );
        let current: Row = sqlx::query_as(
            "SELECT valid_from, valid_from_precision, valid_to, valid_to_precision,
                    invalidated_at, supersedes
             FROM facts WHERE supersedes = $1",
        )
        .bind(fixed)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            current.valid_to,
            Some(t("2024-07-01T00:00:00Z")),
            "张三的区间闭合在李四的上任日"
        );
        assert_eq!(
            current.valid_from,
            Some(t("2023-01-01T00:00:00Z")),
            "起点保持修正后的值"
        );
        assert!(
            row(&pool, li_fact).await?.invalidated_at.is_none(),
            "李四那条一动不动：他是现任，本来就该开放"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}

/// 已被作废的行改不动。少了这道闸，两个人同时改会串出两条各自挂在死行后面的
/// 修正，图上凭空多一条边。
#[tokio::test]
async fn a_row_that_is_already_gone_cannot_be_corrected() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let fact = assert_fact(&pool, &f, f.zhang, "2023-01-01T00:00:00Z", "day").await?;
        let first = utopia_store::temporal::correct_interval(
            &pool,
            fact,
            Validity::starting(Some(t("2023-06-01T00:00:00Z")), Some("month")),
        )
        .await?;
        assert!(first.is_some());

        // 第二次拿着同一个（已作废的）id 再改
        let second = utopia_store::temporal::correct_interval(
            &pool,
            fact,
            Validity::starting(Some(t("2022-01-01T00:00:00Z")), Some("year")),
        )
        .await?;
        assert!(
            second.is_none(),
            "作废行改不动，要让调用方知道没动手——而不是插一条挂在死行后面的修正"
        );
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM facts WHERE supersedes = $1")
            .bind(fact)
            .fetch_one(&pool)
            .await?;
        assert_eq!(n, 1, "一条死行只该有一个后继");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}
