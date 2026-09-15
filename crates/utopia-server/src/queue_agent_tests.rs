//! 事实与冲突两档交给 agent（0043），走整条路：假模型按脚本回话，agent 动手，人再撤回。
//!
//! 租约的形状：第五份补充协议的截止日（0.9），第六份的新截止日模型给了 0.7——不许它
//! 接替，队列里挂着一条低置信事实与一对低置信冲突。另有一条原文根本没说的事实，和两个
//! 同一天开始的租户（后来的那个，原文写着它哪天才成为租户）。
//!
//! 1. agent 确认第六份的截止日：置信度升到 1.0，第五份的截止日关在它开始时，那对冲突撤下。
//! 2. agent 驳回原文没说的那条。
//! 3. agent 把后来那个租户的起点改成原文写的那天：先前那个租户关在那一天。
//! 4. 人把三步都撤回：截止日回到 0.7、第五份重新开着；驳回的事实回来；租户的起点回到原样，
//!    那对「同一天开始」的冲突重新挂上。撤回到第二次，保险丝把开关关掉。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use super::*;
use std::sync::Arc;
use utopia_store::graph::Validity;
use utopia_store::temporal::Uniqueness;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, Request, Respond, ResponseTemplate,
};

/// 按提示词里的那一句回话：审事实的一批、裁冲突的一批
#[derive(Clone)]
struct Scripted;

impl Respond for Scripted {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = request.body_json().expect("chat request is JSON");
        let system = body["messages"][0]["content"].as_str().unwrap_or_default();
        let user = body["messages"][1]["content"].as_str().unwrap_or_default();
        // 每一项按它在批里的编号答：认得出是哪一项才答，其余 unsure
        let mut verdicts = Vec::new();
        for (i, block) in user
            .split("\n\n")
            .filter(|b| !b.trim().is_empty())
            .enumerate()
        {
            let v = if system.contains("You review facts") {
                if block.contains("2020-04-14") {
                    json!({ "i": i, "action": "confirm", "confidence": 0.95,
                            "why": "the sixth amendment's table states it" })
                } else if block.contains("Tower 9") {
                    json!({ "i": i, "action": "reject", "confidence": 0.9,
                            "why": "the quote does not mention it" })
                } else {
                    json!({ "i": i, "action": "unsure", "confidence": 0.2 })
                }
            } else if block.contains("B Corp") {
                json!({ "i": i, "action": "retime_new", "confidence": 0.9,
                        "why": "B Corp became the tenant on June 1, 2021", "date": "2021-06-01" })
            } else {
                json!({ "i": i, "action": "unsure", "confidence": 0.2 })
            };
            verdicts.push(v);
        }
        let content = json!({ "verdicts": verdicts }).to_string();
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "role": "assistant", "content": content } }]
        }))
    }
}

struct Fx {
    state: AppState,
    pool: sqlx::PgPool,
    org: Uuid,
    kb: Uuid,
    user: Uuid,
    lease: Uuid,
    deadline: Uuid,
    tenant: Uuid,
    _server: MockServer,
    dir: std::path::PathBuf,
}

fn t(day: &str) -> DateTime<Utc> {
    format!("{day}T00:00:00Z").parse().unwrap()
}

async fn fixture() -> anyhow::Result<Option<Fx>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let (org, ws, kb, user) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (etype, deadline, tenant, lease) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'queue-agent-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'queue-agent-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases(id,workspace_id,name,governance) VALUES($1,$2,'queue-agent-test',TRUE)",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO users(id,org_id,email,password_hash,display_name,is_admin)
         VALUES($1,$2,$3,'','Queue Agent',TRUE)",
    )
    .bind(user)
    .bind(org)
    .bind(format!("queue-agent-{user}@test.local"))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'lease', 'Lease')",
    )
    .bind(etype)
    .bind(kb)
    .execute(&pool)
    .await?;
    for (id, key) in [(deadline, "option_deadline"), (tenant, "tenant_name")] {
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype, temporal, functional)
             VALUES ($1, $2, $3, $3, 'attribute', 'text', 'state', TRUE)",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .execute(&pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, 'HQ Lease')",
    )
    .bind(lease)
    .bind(kb)
    .bind(etype)
    .execute(&pool)
    .await?;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(Scripted)
        .mount(&server)
        .await;
    utopia_store::settings::upsert(
        &pool,
        ws,
        Some(&server.uri()),
        None,
        Some("scripted-chat"),
        None,
        None,
        None,
        None,
    )
    .await?;
    let dir = std::env::temp_dir().join(format!("utopia-queue-agent-{kb}"));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
    Ok(Some(Fx {
        state,
        pool,
        org,
        kb,
        user,
        lease,
        deadline,
        tenant,
        _server: server,
        dir,
    }))
}

impl Fx {
    /// 抽取的写法：一份自带日期的文档、一段引文、落库、写证据、对账
    async fn observe(
        &self,
        predicate: Uuid,
        value: &str,
        from: &str,
        doc_day: &str,
        quote: &str,
        confidence: f32,
    ) -> anyhow::Result<Uuid> {
        let (d, c) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256, doc_time, doc_time_source)
             VALUES ($1, $2, $3, $3, $4, 'content')",
        )
        .bind(d)
        .bind(self.kb)
        .bind(format!("doc-{doc_day}-{d}.html"))
        .bind(t(doc_day))
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, $4)",
        )
        .bind(c)
        .bind(self.kb)
        .bind(d)
        .bind(quote)
        .execute(&self.pool)
        .await?;
        let validity = Validity::starting(Some(t(from)), Some("day")).attested(Some(t(doc_day)));
        let object = json!({ "value": value });
        let (id, _) = utopia_store::graph::insert_value_fact(
            &self.pool,
            self.kb,
            self.lease,
            Some(predicate),
            &object,
            validity,
            confidence,
        )
        .await?;
        utopia_store::graph::add_evidence(&self.pool, id, c, Some(quote), None).await?;
        utopia_store::temporal::reconcile_new_fact(
            &self.pool,
            self.kb,
            id,
            self.lease,
            predicate,
            None,
            Some(&object),
            Uniqueness::SubjectSide,
            validity,
            confidence,
        )
        .await?;
        Ok(id)
    }

    /// (值, 起点, 终点, 置信度)，按起点排
    async fn timeline(
        &self,
        predicate: Uuid,
    ) -> anyhow::Result<Vec<(String, String, Option<String>, f32)>> {
        Ok(sqlx::query_as(
            "SELECT object_value ->> 'value', to_char(valid_from, 'YYYY-MM-DD'),
                    to_char(valid_to, 'YYYY-MM-DD'), confidence
             FROM facts WHERE kb_id = $1 AND predicate_id = $2 AND invalidated_at IS NULL
             ORDER BY valid_from, object_value ->> 'value'",
        )
        .bind(self.kb)
        .bind(predicate)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn open_conflicts(&self) -> anyhow::Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT reason FROM fact_conflicts WHERE kb_id = $1 AND status = 'open' ORDER BY reason",
        )
        .bind(self.kb)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn govern(&self) -> anyhow::Result<()> {
        crate::governance::govern(&self.state, self.kb).await
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
            .bind(self.kb)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(self.user)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        let _ = std::fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_settles_facts_and_conflicts_and_a_person_can_take_it_back() -> anyhow::Result<()>
{
    let Some(fx) = fixture().await? else {
        return Ok(());
    };
    fx.observe(
        fx.deadline,
        "2020-03-17",
        "2020-02-18",
        "2020-02-18",
        "(Phase 2 Exercise Deadline) | March 17, 2020",
        0.9,
    )
    .await?;
    fx.observe(
        fx.deadline,
        "2020-04-14",
        "2020-03-17",
        "2020-03-17",
        "(Phase 2 Exercise Deadline) | April 14, 2020",
        0.7,
    )
    .await?;
    fx.observe(
        fx.tenant,
        "Tower 9",
        "2019-01-01",
        "2019-01-01",
        "The landlord shall maintain the parking deck.",
        0.5,
    )
    .await?;
    fx.observe(
        fx.tenant,
        "A Corp",
        "2020-01-01",
        "2020-01-01",
        "A Corp is the tenant under the lease.",
        0.9,
    )
    .await?;
    fx.observe(
        fx.tenant,
        "B Corp",
        "2020-01-01",
        "2021-06-01",
        "B Corp became the tenant on June 1, 2021.",
        0.9,
    )
    .await?;
    let before = fx.open_conflicts().await?;
    assert!(before.contains(&"low_confidence".to_string()), "{before:?}");
    assert!(before.contains(&"simultaneous".to_string()), "{before:?}");

    fx.govern().await?;

    let deadlines = fx.timeline(fx.deadline).await?;
    assert_eq!(
        deadlines,
        vec![
            (
                "2020-03-17".into(),
                "2020-02-18".into(),
                Some("2020-03-17".into()),
                0.9
            ),
            ("2020-04-14".into(), "2020-03-17".into(), None, 1.0),
        ],
        "确认之后第六份接替了第五份"
    );
    let tenants = fx.timeline(fx.tenant).await?;
    assert!(
        tenants.iter().all(|r| r.0 != "Tower 9"),
        "原文没说的那条驳回了：{tenants:?}"
    );
    assert_eq!(
        tenants,
        vec![
            (
                "A Corp".into(),
                "2020-01-01".into(),
                Some("2021-06-01".into()),
                0.9
            ),
            ("B Corp".into(), "2021-06-01".into(), None, 0.9),
        ],
        "B Corp 的起点改成原文写的那天，A Corp 关在那一天"
    );
    assert_eq!(fx.open_conflicts().await?, Vec::<String>::new());

    let decisions: Vec<(Uuid, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, target_kind, action, status, summary FROM agent_decisions
          WHERE kb_id = $1 AND status = 'applied' ORDER BY created_at",
    )
    .bind(fx.kb)
    .fetch_all(&fx.pool)
    .await?;
    let applied: Vec<(&str, &str)> = decisions
        .iter()
        .map(|d| (d.1.as_str(), d.2.as_str()))
        .collect();
    assert_eq!(
        applied,
        vec![
            ("fact", "confirm"),
            ("fact", "reject"),
            ("conflict", "retime_new")
        ],
        "{decisions:?}"
    );
    assert!(
        decisions.iter().all(|d| d.4.is_some()),
        "每一笔都留了给人读的一行"
    );

    // 人把三笔都撤回
    for (id, ..) in &decisions {
        let d = gov::get(&fx.pool, fx.kb, *id).await?;
        answer(
            &fx.state,
            fx.kb,
            &d,
            "revert",
            fx.user,
            Some("checking by hand"),
        )
        .await?;
    }
    let deadlines = fx.timeline(fx.deadline).await?;
    assert_eq!(
        deadlines,
        vec![
            ("2020-03-17".into(), "2020-02-18".into(), None, 0.9),
            ("2020-04-14".into(), "2020-03-17".into(), None, 0.7),
        ],
        "撤回确认：回到 0.7，第五份重新开着"
    );
    let tenants = fx.timeline(fx.tenant).await?;
    assert!(
        tenants.iter().any(|r| r.0 == "Tower 9"),
        "撤回驳回：那条回来了"
    );
    assert!(
        tenants
            .iter()
            .any(|r| r.0 == "B Corp" && r.1 == "2020-01-01"),
        "撤回改起点：回到原样：{tenants:?}"
    );
    let after = fx.open_conflicts().await?;
    assert!(
        after.contains(&"simultaneous".to_string()),
        "那对同一天开始的冲突挂回来了：{after:?}"
    );
    let governance: bool =
        sqlx::query_scalar("SELECT governance FROM knowledge_bases WHERE id = $1")
            .bind(fx.kb)
            .fetch_one(&fx.pool)
            .await?;
    assert!(!governance, "撤回到第二次，保险丝关了开关");
    fx.cleanup().await
}
