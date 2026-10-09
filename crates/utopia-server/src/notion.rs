//! Notion 来源：把 integration 能看见的页面同步进来。
//!
//! 按 [0013](../../docs/decisions/0013-a-source-should-hand-over-its-history.md)
//! 的四条判据，它比对象存储强一档：
//!
//! | 判据 | Notion |
//! |---|---|
//! | 真实时间戳 | `last_edited_time`，**是文档自己的编辑时刻**，不是我们抓它的时刻 |
//! | 会不会自我推翻 | 页面被反复改写正是它的常态 |
//! | 稳定身份 | 页面 UUID，改标题、挪位置都不变 |
//! | 企业知识住不住在那儿 | 制度、会议纪要、决策记录——正是这套系统要的东西 |
//!
//! **但它只交出现状，不交出历史。** Notion 的版本历史不在公开 API 里，
//! 所以跟工单系统不同：一次同步只能看见此刻，之前的编辑全靠一次次同步慢慢攒。
//! 这跟 `url` / `rss` 是同一个形状，而工单那两个能一次把变更史拿全。
//!
//! ## 两个容易踩的
//!
//! **`Notion-Version` 头是必填的**，而且值是日期。少了它接口直接 400，
//! 而错误信息只说 "missing version"，不会告诉你该填哪个。
//!
//! **限流是每秒三次**（官方说法是「平均三次」）。顺序发请求还不够：一个 100 毫秒
//! 的响应紧接着下一个请求就是每秒十次。#215 的复测在一个几百页的 workspace 上
//! 跑了三分钟，然后 search 回了 429，整次同步就此失败。所以现在两件事一起做：
//! 请求之间保底间隔（[`MIN_INTERVAL`]），撞上 429 时按 `Retry-After` 等完再发
//! （[`Paced::send`]）。仍然不并发。这套退避是本地的、很小的——抽取那条路的
//! 退避是为模型厂商设计的，判据和节奏都不同，不借用。
//!
//! 错误文案是英文：它会原样落到来源的同步状态里给用户看。

use std::time::{Duration, Instant};

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use reqwest::StatusCode;

/// 请求头里的 API 版本。**写死而不是留给配置**：响应的形状跟着它变，
/// 让用户填一个我们没适配过的版本，换来的是解析静默失配。
const NOTION_VERSION: &str = "2026-03-11";

/// 一次同步最多取多少页。理由同别的来源：摄入不可逆，而一个 workspace
/// 可以有几万页。
const MAX_PAGES_PER_SYNC: usize = 500;

/// 每页所有层级合计最多取多少个 block，包含无文字容器。
/// 再深的页面截断，比让一次同步卡在一页上好。
const MAX_BLOCKS_PER_PAGE: usize = 500;

/// 两次请求之间至少隔这么久。Notion 说的是「平均每秒三次」，取 350 毫秒留一点余量：
/// 总耗时还取决于每页的嵌套层级与分页数。
const MIN_INTERVAL: Duration = Duration::from_millis(350);
/// 一次请求撞上 429 最多等几回。`Retry-After` 通常是个位数秒，连等几回还在限流
/// 就不是节奏问题了，该把错误交出去。
const MAX_RATE_LIMIT_RETRIES: u32 = 5;
/// `Retry-After` 缺失或读不出来时等多久。
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(1);
/// 单次等待上限。头里的值我们照做，但不能让一个离谱的值把同步挂死。
const RETRY_AFTER_CAP: Duration = Duration::from_secs(30);

/// 一个待摄入的页面。
pub struct NotionPage {
    /// `notion://{page_id}`——页面 UUID 是它最稳的身份
    pub external_key: String,
    pub filename: String,
    pub text: String,
    pub last_edited: Option<DateTime<Utc>>,
}

fn client(token: &str) -> anyhow::Result<reqwest::Client> {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert("Notion-Version", NOTION_VERSION.parse()?);
    h.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse()?,
    );
    Ok(reqwest::Client::builder()
        .default_headers(h)
        .timeout(Duration::from_secs(60))
        .build()?)
}

/// 按 Notion 的节奏发请求的客户端：请求之间保底间隔，429 按 `Retry-After` 等。
struct Paced {
    http: reqwest::Client,
    last: Option<Instant>,
    // 私有地址入口让 HTTP 测试走真实取页和限流逻辑，不增加用户配置。
    api_root: String,
}

impl Paced {
    fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            last: None,
            api_root: "https://api.notion.com/v1".into(),
        }
    }

    /// 发一次请求并解析 JSON。`what` 进日志和错误文案（"search" / "blocks"）。
    ///
    /// 429 在这里消化：等 `Retry-After` 再发，最多 [`MAX_RATE_LIMIT_RETRIES`] 回；
    /// 其它非 2xx 直接报错，带上 Notion 自己的 message——它比状态码说得清楚。
    async fn send(
        &mut self,
        what: &str,
        build: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
    ) -> anyhow::Result<serde_json::Value> {
        let mut attempt = 0u32;
        loop {
            if let Some(last) = self.last {
                let since = last.elapsed();
                if since < MIN_INTERVAL {
                    tokio::time::sleep(MIN_INTERVAL - since).await;
                }
            }
            let resp = build(&self.http)
                .send()
                .await
                .with_context(|| format!("notion {what}"))?;
            self.last = Some(Instant::now());

            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RATE_LIMIT_RETRIES {
                let wait = retry_after(
                    resp.headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|h| h.to_str().ok()),
                );
                attempt += 1;
                tracing::info!(
                    what,
                    attempt,
                    wait_ms = wait.as_millis() as u64,
                    "notion rate limited, waiting before the next attempt"
                );
                tokio::time::sleep(wait).await;
                continue;
            }

            let v: serde_json::Value = resp
                .json()
                .await
                .with_context(|| format!("notion {what}: reading the response"))?;
            if !status.is_success() {
                anyhow::bail!(
                    "notion {what} returned {status}: {}",
                    v["message"].as_str().unwrap_or("no message")
                );
            }
            return Ok(v);
        }
    }
}

/// `Retry-After` 头 → 等多久。Notion 给的是秒数（可能带小数）；缺失、读不出来、
/// 或负数都按缺省，离谱的值截到上限。纯函数，下面的单测钉住这几档。
fn retry_after(header: Option<&str>) -> Duration {
    header
        .and_then(|h| h.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_RETRY_AFTER)
        .min(RETRY_AFTER_CAP)
}

/// 取 integration 能看见的所有页面。
///
/// **只搜页面，不搜 data source。** 后者是表格的容器，它自己没有正文；
/// 表格里的每一行是一个页面，会在同一次搜索里出现。
///
/// 第三项是正文没读出来的页，每页一句原因。它们不在第一项里，见 `fetch_pages`。
pub async fn fetch(
    token: &str,
    query: Option<&str>,
) -> anyhow::Result<(Vec<NotionPage>, bool, Vec<String>)> {
    let mut http = Paced::new(client(token)?);
    fetch_pages(&mut http, query).await
}

async fn fetch_pages(
    http: &mut Paced,
    query: Option<&str>,
) -> anyhow::Result<(Vec<NotionPage>, bool, Vec<String>)> {
    let mut out = Vec::new();
    let mut unread: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut truncated = false;

    loop {
        let mut body = serde_json::json!({
            "filter": { "property": "object", "value": "page" },
            "page_size": 100,
        });
        if let Some(q) = query {
            body["query"] = serde_json::Value::String(q.to_string());
        }
        if let Some(c) = &cursor {
            body["start_cursor"] = serde_json::Value::String(c.clone());
        }

        let url = format!("{}/search", http.api_root);
        let v = http.send("search", |c| c.post(&url).json(&body)).await?;

        for p in v["results"].as_array().unwrap_or(&vec![]).clone() {
            // 回收站里的和归档的都不要——它们在界面上已经不算数了
            if p["in_trash"].as_bool() == Some(true) || p["is_archived"].as_bool() == Some(true) {
                continue;
            }
            if out.len() >= MAX_PAGES_PER_SYNC {
                truncated = true;
                break;
            }
            let Some(id) = p["id"].as_str() else { continue };
            let title = page_title(&p);
            // 正文读不出来的页整页跳过：不摄入，库里已有的正文原样留着，不拿仅标题的
            // 版本去替换它。**其余的页照常同步**——中止整轮是另一头的错：有的页永远
            // 读不出来（嵌套块没共享给 integration，Notion 回 404），那样整个来源
            // 就再也不动了。对象存储的目录占位符是同一个教训（#214）。
            let text = match page_text(http, id).await {
                Ok(text) => text,
                Err(error) => {
                    tracing::warn!(%id, error = %format!("{error:#}"), "notion page body could not be read, leaving the stored page as it is");
                    unread.push(format!("notion page {id}: {error:#}"));
                    continue;
                }
            };

            out.push(NotionPage {
                external_key: format!("notion://{id}"),
                filename: format!("{}.md", slug(&title)),
                text: format!("# {title}\n\n{text}"),
                last_edited: p["last_edited_time"]
                    .as_str()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc)),
            });
        }

        if truncated || v["has_more"].as_bool() != Some(true) {
            break;
        }
        cursor = v["next_cursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok((out, truncated, unread))
}

/// 页面标题。
///
/// **标题藏在 `properties` 里那个 `type == "title"` 的属性下，而它的名字不固定**：
/// 数据库里的页面可能叫 `Name`、`名称`、`任务`，普通页面叫 `title`。
/// 按名字找会在别人的 workspace 上找不到，所以按类型找。
fn page_title(page: &serde_json::Value) -> String {
    let props = page["properties"].as_object();
    let t = props.and_then(|m| {
        m.values()
            .find(|v| v["type"] == "title")
            .and_then(|v| v["title"].as_array())
    });
    let s = t
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r["plain_text"].as_str())
                .collect::<String>()
        })
        .unwrap_or_default();
    if s.trim().is_empty() {
        "untitled".into()
    } else {
        s
    }
}

/// 取一页的正文，逐层展开 block。
async fn page_text(http: &mut Paced, page_id: &str) -> anyhow::Result<String> {
    let mut out = String::new();
    let mut block_count = 0;
    append_children(http, page_id, &mut out, &mut block_count).await?;
    Ok(out)
}

/// 每个父块独立分页，先写父块，再读完子树，最后继续兄弟块。
/// 输出和预算由整页共享；递归深度也受同一块数上限约束。
async fn append_children(
    http: &mut Paced,
    parent_id: &str,
    out: &mut String,
    block_count: &mut usize,
) -> anyhow::Result<()> {
    let mut cursor: Option<String> = None;

    while *block_count < MAX_BLOCKS_PER_PAGE {
        let mut url = format!(
            "{}/blocks/{parent_id}/children?page_size=100",
            http.api_root
        );
        if let Some(c) = &cursor {
            url.push_str(&format!("&start_cursor={c}"));
        }
        let v = http.send("blocks", |c| c.get(&url)).await?;

        for b in v["results"].as_array().unwrap_or(&vec![]) {
            if *block_count >= MAX_BLOCKS_PER_PAGE {
                return Ok(());
            }
            *block_count += 1;
            if let Some(line) = render_block(b) {
                out.push_str(&line);
                out.push('\n');
            }
            // 不能因父块没有 rich_text 而跳过 column 等容器。
            // 独立页面由 search 单独摄入，数据库也不是父页正文的一部分。
            let independent_content =
                matches!(b["type"].as_str(), Some("child_page" | "child_database"));
            if *block_count < MAX_BLOCKS_PER_PAGE
                && b["has_children"].as_bool() == Some(true)
                && !independent_content
            {
                let id = b["id"]
                    .as_str()
                    .context("notion blocks: child block is missing id")?;
                if b["type"].as_str() == Some("table") {
                    let mut rows = String::new();
                    Box::pin(append_children(http, id, &mut rows, block_count)).await?;
                    out.push_str(&render_table(&b["table"], &rows)?);
                } else {
                    Box::pin(append_children(http, id, out, block_count)).await?;
                }
            }
        }
        if v["has_more"].as_bool() != Some(true) {
            break;
        }
        cursor = v["next_cursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok(())
}

/// 把一个 block 渲染成一行文本。
///
/// **认不出的类型返回它的纯文本而不是丢掉。** Notion 的 block 类型一直在加，
/// 硬编码一张白名单意味着新类型静默消失；普通文字在 `{type}.rich_text`，
/// 表格行则按 `table_row.cells` 保留每个单元格。
fn render_block(b: &serde_json::Value) -> Option<String> {
    let t = b["type"].as_str()?;
    let inner = &b[t];
    let text = rich_text(&inner["rich_text"]);

    Some(match t {
        "heading_1" => format!("## {text}"),
        "heading_2" => format!("### {text}"),
        "heading_3" => format!("#### {text}"),
        "bulleted_list_item" => format!("- {text}"),
        "numbered_list_item" => format!("1. {text}"),
        "to_do" => {
            let done = inner["checked"].as_bool() == Some(true);
            format!("- [{}] {text}", if done { "x" } else { " " })
        }
        "quote" => format!("> {text}"),
        "code" => {
            let lang = inner["language"].as_str().unwrap_or("");
            format!("```{lang}\n{text}\n```")
        }
        "table_row" => {
            let cells = inner["cells"].as_array()?;
            let cells: Vec<String> = cells
                .iter()
                .map(|cell| {
                    rich_text(cell)
                        .replace('\\', "\\\\")
                        .replace('|', "\\|")
                        .replace("\r\n", "\n")
                        .replace(['\r', '\n'], "<br>")
                })
                .collect();
            format!("| {} |", cells.join(" | "))
        }
        // 分割线与图片没有 rich_text，但它们在正文里也没有信息量
        "divider" | "image" | "video" | "file" => return None,
        // child_page 的标题在 `title` 而不是 rich_text
        "child_page" => format!("- {}", inner["title"].as_str().unwrap_or("")),
        _ if text.trim().is_empty() => return None,
        _ => text,
    })
}

/// 无列头的表用空表头占位，不能把第一条数据误标为列头。
fn render_table(table: &serde_json::Value, rows: &str) -> anyhow::Result<String> {
    if rows.is_empty() {
        return Ok(String::new());
    }
    let width = table["table_width"]
        .as_u64()
        .context("notion table: missing column count")?;
    let width = usize::try_from(width)?;
    anyhow::ensure!(width > 0, "notion table: column count must be positive");
    let has_header = table["has_column_header"].as_bool() == Some(true);
    let separator = format!("|{}\n", " --- |".repeat(width));
    let mut out = String::from("\n");
    if !has_header {
        out.push_str(&format!("|{}\n", "  |".repeat(width)));
        out.push_str(&separator);
    }
    for (index, row) in rows.lines().enumerate() {
        out.push_str(row);
        out.push('\n');
        if index == 0 && has_header {
            out.push_str(&separator);
        }
    }
    out.push('\n');
    Ok(out)
}

/// rich_text 数组拼成纯文本。
fn rich_text(v: &serde_json::Value) -> String {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r["plain_text"].as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// 标题变成能当文件名的东西。
fn slug(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "untitled".into()
    } else {
        s.chars().take(60).collect()
    }
}

#[cfg(test)]
#[path = "notion_tests.rs"]
mod traversal_tests;
#[cfg(test)]
mod tests {
    use super::*;

    /// **标题属性的名字是任意的。** 数据库里的页面可能把它叫 `Name`、`名称`、
    /// `任务`；按名字找会在别人的 workspace 上返回 untitled，而那看起来
    /// 像是「页面没有标题」而不是「我们找错了地方」。
    #[test]
    fn a_title_is_found_by_type_not_by_name() {
        for key in ["title", "Name", "名称", "任务"] {
            let page = serde_json::json!({
                "properties": {
                    key: { "type": "title", "title": [{ "plain_text": "季度复盘" }] },
                    "Status": { "type": "select", "select": { "name": "Done" } }
                }
            });
            assert_eq!(page_title(&page), "季度复盘", "属性名 {key} 时找不到标题");
        }
    }

    /// 富文本是分段的——加粗、链接都会把一句话切开。拼不全就会丢字。
    #[test]
    fn rich_text_segments_join_back_into_one_line() {
        let v = serde_json::json!([
            { "plain_text": "把总部搬到" },
            { "plain_text": "深圳" },
            { "plain_text": "了" }
        ]);
        assert_eq!(rich_text(&v), "把总部搬到深圳了");
    }

    /// **认不出的 block 类型不能丢。** Notion 一直在加类型，而带文字的
    /// block 都把文字放在 `{type}.rich_text` 下——按白名单渲染会让新类型
    /// 静默消失。
    #[test]
    fn an_unknown_block_keeps_its_text() {
        let b = serde_json::json!({
            "type": "some_new_block_type_2027",
            "some_new_block_type_2027": { "rich_text": [{ "plain_text": "还是有内容的" }] }
        });
        assert_eq!(render_block(&b).as_deref(), Some("还是有内容的"));
    }

    /// 没有文字的装饰性 block 该消失，否则正文里全是空行。
    #[test]
    fn a_divider_renders_to_nothing() {
        let b = serde_json::json!({ "type": "divider", "divider": {} });
        assert!(render_block(&b).is_none());
    }

    /// 文件名不能带路径分隔符或换行。
    #[test]
    fn a_slug_is_safe_as_a_filename() {
        assert_eq!(slug("2026 Q3 / 复盘"), "2026-Q3---复盘");
        assert_eq!(slug("///"), "untitled");
        assert!(slug(&"x".repeat(200)).chars().count() <= 60);
    }

    /// 真连一个 Notion workspace。**没有模拟器**——Notion 是闭源 SaaS，
    /// 开源的替代品（AppFlowy、AFFiNE）不说这套 API。所以这条只有在
    /// `Retry-After` 的几档：Notion 给的秒数照做，缺失和垃圾按缺省，离谱的截顶。
    #[test]
    fn retry_after_follows_the_header_within_bounds() {
        assert_eq!(retry_after(Some("3")), Duration::from_secs(3));
        assert_eq!(retry_after(Some(" 0.5 ")), Duration::from_millis(500));
        assert_eq!(retry_after(None), DEFAULT_RETRY_AFTER);
        assert_eq!(retry_after(Some("soon")), DEFAULT_RETRY_AFTER);
        assert_eq!(retry_after(Some("-2")), DEFAULT_RETRY_AFTER);
        assert_eq!(retry_after(Some("600")), RETRY_AFTER_CAP);
    }

    /// 有人给出真 token 时才跑，CI 上永远跳过。
    ///
    /// ```text
    /// # 设置 → 我的连接 → 新建内部集成，然后把一个页面分享给它
    /// UTOPIA_NOTION_TEST_TOKEN=ntn_xxx cargo test -p utopia-server notion
    /// ```
    #[tokio::test]
    async fn it_reads_from_a_real_workspace() -> anyhow::Result<()> {
        let Ok(token) = std::env::var("UTOPIA_NOTION_TEST_TOKEN") else {
            eprintln!("跳过：未设 UTOPIA_NOTION_TEST_TOKEN");
            return Ok(());
        };
        let (pages, _, _) = fetch(&token, None).await?;
        assert!(
            !pages.is_empty(),
            "一页都没有——integration 可能没有被分享任何页面"
        );
        let p = &pages[0];
        assert!(
            p.external_key.starts_with("notion://"),
            "{}",
            p.external_key
        );
        assert!(p.text.starts_with("# "), "正文该以标题开头");
        assert!(
            p.last_edited.is_some(),
            "last_edited_time 是 doc_time 的来源"
        );
        Ok(())
    }
}
