//! 问数语义映射的 Agentic 探索：读挂载源的 schema + KB 既有概念，让 LLM 提议
//! "业务概念（Metric/Dimension 实体）→ 数据资产定义" 的映射。
//!
//! 提议写进 `concept_mappings`（status = proposed）→ Review 页自成一档，
//! Confirm / Reject 之后 status 变 confirmed，问数只读确认过的那些。
//!
//! **从前它是一条 0.6 置信的 `mapped_to` 事实**,借「低置信事实」那一档露面。
//! 搬出来的理由见 0011:它不是关于世界的断言,是配置——而「确认」这个动作
//! 当时是 `UPDATE facts SET confidence = 1.0`,原地改一张不许原地改的表。
//! agent 只提议，口径生效权在人——与消解"宁分勿合"同一哲学。

use crate::llm_util;
use crate::state::AppState;
use uuid::Uuid;

const MAX_SCHEMA_CHARS: usize = 12_000;

/// 探索把 schema 里的量与维度落成 Metric / Dimension 实体，而这两个类不在任何
/// 内置本体包里——0009 之后建库不再自带类。没有它们，下面的 `type_id` 查不到，
/// 每条提议都被 `continue` 吞掉，页面只说"已排队"就再无下文（#223）。
/// 所以探索前把两个类补上：builtin，描述给抽取提示词，本体页可以改
async fn ensure_concept_types(pool: &sqlx::PgPool, kb_id: Uuid) -> anyhow::Result<()> {
    for (key, label, description) in [
        (
            "metric",
            "Metric",
            "An aggregatable business quantity (revenue, order count, average ticket) that              maps to a definition in a mounted database.",
        ),
        (
            "dimension",
            "Dimension",
            "A group-by attribute (region, month, product line) that maps to a column in a              mounted database.",
        ),
    ] {
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label, builtin, description)
             SELECT $1, $2, $3, $4, TRUE, $5
             WHERE NOT EXISTS (SELECT 1 FROM entity_types WHERE kb_id = $2 AND key = $3)",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(key)
        .bind(label)
        .bind(description)
        .execute(pool)
        .await?;
    }
    Ok(())
}

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
    ensure_concept_types(&state.pool, kb_id).await?;

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

        // 定义拆成列写进 concept_mappings（0011）。从前它是一份塞进
        // `object_value` 的 JSON，宾语挂在一条叫 mapped_to 的关系上——
        // 而那条关系是本体里的一行，跟 works_at 并列。**它不是关于世界的
        // 断言，是配置**，所以搬去自己的表
        let def = &p["definition"];
        if !def.is_object() {
            continue;
        }
        let s = |k: &str| {
            def[k]
                .as_str()
                .filter(|x| !x.is_empty())
                .map(str::to_string)
        };
        utopia_store::mappings::propose(
            &state.pool,
            kb_id,
            resolved.entity_id,
            source,
            s("table").as_deref(),
            s("expr").as_deref(),
            s("sql").as_deref(),
            s("unit").as_deref(),
            // summary 是给人看的那句：Review 列表与问数 prompt 都靠它
            p["summary"].as_str().or(def["summary"].as_str()),
            def["derived"].as_bool().unwrap_or(false),
        )
        .await?;
        accepted += 1;
    }

    tracing::info!(%kb_id, proposals = accepted, "映射探索完成，提议已入审核队列");
    // 一条都没提出来时页面上什么都不会变——Pending 还是 0，而"已排队"那句
    // 早就翻篇了。走告警中心说一声，人才知道该去刷新结构或给列加注释
    if accepted == 0 {
        if let Err(e) = utopia_store::alerts::raise(
            &state.pool,
            utopia_store::alerts::NewAlert {
                kb_id: Some(kb_id),
                severity: "info",
                kind: utopia_store::alerts::kind::MAPPING_EXPLORATION_EMPTY,
                min_role: utopia_core::models::Role::Editor,
                subject_type: None,
                subject_id: None,
                detail: serde_json::json!({ "proposals": 0, "sources": source_names }),
            },
        )
        .await
        {
            tracing::warn!(%kb_id, error = %e, "映射探索空结果的告警没写进去");
        }
        state.emit_alert();
    }
    state.emit_review(kb_id);
    Ok(())
}
