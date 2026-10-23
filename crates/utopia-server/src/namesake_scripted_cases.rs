//! Fixed, DB-backed namesake cases for #1136 and #1202; no provider or embedding calls.
//! Run with a migrated UTOPIA_DATABASE_URL and UTOPIA_TEST_REQUIRE_DB=1:
//! cargo test --locked -p utopia-server namesake_scripted_cases -- --nocapture
//! Normal cargo test --workspace runs this too; no CI flags or ignored tests.
//!
//! Resolver pairs have two existing candidates with different department facts.
//! The incoming passage names one department: it conflicts with the profile preference
//! in the failing case and agrees in the control. Gold is that candidate's exact ID,
//! so a resolver that always creates another entity fails the control.
//!
//! Adjudicator pairs test the real HTTP request, not a scripted model's judgement:
//! scalar employee numbers and entity-valued departments must reach their own Record
//! A/B sections. Fixed different replies only complete the production caller.
//! Both seed orders run independently. The named list requires today's known failures
//! to fail and every other check to pass; a fix removes its names from that list.
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

// Both profiles are viable: cosine 1.0 versus 0.8, outside the existing tie guard.
const DEPARTMENTS: [&str; 2] = ["Platform Engineering", "Finance"];
const EMPLOYEES: [&str; 2] = ["E17", "F42"];

#[derive(Clone, Copy)]
enum Check {
    Resolver { target: usize },
    Adjudicator { literal: bool },
    Guard(Guard),
    ObjectOnly,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Guard {
    Unrelated,
    CrossSentence,
    SharedQuote,
    OverlappingQuotes,
    SharedDepartment,
    ConflictingQuotes,
    MissingQuote,
    UngroundedQuote,
    TypedToUntyped,
    UntypedToTyped,
    IncompatibleType,
    Markdown,
}

struct Case {
    id: &'static str,
    check: Check,
}

const CASES: &[Case] = &[
    Case {
        id: "object_only_mention_without_owned_quote_keeps_tie_review",
        check: Check::ObjectOnly,
    },
    Case {
        id: "resolver_passage_over_profile",
        check: Check::Resolver { target: 1 },
    },
    Case {
        id: "resolver_passage_agrees_with_profile",
        check: Check::Resolver { target: 0 },
    },
    Case {
        id: "adjudicator_literal_clues_reach_request",
        check: Check::Adjudicator { literal: true },
    },
    Case {
        id: "adjudicator_entity_clues_reach_request",
        check: Check::Adjudicator { literal: false },
    },
    Case {
        id: "unrelated_sentence_is_not_evidence",
        check: Check::Guard(Guard::Unrelated),
    },
    Case {
        id: "cross_sentence_quote_is_not_evidence",
        check: Check::Guard(Guard::CrossSentence),
    },
    Case {
        id: "two_subjects_share_a_quote",
        check: Check::Guard(Guard::SharedQuote),
    },
    Case {
        id: "two_subjects_have_overlapping_quotes",
        check: Check::Guard(Guard::OverlappingQuotes),
    },
    Case {
        id: "shared_department_is_not_unique",
        check: Check::Guard(Guard::SharedDepartment),
    },
    Case {
        id: "conflicting_quotes_do_not_choose",
        check: Check::Guard(Guard::ConflictingQuotes),
    },
    Case {
        id: "missing_quote_does_not_choose",
        check: Check::Guard(Guard::MissingQuote),
    },
    Case {
        id: "ungrounded_quote_does_not_choose",
        check: Check::Guard(Guard::UngroundedQuote),
    },
    Case {
        id: "typed_mention_can_choose_untyped_namesake",
        check: Check::Guard(Guard::TypedToUntyped),
    },
    Case {
        id: "untyped_mention_can_choose_typed_namesake",
        check: Check::Guard(Guard::UntypedToTyped),
    },
    Case {
        id: "incompatible_type_is_not_chosen",
        check: Check::Guard(Guard::IncompatibleType),
    },
    Case {
        id: "markdown_clue_recalls_the_stored_spelling",
        check: Check::Guard(Guard::Markdown),
    },
];

// #1136: a listed case MUST fail today; every other case MUST pass. A fix removes
// the corresponding names with its code change, so success cannot silently go stale.
const KNOWN_FAILURES: [(&str, &str); 0] = [];

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
    docs: [Uuid; 3],
    chunks: [Uuid; 3],
    quotes: [String; 3],
    departments: [Uuid; 2],
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
        let docs = std::array::from_fn(|_| Uuid::now_v7());
        let chunks = std::array::from_fn(|_| Uuid::now_v7());
        let departments = [Uuid::now_v7(), Uuid::now_v7()];
        for i in 0..2 {
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,$3)")
                .bind(departments[i])
                .bind(kb)
                .bind(DEPARTMENTS[i])
                .execute(pool)
                .await?;
        }
        let quotes = std::array::from_fn(|i| match (case.check, i) {
            (Check::Adjudicator { literal: true }, 0..=1) => format!(
                "Directory {}: Alex Lee has employee number {}.",
                i + 1,
                EMPLOYEES[i]
            ),
            (_, 0..=1) => format!(
                "Directory {}: Alex Lee works for {}.",
                i + 1,
                DEPARTMENTS[i]
            ),
            (Check::Resolver { target }, _) => {
                format!("Alex Lee from {} submitted a report.", DEPARTMENTS[target])
            }
            _ => "Unused incoming passage.".into(),
        });
        for i in 0..3 {
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
            departments,
            model,
            server,
            _dir: dir,
        })
    }

    async fn seed(&self, i: usize, check: Check) -> anyhow::Result<Uuid> {
        let id = Uuid::now_v7();
        let profile = if i == 0 { "[1,0,0]" } else { "[0.8,0.6,0]" };
        sqlx::query("INSERT INTO entities(id,kb_id,canonical_name,profile_embedding,profile_n) VALUES ($1,$2,'Alex Lee',$3::vector,1)")
            .bind(id).bind(self.kb).bind(profile).execute(&self.pool).await?;
        let fact = Uuid::now_v7();
        if matches!(check, Check::Adjudicator { literal: true }) {
            sqlx::query("INSERT INTO facts(id,kb_id,subject_id,layer,phrase,object_value) VALUES ($1,$2,$3,'open','employee number',$4)")
                .bind(fact).bind(self.kb).bind(id).bind(json!({"value":EMPLOYEES[i]})).execute(&self.pool).await?;
        } else {
            sqlx::query("INSERT INTO facts(id,kb_id,subject_id,layer,phrase,object_id) VALUES ($1,$2,$3,'open','works for',$4)")
                .bind(fact).bind(self.kb).bind(id).bind(self.departments[i]).execute(&self.pool).await?;
        }
        sqlx::query("INSERT INTO fact_evidence(fact_id,chunk_id,quote,document_id,doc_version) VALUES ($1,$2,$3,$4,1)")
            .bind(fact).bind(self.chunks[i]).bind(&self.quotes[i]).bind(self.docs[i]).execute(&self.pool).await?;
        Ok(id)
    }

    async fn run(&self, case: &Case, reverse: bool) -> anyhow::Result<Value> {
        let order = if reverse { [1, 0] } else { [0, 1] };
        let mut holders = [Uuid::nil(); 2];
        for i in order {
            holders[i] = self.seed(i, case.check).await?;
        }
        let (passed, observation) = match case.check {
            Check::Guard(guard) => self.guard(guard, holders).await?,
            Check::ObjectOnly => self.object_only(holders).await?,
            Check::Resolver { target } => {
                self.model.steps.lock().unwrap().push_back(Step {
                    prompt_has: self.quotes[2].clone(), stream: true,
                    text: json!({"e":[["Alex Lee","person",1]],
                        "s":[[self.quotes[2],"Alex Lee","submitted",null,"a report",null,null,null]],
                        "n":[]}).to_string(),
                });
                crate::extraction::extract_document(&self.state, self.docs[2], Default::default())
                    .await?;
                let status: String =
                    sqlx::query_scalar("SELECT graph_status FROM documents WHERE id=$1")
                        .bind(self.docs[2])
                        .fetch_one(&self.pool)
                        .await?;
                anyhow::ensure!(status == "done", "extraction did not complete: {status}");
                let owners: Vec<Uuid> = sqlx::query_scalar(
                    "SELECT DISTINCT f.subject_id FROM facts f JOIN fact_evidence fe ON fe.fact_id=f.id
                     WHERE f.kb_id=$1 AND fe.document_id=$2 AND f.layer='open' AND f.phrase='submitted'")
                    .bind(self.kb).bind(self.docs[2]).fetch_all(&self.pool).await?;
                anyhow::ensure!(
                    owners.len() == 1,
                    "incoming source has {} owners",
                    owners.len()
                );
                // Exact pre-existing ID: creating a new entity is not successful disambiguation.
                (
                    owners[0] == holders[target],
                    json!({"expected_owner":holders[target],
                    "actual_owner":owners[0],"passage":self.quotes[2]}),
                )
            }
            Check::Adjudicator { literal } => {
                utopia_store::resolution::create_review(
                    &self.pool,
                    self.kb,
                    holders[order[0]],
                    holders[order[1]],
                    0.7,
                    "scripted_same_name",
                    ReviewStage::Adjudicating,
                )
                .await?;
                self.model.steps.lock().unwrap().push_back(Step {
                    prompt_has: "Alex Lee".into(),
                    stream: false,
                    // The reply only completes the caller. We test its INPUT, not model judgement.
                    text: json!({"verdicts":[{"i":0,"verdict":"different","confidence":0.99,
                        "why":"scripted transport reply"}]})
                    .to_string(),
                });
                crate::adjudication::adjudicate_entities(&self.state, self.kb).await?;
                let requests = self.model.requests.lock().unwrap();
                let user = requests
                    .first()
                    .and_then(|r| r["messages"].as_array())
                    .and_then(|m| m.iter().find(|m| m["role"] == "user"))
                    .and_then(|m| m["content"].as_str())
                    .ok_or_else(|| anyhow::anyhow!("missing user request"))?;
                let pair = user
                    .strip_prefix("Pair 0:\nRecord A: ")
                    .ok_or_else(|| anyhow::anyhow!("missing first record"))?;
                let (left, right) = pair
                    .split_once("\nRecord B: ")
                    .ok_or_else(|| anyhow::anyhow!("missing second record"))?;
                let (phrase, clues) = if literal {
                    ("employee number", EMPLOYEES)
                } else {
                    ("works for", DEPARTMENTS)
                };
                let expected = order.map(|i| format!("{phrase} → {}", clues[i]));
                // Check each real HTTP side independently: overall contains would allow swaps.
                let visible = left.contains(&expected[0]) && right.contains(&expected[1]);
                let correctly_owned =
                    !left.contains(clues[order[1]]) && !right.contains(clues[order[0]]);
                (
                    visible && correctly_owned,
                    json!({"expected_facts":expected,"request_sides":[left,right]}),
                )
            }
        };
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
            "expected one scripted request, got {model_calls}"
        );
        let pending_human: i64 = sqlx::query_scalar("SELECT count(*) FROM resolution_reviews WHERE kb_id=$1 AND stage='human' AND status='pending'")
            .bind(self.kb).fetch_one(&self.pool).await?;
        anyhow::ensure!(
            pending_human
                == if matches!(case.check, Check::ObjectOnly) {
                    2
                } else {
                    0
                },
            "unexpected pending Human reviews: {pending_human}"
        );
        Ok(
            json!({"id":case.id,"order":if reverse {"reverse"} else {"forward"},
            "passed":passed,"observation":observation,"pending_human":pending_human,"model_calls":model_calls}),
        )
    }

    async fn object_only(&self, holders: [Uuid; 2]) -> anyhow::Result<(bool, Value)> {
        let text = "Finance assigned a report to Alex Lee.";
        // Both namesakes tie. Finance is the statement's subject, so its quote
        // cannot rescue the object through the old #331 whole-chunk tie shortcut.
        sqlx::query("UPDATE entities SET profile_embedding='[1,0,0]'::vector WHERE id=$1")
            .bind(holders[1])
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE chunks SET text=$1 WHERE id=$2")
            .bind(text)
            .bind(self.chunks[2])
            .execute(&self.pool)
            .await?;
        self.model.steps.lock().unwrap().push_back(Step {
            prompt_has: text.into(),
            stream: true,
            text: json!({"e":[["Alex Lee","person",1],["Finance","department",1]],
                "s":[[text,"Finance","assigned","Alex Lee",null,null,null,null]],"n":[]})
            .to_string(),
        });
        crate::extraction::extract_document(&self.state, self.docs[2], Default::default()).await?;
        let owners: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT f.object_id FROM facts f JOIN fact_evidence fe ON fe.fact_id=f.id WHERE f.kb_id=$1 AND fe.document_id=$2 AND f.layer='open' AND f.phrase='assigned'")
            .bind(self.kb).bind(self.docs[2]).fetch_all(&self.pool).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM entities WHERE kb_id=$1 AND canonical_name='Alex Lee' AND merged_into IS NULL")
            .bind(self.kb).fetch_one(&self.pool).await?;
        Ok((
            owners.len() == 1 && !holders.contains(&owners[0]) && count == 3,
            json!({"original_candidates":holders,"object_owners":owners,"entities":count}),
        ))
    }

    async fn guard(&self, guard: Guard, holders: [Uuid; 2]) -> anyhow::Result<(bool, Value)> {
        use Guard::*;
        let simple = "Alex Lee submitted a report.";
        let finance = "Alex Lee from Finance submitted a report.";
        let unrelated = "Alex Lee submitted a report. Finance reviewed the annual budget.";
        let shared = "Alex Lee submitted a report while Bob Chen from Finance approved it.";
        let conflicting = "Alex Lee from Platform Engineering submitted a report. Alex Lee from Finance submitted a report.";
        let text = match guard {
            Markdown => "**Alex Lee** from Finance submitted a report.",
            UngroundedQuote => simple,
            Unrelated | CrossSentence => unrelated,
            SharedQuote | OverlappingQuotes => shared,
            ConflictingQuotes => conflicting,
            _ => finance,
        };
        let quote = match guard {
            MissingQuote => None,
            Unrelated => Some(simple),
            UngroundedQuote => Some(finance),
            ConflictingQuotes => Some("Alex Lee from Platform Engineering submitted a report."),
            _ => Some(text),
        };
        let mut entities = vec![json!(["Alex Lee", "person", 1])];
        let mut statements = vec![json!([
            quote,
            "Alex Lee",
            "submitted",
            null,
            "a report",
            null,
            null,
            null
        ])];
        match guard {
            Markdown => {
                // Both the folded name and a legacy formatted spelling are exact
                // candidates; limiting corroboration to one spelling loses the clue.
                entities[0] = json!(["**Alex Lee**", "person", 1]);
                statements[0][1] = json!("**Alex Lee**");
                sqlx::query("UPDATE entities SET canonical_name='**Alex Lee**' WHERE id=$1")
                    .bind(holders[1])
                    .execute(&self.pool)
                    .await?;
            }
            Unrelated => {
                entities.push(json!(["Finance", "department", 1]));
                statements.push(json!([
                    "Finance reviewed the annual budget.",
                    "Finance",
                    "reviewed",
                    null,
                    "the annual budget",
                    null,
                    null,
                    null
                ]));
            }
            SharedQuote | OverlappingQuotes => {
                entities.push(json!(["Bob Chen", "person", 1]));
                let other_quote = if guard == SharedQuote {
                    shared
                } else {
                    "Bob Chen from Finance approved it."
                };
                statements.push(json!([
                    other_quote,
                    "Bob Chen",
                    "approved",
                    null,
                    "it",
                    null,
                    null,
                    null
                ]));
            }
            ConflictingQuotes => statements.push(json!([
                finance,
                "Alex Lee",
                "submitted",
                null,
                "a report",
                null,
                null,
                null
            ])),
            SharedDepartment => {
                sqlx::query("UPDATE facts SET object_id=$1 WHERE kb_id=$2 AND subject_id=$3 AND layer='open'")
                    .bind(self.departments[1]).bind(self.kb).bind(holders[0]).execute(&self.pool).await?;
            }
            _ => {}
        }
        if matches!(guard, TypedToUntyped | UntypedToTyped | IncompatibleType) {
            let person = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO entity_types(id,kb_id,key,label) VALUES ($1,$2,'person','Person')",
            )
            .bind(person)
            .bind(self.kb)
            .execute(&self.pool)
            .await?;
            let typed_holder = if guard == UntypedToTyped {
                holders[1]
            } else {
                holders[0]
            };
            sqlx::query("UPDATE entities SET type_id=$1 WHERE id=$2")
                .bind(person)
                .bind(typed_holder)
                .execute(&self.pool)
                .await?;
            if guard != UntypedToTyped {
                utopia_store::type_bindings::decide(
                    &self.pool,
                    self.kb,
                    "person",
                    &[],
                    Some(person),
                    "bound",
                    &json!({}),
                    "person",
                )
                .await?;
            }
            if guard == IncompatibleType {
                let organization = Uuid::now_v7();
                sqlx::query("INSERT INTO entity_types(id,kb_id,key,label) VALUES ($1,$2,'organization','Organization')")
                    .bind(organization).bind(self.kb).execute(&self.pool).await?;
                sqlx::query("UPDATE entities SET type_id=$1 WHERE id=$2")
                    .bind(organization)
                    .bind(holders[1])
                    .execute(&self.pool)
                    .await?;
            }
        }
        sqlx::query("UPDATE chunks SET text=$1 WHERE id=$2")
            .bind(text)
            .bind(self.chunks[2])
            .execute(&self.pool)
            .await?;
        let expected =
            holders[usize::from(matches!(guard, TypedToUntyped | UntypedToTyped | Markdown))];
        self.model.steps.lock().unwrap().push_back(Step {
            prompt_has: text.into(),
            stream: true,
            text: json!({"e":entities,"s":statements,"n":[]}).to_string(),
        });
        crate::extraction::extract_document(&self.state, self.docs[2], Default::default()).await?;
        let owners: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT f.subject_id FROM facts f JOIN fact_evidence fe ON fe.fact_id=f.id WHERE f.kb_id=$1 AND fe.document_id=$2 AND f.layer='open' AND f.phrase='submitted'")
            .bind(self.kb).bind(self.docs[2]).fetch_all(&self.pool).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM entities WHERE kb_id=$1 AND canonical_name IN ('Alex Lee','**Alex Lee**') AND merged_into IS NULL")
            .bind(self.kb).fetch_one(&self.pool).await?;
        let reviews: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM resolution_reviews WHERE kb_id=$1 AND status='pending'",
        )
        .bind(self.kb)
        .fetch_one(&self.pool)
        .await?;
        let type_preserved = if guard == TypedToUntyped {
            sqlx::query_scalar::<_, bool>("SELECT type_id IS NULL FROM entities WHERE id=$1")
                .bind(expected)
                .fetch_one(&self.pool)
                .await?
        } else {
            true
        };
        Ok((
            owners == vec![expected] && count == 2 && reviews == 0 && type_preserved,
            json!({"expected_owner":expected,"owners":owners,"entities":count,"pending_reviews":reviews,"type_preserved":type_preserved}),
        ))
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
async fn scripted_namesake_cases_have_only_the_named_known_failures() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    // CI migrates first; this fixture must not race a global migration.
    let mut mismatches = Vec::new();
    let mut seen = Vec::new();
    let mut failed = 0;
    for case in CASES {
        for reverse in [false, true] {
            let order = if reverse { "reverse" } else { "forward" };
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
            let passed = row["passed"].as_bool().unwrap();
            failed += usize::from(!passed);
            let name = (case.id, order);
            seen.push(name);
            if KNOWN_FAILURES.contains(&name) == passed {
                mismatches.push(format!(
                    "{} / {order}: {}",
                    case.id,
                    if passed {
                        "unexpected pass: remove this name with its fix"
                    } else {
                        "unlisted failure"
                    }
                ));
            }
        }
    }
    anyhow::ensure!(
        KNOWN_FAILURES.iter().all(|name| seen.contains(name)),
        "unused known-failure name"
    );
    println!(
        "NAMESAKE_SUMMARY {}",
        json!({"cases":seen.len(),"failed_checks":failed,
        "known_failures":KNOWN_FAILURES,"mismatches":mismatches})
    );
    anyhow::ensure!(
        mismatches.is_empty(),
        "namesake expectation mismatch: {}",
        mismatches.join("; ")
    );
    Ok(())
}
