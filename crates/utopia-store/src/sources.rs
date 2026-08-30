//! 摄入来源仓储："来源即文件夹"——source 是容器，挂着它摄入的文档，可定时同步。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_core::models::{Role, Source, SourceView, SyncRun};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// folder = 纯容器（上传/拖拽入内，无同步语义）；url/rss = 拉取型；api = 推送型。
/// 本机目录监听（watch_folder）已否决——自部署用户看不到服务器磁盘；
/// 未来的 watch 形态是对象存储/网盘（P5 连接器，与 BlobStore 接缝配套）。
/// custom = 自定义拉取器：任何实现 Utopia ingest 接口的 URL（返回 items JSON）即可定时摄取。
/// github_issues = 工单：一张工单连同它的状态变更史成为一篇文档。
///
/// **改这里就得改前端那份清单**（`Library.tsx` 的建来源对话框与 `api.ts` 的
/// `SourceView["kind"]`）。两处对不上时的症状是：界面上选得到、建的时候报
/// 「kind must be one of…」——单元测试与 tsc 都看不见，只有端到端会撞上。
pub const KINDS: &[&str] = &["folder", "url", "rss", "api", "custom", "github_issues"];

/// 校验并规范化标准 5 段 cron 表达式（内部用 cron crate 的 6 段：补秒位）。
pub fn validate_cron(expr: &str) -> AppResult<String> {
    let normalized = expr.split_whitespace().collect::<Vec<_>>().join(" ");
    let fields = normalized.split(' ').count();
    if fields != 5 {
        return Err(AppError::invalid_detail(
            "bad_cron_fields",
            "Cron expression must have 5 fields (minute hour day month weekday)",
            format!("got {fields}"),
        ));
    }
    use std::str::FromStr;
    cron::Schedule::from_str(&format!("0 {normalized}")).map_err(|e| {
        AppError::invalid_detail("bad_cron", "Invalid cron expression", e.to_string())
    })?;
    Ok(normalized)
}

/// cron 的下一次触发时刻（服务器本地时区）。
fn cron_next_after(expr: &str, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    use std::str::FromStr;
    let schedule = cron::Schedule::from_str(&format!("0 {expr}")).ok()?;
    let local_after = after.with_timezone(&chrono::Local);
    schedule
        .after(&local_after)
        .next()
        .map(|t| t.with_timezone(&Utc))
}

pub async fn list(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<SourceView>> {
    // config 剔除 auth_header：自定义拉取器的凭据不下发给任何客户端
    let rows: Vec<SourceView> = sqlx::query_as(
        "SELECT s.id, s.kind, s.name, s.config - 'auth_header' AS config, s.icon,
                s.sync_interval_minutes, s.sync_cron,
                s.last_sync_at, s.last_sync_status, s.last_sync_error, s.last_sync_added,
                (SELECT count(*) FROM documents d WHERE d.source_id = s.id) AS doc_count,
                (SELECT count(*) FROM documents d
                 WHERE d.source_id = s.id AND d.missing_since IS NOT NULL) AS missing_count
         FROM sources s WHERE s.kb_id = $1 ORDER BY s.created_at",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get(pool: &PgPool, id: Uuid) -> AppResult<Source> {
    sqlx::query_as("SELECT * FROM sources WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    pool: &PgPool,
    kb_id: Uuid,
    kind: &str,
    name: &str,
    config: &serde_json::Value,
    icon: Option<&str>,
    sync_interval_minutes: Option<i32>,
    sync_cron: Option<&str>,
) -> AppResult<Source> {
    if !KINDS.contains(&kind) {
        return Err(AppError::Validation(format!(
            "kind must be one of: {}",
            KINDS.join(", ")
        )));
    }
    if name.trim().is_empty() {
        return Err(AppError::invalid(
            "source_name_required",
            "Source name is required",
        ));
    }
    // 互斥：cron 优先（UI 只会传其一）
    let cron_norm = sync_cron.map(validate_cron).transpose()?;
    let interval = if cron_norm.is_some() {
        None
    } else {
        sync_interval_minutes
    };
    // serde 缺省的 Value::Null 会以 jsonb null 落库，前端读 config.x 直接炸——规范化为空对象
    let config = if config.is_null() {
        serde_json::json!({})
    } else {
        config.clone()
    };
    let source = sqlx::query_as(
        "INSERT INTO sources (id, kb_id, kind, name, config, icon, sync_interval_minutes, sync_cron)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(kind)
    .bind(name.trim())
    .bind(config)
    .bind(icon)
    .bind(interval)
    .bind(cron_norm)
    .fetch_one(pool)
    .await?;
    Ok(source)
}

/// 设置 api 来源的推送密钥（创建 / 轮换时）。
pub async fn set_ingest_token(pool: &PgPool, source_id: Uuid, token: &str) -> AppResult<()> {
    let res = sqlx::query("UPDATE sources SET ingest_token = $2 WHERE id = $1")
        .bind(source_id)
        .bind(token)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 更新调度：interval 与 cron 互斥，任一被显式设置时都会覆盖两者。
#[allow(clippy::too_many_arguments)]
pub async fn update(
    pool: &PgPool,
    id: Uuid,
    name: Option<&str>,
    config: Option<&serde_json::Value>,
    icon: Option<&str>,
    schedule: Option<(Option<i32>, Option<String>)>,
) -> AppResult<Source> {
    let schedule = match schedule {
        Some((interval, cron)) => {
            let cron_norm = cron.as_deref().map(validate_cron).transpose()?;
            let interval = if cron_norm.is_some() { None } else { interval };
            Some((interval, cron_norm))
        }
        None => None,
    };
    let source = sqlx::query_as(
        "UPDATE sources SET
            name = COALESCE($2, name),
            config = COALESCE($3, config),
            icon = COALESCE($4, icon),
            sync_interval_minutes = CASE WHEN $5 THEN $6 ELSE sync_interval_minutes END,
            sync_cron = CASE WHEN $5 THEN $7 ELSE sync_cron END
         WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(name)
    .bind(config)
    .bind(icon)
    .bind(schedule.is_some())
    .bind(schedule.as_ref().and_then(|(i, _)| *i))
    .bind(schedule.as_ref().and_then(|(_, c)| c.clone()))
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(source)
}

/// 删除来源；其文档保留（source_id 置 NULL，落回 Uploads 组）。
pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM sources WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 到期待同步的来源（调度器每分钟扫）。
/// interval 型在 SQL 里判定；cron 型取回 Rust 侧求下一次触发时刻再过滤。
pub async fn due_sources(pool: &PgPool) -> AppResult<Vec<Source>> {
    let rows: Vec<Source> = sqlx::query_as(
        "SELECT * FROM sources
         WHERE last_sync_status NOT IN ('queued', 'running')
           AND ((sync_interval_minutes IS NOT NULL
                 AND (last_sync_at IS NULL
                      OR last_sync_at + make_interval(mins => sync_interval_minutes) <= now()))
                OR sync_cron IS NOT NULL)",
    )
    .fetch_all(pool)
    .await?;

    let now = chrono::Utc::now();
    Ok(rows
        .into_iter()
        .filter(|s| match &s.sync_cron {
            None => true, // interval 型已在 SQL 判定
            Some(expr) => {
                // 基准取上次同步时刻（没同步过取创建时刻）：错过的触发点在下一轮扫描补上
                let anchor = s.last_sync_at.unwrap_or(s.created_at);
                cron_next_after(expr, anchor).is_some_and(|next| next <= now)
            }
        })
        .collect())
}

/// 标记入队（幂等：已在队列/运行中则返回 false，避免重复入队）。
pub async fn mark_queued(pool: &PgPool, id: Uuid) -> AppResult<bool> {
    let res = sqlx::query(
        "UPDATE sources SET last_sync_status = 'queued'
         WHERE id = $1 AND last_sync_status NOT IN ('queued', 'running')",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn mark_running(pool: &PgPool, id: Uuid) -> AppResult<()> {
    sqlx::query("UPDATE sources SET last_sync_status = 'running' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 一次同步收尾。失败时记一条告警，成功时什么都不做——
/// **"现在好了没有"不是告警中心该回答的问题**，来源页面上就写着。
///
/// 返回值是"记了没有"，调用方据此决定要不要推事件。
pub async fn finish_sync(
    pool: &PgPool,
    id: Uuid,
    error: Option<&str>,
    added: i32,
) -> AppResult<bool> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "UPDATE sources SET last_sync_status = $2, last_sync_error = $3,
                last_sync_added = $4, last_sync_at = now()
         WHERE id = $1
         RETURNING kb_id, name",
    )
    .bind(id)
    .bind(if error.is_some() { "failed" } else { "ok" })
    .bind(error)
    .bind(added)
    .fetch_optional(pool)
    .await?;
    let Some((kb_id, name)) = row else {
        return Ok(false);
    };
    let Some(msg) = error else {
        return Ok(false);
    };
    crate::alerts::raise(
        pool,
        crate::alerts::NewAlert {
            kb_id: Some(kb_id),
            severity: "error",
            kind: crate::alerts::kind::SOURCE_SYNC_FAILED,
            // 内容类给 editor，不只给 admin：管理员需要知道该修连接了，
            // 但**配这个源的人**更需要知道你的东西没进来
            min_role: Role::Editor,
            subject_type: Some("source"),
            subject_id: Some(id),
            // 名字存一份：源被删之后 subject_id 解析不出名字，而告警该留得住
            detail: serde_json::json!({ "name": name, "error": msg }),
        },
    )
    .await?;
    Ok(true)
}

/// 文档打标签（整组替换）。
pub async fn set_document_tags(
    pool: &PgPool,
    kb_id: Uuid,
    document_id: Uuid,
    tags: &[String],
) -> AppResult<()> {
    let cleaned: Vec<String> = tags
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let res = sqlx::query(
        "UPDATE documents SET tags = $3, updated_at = now() WHERE id = $1 AND kb_id = $2",
    )
    .bind(document_id)
    .bind(kb_id)
    .bind(&cleaned)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 同步用去重：该 KB 是否已有同内容文档。
pub async fn document_exists_by_sha(pool: &PgPool, kb_id: Uuid, sha256: &str) -> AppResult<bool> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM documents WHERE kb_id = $1 AND sha256 = $2 LIMIT 1")
            .bind(kb_id)
            .bind(sha256)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// 记录同步时刻（避免调度器在长同步过程中重复触发后又立刻到期）。
pub async fn touch_sync_time(pool: &PgPool, id: Uuid, at: DateTime<Utc>) -> AppResult<()> {
    sqlx::query("UPDATE sources SET last_sync_at = $2 WHERE id = $1")
        .bind(id)
        .bind(at)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 同步运行记录（渠道审计历史）
// ---------------------------------------------------------------------------

pub async fn start_run(pool: &PgPool, source_id: Uuid) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO source_sync_runs (id, source_id) VALUES ($1, $2)")
        .bind(id)
        .bind(source_id)
        .execute(pool)
        .await?;
    Ok(id)
}

pub async fn finish_run(
    pool: &PgPool,
    run_id: Uuid,
    source_id: Uuid,
    error: Option<&str>,
    created_docs: i32,
    updated_docs: i32,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE source_sync_runs SET finished_at = now(), status = $2, error = $3,
                created_docs = $4, updated_docs = $5
         WHERE id = $1",
    )
    .bind(run_id)
    .bind(if error.is_some() { "failed" } else { "ok" })
    .bind(error)
    .bind(created_docs)
    .bind(updated_docs)
    .execute(pool)
    .await?;
    // 每来源只留最近 50 条
    sqlx::query(
        "DELETE FROM source_sync_runs WHERE source_id = $1 AND id NOT IN
         (SELECT id FROM source_sync_runs WHERE source_id = $1
          ORDER BY started_at DESC LIMIT 50)",
    )
    .bind(source_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_runs(pool: &PgPool, source_id: Uuid, limit: i64) -> AppResult<Vec<SyncRun>> {
    let rows: Vec<SyncRun> = sqlx::query_as(
        "SELECT id, started_at, finished_at, status, created_docs, updated_docs, error
         FROM source_sync_runs WHERE source_id = $1
         ORDER BY started_at DESC LIMIT $2",
    )
    .bind(source_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_validation() {
        assert_eq!(validate_cron("30 9 * * *").unwrap(), "30 9 * * *");
        assert_eq!(
            validate_cron("  0  9 * * Mon,Thu ").unwrap(),
            "0 9 * * Mon,Thu"
        );
        assert!(validate_cron("9 * * *").is_err()); // 4 段
        assert!(validate_cron("99 9 * * *").is_err()); // 分钟越界
        assert!(validate_cron("0 0 0 0 0 0").is_err()); // 6 段
    }

    #[test]
    fn cron_next_computes() {
        let after = chrono::Utc::now();
        let next = cron_next_after("*/5 * * * *", after).unwrap();
        assert!(next > after);
        assert!((next - after).num_minutes() <= 5);
    }
}
