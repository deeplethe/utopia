//! Decision-path measurements for #1193, independent of the original eight cases.
//! These assert evidence delivery and graph/review consistency, not the intelligence
//! of a scripted reply. GOLD_ERROR is reported explicitly, without adding any name
//! to KNOWN_FAILURES or treating a wrong merge as a successful identity decision.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Path {
    Batch,
    BatchHigh,
    Cached,
    Facts,
    Quotes,
    QuotesSplit,
}

fn tool(name: &str, args: Value, id: usize) -> Value {
    json!({"id":format!("call_{id}"),"type":"function",
        "function":{"name":name,"arguments":args.to_string()}})
}

impl Fx {
    async fn measure_decision(
        &self,
        path: Path,
        governance: bool,
        gold_same: bool,
        reverse: bool,
    ) -> anyhow::Result<Value> {
        let order = if reverse { [1, 0] } else { [0, 1] };
        let mut holders = [Uuid::nil(); 2];
        // The fixture register explicitly identifies one person per employee number.
        // Gold is the register's identity, never inferred from a model verdict or an
        // ontology uniqueness declaration. The open graph has no such declaration.
        let numbers = if gold_same { ["E17", "E17"] } else { EMPLOYEES };
        let quotes = numbers.map(|n| {
            format!("Register R assigns one number per person and one person per number: Alex Lee is employee {n}.")
        });
        for i in order {
            holders[i] = self.seed(i, Check::Adjudicator { literal: true }).await?;
            sqlx::query(
                "UPDATE facts SET object_value=$1 WHERE subject_id=$2 AND phrase='employee number'",
            )
            .bind(json!({"value":numbers[i]}))
            .bind(holders[i])
            .execute(&self.pool)
            .await?;
            sqlx::query("UPDATE chunks SET text=$1 WHERE id=$2")
                .bind(&quotes[i])
                .bind(self.chunks[i])
                .execute(&self.pool)
                .await?;
            sqlx::query("UPDATE fact_evidence SET quote=$1 WHERE chunk_id=$2")
                .bind(&quotes[i])
                .bind(self.chunks[i])
                .execute(&self.pool)
                .await?;
        }
        sqlx::query("UPDATE knowledge_bases SET governance=$1 WHERE id=$2")
            .bind(governance)
            .bind(self.kb)
            .execute(&self.pool)
            .await?;
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
        if matches!(path, Path::Cached) {
            let items =
                utopia_store::resolution::pending_adjudications(&self.pool, self.kb, 1).await?;
            anyhow::ensure!(items.len() == 1, "missing pending pair");
            let p =
                utopia_store::governance::precedents_for(&self.pool, self.kb, &items[0]).await?;
            let key = crate::adjudication::seed_measurement_cache_key(
                &items[0],
                &utopia_store::governance::render_lines(&p),
            );
            utopia_store::resolution::put_verdict(
                &self.pool,
                self.kb,
                &key,
                Some(true),
                0.8,
                "scripted",
            )
            .await?;
        }
        // Governance deliberately does not consume the ordinary adjudicator cache.
        let asks_batch = governance || !matches!(path, Path::Cached);
        let reads_evidence = matches!(path, Path::Facts | Path::Quotes | Path::QuotesSplit);
        let confidence = if reads_evidence {
            0.6
        } else if matches!(path, Path::BatchHigh) {
            0.9
        } else {
            0.8
        };
        let investigates = reads_evidence || (governance && confidence < 0.85);
        if asks_batch {
            self.model.steps.lock().unwrap().push_back(Step {
                prompt_has: "Alex Lee".into(),
                stream: false,
                text: json!({"verdicts":[{"i":0,"verdict":"same",
                    "confidence":confidence,
                    "why":"scripted same, independent of source gold"}]})
                .to_string(),
            });
        }
        if investigates {
            if reads_evidence {
                let name = if matches!(path, Path::Quotes | Path::QuotesSplit) {
                    "quotes"
                } else {
                    "facts"
                };
                let calls = ["A", "B"]
                    .into_iter()
                    .enumerate()
                    .map(|(i, side)| tool(name, json!({"side":side}), i))
                    .collect::<Vec<_>>();
                self.model.steps.lock().unwrap().push_back(Step {
                    prompt_has: "Alex Lee".into(),
                    stream: false,
                    text: json!({"role":"assistant","content":null,"tool_calls":calls}).to_string(),
                });
            }
            self.model.steps.lock().unwrap().push_back(Step {
                prompt_has: "Alex Lee".into(),
                stream: false,
                text: json!({"role":"assistant","content":null,"tool_calls":[tool("decide", json!({
                    "verdict":if matches!(path, Path::QuotesSplit) {"different"} else {"same"},
                    "confidence":if matches!(path, Path::QuotesSplit) {0.5} else {0.95},
                    "why":"scripted conclusion is independent of source gold"
                }), 2)]})
                .to_string(),
            });
        }
        if governance {
            crate::governance::govern(&self.state, self.kb).await?;
        } else {
            crate::adjudication::adjudicate_entities(&self.state, self.kb).await?;
        }
        anyhow::ensure!(
            self.model.errors.lock().unwrap().is_empty(),
            "transport errors: {:?}",
            self.model.errors.lock().unwrap()
        );
        anyhow::ensure!(
            self.model.steps.lock().unwrap().is_empty(),
            "unconsumed script"
        );
        let requests = self.model.requests.lock().unwrap().clone();
        // Verify ownership in the actual HTTP request, including tool results. A
        // whole-request contains check would pass swapped A/B evidence unnoticed.
        if asks_batch {
            let messages = requests[0]["messages"].as_array().unwrap();
            let user = messages.iter().find(|m| m["role"] == "user").unwrap()["content"]
                .as_str()
                .unwrap();
            let (a, b) = user
                .split_once("\nRecord B: ")
                .ok_or_else(|| anyhow::anyhow!("missing Record B"))?;
            anyhow::ensure!(
                a.contains(numbers[order[0]]) && b.contains(numbers[order[1]]),
                "facts not delivered to their own sides"
            );
            if !gold_same {
                anyhow::ensure!(
                    !a.contains(numbers[order[1]]) && !b.contains(numbers[order[0]]),
                    "cross-owned initial evidence"
                );
            }
        }
        let mut read = Vec::new();
        if reads_evidence {
            for m in requests.last().unwrap()["messages"].as_array().unwrap() {
                if m["role"] != "tool" {
                    continue;
                }
                let id = m["tool_call_id"].as_str().unwrap();
                let i = match id {
                    "call_0" => 0,
                    "call_1" => 1,
                    _ => continue,
                };
                let content = m["content"].as_str().unwrap();
                anyhow::ensure!(
                    content.contains(numbers[order[i]]),
                    "missing owned tool evidence"
                );
                if !gold_same {
                    anyhow::ensure!(
                        !content.contains(numbers[order[1 - i]]),
                        "cross-owned tool evidence"
                    );
                }
                if matches!(path, Path::Quotes | Path::QuotesSplit) {
                    anyhow::ensure!(
                        content.contains(&quotes[order[i]]),
                        "quote was not read verbatim"
                    );
                }
                read.push(json!({"side":if i==0 {"A"} else {"B"},"content":content}));
            }
            anyhow::ensure!(
                read.len() == 2,
                "both tool sides must reach the final request"
            );
        }
        let l = utopia_store::resolution::survivor(&self.pool, self.kb, holders[0]).await?;
        let r = utopia_store::resolution::survivor(&self.pool, self.kb, holders[1]).await?;
        let review: (String, String, Option<String>) =
            sqlx::query_as("SELECT status,stage,reason FROM resolution_reviews WHERE kb_id=$1")
                .bind(self.kb)
                .fetch_one(&self.pool)
                .await?;
        anyhow::ensure!(
            (l == r) == (review.0 == "merged"),
            "graph and review disagree"
        );
        let decisions: Vec<(i32, Value)> = sqlx::query_as(
            "SELECT calls,trace FROM agent_decisions WHERE kb_id=$1 ORDER BY created_at,id",
        )
        .bind(self.kb)
        .fetch_all(&self.pool)
        .await?;
        let human: i64 = sqlx::query_scalar("SELECT count(*) FROM resolution_reviews WHERE kb_id=$1 AND stage='human' AND status='pending'")
            .bind(self.kb).fetch_one(&self.pool).await?;
        // Re-running a terminal pair must not silently add work or model calls.
        if governance {
            crate::governance::govern(&self.state, self.kb).await?;
        } else {
            crate::adjudication::adjudicate_entities(&self.state, self.kb).await?;
        }
        anyhow::ensure!(
            self.model.requests.lock().unwrap().len() == requests.len(),
            "terminal pair was investigated again"
        );
        Ok(json!({"path":format!("{path:?}"),"governance":governance,
            "order":if reverse {"reverse"} else {"forward"},"source_gold_same":gold_same,"batch_confidence":confidence,
            "source_quotes":quotes,"observed_same":l==r,"gold_error":(l==r)!=gold_same,
            "model_calls":requests.len(),"investigation_calls":decisions.iter().map(|d|d.0).sum::<i32>(),
            "source_passages_read":matches!(path,Path::Quotes | Path::QuotesSplit) && !read.is_empty(),
            "tool_results":read,"decisions":decisions,"review":review,"pending_human":human}))
    }
}

#[tokio::test]
async fn namesake_decision_measurements_preserve_evidence_ownership_and_terminal_state(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let mut rows = Vec::new();
    let case = Case {
        id: "decision_measurement",
        check: Check::Adjudicator { literal: true },
    };
    for governance in [false, true] {
        for path in [
            Path::Batch,
            Path::BatchHigh,
            Path::Cached,
            Path::Facts,
            Path::Quotes,
            Path::QuotesSplit,
        ] {
            for gold_same in [false, true] {
                for reverse in [false, true] {
                    let org = Uuid::now_v7();
                    let f = match Fx::new(&pool, &case, org).await {
                        Ok(f) => f,
                        Err(error) => {
                            sqlx::query("DELETE FROM organizations WHERE id=$1")
                                .bind(org)
                                .execute(&pool)
                                .await?;
                            return Err(error);
                        }
                    };
                    let run = tokio::time::timeout(
                        Duration::from_secs(15),
                        f.measure_decision(path, governance, gold_same, reverse),
                    )
                    .await;
                    f.cleanup().await?;
                    let row =
                        run.map_err(|_| anyhow::anyhow!("decision measurement timed out"))??;
                    println!("NAMESAKE_DECISION_MEASUREMENT {row}");
                    rows.push(row);
                }
            }
        }
    }
    println!(
        "NAMESAKE_DECISION_SUMMARY {}",
        json!({"cases":rows.len(),
        "source_gold_errors":rows.iter().filter(|r|r["gold_error"]==true).count(),
        "scope":"evidence transport and decision paths; scripted verdicts do not measure model accuracy",
        "known_failure_list_unchanged":true})
    );
    Ok(())
}
