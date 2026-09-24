//! 勘误 agent 走脚本化的模型端点：结构报了的先送去看，撤落地、keep 记账、引文不是原话的加
//! 被拒；账上记着请求数与端点报的 token；看完的文档不再问。没有 `UTOPIA_DATABASE_URL` 时跳过。
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Model {
    replies: Arc<Mutex<Vec<Value>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}
/// 回一段流，最后一帧带用量——和 OpenAI 协议的 `stream_options.include_usage` 一样
async fn reply(State(m): State<Model>, Json(body): Json<Value>) -> impl IntoResponse {
    m.requests.lock().unwrap().push(body);
    let text = {
        let mut replies = m.replies.lock().unwrap();
        if replies.is_empty() {
            panic!("unexpected model request");
        }
        replies.remove(0).to_string()
    };
    let frame = json!({"choices":[{"delta":{"content":text}}]});
    let done = json!({"choices":[{"delta":{},"finish_reason":"stop"}]});
    let usage = json!({"choices":[],"usage":{"prompt_tokens":123,"completion_tokens":45}});
    (
        [("content-type", "text/event-stream")],
        format!("data: {frame}\n\ndata: {done}\n\ndata: {usage}\n\ndata: [DONE]\n\n"),
    )
}

struct Fx {
    pool: sqlx::PgPool,
    state: AppState,
    org: Uuid,
    kb: Uuid,
    doc: Uuid,
    fine: Uuid,
    absent: Uuid,
    model: Model,
    _server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Fx {
    /// 一个库：organization / place / person；based_in、ceo；文档「Acme is based in London. Jane Roe runs Acme.」；
    /// 两条类型化行：Acme —based_in→ London（对）、Acme —based_in→ Paris（Paris 不在文档里）
    async fn new(replies: Vec<Value>) -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let ids: Vec<Uuid> = (0..18).map(|_| Uuid::now_v7()).collect();
        let (org, ws, kb, doc, chunk, organization, place, person, based_in, ceo) = (
            ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6], ids[7], ids[8], ids[9],
        );
        let (acme, london, paris, jane, s1, t1, s2, t2) = (
            ids[10], ids[11], ids[12], ids[13], ids[14], ids[15], ids[16], ids[17],
        );
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','errata-server');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','errata-server');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','errata-server');
             INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','acme.txt','x');
             INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES
                 ('{chunk}','{kb}','{doc}',0,'Acme is based in London. Jane Roe runs Acme.');
             INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES
                 ('{organization}','{kb}','organization','Organization','#000','circle'),
                 ('{place}','{kb}','place','Place','#000','circle'),
                 ('{person}','{kb}','person','Person','#000','circle');
             INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,functional,description) VALUES
                 ('{based_in}','{kb}','based_in','based in','relation','state',false,'where an organization is based'),
                 ('{ceo}','{kb}','ceo','chief executive','relation','state',true,'who runs it');
             INSERT INTO relation_type_domains(relation_type_id,entity_type_id) VALUES
                 ('{based_in}','{organization}'), ('{ceo}','{organization}');
             INSERT INTO relation_type_ranges(relation_type_id,entity_type_id) VALUES
                 ('{based_in}','{place}'), ('{ceo}','{person}');
             INSERT INTO entities(id,kb_id,canonical_name,type_id) VALUES
                 ('{acme}','{kb}','Acme','{organization}'), ('{london}','{kb}','London','{place}'),
                 ('{paris}','{kb}','Paris','{place}'), ('{jane}','{kb}','Jane Roe','{person}');
             INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES
                 ('{s1}','{kb}','{acme}','{london}','open','based in'),
                 ('{s2}','{kb}','{acme}','{paris}','open','based in');
             INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,layer,from_statement_id) VALUES
                 ('{t1}','{kb}','{acme}','{based_in}','{london}','typed','{s1}'),
                 ('{t2}','{kb}','{acme}','{based_in}','{paris}','typed','{s2}');
             INSERT INTO typed_fact_sources(fact_id,statement_id) VALUES ('{t1}','{s1}'), ('{t2}','{s2}');
             INSERT INTO fact_evidence(fact_id,chunk_id,quote,document_id,doc_version) VALUES
                 ('{s1}','{chunk}','based in London','{doc}',1), ('{t1}','{chunk}','based in London','{doc}',1),
                 ('{s2}','{chunk}','based in','{doc}',1), ('{t2}','{chunk}','based in','{doc}',1);"
        ))
        .execute(&pool)
        .await?;
        let model = Model {
            replies: Arc::new(Mutex::new(replies)),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let router = Router::new()
            .route("/chat/completions", post(reply))
            .with_state(model.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        utopia_store::settings::upsert(
            &pool,
            ws,
            Some(&endpoint),
            None,
            Some("scripted"),
            None,
            None,
            None,
            None,
        )
        .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
        Ok(Some(Self {
            pool,
            state,
            org,
            kb,
            doc,
            fine: t1,
            absent: t2,
            model,
            _server: server,
            _dir: dir,
        }))
    }
    async fn cleanup(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(self.kb.to_string())
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    fn prompt_of(&self, n: usize) -> String {
        self.model.requests.lock().unwrap()[n]["messages"][1]["content"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
    async fn live(&self, fact: Uuid) -> anyhow::Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT invalidated_at IS NULL FROM facts WHERE id=$1")
                .bind(fact)
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

#[tokio::test]
async fn the_agent_reviews_flagged_facts_first_and_each_verdict_is_a_recorded_action(
) -> anyhow::Result<()> {
    // 0 是报了的 Paris 行（在前），1 是对的 London 行；加一条 ceo（原话在），再加一条原话不在的
    let script = json!({"a":[
        [0,"retract","Paris is not in the document","Acme is based in London"],
        [1,"keep"],
        [null,"add",{"subject":"Acme","property":"ceo","object":"Jane Roe"},"stated","Jane Roe runs Acme"],
        [null,"add",{"subject":"Acme","property":"ceo","object":"Jane Roe"},"made up","Jane Roe owns Acme"]
    ]});
    // 第二票：Paris 那条结构报了，agent 要撤，再问一遍；答 not_stated 才撤
    let second = json!({"c":[[0,"not_stated"]]});
    let Some(f) = Fx::new(vec![script, second]).await? else {
        return Ok(());
    };
    let run = async {
        review(&f.state, f.kb).await?;
        let prompt = f.prompt_of(0);
        let confirm = f.prompt_of(1);
        assert!(confirm.contains("0: Acme —based_in→ Paris") && !confirm.contains("London (place)"), "{confirm}");
        assert!(prompt.contains("DOCUMENT:\nAcme is based in London. Jane Roe runs Acme."), "{prompt}");
        assert!(prompt.contains("0: Acme (organization) —based_in→ Paris (place) FLAG name_absent"), "{prompt}");
        assert!(prompt.contains("1: Acme (organization) —based_in→ London (place)"), "{prompt}");
        assert!(prompt.contains("- ceo (chief executive): who runs it [thing] subject: organization object: person"), "{prompt}");
        assert!(!f.live(f.absent).await?, "the retraction landed");
        assert!(f.live(f.fine).await?);
        let actions: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT action, status, detail FROM errata_actions WHERE kb_id=$1 ORDER BY created_at, id",
        )
        .bind(f.kb)
        .fetch_all(&f.pool)
        .await?;
        assert_eq!(
            actions,
            vec![
                ("retract".into(), "applied".into(), None),
                ("keep".into(), "applied".into(), None),
                ("add".into(), "applied".into(), None),
                ("add".into(), "refused".into(), Some("the quote is not in the document".into())),
            ]
        );
        let ceo: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM facts f JOIN relation_types r ON r.id = f.predicate_id
              WHERE f.kb_id=$1 AND r.key='ceo' AND f.invalidated_at IS NULL",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        assert_eq!(ceo, 1, "one Jane Roe row: the refused add wrote nothing");
        // 账：一次请求，端点报的用量
        let run_row: (i32, i32, i32, Option<i64>, Option<i64>, bool) = sqlx::query_as(
            "SELECT flagged, sampled, requests, prompt_tokens, completion_tokens, finished_at IS NOT NULL
               FROM errata_runs WHERE kb_id=$1",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        assert_eq!(run_row, (1, 1, 2, Some(246), Some(90), true), "two requests: the verdicts and the second vote");
        // 看完了：再跑不问模型（脚本空了，问了就 panic），也不排下一次
        review(&f.state, f.kb).await?;
        assert_eq!(f.model.requests.lock().unwrap().len(), 2);
        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind=$1 AND payload->>'kb_id'=$2",
        )
        .bind(utopia_store::errata::JOB_KIND)
        .bind(f.kb.to_string())
        .fetch_one(&f.pool)
        .await?;
        assert_eq!(queued, 0);
        let _ = f.doc;
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn an_unreadable_reply_leaves_the_facts_unreviewed_and_the_job_retries() -> anyhow::Result<()>
{
    let Some(f) = Fx::new(vec![json!("not the protocol")]).await? else {
        return Ok(());
    };
    let run = async {
        let err = review(&f.state, f.kb)
            .await
            .expect_err("the job reports the failure");
        assert!(err.to_string().contains("failed errata review"), "{err}");
        let actions: i64 = sqlx::query_scalar("SELECT count(*) FROM errata_actions WHERE kb_id=$1")
            .bind(f.kb)
            .fetch_one(&f.pool)
            .await?;
        assert_eq!(actions, 0);
        assert!(f.live(f.absent).await? && f.live(f.fine).await?);
        // 账还是记的：问了一次，没看成
        let run_row: (i32, bool) = sqlx::query_as(
            "SELECT requests, finished_at IS NOT NULL FROM errata_runs WHERE kb_id=$1",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        assert_eq!(run_row, (1, true));
        assert_eq!(
            utopia_store::errata::documents_due(&f.pool, f.kb, 10).await?,
            vec![f.doc]
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn a_retraction_the_second_vote_calls_stated_becomes_a_keep() -> anyhow::Result<()> {
    let script = json!({"a":[
        [0,"retract","Paris is not in the document","Acme is based in London"],
        [1,"keep"]
    ]});
    let second = json!({"c":[[0,"stated"]]});
    let Some(f) = Fx::new(vec![script, second]).await? else {
        return Ok(());
    };
    let run = async {
        review(&f.state, f.kb).await?;
        assert!(f.live(f.absent).await?, "one vote does not retract");
        let actions: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT action, status, reason FROM errata_actions WHERE kb_id=$1 ORDER BY created_at, id",
        )
        .bind(f.kb)
        .fetch_all(&f.pool)
        .await?;
        assert_eq!(actions[0].0, "keep");
        assert_eq!(actions[0].1, "applied");
        assert!(actions[0].2.starts_with("second vote: stated"), "{}", actions[0].2);
        assert_eq!(actions.len(), 2);
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}
