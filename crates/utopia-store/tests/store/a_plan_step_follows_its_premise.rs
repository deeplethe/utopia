//! 计划步骤的前提写成业务规则（0021），观察的变化落到前提上，推导（0002 R1）让依赖
//! 旧值的步骤退场、无关的步骤不动、历史留在两根轴上（#875）。
//!
//! 步骤 S_A「从桌上取 cup-7」的前提是 cup-7 的 `location` 为 desk；S_B「从架上取
//! box-3」读 box-3 自己的 `location`，是对照。结论只说建模的语义前提成立，不保证动作
//! 安全或成功。
//!
//! 前两个测试绕过了抽取与对齐：属性事实走类型化图谱的门（`graph::insert_value_fact`），
//! 随后按唯一性方向对账——与人点头一条记忆事实（`pending::confirm`）同一条路。第三个
//! 从开放陈述起步，经显式绑定、类型化物化与显式时间线对账，验证随后推导的撤回。
//! 三个测试均使用真实 store 路径，不依赖模型或 HTTP 回放，不声称自动对齐闭环成立。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use utopia_core::models::RelationAxioms;
use utopia_store::business_rules::{self, ConditionInput};
use utopia_store::graph::{self, FactObject, Validity};
use utopia_store::phrase_bindings::{self, Decision, PhraseSignature};
use utopia_store::temporal::{self, Uniqueness};
use utopia_store::{materialize, ontology, reasoning};
use uuid::Uuid;

/// 世界时间。t0 早于一切、晚到；t1 初始观察；遮挡在 t1 与 t2 之间；t2 移动；
/// t_end 是 B 那一段被人关上的时刻；NOW 是「现在」这一刻的世界时间
const T0: &str = "2026-09-23T07:50:00Z";
const T1: &str = "2026-09-23T08:00:00Z";
const T_OCC: &str = "2026-09-23T08:05:00Z";
const T_MID: &str = "2026-09-23T08:07:00Z";
const T2: &str = "2026-09-23T08:10:00Z";
const T_END: &str = "2026-09-23T08:30:00Z";
const NOW: &str = "2026-09-23T09:00:00Z";

fn t(s: &str) -> DateTime<Utc> {
    s.parse().expect("fixed timestamp")
}

struct Fixture {
    org: Uuid,
    kb: Uuid,
    cup: Uuid,
    boxed: Uuid,
    sa_ready: Uuid,
    sb_ready: Uuid,
    location: Uuid,
    rfid_zone: Uuid,
    visibility: Uuid,
    a: Uuid,
    b: Uuid,
}

async fn seed(pool: &PgPool, name: &str) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
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
    let class = |key: &'static str, label: &'static str| {
        let pool = pool.clone();
        async move {
            ontology::create_entity_type(&pool, kb, key, label, "#7fd0ff", "circle", &[], "").await
        }
    };
    let cup = class("cup", "Cup").await?;
    let boxed = class("box", "Box").await?;
    // 步骤可执行写成归类结论：规则只会推类型或属性，不为这个实验加语法（0021）
    let sa_ready = class("step_sa_ready", "S_A precondition holds").await?;
    let sb_ready = class("step_sb_ready", "S_B precondition holds").await?;
    let attribute = |key: &'static str, functional: bool| {
        let pool = pool.clone();
        async move {
            ontology::create_relation_type(
                &pool,
                kb,
                key,
                key,
                "state",
                RelationAxioms {
                    functional,
                    ..Default::default()
                },
                "",
                "attribute",
                &[cup, boxed],
                &[],
                Some("text"),
                None,
            )
            .await
        }
    };
    // 一个东西同一时刻只在一处：声明 functional，时间线才会让后一个位置关上前一个
    let location = attribute("location", true).await?;
    let rfid_zone = attribute("rfid_zone", false).await?;
    let visibility = attribute("visibility", false).await?;
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    for (id, type_id, name) in [(a, cup, "cup-7"), (b, boxed, "box-3")] {
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
    Ok(Fixture {
        org,
        kb,
        cup,
        boxed,
        sa_ready,
        sb_ready,
        location,
        rfid_zone,
        visibility,
        a,
        b,
    })
}

async fn cleanup(pool: &PgPool, org: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(pool)
        .await?;
    Ok(())
}

/// 一条步骤规则：主类 `class` 的 `location`（或别的属性）落在 `places` 里 → 归到 `ready`。
/// `groups` 是组间「或」的几组，每组一条条件（0029）
async fn step_rule(
    pool: &PgPool,
    f: &Fixture,
    name: &str,
    class: Uuid,
    ready: Uuid,
    groups: &[(Uuid, &str)],
) -> anyhow::Result<Uuid> {
    let conditions: Vec<ConditionInput> = groups
        .iter()
        .enumerate()
        .map(|(g, (predicate, place))| ConditionInput {
            group: g as i32,
            predicate_id: *predicate,
            op: "in".into(),
            operand: Some(json!([place])),
        })
        .collect();
    Ok(business_rules::create(
        pool,
        f.kb,
        name,
        "the modelled precondition of a plan step; not a safety or success guarantee",
        class,
        "typing",
        Some(ready),
        None,
        None,
        None,
        &conditions,
    )
    .await?)
}

/// 一次观察：属性事实走类型化图谱的门，带唯一性的状态关系随后对账——与
/// `pending::confirm` 落一条事实的顺序一样
async fn observe(
    pool: &PgPool,
    f: &Fixture,
    subject: Uuid,
    predicate: Uuid,
    value: &str,
    at: &str,
) -> anyhow::Result<Uuid> {
    let validity = Validity {
        from: Some(t(at)),
        from_precision: Some("second"),
        attested_at: Some(t(at)),
        ..Default::default()
    };
    let value = json!({ "value": value });
    let (id, _) =
        graph::insert_value_fact(pool, f.kb, subject, Some(predicate), &value, validity, 1.0)
            .await?;
    let (functional, temporal): (bool, String) =
        sqlx::query_as("SELECT functional, temporal FROM relation_types WHERE id = $1")
            .bind(predicate)
            .fetch_one(pool)
            .await?;
    if functional && temporal == "state" {
        temporal::reconcile_new_fact(
            pool,
            f.kb,
            id,
            subject,
            predicate,
            None,
            Some(&value),
            Uniqueness::SubjectSide,
            validity,
            1.0,
        )
        .await?;
    }
    Ok(id)
}

#[derive(Debug, sqlx::FromRow)]
struct Row {
    id: Uuid,
    valid_from: Option<DateTime<Utc>>,
    valid_to: Option<DateTime<Utc>>,
    valid_to_precision: Option<String>,
}

/// 此刻活着的那几行（主语、属性、值）
async fn live_rows(
    pool: &PgPool,
    f: &Fixture,
    subject: Uuid,
    predicate: Uuid,
    value: &str,
) -> anyhow::Result<Vec<Row>> {
    Ok(sqlx::query_as(
        "SELECT id, valid_from, valid_to, valid_to_precision FROM facts
          WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3
            AND object_value = $4 AND invalidated_at IS NULL
          ORDER BY valid_from NULLS FIRST, recorded_at",
    )
    .bind(f.kb)
    .bind(subject)
    .bind(predicate)
    .bind(json!({ "value": value }))
    .fetch_all(pool)
    .await?)
}

/// 一条规则在一个实体上此刻持有的结论（记录轴 = 现在），按世界起点排
async fn conclusions(
    pool: &PgPool,
    f: &Fixture,
    entity: Uuid,
    rule: Uuid,
) -> anyhow::Result<Vec<Row>> {
    Ok(sqlx::query_as(
        "SELECT id, valid_from, valid_to, valid_to_precision FROM derived_facts
          WHERE kb_id = $1 AND subject_id = $2 AND attribute_rule_id = $3
            AND invalidated_at IS NULL
          ORDER BY valid_from, id",
    )
    .bind(f.kb)
    .bind(entity)
    .bind(rule)
    .fetch_all(pool)
    .await?)
}

/// 规则在这个实体上、世界时刻 `at` 成立吗——走实体面板与 MCP `entity_facts` 读的那一个函数
async fn holds_at(
    pool: &PgPool,
    f: &Fixture,
    entity: Uuid,
    rule: Uuid,
    at: &str,
    as_of: Option<DateTime<Utc>>,
) -> anyhow::Result<bool> {
    Ok(
        reasoning::derived_for_entity(pool, f.kb, entity, Some(t(at)), as_of)
            .await?
            .iter()
            .any(|d| d.attribute_rule_id == Some(rule) && d.subject_id == entity),
    )
}

/// 一条派生的前提（`fact_derivations`，按 seq）
async fn premises(pool: &PgPool, derived: Uuid) -> anyhow::Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT premise_fact_id FROM fact_derivations
          WHERE derived_fact_id = $1 AND premise_fact_id IS NOT NULL ORDER BY seq",
    )
    .bind(derived)
    .fetch_all(pool)
    .await?)
}

/// 记录轴上的一刻：库自己的钟，免得两个容器的钟对不齐
async fn db_now(pool: &PgPool) -> anyhow::Result<DateTime<Utc>> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await?)
}

/// 初始 → 重复 → 遮挡 → 移动 → 晚到的旧观察 → 明确结束 → 撤回，一条时间线走完
#[tokio::test]
async fn a_step_leaves_with_its_premise_and_the_other_step_stays() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "issue875-step-premise").await?;

    let run = async {
        let rule_a = step_rule(
            &pool,
            &f,
            "S_A pick cup from desk",
            f.cup,
            f.sa_ready,
            &[(f.location, "desk")],
        )
        .await?;
        let rule_b = step_rule(
            &pool,
            &f,
            "S_B pick box from shelf",
            f.boxed,
            f.sb_ready,
            &[(f.location, "shelf")],
        )
        .await?;

        // ---- 初始观察：A 在桌上、B 在架上
        let a_desk = observe(&pool, &f, f.a, f.location, "desk", T1).await?;
        let b_shelf = observe(&pool, &f, f.b, f.location, "shelf", T1).await?;
        let r = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(
            (r.attribute_rules, r.rule_hits, r.inserted, r.invalidated),
            (2, 2, 2, 0),
            "{r:?}"
        );
        let sa = conclusions(&pool, &f, f.a, rule_a).await?;
        let sb = conclusions(&pool, &f, f.b, rule_b).await?;
        assert_eq!((sa.len(), sb.len()), (1, 1));
        assert_eq!(sa[0].valid_from, Some(t(T1)));
        assert_eq!(sa[0].valid_to, None, "前提还开着，结论也开着");
        // 前提定位得到：派生指着那条读数，证明读得出它
        assert_eq!(premises(&pool, sa[0].id).await?, vec![a_desk]);
        assert_eq!(premises(&pool, sb[0].id).await?, vec![b_shelf]);
        let proof = reasoning::proof(&pool, f.kb, sa[0].id)
            .await?
            .expect("a live conclusion has a proof");
        assert_eq!(proof.steps.len(), 1);
        assert_eq!(proof.steps[0].fact_id, a_desk);
        assert!(!proof.steps[0].retracted);
        let (matches, total) = business_rules::matches(&pool, f.kb, rule_a, 20, 0).await?;
        assert_eq!(total, 1);
        assert_eq!(matches[0]["entity_id"], json!(f.a));
        assert!(holds_at(&pool, &f, f.a, rule_a, NOW, None).await?);
        assert!(holds_at(&pool, &f, f.b, rule_b, NOW, None).await?);

        // ---- 同一份观察再来一遍：同断言同起点复用那一行，推导无事可做
        assert_eq!(
            observe(&pool, &f, f.a, f.location, "desk", T1).await?,
            a_desk
        );
        let r = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!((r.inserted, r.invalidated, r.reproved), (0, 0, 0), "{r:?}");

        // ---- 遮挡：没人写 location，另一个属性记下「看不见」。没看见 ≠ 不在桌上
        observe(&pool, &f, f.a, f.visibility, "occluded", T_OCC).await?;
        let r = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!((r.inserted, r.invalidated), (0, 0), "{r:?}");
        assert_eq!(conclusions(&pool, &f, f.a, rule_a).await?[0].id, sa[0].id);
        assert!(holds_at(&pool, &f, f.a, rule_a, NOW, None).await?);

        // ---- 移动：A 被正面看到在架上。时间线把桌面那一段关在 t2（作废 + 改写）
        let before_move = db_now(&pool).await?;
        observe(&pool, &f, f.a, f.location, "shelf", T2).await?;
        let desk = live_rows(&pool, &f, f.a, f.location, "desk").await?;
        assert_eq!(desk.len(), 1);
        assert_ne!(desk[0].id, a_desk, "关上的是改写出来的新行，旧行作废");
        assert_eq!(desk[0].valid_to, Some(t(T2)));
        let r = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!((r.inserted, r.invalidated), (1, 1), "{r:?}");
        assert!(
            !holds_at(&pool, &f, f.a, rule_a, NOW, None).await?,
            "S_A 在现在不再成立"
        );
        assert!(
            holds_at(&pool, &f, f.a, rule_a, T_MID, None).await?,
            "t1..t2 之间它成立过：世界轴上的历史"
        );
        let sa_now = conclusions(&pool, &f, f.a, rule_a).await?;
        assert_eq!(sa_now.len(), 1);
        assert_eq!(
            (sa_now[0].valid_from, sa_now[0].valid_to),
            (Some(t(T1)), Some(t(T2)))
        );
        assert_eq!(premises(&pool, sa_now[0].id).await?, vec![desk[0].id]);
        // 旧结论作废而不删，记录轴回到移动之前它还在，前提链还指着当时那条读数
        let (old_invalidated,): (Option<DateTime<Utc>>,) =
            sqlx::query_as("SELECT invalidated_at FROM derived_facts WHERE id = $1")
                .bind(sa[0].id)
                .fetch_one(&pool)
                .await?;
        assert!(old_invalidated.is_some());
        assert_eq!(premises(&pool, sa[0].id).await?, vec![a_desk]);
        assert!(
            holds_at(&pool, &f, f.a, rule_a, NOW, Some(before_move)).await?,
            "当时所知：移动之前的库认为 S_A 至今成立"
        );
        // 无关步骤原样：同一行、没作废
        let sb_after = conclusions(&pool, &f, f.b, rule_b).await?;
        assert_eq!(sb_after.len(), 1);
        assert_eq!(sb_after[0].id, sb[0].id);
        assert!(holds_at(&pool, &f, f.b, rule_b, NOW, None).await?);

        // ---- 晚到的旧观察：t0 时 A 在桌上，移动之后才送到。时间线按世界时间排，
        //      它止于 t2，不会让桌面在「现在」复活
        observe(&pool, &f, f.a, f.location, "desk", T0).await?;
        reasoning::materialize(&pool, f.kb).await?;
        assert!(!holds_at(&pool, &f, f.a, rule_a, NOW, None).await?);
        assert!(holds_at(&pool, &f, f.a, rule_a, "2026-09-23T07:55:00Z", None).await?);
        let a_rows_before_end: Vec<Uuid> = conclusions(&pool, &f, f.a, rule_a)
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect();

        // ---- 明确结束：B 在架上这一段止于 t_end（人在 Review 里关上它的那一步）
        let b_rows = live_rows(&pool, &f, f.b, f.location, "shelf").await?;
        assert_eq!(b_rows.len(), 1);
        temporal::close_superseded(&pool, b_rows[0].id, t(T_END), "second").await?;
        let r = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!((r.inserted, r.invalidated), (1, 1), "{r:?}");
        assert!(!holds_at(&pool, &f, f.b, rule_b, NOW, None).await?);
        assert!(holds_at(&pool, &f, f.b, rule_b, T_MID, None).await?);
        let a_rows_after_end: Vec<Uuid> = conclusions(&pool, &f, f.a, rule_a)
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(a_rows_after_end, a_rows_before_end, "关 B 不碰 A 的结论");

        // ---- 撤回：B 那条读数被判为错读。结论整条作废，没有替代；证明还读得出当时靠的是什么
        let b_closed = live_rows(&pool, &f, f.b, f.location, "shelf").await?;
        let sb_closed = conclusions(&pool, &f, f.b, rule_b).await?;
        assert_eq!((b_closed.len(), sb_closed.len()), (1, 1));
        graph::reject_fact(&pool, f.kb, b_closed[0].id).await?;
        let r = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!((r.inserted, r.invalidated), (0, 1), "{r:?}");
        assert!(conclusions(&pool, &f, f.b, rule_b).await?.is_empty());
        assert!(!holds_at(&pool, &f, f.b, rule_b, T_MID, None).await?);
        let history = reasoning::proof(&pool, f.kb, sb_closed[0].id)
            .await?
            .expect("an invalidated conclusion keeps its proof");
        assert!(history.derived.invalidated_at.is_some());
        assert_eq!(history.steps[0].fact_id, b_closed[0].id);
        assert!(history.steps[0].retracted, "撤掉的前提照样列出并打上标记");
        anyhow::Ok(())
    }
    .await;

    cleanup(&pool, f.org).await?;
    run
}

/// 一个步骤两条独立的证明（组间「或」）：撤掉一条仍成立、证明换成另一条；
/// 最后一条也没了才退场
#[tokio::test]
async fn a_step_with_two_proofs_stays_until_its_last_support_goes() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "issue875-two-proofs").await?;

    let run = async {
        let rule = step_rule(
            &pool,
            &f,
            "S_A pick cup from desk, either sensor",
            f.cup,
            f.sa_ready,
            &[(f.location, "desk"), (f.rfid_zone, "desk")],
        )
        .await?;
        let seen = observe(&pool, &f, f.a, f.location, "desk", T1).await?;
        let tagged = observe(&pool, &f, f.a, f.rfid_zone, "desk", T1).await?;
        let r = reasoning::materialize(&pool, f.kb).await?;
        // 两组推出同一段区间，落成一行；留下的证明是组序在前的那条（0029）
        assert_eq!((r.rule_hits, r.inserted), (1, 1), "{r:?}");
        let step = conclusions(&pool, &f, f.a, rule).await?;
        assert_eq!(step.len(), 1);
        assert_eq!(premises(&pool, step[0].id).await?, vec![seen]);

        graph::reject_fact(&pool, f.kb, seen).await?;
        let r = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(
            (r.inserted, r.invalidated, r.reproved),
            (0, 0, 1),
            "结论没变、理由变了：同一行重写证明（0030）{r:?}"
        );
        let still = conclusions(&pool, &f, f.a, rule).await?;
        assert_eq!(still.len(), 1);
        assert_eq!(still[0].id, step[0].id);
        assert_eq!(premises(&pool, still[0].id).await?, vec![tagged]);
        assert!(holds_at(&pool, &f, f.a, rule, NOW, None).await?);

        graph::reject_fact(&pool, f.kb, tagged).await?;
        let r = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!((r.inserted, r.invalidated), (0, 1), "{r:?}");
        assert!(conclusions(&pool, &f, f.a, rule).await?.is_empty());
        assert!(!holds_at(&pool, &f, f.a, rule, NOW, None).await?);
        anyhow::Ok(())
    }
    .await;

    cleanup(&pool, f.org).await?;
    run
}

// ===================== 从开放陈述起步：对齐那一段 =====================

/// 一份带日期的文档和它的一块（推送来的陈述就是这样一份文档，`doc_time_source = 'source'`）
async fn document(
    pool: &PgPool,
    f: &Fixture,
    name: &str,
    doc_time: &str,
    text: &str,
) -> anyhow::Result<(Uuid, Uuid)> {
    let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, doc_time, doc_time_source)
         VALUES ($1, $2, $3, $4, $5, 'source')",
    )
    .bind(doc)
    .bind(f.kb)
    .bind(name)
    .bind(format!("{name}-{doc}"))
    .bind(t(doc_time))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, $4)",
    )
    .bind(chunk)
    .bind(f.kb)
    .bind(doc)
    .bind(text)
    .execute(pool)
    .await?;
    Ok((doc, chunk))
}

/// 一条开放陈述「cup-7 —is on→ <place>」，证据是那一块、没有引文（0054 决定 4）
async fn statement(
    pool: &PgPool,
    f: &Fixture,
    chunk: Uuid,
    place: &str,
    attested: &str,
) -> anyhow::Result<Uuid> {
    let value = json!({ "value": place });
    let (id, _) = graph::insert_open_statement(
        pool,
        f.kb,
        f.a,
        "is on",
        FactObject::Value(&value),
        Some(t(attested)),
        1.0,
    )
    .await?;
    graph::add_evidence_located(pool, id, chunk, None, Some("is on"), None).await?;
    Ok(id)
}

/// 人把签名（is on × cup × 值）绑到 `location`，与 Review 里点下去的那一次同一个函数
async fn bind_is_on(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
    let sig = PhraseSignature {
        phrase: "is on".into(),
        subject_type_id: Some(f.cup),
        subject_type_key: Some("cup".into()),
        object_type_id: None,
        object_type_key: None,
        object_is_value: true,
        count: 1,
        examples: Vec::new(),
        quotes: Vec::new(),
    };
    let written = phrase_bindings::decide(
        pool,
        f.kb,
        &sig,
        Decision {
            relation_type_id: Some(f.location),
            direction: Some("forward"),
            status: "bound",
            votes: &json!({ "person": { "property": "location", "direction": "forward" } }),
            decided_by: "person",
            basis: None,
        },
    )
    .await?;
    assert!(written);
    Ok(())
}

/// 类型化行里 A 的 location = desk 那一行（活着的）
async fn typed_desk(pool: &PgPool, f: &Fixture) -> anyhow::Result<Vec<Row>> {
    Ok(sqlx::query_as(
        "SELECT id, valid_from, valid_to, valid_to_precision FROM facts
          WHERE kb_id = $1 AND layer = 'typed' AND subject_id = $2 AND predicate_id = $3
            AND object_value = '{\"value\": \"desk\"}'::jsonb AND invalidated_at IS NULL",
    )
    .bind(f.kb)
    .bind(f.a)
    .bind(f.location)
    .fetch_all(pool)
    .await?)
}

/// 两份观察各是一份文档（一次观察一个身份）。显式对账之后桌面那一段关在第二份的日期上，
/// 步骤跟着退场——`POST /kbs/{id}/ontology/relation-types/{type_id}/reconcile` 就是这一步
#[tokio::test]
async fn an_explicit_reconcile_closes_the_earlier_place_and_the_step_leaves() -> anyhow::Result<()>
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, "issue875-reconcile").await?;

    let run = async {
        let rule = step_rule(
            &pool,
            &f,
            "S_A pick cup from desk",
            f.cup,
            f.sa_ready,
            &[(f.location, "desk")],
        )
        .await?;
        let (_, c1) = document(&pool, &f, "obs-1.json", T1, "cup-7 is on desk").await?;
        let (_, c2) = document(&pool, &f, "obs-2.json", T2, "cup-7 is on shelf").await?;
        statement(&pool, &f, c1, "desk", T1).await?;
        statement(&pool, &f, c2, "shelf", T2).await?;
        bind_is_on(&pool, &f).await?;
        let typed = materialize::materialize(&pool, f.kb).await?;
        assert_eq!(typed.added, 2, "{typed:?}");

        let report = temporal::reconcile_predicate(&pool, f.kb, f.location).await?;
        assert_eq!(
            (report.corrected.len(), report.conflicts),
            (1, 0),
            "{report:?}"
        );
        let desk = typed_desk(&pool, &f).await?;
        assert_eq!(desk.len(), 1);
        assert_eq!(desk[0].valid_from, None, "没有模型读时间词：起点留空");
        assert_eq!(
            desk[0].valid_to_precision.as_deref(),
            Some("unknown"),
            "没起点的后任：前任写成「结束了，不知哪天」，锚在后任那份文档的日期上"
        );
        reasoning::materialize(&pool, f.kb).await?;
        assert!(!holds_at(&pool, &f, f.a, rule, NOW, None).await?);
        assert!(holds_at(&pool, &f, f.a, rule, T_MID, None).await?);
        anyhow::Ok(())
    }
    .await;

    cleanup(&pool, f.org).await?;
    run
}
