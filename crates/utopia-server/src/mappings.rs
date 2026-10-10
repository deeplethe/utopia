//! Schema exploration records one table alignment for human adoption (0036).
//! Manual SQL definitions still use concept_mappings until #556 renders them.

use crate::llm_util;
use crate::state::AppState;
use utopia_store::exploration_runs::drop_reason;
use uuid::Uuid;

const MAX_SCHEMA_CHARS: usize = 12_000;

/// schema 文本开头那一句图例：列行里的方括号是什么
const COLUMN_MARKERS: &str =
    "Column markers: [PK] single-column primary key; [FK→schema.table] foreign key to that table.
";

/// schema 文本里的一列：`  名字 类型 [PK, FK→schema.table] -- 注释`。
///
/// 键标记放在类型后面、注释前面，用方括号：`-- ` 已经是注释的分隔，写成 `-- PK`
/// 就与一条内容恰好是「PK」的注释分不开。主键又是外键（一对一的扩展表）两个都标。
/// 可空不写进来：宽表上几乎每列都是 NOT NULL，每列多十几个字符会让
/// `MAX_SCHEMA_CHARS` 装下的列明显变少，而两个提示词都用不到它（#502）
fn column_line(c: &crate::query_engine::SchemaColumn) -> String {
    let mut marks = Vec::new();
    if c.is_primary_key {
        marks.push("PK".to_string());
    }
    if let Some(target) = &c.references_table {
        marks.push(format!("FK→{target}"));
    }
    let marks = if marks.is_empty() {
        String::new()
    } else {
        format!(" [{}]", marks.join(", "))
    };
    let comment = c
        .comment
        .as_deref()
        .map(|x| format!(" -- {x}"))
        .unwrap_or_default();
    format!(
        "  {} {}{marks}{comment}
",
        c.column, c.data_type
    )
}

/// At most one proposal per table; the schema budget bounds what is visible.
fn proposal_cap(tables: i32) -> i32 {
    tables.clamp(1, 60)
}

/// 模型回的 JSON 常裹着代码栅栏；剥掉它。
fn json_body(reply: &str) -> &str {
    reply
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
}

/// 一轮描述最多留几个问题。多了没人答；八个已经是一页
const MAX_DATA_QUESTIONS: usize = 8;

/// 模型回的 `{"description": …, "questions": […]}`。描述空的整个不要——
/// 一个空描述盖掉上一轮的好描述，比没写更糟
fn parse_description(reply: &str) -> Option<(String, Vec<String>)> {
    let v: serde_json::Value = serde_json::from_str(json_body(reply)).ok()?;
    let description = v["description"].as_str()?.trim().to_string();
    if description.is_empty() {
        return None;
    }
    let questions = v["questions"]
        .as_array()
        .map(|qs| {
            qs.iter()
                .filter_map(|q| q.as_str())
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .map(str::to_string)
                .take(MAX_DATA_QUESTIONS)
                .collect()
        })
        .unwrap_or_default();
    Some((description, questions))
}

/// 探索写库的数据描述，并列出它拿不准的地方（#570）。
///
/// **两半，两个来路。** schema 与注释说了的——一行是什么、键、单位、码值、时间轴、
/// 哪两列长得像——探索读得出来，这里写；schema 没说的——测试单不算数、有效订单是
/// 2/3/4、GMV 用实付不用优惠前——探索生成不了，**而且不能猜**：猜出来的约定进了
/// 提示词，问数会照着算，比没有更糟。所以拿不准的写成问题，人答，答案进
/// `data_conventions`，这里不碰那一列。
///
/// 语言跟本体语言走（0004：生成的文字跟语料走，不跟界面走）。
async fn describe_data(
    state: &AppState,
    client: &utopia_llm::LlmClient,
    settings: &utopia_core::models::LlmSettings,
    kb: &utopia_core::models::KnowledgeBase,
    schema_txt: &str,
) -> anyhow::Result<()> {
    let lang = if kb.ontology_lang == "zh" {
        "Chinese"
    } else {
        "English"
    };
    let prompt = format!(
        "You are documenting a database for analysts who will ask questions about it in plain \
         language. Reply with ONLY a JSON object {{\"description\": \"...\", \"questions\": [\"...\"]}}.\n\
         description ({lang}, 10 to 20 short lines): what each table is a table of and what one \
         row is; which columns are keys; the unit of money and quantity columns ONLY where a \
         comment states it; what status or code values mean ONLY where a comment states it; \
         which columns are the time axis; which columns look alike and how they differ (gross vs \
         net, list vs paid). State only what the schema and its comments say. Do not invent \
         business rules, thresholds, or which rows count.\n\
         questions ({lang}, at most {MAX_DATA_QUESTIONS}): the conventions you would need to \
         compute business figures correctly but the schema does not state, each phrased so the \
         owner of the data can answer in one line — which status values count, whether flagged \
         rows (test, internal, gift) are excluded, which of two similar amount columns is the \
         figure, units where no comment states them, whether net figures subtract refunds. Ask \
         only what the schema leaves open.\n\
         Schemas:\n{schema_txt}"
    );
    let _permit = llm_util::acquire_chat(state, settings).await;
    let reply = client
        .chat(&[utopia_llm::ChatMessage {
            role: "user".into(),
            content: prompt,
        }])
        .await?;
    let Some((description, questions)) = parse_description(&reply) else {
        anyhow::bail!(
            "description reply did not parse: {}",
            reply.chars().take(120).collect::<String>()
        );
    };
    utopia_store::kbs::set_data_description(&state.pool, kb.id, &description, &questions).await?;
    tracing::info!(kb_id = %kb.id, lines = description.lines().count(), questions = questions.len(), "数据描述已写");
    Ok(())
}

/// 探索把 schema 里的量与维度落成 Metric / Dimension 实体，而这两个类不在任何
/// 内置本体包里——0009 之后建库不再自带类。没有它们，下面的 `type_id` 查不到，
/// 每条提议都被 `continue` 吞掉，页面只说"已排队"就再无下文（#223）。
/// 所以探索前把两个类补上：builtin，描述给抽取提示词，本体页可以改
pub(crate) async fn ensure_concept_types(pool: &sqlx::PgPool, kb_id: Uuid) -> anyhow::Result<()> {
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

/// 一轮探索开账、干活、收账（#503）。
///
/// **先开行再干活**：跑挂了的那一轮也留一行，因为「失败」与「跑了但一条都没提」
/// 从前在页面上都是「没有新提议」，而该做的事完全不同。
pub async fn explore_mappings(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    let run = utopia_store::exploration_runs::start(&state.pool, kb_id).await?;
    match explore(state, kb_id, run).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // 报错路径上的失败不该淹掉它要报的那件事（与 `source_name` 同一条理由）
            let _ = utopia_store::exploration_runs::fail(&state.pool, run, &e.to_string()).await;
            Err(e)
        }
    }
}

async fn explore(state: &AppState, kb_id: Uuid, run: Uuid) -> anyhow::Result<()> {
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
    //
    // **上限是跨源的一个总数。** 从前那个 `break` 只跳出当前源的列循环，
    // 下一个源接着往同一个字符串里追加——`MAX_SCHEMA_CHARS` 读起来像个上限，
    // 实际上是「每个源各自超一次」的下限。
    let mut schema_txt = String::from(COLUMN_MARKERS);
    let mut tables_scanned = 0i32;
    let mut columns_scanned = 0i32;
    let mut truncated = false;
    let mut schemas = std::collections::HashMap::new();
    for ds in &sources {
        if truncated {
            break;
        }
        let (engine, conn) = utopia_store::datasources::engine_and_conn(&state.pool, ds.id).await?;
        let cols = crate::query_engine::engine_for(&engine, &conn)?
            .fetch_schema()
            .await?;
        schema_txt.push_str(&format!("\n=== source: {} ===\n", ds.name));
        let mut current = String::new();
        for c in &cols {
            let key = format!("{}.{}", c.schema, c.table);
            if key != current {
                current = key.clone();
                tables_scanned += 1;
                schema_txt.push_str(&format!("table {key}:\n"));
            }
            columns_scanned += 1;
            schema_txt.push_str(&column_line(c));
            if schema_txt.len() > MAX_SCHEMA_CHARS {
                schema_txt.push_str("(truncated)\n");
                truncated = true;
                break;
            }
        }
        schemas.insert(ds.id, cols);
    }

    let cap = proposal_cap(tables_scanned);
    let source_list: Vec<String> = sources.iter().map(|d| d.name.clone()).collect();
    let _ = utopia_store::exploration_runs::scanned(
        &state.pool,
        run,
        &source_list,
        tables_scanned,
        columns_scanned,
        truncated,
        cap,
    )
    .await;

    // 先写库的数据描述与拿不准的问题，再提口径。**描述不依赖提议成不成**：
    // 提议那一步解析失败整轮报错，描述已经落下了；反过来描述写不出来也不该
    // 拖垮提议——它是顺手的，warn 一句继续
    if let Err(e) = describe_data(state, &client, &settings, &kb, &schema_txt).await {
        tracing::warn!(%kb_id, error = %e, "数据描述没写成，提议照常");
    }

    let ontology = crate::table_exploration::ontology_context(&state.pool, kb_id).await?;
    let prompt = crate::table_exploration::prompt(&schema_txt, &ontology, cap, &kb.ontology_lang);

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
    // 丢弃要计数，还要留一条例子——**光有计数诊断不动**：「十二条源名对不上」
    // 得配上「模型说的是 tpch，挂的是 tpch-2026-09-08」才知道该改什么
    let mut drops: std::collections::BTreeMap<&'static str, (i64, String)> = Default::default();
    let mut note = |reason: &'static str, example: String| {
        let e = drops.entry(reason).or_insert((0, example));
        e.0 += 1;
    };
    let mut covered: std::collections::BTreeSet<String> = Default::default();
    let mut seen = std::collections::HashSet::new();
    for p in proposals.iter().take(cap as usize) {
        let said = p["source"].as_str().map(str::trim).unwrap_or("");
        let source = if sources.len() == 1 {
            &sources[0]
        } else if let Some(source) = sources.iter().find(|s| s.name.eq_ignore_ascii_case(said)) {
            source
        } else {
            note(
                drop_reason::SOURCE,
                format!("Unknown mounted source {said:?}"),
            );
            continue;
        };
        let Some(columns) = schemas.get(&source.id) else {
            note(
                drop_reason::SOURCE,
                format!("Source {} was outside the schema budget", source.name),
            );
            continue;
        };
        let proposed = match crate::table_exploration::prepare(
            &state.pool,
            kb_id,
            source.id,
            &source.name,
            p.clone(),
            columns,
        )
        .await
        {
            Ok(proposed) => proposed,
            Err(utopia_core::AppError::Invalid { message, .. }) => {
                note(drop_reason::DEFINITION, message);
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let key = utopia_store::table_alignments::key(source.id, &proposed.draft.table);
        if !seen.insert(key) {
            note(
                drop_reason::DEFINITION,
                format!("Table {} was proposed twice", proposed.draft.table),
            );
            continue;
        }
        if utopia_store::table_alignments::save(&state.pool, kb_id, &proposed).await? {
            accepted += 1;
            covered.insert(format!("{}:{}", source.name, proposed.draft.table));
        } else {
            note(drop_reason::DECIDED, proposed.draft.table.clone());
        }
    }
    if proposals.len() > cap as usize {
        drops.insert(
            drop_reason::CAP,
            (
                (proposals.len() - cap as usize) as i64,
                format!("model returned {}, cap {cap}", proposals.len()),
            ),
        );
    }

    let dropped = serde_json::Value::Object(
        drops
            .iter()
            .map(|(k, (n, ex))| {
                (
                    (*k).to_string(),
                    serde_json::json!({ "n": n, "example": ex }),
                )
            })
            .collect(),
    );
    let covered: Vec<String> = covered.into_iter().collect();
    if let Err(e) = utopia_store::exploration_runs::finish(
        &state.pool,
        run,
        proposals.len() as i32,
        accepted as i32,
        dropped,
        &covered,
    )
    .await
    {
        // 账没记上不该让提议白跑，但也不能装作记上了：留在日志里
        tracing::warn!(%kb_id, run = %run, error = %e, "探索账没记上");
    }
    tracing::info!(
        %kb_id,
        proposals = accepted,
        returned = proposals.len(),
        tables = tables_scanned,
        covered = covered.len(),
        truncated,
        "映射探索完成，提议已入审核队列"
    );
    // 一条都没提出来时页面上什么都不会变——Pending 还是 0，而"已排队"那句
    // 早就翻篇了。走告警中心说一声，人才知道该去刷新结构或给列加注释。
    //
    // **告警要带上是怎么空的。** 从前只说「0 条，这些源」，而「模型一条没回」
    // 与「回了十二条全被源名挡掉」是两件事，该做的动作也不同——前者去给列加注释，
    // 后者去看源名。`dropped` 就是这个区别，它现在也在这条告警里
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
                detail: serde_json::json!({
                    "proposals": 0,
                    "sources": source_names,
                    "returned": proposals.len(),
                    "dropped": drops.iter().map(|(k, (n, _))| (*k, *n))
                        .collect::<std::collections::BTreeMap<_, _>>(),
                    "tables_scanned": tables_scanned,
                }),
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

#[cfg(test)]
mod tests {
    use super::{column_line, parse_description};
    use crate::query_engine::SchemaColumn;

    fn column(pk: bool, fk: Option<&str>, comment: Option<&str>) -> SchemaColumn {
        SchemaColumn {
            schema: "a".into(),
            table: "t".into(),
            column: "id".into(),
            data_type: "integer".into(),
            comment: comment.map(str::to_string),
            is_primary_key: pk,
            references_table: fk.map(str::to_string),
        }
    }

    #[test]
    fn a_column_line_marks_its_keys_before_the_comment() {
        assert_eq!(
            column_line(&column(false, None, None)),
            "  id integer
"
        );
        assert_eq!(
            column_line(&column(true, None, Some("Order id"))),
            "  id integer [PK] -- Order id
"
        );
        assert_eq!(
            column_line(&column(false, Some("a.orders"), None)),
            "  id integer [FK→a.orders]
"
        );
        // 一对一的扩展表：主键同时是外键，两个都标
        assert_eq!(
            column_line(&column(true, Some("a.p1"), None)),
            "  id integer [PK, FK→a.p1]
"
        );
        // 注释里写着 PK 的普通列，与标记分得开
        assert_eq!(
            column_line(&column(false, None, Some("PK"))),
            "  id integer -- PK
"
        );
    }

    #[test]
    fn a_description_is_kept_only_when_it_says_something() {
        let (d, qs) = parse_description(
            "```json\n{\"description\": \"dw.dwd_ord_dtl: one row per order line.\", \
             \"questions\": [\" Which ord_st values count? \", \"\", \"Exclude is_test = 1?\"]}\n```",
        )
        .unwrap();
        assert_eq!(d, "dw.dwd_ord_dtl: one row per order line.");
        // 空问题丢掉、首尾空白剥掉，顺序保留
        assert_eq!(
            qs,
            vec!["Which ord_st values count?", "Exclude is_test = 1?"]
        );
        // 空描述整个不要：一个空描述盖掉上一轮的好描述，比没写更糟
        assert!(parse_description("{\"description\": \"  \", \"questions\": []}").is_none());
        assert!(parse_description("not json").is_none());
        // 问题没给也行，描述照收
        assert_eq!(
            parse_description("{\"description\": \"x\"}").unwrap().1,
            Vec::<String>::new()
        );
    }
}
