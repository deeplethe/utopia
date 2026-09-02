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
//! **限流是每秒三次**（官方说法是「平均三次」）。所以取页面内容时是顺序而不是
//! 并发——并发上去只会换来一串 429，而我们的重试退避是为抽取那条路设计的，
//! 不该被摄入路径借用。

use anyhow::Context as _;
use chrono::{DateTime, Utc};

/// 请求头里的 API 版本。**写死而不是留给配置**：响应的形状跟着它变，
/// 让用户填一个我们没适配过的版本，换来的是解析静默失配。
const NOTION_VERSION: &str = "2026-03-11";

/// 一次同步最多取多少页。理由同别的来源：摄入不可逆，而一个 workspace
/// 可以有几万页。
const MAX_PAGES_PER_SYNC: usize = 500;

/// 每页最多取多少个 block。再深的页面截断，比让一次同步卡在一页上好。
const MAX_BLOCKS_PER_PAGE: usize = 500;

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
        .timeout(std::time::Duration::from_secs(60))
        .build()?)
}

/// 取 integration 能看见的所有页面。
///
/// **只搜页面，不搜 data source。** 后者是表格的容器，它自己没有正文；
/// 表格里的每一行是一个页面，会在同一次搜索里出现。
pub async fn fetch(token: &str, query: Option<&str>) -> anyhow::Result<(Vec<NotionPage>, bool)> {
    let http = client(token)?;
    let mut out = Vec::new();
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

        let resp = http
            .post("https://api.notion.com/v1/search")
            .json(&body)
            .send()
            .await
            .context("notion search")?;
        let status = resp.status();
        let v: serde_json::Value = resp.json().await.context("notion search 响应")?;
        if !status.is_success() {
            anyhow::bail!(
                "notion search 回了 {status}: {}",
                v["message"].as_str().unwrap_or("unknown")
            );
        }

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
            let text = page_text(&http, id).await.unwrap_or_else(|e| {
                tracing::warn!(%id, error = %e, "页面正文取不回来，只留标题");
                String::new()
            });

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
    Ok((out, truncated))
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
async fn page_text(http: &reqwest::Client, page_id: &str) -> anyhow::Result<String> {
    let mut out = String::new();
    let mut n = 0usize;
    let mut cursor: Option<String> = None;

    loop {
        let mut url = format!("https://api.notion.com/v1/blocks/{page_id}/children?page_size=100");
        if let Some(c) = &cursor {
            url.push_str(&format!("&start_cursor={c}"));
        }
        let resp = http.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("blocks 回了 {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;

        for b in v["results"].as_array().unwrap_or(&vec![]) {
            if n >= MAX_BLOCKS_PER_PAGE {
                return Ok(out);
            }
            n += 1;
            if let Some(line) = render_block(b) {
                out.push_str(&line);
                out.push('\n');
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
    Ok(out)
}

/// 把一个 block 渲染成一行文本。
///
/// **认不出的类型返回它的纯文本而不是丢掉。** Notion 的 block 类型一直在加，
/// 硬编码一张白名单意味着新类型静默消失；而所有带文字的 block 都把文字放在
/// `{type}.rich_text` 下，这个形状很稳。
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
        // 分割线与图片没有 rich_text，但它们在正文里也没有信息量
        "divider" | "image" | "video" | "file" => return None,
        // child_page 的标题在 `title` 而不是 rich_text
        "child_page" => format!("- {}", inner["title"].as_str().unwrap_or("")),
        _ if text.trim().is_empty() => return None,
        _ => text,
    })
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
        let (pages, _) = fetch(&token, None).await?;
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
