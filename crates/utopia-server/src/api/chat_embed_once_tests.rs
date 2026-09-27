//! 同一轮里，同一句话只嵌一次。
//!
//! 同名实体的候选按问题排（`rank_by_question`）：问题嵌成向量，与候选的上下文画像比。
//! 一轮里每按名字查一次实体都走到那里，句子是同一句；从前每次都重新嵌一遍，读一次模型
//! 设置、调一次嵌入服务。谓词对不上时的对齐（`aligned_predicates`）也是这样。
//!
//! 假嵌入服务记下每次请求的文字，每条都回同一个方向。两个都叫 Acme 的实体，画像近的
//! 那个事实少：不按问题排（MCP 没有问题），按事实数排在前面的是远的那个。
//!
//! - 工具层（同一个 sink 就是同一轮）：find_entities、按名字的 entity_facts 与
//!   neighbors 都挑中近的那个，问题只嵌一次；同一个对不上的谓词问两次，那个词只嵌
//!   一次；换一个 sink（下一轮），问题再嵌一次
//! - 整条对话：模型先 find_entities，再按名字 entity_facts，嵌入服务只收到一次请求
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。
use super::*;
use crate::api::tools::{dispatch, ToolCtx, ToolSink};

const QUESTION: &str = "Where is the Acme lab?";

/// 假嵌入服务：记下每次请求的输入，每条都回 (1, 0, 0)
#[derive(Clone, Default)]
struct Embedder {
    inputs: Arc<Mutex<Vec<Vec<String>>>>,
}

async fn embeddings(
    State(e): State<Embedder>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let texts: Vec<String> = body["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str().map(str::to_string))
        .collect();
    let data: Vec<serde_json::Value> = (0..texts.len())
        .map(|i| json!({"index": i, "embedding": [1.0, 0.0, 0.0]}))
        .collect();
    e.inputs.lock().unwrap().push(texts);
    Json(json!({"data": data}))
}

impl Embedder {
    /// 每次请求一行，行里是那次嵌的文字
    fn requests(&self) -> Vec<Vec<String>> {
        self.inputs.lock().unwrap().clone()
    }
}

struct Acme {
    near: Uuid,
    far: Uuid,
}

/// 起假嵌入服务，把这个库所在工作区的嵌入模型指过去；种两个 Acme：
/// 近的那个画像与假向量同向、一条事实，远的那个画像正交、两条事实
async fn seed(f: &Fx) -> anyhow::Result<(Embedder, tokio::task::JoinHandle<()>, Acme)> {
    let embedder = Embedder::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let app = axum::Router::new()
        .route("/embeddings", axum::routing::post(embeddings))
        .with_state(embedder.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    sqlx::query(
        "UPDATE llm_settings SET embed_base_url = $1, embed_model = 'fake-embedding'
          WHERE workspace_id = (SELECT workspace_id FROM knowledge_bases WHERE id = $2)",
    )
    .bind(&base)
    .bind(f.kb)
    .execute(&f.pool)
    .await?;

    let id = Uuid::now_v7;
    let (org, place, located_in) = (id(), id(), id());
    let (near, far, harbor, port) = (id(), id(), id(), id());
    // 插进去的只有本地生成的 UUID
    sqlx::raw_sql(&format!(
        r#"
        INSERT INTO entity_types(id,kb_id,key,label) VALUES
            ('{org}','{kb}','organization','Organization'), ('{place}','{kb}','place','Place');
        INSERT INTO relation_types(id,kb_id,key,label,temporal) VALUES
            ('{located_in}','{kb}','located_in','located in','state');
        INSERT INTO entities(id,kb_id,type_id,canonical_name,profile_embedding) VALUES
            ('{near}','{kb}','{org}','Acme','[1,0,0]'),
            ('{far}','{kb}','{org}','Acme','[0,1,0]'),
            ('{harbor}','{kb}','{place}','Harbor',NULL),
            ('{port}','{kb}','{place}','Port',NULL);
        INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,confidence) VALUES
            ('{f1}','{kb}','{near}','{located_in}','{harbor}',0.9),
            ('{f2}','{kb}','{far}','{located_in}','{harbor}',0.9),
            ('{f3}','{kb}','{far}','{located_in}','{port}',0.9);
        "#,
        kb = f.kb,
        f1 = id(),
        f2 = id(),
        f3 = id(),
    ))
    .execute(&f.pool)
    .await?;
    Ok((embedder, server, Acme { near, far }))
}

/// 一轮对话的工具上下文；`question` 为 None 就是 MCP 那样没有问题的调用。
/// `embed` 按工作区读模型设置，所以用这个库真正所在的工作区
async fn ctx<'f>(f: &'f Fx, question: Option<&'f str>) -> anyhow::Result<ToolCtx<'f>> {
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM knowledge_bases WHERE id = $1")
            .bind(f.kb)
            .fetch_one(&f.pool)
            .await?;
    Ok(ToolCtx {
        state: &f.state,
        kb_id: f.kb,
        workspace_id,
        mounted_sources: &[],
        can_write: false,
        actor: Some(f.user.id),
        via_token: None,
        question,
    })
}

#[tokio::test]
async fn a_turn_embeds_its_question_and_each_word_once() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    let (embedder, server, acme) = seed(&f).await?;
    let run = async {
        // 没有问题（MCP 那样）：不嵌，按事实数挑，排在前面的是远的那个
        let plain = dispatch(
            &ctx(&f, None).await?,
            &mut ToolSink::default(),
            "find_entities",
            &json!({"name": "Acme"}),
        )
        .await;
        assert!(
            plain
                .text
                .lines()
                .nth(1)
                .is_some_and(|l| l.starts_with(&acme.far.to_string())),
            "{}",
            plain.text
        );
        assert!(embedder.requests().is_empty());

        let ctx = ctx(&f, Some(QUESTION)).await?;
        let mut turn = ToolSink::default();

        // 三次按名字查 Acme：都挑中画像近的那个，问题只嵌一次
        let found = dispatch(&ctx, &mut turn, "find_entities", &json!({"name": "Acme"})).await;
        assert!(
            found
                .text
                .starts_with(&format!("Best match: {} |", acme.near)),
            "{}",
            found.text
        );
        let facts = dispatch(
            &ctx,
            &mut turn,
            "entity_facts",
            &json!({"entity_id": "Acme"}),
        )
        .await;
        assert!(
            facts.text.contains("closest to the question") && !facts.text.contains("Port"),
            "the near Acme (one fact, Harbor): {}",
            facts.text
        );
        let near = dispatch(&ctx, &mut turn, "neighbors", &json!({"entity": "Acme"})).await;
        assert!(
            near.text.contains("closest to the question") && !near.text.contains("Port"),
            "{}",
            near.text
        );
        assert_eq!(embedder.requests(), vec![vec![QUESTION.to_string()]]);

        // 对不上的谓词问两次：那个词只嵌一次，问题仍不重嵌
        for _ in 0..2 {
            dispatch(
                &ctx,
                &mut turn,
                "entity_facts",
                &json!({"entity_id": "Acme", "predicate": "headquarters"}),
            )
            .await;
        }
        assert_eq!(
            embedder.requests(),
            vec![vec![QUESTION.to_string()], vec!["headquarters".to_string()]]
        );

        // 下一轮是另一个 sink：问题再嵌一次，挑的还是近的那个
        let mut next = ToolSink::default();
        let again = dispatch(&ctx, &mut next, "find_entities", &json!({"name": "Acme"})).await;
        assert!(
            again
                .text
                .starts_with(&format!("Best match: {} |", acme.near)),
            "{}",
            again.text
        );
        assert_eq!(embedder.requests().len(), 3);
        anyhow::Ok(())
    }
    .await;
    server.abort();
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn a_chat_turn_embeds_its_question_once() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("find_entities", r#"{"name":"Acme"}"#),
        Reply::Tool("entity_facts", r#"{"entity_id":"Acme"}"#),
        Reply::Text("The Acme lab is in Harbor."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let (embedder, server, _) = seed(&f).await?;
    let run = async {
        let sse = f.ask(QUESTION).await?;
        assert!(sse.contains("event: done"), "{sse}");
        assert_eq!(sse.matches("event: step").count(), 2, "{sse}");
        assert_eq!(embedder.requests(), vec![vec![QUESTION.to_string()]]);
        anyhow::Ok(())
    }
    .await;
    server.abort();
    f.cleanup().await?;
    run
}
