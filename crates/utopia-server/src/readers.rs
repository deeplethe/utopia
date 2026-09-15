//! 读字的服务（0040）：扫描件和图片交给版面识别服务 MinerU（`mineru-api`），录音交给会标
//! 说话人的转写模型。
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
use utopia_ingest::Reading;

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

/// 转写一段录音最多等多久。OpenAI 一次收 25MB，大约是一小时的压缩音频；本地服务读一小时
/// 录音在 CPU 上要十几分钟
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(1800);

/// 工作区配的转写模型：OpenAI 的 `/audio/transcriptions`，要 `diarized_json`
pub struct Transcriber<'a> {
    base: &'a str,
    key: Option<&'a str>,
    model: &'a str,
}

impl<'a> Transcriber<'a> {
    pub fn from_settings(s: &'a LlmSettings) -> Option<Self> {
        Some(Transcriber {
            base: s.transcribe_base_url.as_deref()?.trim_end_matches('/'),
            key: s.transcribe_api_key.as_deref().filter(|k| !k.is_empty()),
            model: s.transcribe_model.as_deref()?,
        })
    }

    async fn transcribe(
        &self,
        filename: &str,
        bytes: Vec<u8>,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let client = crate::query_engine::http()?;
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.model.to_string())
            .text("response_format", "diarized_json")
            // 超过 30 秒的录音，diarize 模型要求给切分策略
            .text("chunking_strategy", "auto");
        let mut req = client
            .post(format!("{}/audio/transcriptions", self.base))
            .multipart(form)
            .timeout(timeout);
        if let Some(key) = self.key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .await
            .context("the transcription model is unreachable")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "The transcription model refused this recording ({status}): {}",
                body.chars().take(300).collect::<String>()
            ));
        }
        resp.json()
            .await
            .context("The transcription model returned something that is not JSON")
    }

    /// 读这段录音。一次请求读完——没有可以接着问的远端任务；分不出说话人的结果在
    /// `transcript::reading` 里拒收（`NoSpeakers`）
    pub async fn read(&self, doc: &Document, bytes: Vec<u8>) -> anyhow::Result<Reading> {
        let response = self
            .transcribe(&doc.filename, bytes, TRANSCRIBE_TIMEOUT)
            .await?;
        utopia_ingest::transcript::reading(&response, self.model)
    }

    /// 连通性测试：送一秒静音。端点和模型认这个请求就算通——静音里没有说话人可标，
    /// 标不标得出要等第一段真录音
    pub async fn check(&self) -> anyhow::Result<()> {
        self.transcribe("silence.wav", silent_wav(), Duration::from_secs(60))
            .await
            .map(|_| ())
    }
}

/// 一秒 16kHz 单声道静音的 WAV
fn silent_wav() -> Vec<u8> {
    let (rate, samples): (u32, u32) = (16_000, 16_000);
    let data_len = samples * 2;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w.resize(44 + data_len as usize, 0);
    w
}

fn jstr(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}
