//! 读字的服务（0040）：扫描件和图片交给版面识别服务 MinerU（`mineru-api`）。
//!
//! 识别一份几百页的扫描件要几分钟到几十分钟。处理任务不在这里干等：第一次来交文件、把
//! 远端任务号记在文档上（`documents.reader_task`），挂 `Deferred` 回队列；之后每次来问
//! 一声，没好再挂回去，好了取回版面交给分块。`Deferred` 不烧重试预算，进程重启也从记下
//! 的任务号接着问，不重交。
//!
//! 接口：`POST /tasks`（multipart，`files` + 选项）交任务，`GET /tasks/{id}` 问状态
//! （pending / processing / completed / failed），`GET /tasks/{id}/result` 取结果——
//! `results` 按文件名去掉扩展名为键，`content_list` 是一段 JSON **字符串**。`mineru-api`
//! 本身不认证；部署在它前面挂了反向代理的，密钥按 Bearer 带上。

use std::time::Duration;

use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use utopia_core::models::{Document, LlmSettings};
use utopia_core::{Deferred, Terminal};
use utopia_ingest::mineru::Reading;

use crate::state::AppState;

/// 多久问一次。一页扫描件在 GPU 上一两秒，十秒问一次，几十页的文件问几次就好
const POLL: Duration = Duration::from_secs(10);

/// 交上去的任务最多等多久。服务把任务吞了、却一直报 processing 的时候，文档不能永远停在
/// parsing：六小时够一份上千页的扫描件在 CPU 上读完
const PATIENCE_HOURS: i64 = 6;

/// 传文件、取结果的超时。共用客户端的 20 秒是给探针和小请求的，一份几十兆的扫描件传不完
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(600);

/// 工作区配的版面识别服务
pub struct Ocr<'a> {
    base: &'a str,
    key: Option<&'a str>,
    backend: Option<&'a str>,
}

impl<'a> Ocr<'a> {
    pub fn from_settings(s: &'a LlmSettings) -> Option<Self> {
        Some(Ocr {
            base: s.ocr_base_url.as_deref()?.trim_end_matches('/'),
            key: s.ocr_api_key.as_deref().filter(|k| !k.is_empty()),
            backend: s.ocr_backend.as_deref().filter(|b| !b.is_empty()),
        })
    }

    fn request(
        &self,
        client: &reqwest::Client,
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::RequestBuilder {
        let req = client.request(method, format!("{}/{path}", self.base));
        match self.key {
            Some(key) => req.bearer_auth(key),
            None => req,
        }
    }

    /// 连通性测试：服务活着就回它报的版本
    pub async fn health(&self) -> anyhow::Result<Value> {
        let client = crate::query_engine::http()?;
        let resp = self
            .request(&client, reqwest::Method::GET, "health")
            .send()
            .await
            .context("the OCR service is unreachable")?;
        let status = resp.status();
        if !status.is_success() {
            return Err(anyhow!("the OCR service answered {status}"));
        }
        Ok(resp.json().await.unwrap_or(Value::Null))
    }

    /// 读这份文件。没读完返回挂着 `Deferred` 的错误，调用方原样往上抛
    pub async fn read(
        &self,
        state: &AppState,
        doc: &Document,
        bytes: Vec<u8>,
    ) -> anyhow::Result<Reading> {
        let pool = &state.pool;
        let stored = utopia_store::documents::reader_task(pool, doc.id).await?;
        let current = stored.as_ref().filter(|t| {
            t["reader"] == "ocr" && t["sha256"] == doc.sha256.as_str() && t["service"] == self.base
        });
        let Some(task) = current else {
            // 没交过；或者交的是旧版本的文件、交给的是换掉之前的服务——作废重交
            if stored.is_some() {
                utopia_store::documents::clear_reader_task(pool, doc.id).await?;
            }
            let task_id = self.submit(&doc.filename, bytes).await?;
            let task = json!({
                "reader": "ocr",
                "service": self.base,
                "task_id": task_id,
                "sha256": doc.sha256,
                "submitted_at": chrono::Utc::now(),
            });
            if !utopia_store::documents::claim_reader_task(pool, doc.id, &task).await? {
                tracing::info!(document = %doc.id, "another run submitted this file first");
            }
            return Err(anyhow!("waiting for the OCR service").context(Deferred::new(POLL)));
        };

        let task_id = task["task_id"].as_str().unwrap_or_default();
        let client = crate::query_engine::http()?;
        let resp = self
            .request(&client, reqwest::Method::GET, &format!("tasks/{task_id}"))
            .send()
            .await
            .context("the OCR service is unreachable")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            // 服务重启过、或者结果过了保留期：任务没了，下一轮重交
            utopia_store::documents::clear_reader_task(pool, doc.id).await?;
            return Err(anyhow!("the OCR service no longer knows the task")
                .context(Deferred::new(Duration::from_secs(1))));
        }
        let status: Value = resp
            .error_for_status()
            .context("the OCR service could not report the task")?
            .json()
            .await?;
        match status["status"].as_str() {
            Some("completed") => {}
            Some("failed") => {
                // 清掉任务号，普通重试会重交一次：显存不够、服务过载这类失败，下一次未必还失败
                utopia_store::documents::clear_reader_task(pool, doc.id).await?;
                return Err(anyhow!(
                    "The OCR service could not read this file: {}",
                    status["error"].as_str().unwrap_or("no reason given")
                ));
            }
            _ => {
                let submitted = task["submitted_at"]
                    .as_str()
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
                if submitted.is_some_and(|t| {
                    chrono::Utc::now().signed_duration_since(t)
                        > chrono::Duration::hours(PATIENCE_HOURS)
                }) {
                    utopia_store::documents::clear_reader_task(pool, doc.id).await?;
                    return Err(anyhow!(
                        "The OCR service did not finish reading this file within {PATIENCE_HOURS} hours"
                    )
                    .context(Terminal));
                }
                return Err(anyhow!("waiting for the OCR service").context(Deferred::new(POLL)));
            }
        }

        let result: Value = self
            .request(
                &client,
                reqwest::Method::GET,
                &format!("tasks/{task_id}/result"),
            )
            .timeout(TRANSFER_TIMEOUT)
            .send()
            .await
            .context("the OCR service is unreachable")?
            .error_for_status()
            .context("the OCR service could not return the result")?
            .json()
            .await?;
        // 一次只交一份文件，结果里就一项；键是服务规整过的文件名，不去猜它的规则
        let entry = result["results"]
            .as_object()
            .and_then(|m| m.values().next())
            .ok_or_else(|| anyhow!("The OCR service returned no result for this file"))?;
        let list = match &entry["content_list"] {
            Value::String(s) => serde_json::from_str(s)
                .context("The OCR service returned a content list that is not JSON")?,
            other => other.clone(),
        };
        let model = ["mineru", jstr(&result["version"]), jstr(&result["backend"])]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(utopia_ingest::mineru::reading(&list, &model))
    }

    async fn submit(&self, filename: &str, bytes: Vec<u8>) -> anyhow::Result<String> {
        let client = crate::query_engine::http()?;
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")?;
        let mut form = reqwest::multipart::Form::new()
            .part("files", part)
            .text("return_content_list", "true")
            .text("return_md", "false");
        if let Some(backend) = self.backend {
            form = form.text("backend", backend.to_string());
        }
        let resp = self
            .request(&client, reqwest::Method::POST, "tasks")
            .multipart(form)
            .timeout(TRANSFER_TIMEOUT)
            .send()
            .await
            .context("the OCR service is unreachable")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "The OCR service refused this file ({status}): {}",
                body.chars().take(300).collect::<String>()
            ));
        }
        let v: Value = resp.json().await?;
        v["task_id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow!("The OCR service accepted the file but returned no task id"))
    }
}

fn jstr(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
