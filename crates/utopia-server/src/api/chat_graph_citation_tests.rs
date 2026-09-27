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
//! - 号打开的是那句话（#968 的后续）：条目带着块里说出这条事实的引文，一块被引几句就记
//!   几句；块被新版本取代后换成仍有效的那一块，`as_of` 回到取代之前又是原来那块
//! - 检索和图谱工具在一轮里共用一个号：图谱引到检索登过的块，号不变、补上那句话；只补了
//!   引文、条数没变，来源帧也再发一次
//! - 原句里没有的日期在行上说出来（#970）：时间线推出来的终点写 `end derived`，人改过的
//!   区间写 `corrected`，号照旧；对话与 MCP 一样，规则 4 说不许把它们归到原句
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

/// 号 `n` 那一条带着的引文
fn quotes_of(sink: &ToolSink, n: usize) -> Vec<String> {
    sink.sources[n - 1]["quotes"]
        .as_array()
        .map(|q| {
            q.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 种子里还在的几篇文档进全文索引：检索要找得到它们
async fn index_seed(f: &Fx) -> anyhow::Result<()> {
    let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT c.document_id, c.id, c.text FROM chunks c JOIN documents d ON d.id = c.document_id
          WHERE c.kb_id = $1 AND d.deleted_at IS NULL ORDER BY c.document_id, c.seq",
    )
    .bind(f.kb)
    .fetch_all(&f.pool)
    .await?;
    let mut by_doc: std::collections::BTreeMap<Uuid, Vec<(String, String)>> = Default::default();
    for (doc, chunk, text) in rows {
        by_doc
            .entry(doc)
            .or_default()
            .push((chunk.to_string(), text));
    }
    for (doc, chunks) in by_doc {
        f.state
            .search
            .reindex_document(&f.kb.to_string(), &doc.to_string(), &chunks)?;
    }
    Ok(())
}

/// 号打开的是说出这条事实的那一句：条目带着引文，同一块两条事实两句都在；块被取代后
/// 换成仍有效的那一块和它那一句，`as_of` 回到取代之前又是原来的
#[tokio::test]
async fn a_graph_number_opens_the_sentence_it_was_read_from() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    let run = async {
        let s = seed(&f.pool, f.kb).await?;
        let pair: Uuid = sqlx::query_scalar(
            "SELECT f.id FROM facts f JOIN relation_types r ON r.id = f.predicate_id
              WHERE f.kb_id = $1 AND r.key = 'works_with'",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        // 章程那一块说了两句，张三的两条事实各从一句读出来
        sqlx::query("UPDATE chunks SET text = $2 WHERE id = $1")
            .bind(s.charter)
            .bind("Zhang San leads Project Aurora from 2023-01-10. Zhang San works with Li Si.")
            .execute(&f.pool)
            .await?;
        sqlx::query(
            "UPDATE fact_evidence SET quote = 'Zhang San works with Li Si.' WHERE fact_id = $1",
        )
        .bind(pair)
        .execute(&f.pool)
        .await?;
        let chat = ctx(&f, None);
        let mut sink = ToolSink::default();
        let text = dispatch(
            &chat,
            &mut sink,
            "entity_facts",
            &json!({"entity": "Zhang San"}),
        )
        .await
        .text;
        // 张三 works with 李四有两行：断言的那行带号，规则推出来的那行（`[rule:`）不带
        let works_line = text
            .lines()
            .flat_map(|l| l.split(" · "))
            .find(|l| l.contains("Li Si") && !l.contains("[rule:"))
            .unwrap_or_else(|| panic!("no stated works-with line in:\n{text}"));
        let (leads, works) = (marks(line(&text, "Project Aurora")), marks(works_line));
        assert_eq!(leads, works, "one chunk, one number: {text}");
        assert_eq!(chunk_of(&sink, leads[0]), s.charter);
        let mut quotes = quotes_of(&sink, leads[0]);
        quotes.sort();
        assert_eq!(
            quotes,
            [
                "Zhang San leads Project Aurora from 2023-01-10.",
                "Zhang San works with Li Si."
            ],
            "both sentences the chunk was cited for"
        );

        // 章程那一块被新版本取代：张三领 Aurora 那一条换成仍有效的回顾那一块、那一句
        sqlx::query("UPDATE chunks SET superseded_at = now() WHERE id = $1")
            .bind(s.charter)
            .execute(&f.pool)
            .await?;
        let mut now = ToolSink::default();
        let text = dispatch(
            &chat,
            &mut now,
            "entity_facts",
            &json!({"entity": "Project Aurora"}),
        )
        .await
        .text;
        let zhang = marks(line(&text, "Zhang San"));
        assert_eq!(chunk_of(&now, zhang[0]), s.recap, "{text}");
        assert_eq!(
            quotes_of(&now, zhang[0]),
            ["Zhang San led Project Aurora until the handover."]
        );
        // 回到取代之前：那时章程那一块还是现行的
        let mut then = ToolSink::default();
        let text = dispatch(
            &chat,
            &mut then,
            "entity_facts",
            &json!({"entity": "Project Aurora", "as_of": "2026-03-01"}),
        )
        .await
        .text;
        let zhang = marks(line(&text, "Zhang San"));
        assert_eq!(chunk_of(&then, zhang[0]), s.charter, "{text}");
        assert_eq!(
            quotes_of(&then, zhang[0]),
            ["Zhang San leads Project Aurora from 2023-01-10."]
        );
        // MCP 没有来源清单，也就没有引文
        let mut empty = ToolSink::default();
        dispatch(
            &ctx(&f, Some(Uuid::now_v7())),
            &mut empty,
            "entity_facts",
            &json!({"entity": "Project Aurora"}),
        )
        .await;
        assert!(empty.sources.is_empty());
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

/// 一轮里检索和图谱工具共用一个号：检索先登了交接备忘录那一块，图谱引到它时号不变，
/// 条目补上李四那句话；反过来也一样，检索不另起一个号，引文留着
#[tokio::test]
async fn a_search_and_a_graph_tool_share_one_number() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    let run = async {
        let s = seed(&f.pool, f.kb).await?;
        index_seed(&f).await?;
        let chat = ctx(&f, None);
        let number_of = |sink: &ToolSink, chunk: Uuid| {
            sink.sources
                .iter()
                .position(|x| x["chunk_id"] == chunk.to_string())
                .map(|i| i + 1)
        };
        let li_says = "Li Si leads Project Aurora from 2024-07-05.";

        let mut sink = ToolSink::default();
        let found = dispatch(
            &chat,
            &mut sink,
            "search_chunks",
            &json!({"query": "Li Si leads Project Aurora"}),
        )
        .await
        .text;
        let n = number_of(&sink, s.handover).unwrap_or_else(|| panic!("the handover hit: {found}"));
        assert!(
            quotes_of(&sink, n).is_empty(),
            "a search hit carries no quote"
        );
        let listed = sink.sources.len();
        let text = dispatch(
            &chat,
            &mut sink,
            "entity_facts",
            &json!({"entity": "Project Aurora"}),
        )
        .await
        .text;
        assert_eq!(
            marks(line(&text, "Li Si")),
            [n],
            "the search's number: {text}"
        );
        assert_eq!(
            quotes_of(&sink, n),
            [li_says],
            "the entry gains the sentence"
        );
        assert_eq!(
            number_of(&sink, s.handover),
            Some(n),
            "still one entry for the chunk"
        );
        assert!(sink.sources.len() >= listed);

        let mut reverse = ToolSink::default();
        let text = dispatch(
            &chat,
            &mut reverse,
            "entity_facts",
            &json!({"entity": "Project Aurora"}),
        )
        .await
        .text;
        let m = marks(line(&text, "Li Si"))[0];
        let listed = reverse.sources.len();
        dispatch(
            &chat,
            &mut reverse,
            "search_chunks",
            &json!({"query": "Li Si leads Project Aurora"}),
        )
        .await;
        assert_eq!(
            number_of(&reverse, s.handover),
            Some(m),
            "the graph's number"
        );
        assert_eq!(quotes_of(&reverse, m), [li_says], "the quote stays");
        assert!(reverse.sources.len() >= listed);
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

/// 整条对话：检索登了三块，图谱再引其中两块，只补了引文、条数没变——来源帧照样再发
/// 一次，存下的与最后一帧一致
#[tokio::test]
async fn a_quote_added_to_a_search_hit_is_published() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("search_chunks", r#"{"query":"Project Aurora"}"#),
        Reply::Tool("entity_facts", r#"{"entity_id":"Li Si"}"#),
        Reply::Text("Li Si leads Project Aurora."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let run = async {
        seed(&f.pool, f.kb).await?;
        index_seed(&f).await?;
        let sse = f.ask("Who leads Project Aurora?").await?;
        assert!(sse.contains("event: done"), "{sse}");
        let published: Vec<serde_json::Value> = sse
            .split("\n\n")
            .filter(|frame| frame.lines().any(|l| l == "event: sources"))
            .filter_map(|frame| frame.lines().find_map(|l| l.strip_prefix("data: ")))
            .filter_map(|data| serde_json::from_str(data).ok())
            .collect();
        assert!(published.len() >= 2, "{published:?}");
        let (first, last) = (&published[0], published.last().unwrap());
        assert_eq!(
            first.as_array().map(Vec::len),
            last.as_array().map(Vec::len),
            "the graph step added no entry, only quotes: {published:?}"
        );
        assert!(
            last.as_array()
                .unwrap()
                .iter()
                .any(|x| x["quotes"].as_array().is_some_and(|q| !q.is_empty())),
            "{last}"
        );
        let stored: serde_json::Value = sqlx::query_scalar(
            "SELECT m.sources FROM conversation_messages m
               JOIN conversations c ON c.id = m.conversation_id
              WHERE c.kb_id = $1 AND m.role = 'assistant'",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        assert_eq!(&stored, last);
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

/// #970 的两处：周七 2025-09-01 接手，时间线把李四那一段关在那天；人把张三的起点从章程的
/// 2023-01-10 改成 2023-02-01。两行的号仍打开它们读出来的原句，而原句里没有这两个日期，
/// 所以行上说出日期从哪来
#[tokio::test]
async fn a_fact_line_says_when_a_date_is_not_the_passages() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    let run = async {
        let s = seed(&f.pool, f.kb).await?;
        let (leads, person, aurora): (Uuid, Uuid, Uuid) = sqlx::query_as(
            "SELECT r.id, t.id, e.id FROM relation_types r, entity_types t, entities e
              WHERE r.kb_id = $1 AND r.key = 'leads' AND t.kb_id = $1 AND t.key = 'person'
                AND e.kb_id = $1 AND e.canonical_name = 'Project Aurora'",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        let row_of = |name: &'static str| {
            let pool = f.pool.clone();
            let kb = f.kb;
            async move {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT f.id FROM facts f JOIN entities e ON e.id = f.subject_id
                      WHERE f.kb_id = $1 AND e.canonical_name = $2 AND f.predicate_id = $3
                        AND f.invalidated_at IS NULL",
                )
                .bind(kb)
                .bind(name)
                .bind(leads)
                .fetch_one(&pool)
                .await
            }
        };
        let (zhang, li) = (row_of("Zhang San").await?, row_of("Li Si").await?);
        let (zhou, d_update, update, zhou_fact) =
            (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        // 插进去的只有本地生成的 UUID 和写死的日期
        sqlx::raw_sql(&format!(
            r#"
            INSERT INTO entities(id,kb_id,type_id,canonical_name,created_at) VALUES
                ('{zhou}','{kb}','{person}','Zhou Qi','2026-01-01');
            INSERT INTO documents(id,kb_id,filename,sha256,created_at,doc_time) VALUES
                ('{d_update}','{kb}','leadership-update.md',repeat('5',64),'2026-01-01','2025-09-01');
            INSERT INTO chunks(id,kb_id,document_id,seq,text,created_at) VALUES
                ('{update}','{kb}','{d_update}',0,'Zhou Qi leads Project Aurora from 2025-09-01.','2026-01-01');
            INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,valid_from,valid_from_precision,
                              recorded_at,confidence) VALUES
                ('{zhou_fact}','{kb}','{zhou}','{leads}','{aurora}','2025-09-01','day','2026-01-01',0.9);
            INSERT INTO fact_evidence(fact_id,chunk_id,document_id,doc_version,quote) VALUES
                ('{zhou_fact}','{update}','{d_update}',1,'Zhou Qi leads Project Aurora from 2025-09-01.');
            UPDATE relation_types SET inverse_functional = TRUE WHERE id = '{leads}';
            "#,
            kb = f.kb
        ))
        .execute(&f.pool)
        .await?;
        // 一个项目同时只有一个 lead：对账把李四那一段关在周七接手那天
        utopia_store::temporal::reconcile_moved_facts(&f.pool, f.kb, &[li, zhou_fact]).await?;
        // 人改张三的起点，终点照旧；审计记在被改的那一行上（PATCH /facts/{id} 同一条路）
        let day = |s: &str| s.parse::<chrono::DateTime<chrono::Utc>>().unwrap();
        utopia_store::temporal::correct_interval(
            &f.pool,
            zhang,
            utopia_store::graph::Validity {
                from: Some(day("2023-02-01T00:00:00Z")),
                from_precision: Some("day"),
                to: Some(day("2024-07-05T00:00:00Z")),
                to_precision: Some("day"),
                attested_at: None,
                from_grade: None,
            },
        )
        .await?
        .expect("the row was live");
        utopia_store::audit::record(
            &f.pool,
            Some(f.kb),
            f.user.id,
            "fact.time_corrected",
            "fact",
            Some(zhang),
            json!({ "note": "The charter date was the approval date" }),
        )
        .await?;

        let chat = ctx(&f, None);
        let mut sink = ToolSink::default();
        for tool in ["entity_facts", "neighbors", "timeline"] {
            let text = dispatch(&chat, &mut sink, tool, &json!({"entity": "Project Aurora"}))
                .await
                .text;
            let zhang_line = line(&text, "Zhang San");
            assert!(
                zhang_line.contains("2023-02-01 → 2024-07-05, corrected)"),
                "{tool}: {text}"
            );
            let li_line = line(&text, "Li Si");
            assert!(
                li_line.contains("2024-07-05 → 2025-09-01, end derived)"),
                "{tool}: {text}"
            );
            assert!(
                line(&text, "Zhou Qi").contains("2025-09-01 → now)"),
                "{tool}: {text}"
            );
            // 号照旧：打开的仍是那条事实读出来的原句
            let (z, l) = (marks(zhang_line), marks(li_line));
            assert_eq!((z.len(), l.len()), (1, 1), "{tool}: {text}");
            assert_eq!(chunk_of(&sink, z[0]), s.charter, "{tool}");
            assert_eq!(chunk_of(&sink, l[0]), s.handover, "{tool}");
        }
        let path = dispatch(
            &chat,
            &mut sink,
            "paths_between",
            &json!({"from": "Wang Wu", "to": "Li Si"}),
        )
        .await
        .text;
        assert!(
            path.contains("2024-07-05 → 2025-09-01, end derived)"),
            "{path}"
        );

        // MCP 的文字带着同样的标记：它们是这一行的事实，不是引用
        let mcp = ctx(&f, Some(Uuid::now_v7()));
        let plain = dispatch(
            &mcp,
            &mut ToolSink::default(),
            "entity_facts",
            &json!({"entity": "Project Aurora"}),
        )
        .await
        .text;
        assert!(
            plain.contains(", corrected)") && plain.contains(", end derived)"),
            "{plain}"
        );
        assert!(marks(&plain).is_empty(), "{plain}");

        // 规则 4 说这两种日期不在原句里
        assert!(
            SYSTEM_PROMPT.contains("An end marked `end derived`")
                && SYSTEM_PROMPT.contains("a range marked `corrected`"),
            "{SYSTEM_PROMPT}"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}
