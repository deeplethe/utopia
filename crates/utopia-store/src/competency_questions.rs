//! 能力问题（0061 决定 1）：一个库该回答什么，写成行。本体的元素凭它们判：一条属性
//! 值不值得批，看它服务哪个问题。人写的直接 accepted；代理从开放图谱里提的先 proposed。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Question {
    pub id: Uuid,
    pub kb_id: Uuid,
    pub question: String,
    pub expected_answer: Option<String>,
    pub needs: Option<serde_json::Value>,
    /// person | agent
    pub origin: String,
    /// accepted | proposed | rejected | retired
    pub status: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_result: Option<serde_json::Value>,
}

const COLS: &str = "id, kb_id, question, expected_answer, needs, origin, status, created_by,
                    created_at, updated_at, last_checked_at, last_result";

fn validate(question: &str) -> AppResult<&str> {
    let q = question.trim();
    if q.is_empty() {
        return Err(AppError::invalid(
            "empty_question",
            "A question needs words.",
        ));
    }
    if q.chars().count() > 2000 {
        return Err(AppError::invalid(
            "long_question",
            "A question is at most 2000 characters.",
        ));
    }
    Ok(q)
}

/// 库里全部问题，新的在前
pub async fn list(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Question>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM competency_questions WHERE kb_id = $1 ORDER BY created_at DESC, id"
    ))
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 认下的问题：代理提案对着的是这些
pub async fn accepted(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Question>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {COLS} FROM competency_questions WHERE kb_id = $1 AND status = 'accepted'
          ORDER BY created_at, id"
    ))
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

pub struct NewQuestion<'a> {
    pub question: &'a str,
    pub expected_answer: Option<&'a str>,
    pub needs: Option<&'a serde_json::Value>,
    /// person | agent
    pub origin: &'a str,
    /// accepted | proposed
    pub status: &'a str,
    pub created_by: Option<Uuid>,
}

pub async fn create(pool: &PgPool, kb_id: Uuid, q: NewQuestion<'_>) -> AppResult<Uuid> {
    let question = validate(q.question)?;
    if !matches!(q.origin, "person" | "agent") || !matches!(q.status, "accepted" | "proposed") {
        return Err(AppError::invalid(
            "bad_question",
            "origin is person or agent; status is accepted or proposed",
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO competency_questions
             (id, kb_id, question, expected_answer, needs, origin, status, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(question)
    .bind(q.expected_answer.map(str::trim))
    .bind(q.needs)
    .bind(q.origin)
    .bind(q.status)
    .bind(q.created_by)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 改问题的文字、期望答案或形状。没给的不动；期望答案给空串是清掉
pub async fn update(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
    question: Option<&str>,
    expected_answer: Option<&str>,
    needs: Option<&serde_json::Value>,
) -> AppResult<()> {
    let question = question.map(validate).transpose()?;
    let res = sqlx::query(
        "UPDATE competency_questions
            SET question = COALESCE($3, question),
                expected_answer = CASE WHEN $4 IS NULL THEN expected_answer
                                       WHEN btrim($4) = '' THEN NULL ELSE btrim($4) END,
                needs = COALESCE($5, needs),
                updated_at = now()
          WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(id)
    .bind(question)
    .bind(expected_answer)
    .bind(needs)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 人对一条问题表态：认（accepted）、拒（rejected）、退役（retired）
pub async fn set_status(pool: &PgPool, kb_id: Uuid, id: Uuid, status: &str) -> AppResult<()> {
    if !matches!(status, "accepted" | "rejected" | "retired") {
        return Err(AppError::invalid(
            "bad_status",
            "status is accepted, rejected or retired",
        ));
    }
    let res = sqlx::query(
        "UPDATE competency_questions SET status = $3, updated_at = now()
          WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(id)
    .bind(status)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub async fn delete(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM competency_questions WHERE id = $2 AND kb_id = $1")
        .bind(kb_id)
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 一条问题问过了：什么时候、答成了什么（0061 决定 5）。`result` 至少带 `answered: bool`
pub async fn record_result(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
    result: &serde_json::Value,
) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE competency_questions SET last_checked_at = now(), last_result = $3
          WHERE kb_id = $1 AND id = $2",
    )
    .bind(kb_id)
    .bind(id)
    .bind(result)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// 同一句话（不分大小写、去首尾空白）已经在库里了吗——代理提问题不重复人的
pub async fn exists_text(pool: &PgPool, kb_id: Uuid, question: &str) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM competency_questions
                         WHERE kb_id = $1 AND lower(btrim(question)) = lower(btrim($2)))",
    )
    .bind(kb_id)
    .bind(question)
    .fetch_one(pool)
    .await?)
}

/// 两个数里的第一个：接受了的问题里，问过的、答对的各几条
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct QuestionReport {
    pub accepted: i64,
    pub proposed: i64,
    pub checked: i64,
    pub answered: i64,
}

pub async fn report(pool: &PgPool, kb_id: Uuid) -> AppResult<QuestionReport> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'accepted') AS accepted,
                count(*) FILTER (WHERE status = 'proposed') AS proposed,
                count(*) FILTER (WHERE status = 'accepted' AND last_checked_at IS NOT NULL) AS checked,
                count(*) FILTER (WHERE status = 'accepted' AND (last_result->>'answered')::boolean) AS answered
           FROM competency_questions WHERE kb_id = $1",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?)
}
