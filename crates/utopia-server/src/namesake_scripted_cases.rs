//! Scripted namesake diagnostics: source identity is the oracle, never the model verdict.
//! Default CI checks an explicit known-debt ceiling; it does not certify gold correctness.
//! UTOPIA_NAMESAKE_STRICT=1 makes every gold violation fail, including existing defects.
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use utopia_store::resolution::ReviewStage;
use uuid::Uuid;

struct Case {
    id: &'static str,
    gold: [&'static str; 2],
    verdict: Option<&'static str>,
    // Only this named violation is allowed, at most once per case/order. Controls have none.
    debt: Option<&'static str>,
}

const CASES: [Case; 4] = [
    Case {
        id: "resolver_distinct",
        gold: ["employee_E17", "employee_F42"],
        verdict: None,
        debt: Some("false_merge"),
    },
    Case {
        id: "adjudicator_distinct_same",
        gold: ["employee_E17", "employee_F42"],
        verdict: Some("same"),
        debt: Some("false_merge"),
    },
    Case {
        id: "adjudicator_true_same",
        gold: ["employee_E17", "employee_E17"],
        verdict: Some("same"),
        debt: None,
    },
    Case {
        id: "adjudicator_distinct_different",
        gold: ["employee_E17", "employee_F42"],
        verdict: Some("different"),
        debt: None,
    },
];

struct Step {
    prompt_has: String,
    text: String,
    stream: bool,
}

#[derive(Clone, Default)]
struct Model {
    steps: Arc<Mutex<VecDeque<Step>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    errors: Arc<Mutex<Vec<String>>>,
}

async fn reply(State(m): State<Model>, Json(body): Json<Value>) -> axum::response::Response {
    m.requests.lock().unwrap().push(body.clone());
    let step = m.steps.lock().unwrap().pop_front();
    let error = match &step {
        None => Some("unexpected model request: script exhausted".to_string()),
        Some(s) if !body["messages"].to_string().contains(&s.prompt_has) => {
            Some(format!("unexpected prompt: missing {}", s.prompt_has))
        }
        Some(s) if (body["stream"] == true) != s.stream => {
            Some("unexpected chat transport".to_string())
        }
        _ => None,
    };
    if let Some(error) = error {
        // Handler panics alone can be lost in a detached task; the caller checks this ledger.
        m.errors.lock().unwrap().push(error.clone());
        return (StatusCode::BAD_REQUEST, error).into_response();
    }
    let step = step.unwrap();
    if !step.stream {
        return Json(
            json!({"choices":[{"message":{"role":"assistant","content":step.text},
            "finish_reason":"stop"}]}),
        )
        .into_response();
    }
    let frame = json!({"choices":[{"delta":{"content":step.text}}]});
    let done = json!({"choices":[{"delta":{},"finish_reason":"stop"}]});
    (
        [("content-type", "text/event-stream")],
        format!("data: {frame}\n\ndata: {done}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

struct Fx {
    pool: sqlx::PgPool,
    state: AppState,
    org: Uuid,
    kb: Uuid,
    docs: [Uuid; 2],
    chunks: [Uuid; 2],
    quotes: [String; 2],
    model: Model,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Fx {
    async fn new(pool: &sqlx::PgPool, case: &Case, org: Uuid) -> anyhow::Result<Self> {
        let (ws, kb) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query("INSERT INTO organizations(id,name) VALUES ($1,'scripted-namesake')")
            .bind(org)
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES ($1,$2,'scripted-namesake')")
            .bind(ws)
            .bind(org)
            .execute(pool)
            .await?;
        sqlx::query(
            "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ($1,$2,'scripted-namesake')",
        )
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
        let docs = [Uuid::now_v7(), Uuid::now_v7()];
        let chunks = [Uuid::now_v7(), Uuid::now_v7()];
        // Employee numbers are explicit source identities, not an ontology inference or a
        // production merge rule. Different workers can share a name and employer.
        let quotes = std::array::from_fn(|i| {
            format!(
                "Directory {}: Alex Lee (employee {}) works for Acme.",
                i + 1,
                case.gold[i].strip_prefix("employee_").unwrap()
            )
        });
        for i in 0..2 {
            sqlx::query("INSERT INTO documents(id,kb_id,filename,sha256) VALUES ($1,$2,$3,$4)")
                .bind(docs[i])
                .bind(kb)
                .bind(format!("directory-{i}.txt"))
                .bind(
                    Sha256::digest(quotes[i].as_bytes())
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>(),
                )
                .execute(pool)
                .await?;
            // Fixed context vectors exercise the real resolver without downloading an
            // embedding model or involving the independent name-vector recall channel.
            sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text,embedding) VALUES ($1,$2,$3,0,$4,'[1,0,0]'::vector)")
                .bind(chunks[i]).bind(kb).bind(docs[i]).bind(&quotes[i]).execute(pool).await?;
        }
        let model = Model::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let router = Router::new()
            .route("/chat/completions", post(reply))
            .with_state(model.clone());
        utopia_store::settings::upsert(
            pool,
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
        let config = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &config, search, "test-only".into());
        // Start only after fallible fixture initialization, so errors cannot leak a server.
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Ok(Self {
            pool: pool.clone(),
            state,
            org,
            kb,
            docs,
            chunks,
            quotes,
            model,
            server,
            _dir: dir,
        })
    }

    async fn seed(&self, i: usize) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO entities(id,kb_id,canonical_name,profile_embedding,profile_n) VALUES ($1,$2,'Alex Lee','[1,0,0]'::vector,1)")
            .bind(id).bind(self.kb).execute(&self.pool).await?;
        let fact = Uuid::now_v7();
        sqlx::query("INSERT INTO facts(id,kb_id,subject_id,layer,phrase,object_value) VALUES ($1,$2,$3,'open','works for','{\"value\":\"Acme\"}'::jsonb)")
            .bind(fact).bind(self.kb).bind(id).execute(&self.pool).await?;
        sqlx::query("INSERT INTO fact_evidence(fact_id,chunk_id,quote,document_id,doc_version) VALUES ($1,$2,$3,$4,1)")
            .bind(fact).bind(self.chunks[i]).bind(&self.quotes[i]).bind(self.docs[i]).execute(&self.pool).await?;
        Ok(id)
    }

    async fn run(&self, case: &Case, reverse: bool) -> anyhow::Result<Value> {
        let order = if reverse { [1, 0] } else { [0, 1] };
        let first = self.seed(order[0]).await?;
        if let Some(verdict) = case.verdict {
            let second = self.seed(order[1]).await?;
            utopia_store::resolution::create_review(
                &self.pool,
                self.kb,
                first,
                second,
                0.7,
                "scripted_same_name",
                ReviewStage::Adjudicating,
            )
            .await?;
            self.model.steps.lock().unwrap().push_back(Step {
                prompt_has: "Alex Lee".into(),
                stream: false,
                text: json!({"verdicts":[{"i":0,"verdict":verdict,"confidence":0.99,
                    "why":"same name and employer"}]})
                .to_string(),
            });
            crate::adjudication::adjudicate_entities(&self.state, self.kb).await?;
        } else {
            // Two bare-name candidates already exist. The wrong holder has the
            // uniquely highest profile score; source identity must still win.
            // Equal vectors would exercise the existing tie guard instead.
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name,profile_embedding,profile_n) VALUES ($1,$2,'Alex Lee','[0,1,0]'::vector,1)")
                .bind(Uuid::now_v7()).bind(self.kb).execute(&self.pool).await?;
            let i = order[1];
            self.model.steps.lock().unwrap().push_back(Step {
                prompt_has: self.quotes[i].clone(),
                stream: true,
                text: json!({"e":[["Alex Lee","person",1]],
                    "s":[[self.quotes[i],"Alex Lee","works for",null,"Acme",null,null,null]],
                    "n":[]})
                .to_string(),
            });
            crate::extraction::extract_document(&self.state, self.docs[i], Default::default())
                .await?;
            let status: String =
                sqlx::query_scalar("SELECT graph_status FROM documents WHERE id=$1")
                    .bind(self.docs[i])
                    .fetch_one(&self.pool)
                    .await?;
            anyhow::ensure!(status == "done", "extraction did not complete: {status}");
        }
        anyhow::ensure!(
            self.model.errors.lock().unwrap().is_empty(),
            "script errors: {:?}",
            self.model.errors.lock().unwrap()
        );
        anyhow::ensure!(
            self.model.steps.lock().unwrap().is_empty(),
            "unconsumed model script"
        );
        let model_calls = self.model.requests.lock().unwrap().len();
        anyhow::ensure!(
            model_calls == 1,
            "expected exactly one scripted request, got {model_calls}"
        );

        let mut actual = Vec::new();
        for doc in self.docs {
            // Read each source's fact owner after production resolution/merging, including
            // duplicate facts invalidated by merge deduplication: their moved subject
            // still records source identity attribution.
            // Missing/duplicate extraction is a harness/coverage failure, never known debt.
            let owners: Vec<Uuid> = sqlx::query_scalar(
                "SELECT DISTINCT COALESCE(e.merged_into,e.id) FROM facts f
                 JOIN fact_evidence fe ON fe.fact_id=f.id JOIN entities e ON e.id=f.subject_id
                 WHERE f.kb_id=$1 AND fe.document_id=$2 AND f.layer='open'
                   AND f.phrase='works for'",
            )
            .bind(self.kb)
            .bind(doc)
            .fetch_all(&self.pool)
            .await?;
            anyhow::ensure!(
                owners.len() == 1,
                "source {doc} has {} fact owners",
                owners.len()
            );
            actual.push(owners[0]);
        }
        let mut violations = Vec::new();
        if case.gold[0] == case.gold[1] && actual[0] != actual[1] {
            violations.push("false_split");
        }
        if case.gold[0] != case.gold[1] && actual[0] == actual[1] {
            violations.push("false_merge");
        }
        let entities: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM entities WHERE kb_id=$1 AND merged_into IS NULL",
        )
        .bind(self.kb)
        .fetch_one(&self.pool)
        .await?;
        let pending_human: i64 = sqlx::query_scalar("SELECT count(*) FROM resolution_reviews WHERE kb_id=$1 AND stage='human' AND status='pending'")
            .bind(self.kb).fetch_one(&self.pool).await?;
        let budget_violations = if pending_human > 0 {
            vec!["unexpected_human_review"]
        } else {
            Vec::new()
        };
        Ok(
            json!({"id":case.id,"order":if reverse {"reverse"} else {"forward"},
            "gold":case.gold,"actual":actual,"violations":violations,
            "budget_violations":budget_violations,
            "entities":entities,"pending_human":pending_human,"model_calls":model_calls}),
        )
    }

    async fn cleanup(&self) -> anyhow::Result<()> {
        self.server.abort();
        sqlx::query(
            "DELETE FROM jobs WHERE payload->>'kb_id'=$1 OR payload->>'document_id'=ANY($2)",
        )
        .bind(self.kb.to_string())
        .bind(self.docs.map(|d| d.to_string()).to_vec())
        .execute(&self.pool)
        .await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn scripted_namesake_cases_report_gold_violations_without_relaxing_the_oracle(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    // CI migrates before workspace tests; this fixture does not race a global migration.
    let strict = std::env::var("UTOPIA_NAMESAKE_STRICT").as_deref() == Ok("1");
    let mut failures = Vec::new();
    let mut total = 0;
    let mut budget_total = 0;
    for case in &CASES {
        for reverse in [false, true] {
            let org = Uuid::now_v7();
            let f = match Fx::new(&pool, case, org).await {
                Ok(f) => f,
                Err(error) => {
                    sqlx::query("DELETE FROM organizations WHERE id=$1")
                        .bind(org)
                        .execute(&pool)
                        .await?;
                    return Err(error);
                }
            };
            let run = tokio::time::timeout(Duration::from_secs(15), f.run(case, reverse)).await;
            f.cleanup().await?;
            let row = run.map_err(|_| anyhow::anyhow!("{} timed out", case.id))??;
            println!("NAMESAKE_CASE {row}");
            let violations = row["violations"].as_array().unwrap();
            let budget_violations = row["budget_violations"].as_array().unwrap();
            total += violations.len();
            budget_total += budget_violations.len();
            // Improvements are accepted. A new type, control failure or more than one
            // allowed failure is rejected; no assertion requires wrong behavior to remain.
            let within_debt = violations.len() <= usize::from(case.debt.is_some())
                && violations.iter().all(|v| v.as_str() == case.debt);
            if !within_debt || (strict && !violations.is_empty()) || !budget_violations.is_empty() {
                failures.push(format!(
                    "{} / {}: gold={} budget={}",
                    case.id, row["order"], row["violations"], row["budget_violations"]
                ));
            }
        }
    }
    println!(
        "NAMESAKE_SUMMARY {}",
        json!({"cases":8,"gold_violations":total,
        "budget_violations":budget_total,
        "mode":if strict {"strict_gold"} else {"known_debt_ceiling"},
        "gold_passed":total == 0,"unaccepted":failures})
    );
    anyhow::ensure!(
        failures.is_empty(),
        "namesake gold violations outside acceptance: {}",
        failures.join("; ")
    );
    Ok(())
}
