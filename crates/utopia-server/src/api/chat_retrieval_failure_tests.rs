//! 两种检索失败要各说各的原因，底层诊断只能留在服务端。
use super::*;

#[tokio::test]
async fn mapping_and_document_retrieval_failures_have_distinct_safe_codes() -> anyhow::Result<()> {
    for (table, code, message) in [
        (
            "concept_mappings",
            "mapping_search_failed",
            "Could not retrieve relevant mappings.",
        ),
        ("chunks", "search_failed", "Could not search the documents."),
    ] {
        let Some(mut f) = fixture(Scripted::new(vec![Reply::Http(400), Reply::Http(400)])).await?
        else {
            return Ok(());
        };
        let mapping = table == "concept_mappings";
        let source = if mapping {
            let id = utopia_store::datasources::create(
                &f.pool,
                &format!("mapping failure {}", f.kb),
                "postgres",
                "postgres://unused",
                f.user.id,
            )
            .await?;
            utopia_store::datasources::mount(&f.pool, f.kb, id).await?;
            Some(id)
        } else {
            // 真正走向量检索，让分块查询失败，不能只验证手工构造的错误帧。
            sqlx::query(
                "UPDATE llm_settings SET embed_base_url=chat_base_url, embed_model='test-embed'
                 WHERE workspace_id=(SELECT workspace_id FROM knowledge_bases WHERE id=$1)",
            )
            .bind(f.kb)
            .execute(&f.pool)
            .await?;
            Mock::given(method("POST"))
                .and(path("/embeddings"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "data": [{"index": 0, "embedding": [0.1, 0.2, 0.3]}]
                })))
                .mount(&f._server)
                .await;
            None
        };
        // 只给本夹具的连接遮住对应表，让真实查询因缺列而失败。
        // 不改共享表、权限或生产查询，也不会干扰并行测试。
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(move |connection, _| {
                Box::pin(async move {
                    sqlx::query(&format!(
                        "CREATE TEMP TABLE {table} (unrelated_column text)"
                    ))
                    .execute(connection)
                    .await?;
                    Ok(())
                })
            })
            .connect_with((*f.pool.connect_options()).clone())
            .await?;
        f.state.pool = pool.clone();
        let result = async {
            let sse = f.ask("What is the revenue?").await?;
            assert_eq!(sse.matches("event: error").count(), 1, "{sse}");
            assert!(!sse.contains("event: done"), "{sse}");
            let error = sse
                .split("\n\n")
                .find(|frame| frame.starts_with("event: error"))
                .and_then(|frame| frame.lines().find_map(|line| line.strip_prefix("data: ")))
                .expect("an error frame carries data");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(error)?,
                json!({"code":code, "error":message})
            );
            assert!(!sse.contains("does not exist"), "{sse}");
            let id: Uuid = sqlx::query_scalar("SELECT id FROM conversations WHERE kb_id=$1")
                .bind(f.kb)
                .fetch_one(&f.pool)
                .await?;
            let messages = utopia_store::conversations::messages(&f.pool, id).await?;
            assert_eq!(messages.len(), 1, "only the question is stored");
            assert_eq!(messages[0].role, "user");
            assert!(f.state.live.attach(id).await.is_none());
            assert_eq!(f.requests().len(), if mapping { 0 } else { 2 });
            Ok::<_, anyhow::Error>(())
        }
        .await;
        pool.close().await;
        if let Some(id) = source {
            utopia_store::datasources::delete(&f.pool, id).await?;
        }
        f.cleanup().await?;
        result?;
    }
    Ok(())
}
