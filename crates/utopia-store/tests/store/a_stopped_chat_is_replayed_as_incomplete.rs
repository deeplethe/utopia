//! 停止标记随原文回放，模型上下文另外说明回答未完成（#934）。
use serde_json::json;
use sqlx::PgPool;
use utopia_store::conversations::{self, TurnRecord};
use uuid::Uuid;

#[tokio::test]
async fn a_stopped_chat_is_replayed_as_incomplete() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb, user) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'stopped-chat-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    let result = async {
        sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'stopped-chat-test')")
            .bind(ws)
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'stopped-chat-test')")
            .bind(kb)
            .bind(ws)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO users (id, org_id, email, password_hash, display_name) VALUES ($1, $2, $3, '', 'Stopped Chat')")
            .bind(user)
            .bind(org)
            .bind(format!("{user}@stopped-chat.test"))
            .execute(&pool)
            .await?;

        for (content, stopped, expected_context) in [
            ("A complete answer.", false, "A complete answer."),
            (
                "已生成的部分",
                true,
                "[This response was stopped by the user before completion.]\n\n已生成的部分",
            ),
            ("", true, "[This response was stopped by the user before completion.]\n\n"),
        ] {
            let conversation = conversations::create(&pool, kb, user, "question").await?;
            let question = conversations::append_message(
                &pool, conversation, "user", "question", &TurnRecord::empty(),
            ).await?;
            let record = TurnRecord {
                stopped,
                sources: json!([{"number": 1, "title": "Record"}]),
                tool_exchange: json!([
                    {"role": "assistant", "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "no_evidence_needed", "arguments": "{}"}}]},
                    {"role": "tool", "tool_call_id": "call_1", "content": "ok"},
                ]),
                ..TurnRecord::empty()
            };
            let answer = conversations::append_message(
                &pool, conversation, "assistant", content, &record,
            ).await?;

            let messages = conversations::messages(&pool, conversation).await?;
            assert_eq!(messages.len(), 2);
            assert!(!messages[0].stopped);
            assert_eq!(messages[1].content, content, "历史保留原文");
            assert_eq!(messages[1].stopped, stopped);
            assert_eq!(serde_json::to_value(&messages[1])?["stopped"], stopped);

            let history = conversations::recent_context(&pool, conversation, 20).await?;
            assert_eq!(history.turn_ids, [question, answer]);
            assert_eq!(history.turns[0], ("user".into(), "question".into()));
            assert_eq!(history.turns[1], ("assistant".into(), expected_context.into()));
            assert_eq!(history.last_sources, record.sources.as_array().unwrap().clone());
            assert_eq!(history.last_tool_exchange, record.tool_exchange.as_array().unwrap().clone());
        }
        Ok::<_, anyhow::Error>(())
    }.await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
