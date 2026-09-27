//! 图谱答出来的回答点得开原句（#935）。
//!
//! 四个图谱工具（entity_facts、neighbors、timeline、paths_between）在对话里给每条显示出来
//! 的事实印上它第一条有效证据的 `[n]`，证据块登进这一轮的来源清单，与检索结果共用一套号。
//! 这里钉住：
//!
//! - 每个工具的号都指向那条事实的证据块；几条证据时取最早那篇文档的
//! - 同一块在一轮里只有一个号，换一个工具再印还是它
//! - 证据只在已删文档里的事实不带号；`as_of` 回到删除之前，它又带上号
//! - 派生事实那一行不带号，它的前提各带各的
//! - MCP 的文字一字不改，来源清单是空的（号码对没有清单的调用方没有意义）
//! - 整条对话：工具印的号随 `sources` 帧发出去、跟着回答落库，模型读到的事实行带着号
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。
use super::*;
use crate::api::tools::{dispatch, ToolCtx, ToolSink};

struct Seed {
    /// 张三 leads Aurora 的两条证据：早的那篇（charter）与晚的那篇（recap）
    charter: Uuid,
    recap: Uuid,
    /// 李四 leads Aurora 的证据
    handover: Uuid,
    /// 王五 advises Aurora 的唯一证据，在一篇删掉的文档里
    deleted: Uuid,
}

/// 一个项目、三个人、四篇文档（一篇已删），外加一条对称规则推出来的事实
async fn seed(pool: &sqlx::PgPool, kb: Uuid) -> anyhow::Result<Seed> {
    let id = Uuid::now_v7;
    let (person, project, leads, advises, works_with, rule) = (id(), id(), id(), id(), id(), id());
    let (zhang, li, wang, aurora) = (id(), id(), id(), id());
    let (d_charter, d_recap, d_handover, d_deleted) = (id(), id(), id(), id());
    let (charter, recap, handover, deleted) = (id(), id(), id(), id());
    let (f_zhang, f_li, f_wang, f_pair, derived) = (id(), id(), id(), id(), id());
    // 插进去的只有本地生成的 UUID 和写死的日期
    sqlx::raw_sql(&format!(
        r#"
        INSERT INTO entity_types(id,kb_id,key,label) VALUES
            ('{person}','{kb}','person','Person'), ('{project}','{kb}','project','Project');
        INSERT INTO relation_types(id,kb_id,key,label,temporal) VALUES
            ('{leads}','{kb}','leads','leads','state'),
            ('{advises}','{kb}','advises','advises','state'),
            ('{works_with}','{kb}','works_with','works with','state');
        INSERT INTO entities(id,kb_id,type_id,canonical_name,created_at) VALUES
            ('{zhang}','{kb}','{person}','Zhang San','2026-01-01'),
            ('{li}','{kb}','{person}','Li Si','2026-01-01'),
            ('{wang}','{kb}','{person}','Wang Wu','2026-01-01'),
            ('{aurora}','{kb}','{project}','Project Aurora','2026-01-01');
        INSERT INTO documents(id,kb_id,filename,sha256,created_at,doc_time,deleted_at) VALUES
            ('{d_charter}','{kb}','charter.md',repeat('1',64),'2026-01-01','2023-01-05',NULL),
            ('{d_recap}','{kb}','recap.md',repeat('2',64),'2025-12-01','2025-01-01',NULL),
            ('{d_handover}','{kb}','handover.md',repeat('3',64),'2026-01-01','2024-07-05',NULL),
            ('{d_deleted}','{kb}','advisor-note.md',repeat('4',64),'2026-01-01','2023-03-02','2026-06-01');
        INSERT INTO chunks(id,kb_id,document_id,seq,text,created_at) VALUES
            ('{charter}','{kb}','{d_charter}',0,'Zhang San leads Project Aurora from 2023-01-10.','2026-01-01'),
            ('{recap}','{kb}','{d_recap}',0,'Zhang San led Project Aurora until the handover.','2026-01-01'),
            ('{handover}','{kb}','{d_handover}',0,'Li Si leads Project Aurora from 2024-07-05.','2026-01-01'),
            ('{deleted}','{kb}','{d_deleted}',0,'Wang Wu advises Project Aurora.','2026-01-01');
        INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,valid_from,valid_from_precision,
                          valid_to,valid_to_precision,recorded_at,confidence) VALUES
            ('{f_zhang}','{kb}','{zhang}','{leads}','{aurora}','2023-01-10','day','2024-07-05','day','2026-01-01',0.9),
            ('{f_li}','{kb}','{li}','{leads}','{aurora}','2024-07-05','day',NULL,NULL,'2026-01-01',0.9),
            ('{f_wang}','{kb}','{wang}','{advises}','{aurora}','2023-03-02','day',NULL,NULL,'2026-01-01',0.9),
            ('{f_pair}','{kb}','{zhang}','{works_with}','{li}','2023-01-10','day',NULL,NULL,'2026-01-01',0.9);
        -- 晚的那篇先插、也先入库：取哪条证据按文档自己的日期，不按插入或入库的先后
        INSERT INTO fact_evidence(fact_id,chunk_id,document_id,doc_version,quote) VALUES
            ('{f_zhang}','{recap}','{d_recap}',1,'Zhang San led Project Aurora until the handover.'),
            ('{f_zhang}','{charter}','{d_charter}',1,'Zhang San leads Project Aurora from 2023-01-10.'),
            ('{f_li}','{handover}','{d_handover}',1,'Li Si leads Project Aurora from 2024-07-05.'),
            ('{f_wang}','{deleted}','{d_deleted}',1,'Wang Wu advises Project Aurora.'),
            ('{f_pair}','{charter}','{d_charter}',1,'Zhang San leads Project Aurora from 2023-01-10.');
        INSERT INTO rules(id,kb_id,predicate_id,kind) VALUES ('{rule}','{kb}','{works_with}','symmetric');
        INSERT INTO derived_facts(id,kb_id,subject_id,predicate_id,object_id,rule_id,derived_at,
                                  valid_from,valid_from_precision) VALUES
            ('{derived}','{kb}','{li}','{works_with}','{zhang}','{rule}','2026-01-01','2023-01-10','day');
        INSERT INTO fact_derivations(derived_fact_id,premise_fact_id,seq) VALUES ('{derived}','{f_pair}',0);
        "#
    ))
    .execute(pool)
    .await?;
    Ok(Seed {
        charter,
        recap,
        handover,
        deleted,
    })
}

fn ctx(f: &Fx, via_token: Option<Uuid>) -> ToolCtx<'_> {
    ToolCtx {
        state: &f.state,
        kb_id: f.kb,
        workspace_id: Uuid::nil(),
        mounted_sources: &[],
        can_write: false,
        actor: Some(f.user.id),
        via_token,
        question: None,
    }
}

/// 一段文字里的引用号：`[3]` 算，置信度 `[90%]` 和 `[rule: …]` 不算
fn marks(text: &str) -> Vec<usize> {
    text.match_indices('[')
        .filter_map(|(at, _)| {
            let rest = &text[at + 1..];
            let end = rest.find(']')?;
            rest[..end].parse().ok()
        })
        .collect()
}

/// 含 `needle` 的那一行。neighbors 把一组的几项写在同一行、用 ` · ` 隔开，取的是那一项
fn line<'t>(text: &'t str, needle: &str) -> &'t str {
    text.lines()
        .flat_map(|l| l.split(" · "))
        .find(|item| item.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in:\n{text}"))
}

/// 号 `n` 在清单里登的是哪一块
fn chunk_of(sink: &ToolSink, n: usize) -> Uuid {
    sink.sources[n - 1]["chunk_id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no source {n} in {:?}", sink.sources))
}

#[tokio::test]
async fn each_graph_tool_cites_the_first_live_evidence_of_each_fact() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    let run = async {
        let s = seed(&f.pool, f.kb).await?;
        let chat = ctx(&f, None);
        let mut sink = ToolSink::default();

        let facts = dispatch(
            &chat,
            &mut sink,
            "entity_facts",
            &json!({"entity": "Project Aurora"}),
        )
        .await;
        let text = &facts.text;
        let zhang = marks(line(text, "Zhang San"));
        let li = marks(line(text, "Li Si"));
        assert_eq!(zhang.len(), 1, "{text}");
        assert_eq!(
            chunk_of(&sink, zhang[0]),
            s.charter,
            "the earliest document's chunk, not the recap"
        );
        assert_eq!(li.len(), 1, "{text}");
        assert_eq!(chunk_of(&sink, li[0]), s.handover);
        assert!(
            marks(line(text, "Wang Wu")).is_empty(),
            "evidence only in a deleted document: {text}"
        );
        assert_eq!(sink.sources.len(), 2, "{:?}", sink.sources);
        assert_eq!(sink.sources[zhang[0] - 1]["filename"], "charter.md");
        assert!(
            sink.sources
                .iter()
                .all(|x| x["chunk_id"] != s.recap.to_string()),
            "one number per fact: the later evidence is not listed"
        );

        // 同一块同一个号：换一个工具再印，清单不长
        let near = dispatch(
            &chat,
            &mut sink,
            "neighbors",
            &json!({"entity": "Project Aurora"}),
        )
        .await;
        assert_eq!(marks(line(&near.text, "Zhang San")), zhang, "{}", near.text);
        assert_eq!(marks(line(&near.text, "Li Si")), li, "{}", near.text);
        let dated = dispatch(
            &chat,
            &mut sink,
            "timeline",
            &json!({"entity": "Project Aurora"}),
        )
        .await;
        assert_eq!(
            marks(line(&dated.text, "Zhang San")),
            zhang,
            "{}",
            dated.text
        );
        assert_eq!(marks(line(&dated.text, "Li Si")), li, "{}", dated.text);
        assert!(
            marks(line(&dated.text, "Wang Wu")).is_empty(),
            "{}",
            dated.text
        );
        // 路径的每一跳是一条事实，各带各的号：王五那一跳的证据在删掉的文档里，不带
        let path = dispatch(
            &chat,
            &mut sink,
            "paths_between",
            &json!({"from": "Wang Wu", "to": "Li Si"}),
        )
        .await;
        let hops = line(&path.text, "—advises→");
        assert_eq!(marks(hops), [li[0]], "{}", path.text);
        assert_eq!(
            sink.sources.len(),
            2,
            "no chunk was numbered twice: {:?}",
            sink.sources
        );

        // 派生那一行不带号；它的前提（张三 works with 李四）带着自己的号
        let li_facts = dispatch(
            &chat,
            &mut sink,
            "entity_facts",
            &json!({"entity": "Li Si"}),
        )
        .await;
        let derived = line(&li_facts.text, "[rule:");
        assert!(marks(derived).is_empty(), "{}", li_facts.text);
        let premise = marks(line(&li_facts.text, "Zhang San"));
        assert_eq!(premise.len(), 1, "{}", li_facts.text);
        assert_eq!(chunk_of(&sink, premise[0]), s.charter);

        // 回到删除之前：那篇文档当时还在，王五的事实带上号
        let mut before = ToolSink::default();
        let then = dispatch(
            &chat,
            &mut before,
            "entity_facts",
            &json!({"entity": "Project Aurora", "as_of": "2026-03-01"}),
        )
        .await;
        let wang = marks(line(&then.text, "Wang Wu"));
        assert_eq!(wang.len(), 1, "{}", then.text);
        assert_eq!(chunk_of(&before, wang[0]), s.deleted);

        // MCP：文字与对话里去掉号之后一字不差，清单是空的
        let mcp = ctx(&f, Some(Uuid::now_v7()));
        for (tool, args) in [
            ("entity_facts", json!({"entity": "Project Aurora"})),
            ("neighbors", json!({"entity": "Project Aurora"})),
            ("timeline", json!({"entity": "Project Aurora"})),
            ("paths_between", json!({"from": "Wang Wu", "to": "Li Si"})),
        ] {
            let mut empty = ToolSink::default();
            let plain = dispatch(&mcp, &mut empty, tool, &args).await.text;
            let numbered = dispatch(&chat, &mut ToolSink::default(), tool, &args)
                .await
                .text;
            let mut stripped = numbered.clone();
            for n in 1..=2 {
                stripped = stripped.replace(&format!(" [{n}]"), "");
            }
            assert_eq!(plain, stripped, "{tool} over MCP keeps its text");
            assert!(marks(&plain).is_empty(), "{tool}: {plain}");
            assert!(empty.sources.is_empty(), "{tool}: {:?}", empty.sources);
        }
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn a_graph_answer_publishes_and_stores_its_sources() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("entity_facts", r#"{"entity_id":"Project Aurora"}"#),
        Reply::Text("Li Si leads Project Aurora."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let run = async {
        let s = seed(&f.pool, f.kb).await?;
        let sse = f.ask("Who leads Project Aurora?").await?;
        assert!(sse.contains("event: done"), "{sse}");

        // `sources` 帧：两块证据，按印出来的顺序编号
        let published: Vec<serde_json::Value> = sse
            .split("\n\n")
            .filter(|frame| frame.lines().any(|l| l == "event: sources"))
            .filter_map(|frame| frame.lines().find_map(|l| l.strip_prefix("data: ")))
            .filter_map(|data| serde_json::from_str(data).ok())
            .collect();
        let last = published.last().expect("a sources frame");
        let chunks: Vec<&str> = last
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["chunk_id"].as_str())
            .collect();
        assert_eq!(chunks.len(), 2, "{last}");
        assert!(chunks.contains(&s.charter.to_string().as_str()), "{last}");
        assert!(chunks.contains(&s.handover.to_string().as_str()), "{last}");

        // 落库的回答带着同一张清单，重开会话时号码照样点得开
        let stored: serde_json::Value = sqlx::query_scalar(
            "SELECT m.sources FROM conversation_messages m
               JOIN conversations c ON c.id = m.conversation_id
              WHERE c.kb_id = $1 AND m.role = 'assistant'",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        assert_eq!(&stored, last);

        // 模型读到的事实行带着号：置信度后面跟着它
        let requests = f.requests();
        let sent = requests[1]["messages"].to_string();
        assert!(sent.contains("Li Si") && sent.contains("%] ["), "{sent}");
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}
