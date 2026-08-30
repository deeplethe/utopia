//! 问数语义映射的 Agentic 探索：读挂载源的 schema + KB 既有概念，让 LLM 提议
//! "业务概念（Metric/Dimension 实体）→ 数据资产定义" 的映射。
//!
//! 提议以低置信（0.6）mapped_to 事实落库 → 自动流入 Review 的低置信区，
//! Confirm/Reject 零新 UI；确认（置信提满）后的映射注入问数 prompt。
//! agent 只提议，口径生效权在人——与消解"宁分勿合"同一哲学。

use crate::llm_util;
use crate::state::AppState;
use uuid::Uuid;

const PROPOSAL_CONFIDENCE: f32 = 0.6;
const MAX_SCHEMA_CHARS: usize = 12_000;

pub async fn explore_mappings(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    let kb = utopia_store::kbs::get(&state.pool, kb_id).await?;
    let settings = utopia_store::settings::get(&state.pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured"))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured"))?;

    let sources = utopia_store::datasources::mounted(&state.pool, kb_id).await?;
    if sources.is_empty() {
        anyhow::bail!("No data sources mounted");
    }

    // 各源 schema（引擎直读，保证新鲜；限量防 prompt 爆炸）
    let mut schema_txt = String::new();
    for ds in &sources {
        let (engine, conn) = utopia_store::datasources::engine_and_conn(&state.pool, ds.id).await?;
        let cols = crate::query_engine::engine_for(&engine, &conn)?
            .fetch_schema()
            .await?;
        schema_txt.push_str(&format!("\n=== source: {} ===\n", ds.name));
        let mut current = String::new();
        for c in cols {
            let key = format!("{}.{}", c.schema, c.table);
            if key != current {
                current = key.clone();
                schema_txt.push_str(&format!("table {key}:\n"));
            }
            schema_txt.push_str(&format!(
                "  {} {}{}\n",
                c.column,
                c.data_type,
                c.comment.map(|x| format!(" -- {x}")).unwrap_or_default()
            ));
            if schema_txt.len() > MAX_SCHEMA_CHARS {
                schema_txt.push_str("(truncated)\n");
                break;
            }
        }
    }

    // 既有概念（供归并复用，避免重复起名）
    let existing: Vec<(String,)> = sqlx::query_as(
        "SELECT e.canonical_name FROM entities e
         JOIN entity_types t ON t.id = e.type_id
         WHERE e.kb_id = $1 AND e.merged_into IS NULL AND t.key IN ('metric','dimension')
         ORDER BY e.canonical_name LIMIT 100",
    )
    .bind(kb_id)
    .fetch_all(&state.pool)
    .await?;
    let existing_names: Vec<String> = existing.into_iter().map(|(n,)| n).collect();

    let prompt = format!(
        "You are building the semantic layer of a BI system. Given database schemas, propose \
         business concepts a user would ask about, each mapped to a concrete definition.\n\
         Existing concepts (reuse these names when the meaning matches): {}\n\
         Schemas:\n{}\n\
         Reply with ONLY a JSON array, each item:\n\
         {{\"name\": \"business concept name\", \"kind\": \"metric\"|\"dimension\", \
         \"source\": \"data source name\", \
         \"definition\": {{\"table\": \"schema.table\", \"expr\": \"SQL expression\", \
         \"sql\": \"full SELECT if joins are needed (optional)\", \"unit\": \"optional\"}}, \
         \"summary\": \"one line: source + expression, shown to reviewers\", \
         \"rationale\": \"why this mapping, citing column comments\"}}\n\
         Metrics are aggregatable quantities (use sum/count/avg in expr); dimensions are \
         group-by columns. Propose at most 12, only well-grounded ones.",
        if existing_names.is_empty() {
            "(none)".into()
        } else {
            existing_names.join(", ")
        },
        schema_txt
    );

    let _permit = llm_util::acquire_chat(state, &settings).await;
    let reply = client
        .chat(&[utopia_llm::ChatMessage {
            role: "user".into(),
            content: prompt,
        }])
        .await?;
    let json_str = reply
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let proposals: Vec<serde_json::Value> = serde_json::from_str(json_str)
        .map_err(|e| anyhow::anyhow!("Mapping proposal parse error: {e}"))?;

    let source_names: Vec<&str> = sources.iter().map(|d| d.name.as_str()).collect();
    let mut accepted = 0usize;
    for p in proposals.iter().take(12) {
        let name = p["name"].as_str().map(str::trim).unwrap_or("");
        let kind = p["kind"].as_str().unwrap_or("");
        let source = p["source"].as_str().map(str::trim).unwrap_or("");
        if name.is_empty()
            || !matches!(kind, "metric" | "dimension")
            || !source_names.iter().any(|s| s.eq_ignore_ascii_case(source))
        {
            continue;
        }
        let type_id: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM entity_types WHERE kb_id = $1 AND key = $2")
                .bind(kb_id)
                .bind(kind)
                .fetch_optional(&state.pool)
                .await?;
        let Some((type_id,)) = type_id else { continue };

        // 概念实体：走消解（同名归并；无向量上下文按 v1 兼容归并）
        let resolved = utopia_store::resolution::resolve_mention(
            &state.pool,
            kb_id,
            Some(type_id),
            name,
            None,
        )
        .await?;

        // 定义带 source + summary（Review 与 prompt 展示都靠 summary）
        let mut definition = p["definition"].clone();
        if !definition.is_object() {
            continue;
        }
        definition["source"] = serde_json::json!(source);
        if let Some(s) = p["summary"].as_str() {
            definition["summary"] = serde_json::json!(s);
        }

        let mapped_to: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM relation_types WHERE kb_id = $1 AND key = 'mapped_to'")
                .bind(kb_id)
                .fetch_optional(&state.pool)
                .await?;
        let Some((mapped_to,)) = mapped_to else {
            continue;
        };

        let (fact_id, created) = utopia_store::graph::insert_value_fact(
            &state.pool,
            kb_id,
            resolved.entity_id,
            Some(mapped_to),
            &definition,
            // 问数映射不带时间：两端都空
            utopia_store::graph::Validity::default(),
            PROPOSAL_CONFIDENCE,
        )
        .await?;
        if created {
            accepted += 1;
            // 证据挂 schema 文档的首个 live chunk（rationale 作 quote）
            if let Some((chunk_id,)) = sqlx::query_as::<_, (Uuid,)>(
                "SELECT c.id FROM chunks c JOIN documents d ON d.id = c.document_id
                 WHERE d.kb_id = $1 AND d.external_key LIKE 'datasource:%:schema'
                   AND c.superseded_at IS NULL
                 ORDER BY c.seq LIMIT 1",
            )
            .bind(kb_id)
            .fetch_optional(&state.pool)
            .await?
            {
                let rationale = p["rationale"].as_str().unwrap_or("");
                // 谓词是代码写死的 mapped_to，不是模型给的说法，没有表层形式可留
                utopia_store::graph::add_evidence(
                    &state.pool,
                    fact_id,
                    chunk_id,
                    (!rationale.is_empty()).then_some(rationale),
                    None,
                )
                .await?;
            }
        }
    }

    tracing::info!(%kb_id, proposals = accepted, "映射探索完成，提议已入审核队列");
    state.emit_review(kb_id);
    Ok(())
}
