//! Jira 工单来源：一张工单 = 一篇文档，正文里带**字段级的变更史**。
//!
//! 与 [`crate::github_issues`] 是同一个判断（别取现在，取变化），但 Jira 给的
//! 原料更强，取法也更省：
//!
//! ## 一次调用就够
//!
//! GitHub 那边要三次拉取，事件还被迫走逐工单（`issues/events` 不支持 `since`，
//! 且会被 PR 事件淹没）。Jira 的 `search` 一次就能带回全部：
//!
//! ```text
//! GET /rest/api/2/search?jql=…&expand=changelog&fields=…,comment
//! ```
//!
//! 工单本体、完整变更史、评论一并返回，**没有 N+1**。
//!
//! ## 变更史是字段级的 from → to
//!
//! GitHub 的事件只说"发生了 labeled"，Jira 直接说"哪个字段从什么变成什么"：
//!
//! ```text
//! 2026-08-24  Mickael Maison  Version: (空) → 4.1.0
//! 2026-08-24  Luke Chen       status: Patch Available → Resolved
//! ```
//!
//! 对账本来说这是更好的原料——`from`/`to` 本身就是一次认知变更的两端。
//!
//! ## 增量靠 JQL，不靠 since 参数
//!
//! Jira 没有 `since`，但 JQL 能表达：`updated >= "2026-08-30 12:00"`。
//! 时间格式必须是 Jira 认的那种（不是 RFC3339），且**要带引号**——
//! 这两点写错的症状都是 400，而不是"查不到"。
//!
//! ## Server/DC 与 Cloud 的差别
//!
//! 本模块按 **API v2**（Jira Server/DC）写。Cloud 的 v3 把 `description` 与
//! 评论正文换成了 ADF（一棵 JSON 树而非字符串）——那需要一个渲染器，是另一件事。
//! v2 在 Cloud 上通常仍可用且返回字符串，所以先只做 v2；真遇到只有 v3 的实例
//! 再补，那时才知道 ADF 的哪些节点是必须处理的。

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// 一次同步最多翻多少页。Jira 的 `total` 常常是几万，全量拉回来没有意义——
/// 增量窗口之外的等下一次 JQL 取。
const MAX_PAGES: u32 = 10;
const PAGE_SIZE: u32 = 50;

/// 想要的字段。**必须显式列**：不列 `comment` 就不返回评论，
/// 而默认返回全部字段会把响应撑到几百 KB 一条。
const FIELDS: &str = "summary,status,issuetype,priority,created,updated,resolutiondate,\
                      labels,assignee,reporter,description,comment";

#[derive(Debug, Deserialize)]
pub struct SearchPage {
    #[serde(default)]
    pub issues: Vec<Issue>,
    #[serde(default)]
    pub total: i64,
}

#[derive(Debug, Deserialize)]
pub struct Issue {
    pub key: String,
    pub fields: Fields,
    /// 只有 `expand=changelog` 时才有
    #[serde(default)]
    pub changelog: Option<Changelog>,
}

#[derive(Debug, Deserialize)]
pub struct Fields {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub created: Option<JiraTime>,
    pub updated: Option<JiraTime>,
    pub resolutiondate: Option<JiraTime>,
    pub status: Option<Named>,
    pub issuetype: Option<Named>,
    pub priority: Option<Named>,
    pub assignee: Option<User>,
    pub reporter: Option<User>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub comment: Option<Comments>,
}

/// Jira 的时间戳是 `2026-08-24T11:11:52.944+0000`——**没有冒号的时区偏移**，
/// 不是 RFC3339。chrono 的 `DateTime<Utc>` 默认解不了它。
#[derive(Debug, Clone, Copy)]
pub struct JiraTime(pub DateTime<Utc>);

impl<'de> Deserialize<'de> for JiraTime {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        DateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.3f%z")
            .or_else(|_| DateTime::parse_from_rfc3339(&s))
            .map(|t| JiraTime(t.with_timezone(&Utc)))
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Deserialize)]
pub struct Named {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    #[serde(alias = "displayName")]
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Comments {
    #[serde(default)]
    pub comments: Vec<Comment>,
}

#[derive(Debug, Deserialize)]
pub struct Comment {
    pub author: Option<User>,
    pub created: Option<JiraTime>,
    pub body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Changelog {
    #[serde(default)]
    pub histories: Vec<History>,
}

#[derive(Debug, Deserialize)]
pub struct History {
    pub created: Option<JiraTime>,
    pub author: Option<User>,
    #[serde(default)]
    pub items: Vec<ChangeItem>,
}

#[derive(Debug, Deserialize)]
pub struct ChangeItem {
    pub field: Option<String>,
    #[serde(alias = "fromString")]
    pub from_string: Option<String>,
    #[serde(alias = "toString")]
    pub to_string: Option<String>,
}

fn name(n: &Option<Named>) -> Option<&str> {
    n.as_ref()?.name.as_deref()
}
fn who(u: &Option<User>) -> &str {
    u.as_ref()
        .and_then(|x| x.display_name.as_deref())
        .unwrap_or("?")
}

/// 把一张工单排成一篇文档。**纯函数，不联网**——取回与组织分开，组织这一半测得动。
pub fn render(issue: &Issue) -> String {
    let f = &issue.fields;
    let mut out = String::new();
    out.push_str(&format!(
        "# {} {}\n\n",
        issue.key,
        f.summary.as_deref().unwrap_or("")
    ));

    // 抬头写成带日期的陈述句，不是键值对：抽取器读的是句子，
    // "Reported by X on 2026-08-24" 能抽出带 valid_from 的事实
    if let (Some(r), Some(c)) = (&f.reporter, &f.created) {
        out.push_str(&format!(
            "Reported by {} on {}.\n",
            who(&Some(User {
                display_name: r.display_name.clone()
            })),
            c.0.format("%Y-%m-%d")
        ));
    }
    if let Some(t) = name(&f.issuetype) {
        out.push_str(&format!("Type {t}.\n"));
    }
    if let Some(s) = name(&f.status) {
        out.push_str(&format!("Currently {s}.\n"));
    }
    if let Some(p) = name(&f.priority) {
        out.push_str(&format!("Priority {p}.\n"));
    }
    if let Some(a) = &f.assignee {
        out.push_str(&format!(
            "Assigned to {}.\n",
            a.display_name.as_deref().unwrap_or("?")
        ));
    }
    if let Some(r) = &f.resolutiondate {
        out.push_str(&format!("Resolved on {}.\n", r.0.format("%Y-%m-%d")));
    }
    if !f.labels.is_empty() {
        out.push_str(&format!("Labelled {}.\n", f.labels.join(", ")));
    }

    if let Some(d) = f
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        out.push_str("\n## Description\n\n");
        out.push_str(d);
        out.push('\n');
    }

    // **字段级变更史。** Jira 比 GitHub 多给的正是这个：不只是"发生了什么事件"，
    // 而是"哪个字段从什么变成了什么"——from/to 本身就是一次认知变更的两端
    let mut lines: Vec<(DateTime<Utc>, String)> = Vec::new();
    for h in issue.changelog.iter().flat_map(|c| c.histories.iter()) {
        let Some(at) = h.created else { continue };
        for item in &h.items {
            let Some(field) = item.field.as_deref() else {
                continue;
            };
            let from = item.from_string.as_deref().unwrap_or("(empty)");
            let to = item.to_string.as_deref().unwrap_or("(empty)");
            lines.push((
                at.0,
                format!(
                    "- {} — {} changed {field}: {from} → {to}\n",
                    at.0.format("%Y-%m-%d"),
                    who(&h.author),
                ),
            ));
        }
    }
    if !lines.is_empty() {
        // 端点的顺序不是契约，排序自己做——排错了的后果是历史倒着讲
        lines.sort_by_key(|(t, _)| *t);
        out.push_str("\n## History\n\n");
        for (_, l) in &lines {
            out.push_str(l);
        }
    }

    let comments: Vec<&Comment> = f
        .comment
        .as_ref()
        .map(|c| c.comments.iter().collect())
        .unwrap_or_default();
    if !comments.is_empty() {
        out.push_str("\n## Comments\n\n");
        for c in comments {
            let body = c.body.as_deref().unwrap_or("").trim();
            if body.is_empty() {
                continue;
            }
            let at = c
                .created
                .map(|t| t.0.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "?".into());
            out.push_str(&format!("### {} on {}\n\n{}\n\n", who(&c.author), at, body));
        }
    }
    out
}

/// 增量用的 JQL。**时间格式是 Jira 自己的**（`yyyy-MM-dd HH:mm`），不是 RFC3339，
/// 而且要带引号——两点写错的症状都是 400，不是"查不到"。
pub fn jql(project: &str, since: Option<DateTime<Utc>>) -> String {
    let mut q = format!("project = {project}");
    if let Some(t) = since {
        q.push_str(&format!(
            " AND updated >= \"{}\"",
            t.format("%Y-%m-%d %H:%M")
        ));
    }
    q.push_str(" ORDER BY updated ASC");
    q
}

/// 分页取。Jira 用 `startAt`/`maxResults`，`total` 常常是几万——
/// 靠 MAX_PAGES 封顶，剩下的等下一次增量窗口。
///
/// **把 `total` 一起返回**：截断了就得说出来。取回 500 条而服务端有 14506 条时，
/// 界面上"同步完成"是一句误导——真实情况是"这一轮只覆盖了一小段"。
pub async fn fetch_all(
    http: &reqwest::Client,
    base_url: &str,
    jql: &str,
    auth: Option<&str>,
) -> anyhow::Result<(Vec<Issue>, i64)> {
    let mut out = Vec::new();
    let mut total = 0i64;
    for page in 0..MAX_PAGES {
        let mut url = reqwest::Url::parse(&format!(
            "{}/rest/api/2/search",
            base_url.trim_end_matches('/')
        ))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("jql", jql);
            q.append_pair("expand", "changelog");
            q.append_pair("fields", FIELDS);
            q.append_pair("maxResults", &PAGE_SIZE.to_string());
            q.append_pair("startAt", &(page * PAGE_SIZE).to_string());
        }
        let mut req = http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(a) = auth {
            req = req.header(reqwest::header::AUTHORIZATION, a);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            // Jira 把 JQL 语法错误也报成 400，正文里才是原因。
            // 只说 "HTTP 400" 会让人去查网络，而真正该改的是 project key 或时间格式
            let detail = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "HTTP {status} from Jira: {}",
                detail.chars().take(200).collect::<String>()
            );
        }
        let page_data: SearchPage = resp.json().await?;
        total = page_data.total;
        let n = page_data.issues.len();
        out.extend(page_data.issues);
        if n < PAGE_SIZE as usize {
            break;
        }
    }
    Ok((out, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Jira 的时间戳不是 RFC3339。** `+0000` 没有冒号，chrono 的默认实现解不了。
    /// 这一条是整个模块最容易悄悄坏掉的地方：解不出来就是整页工单丢掉。
    #[test]
    fn jira_timestamps_are_not_rfc3339() {
        let t: JiraTime = serde_json::from_str("\"2026-08-24T11:11:52.944+0000\"").unwrap();
        assert_eq!(t.0.format("%Y-%m-%d %H:%M").to_string(), "2026-08-24 11:11");
        // 真 RFC3339 也要能收（Cloud 某些端点会给这种）
        let t2: JiraTime = serde_json::from_str("\"2026-08-24T11:11:52.944+00:00\"").unwrap();
        assert_eq!(t2.0.format("%Y-%m-%d").to_string(), "2026-08-24");
    }

    /// JQL 的时间格式与引号：两点写错都报 400，而不是"查不到"。
    #[test]
    fn the_incremental_jql_uses_jiras_own_time_format() {
        let t = DateTime::parse_from_rfc3339("2026-08-30T12:34:56Z")
            .unwrap()
            .with_timezone(&Utc);
        let q = jql("KAFKA", Some(t));
        assert!(q.contains("project = KAFKA"), "{q}");
        assert!(q.contains("updated >= \"2026-08-30 12:34\""), "{q}");
        assert!(q.contains("ORDER BY updated ASC"), "{q}");
        // 没有 since 时不该带上 updated 子句。**查子句不查词**——
        // ORDER BY updated ASC 里也有 "updated"，第一版断言就栽在这上面
        assert!(!jql("KAFKA", None).contains("updated >="));
    }

    /// **拿真实响应钉住字段形状。**
    ///
    /// 手写 JSON 只能证明"我以为的形状"。夹具取自 issues.apache.org（匿名可读），
    /// 与 GitHub 那份同一个道理——而且这次它要钉的东西更多：Jira 的驼峰命名
    /// （`fromString`/`displayName`）、非 RFC3339 的时间、以及 changelog 的嵌套。
    #[test]
    fn the_real_jira_shapes_still_parse() {
        let raw = include_str!("../tests/fixtures/jira_issues.json");
        let page: SearchPage = serde_json::from_str(raw).expect("真实响应该解得出来");
        assert!(!page.issues.is_empty(), "夹具是空的，这条测试什么都没验");

        for issue in &page.issues {
            let doc = render(issue);
            assert!(doc.starts_with(&format!("# {} ", issue.key)), "{doc}");
        }

        // 变更史是这个来源存在的理由。夹具里至少有一张带 changelog 的，
        // 且必须排成"字段: 旧值 → 新值"——只写"发生了变更"等于没说
        let with_log = page
            .issues
            .iter()
            .find(|i| {
                i.changelog
                    .as_ref()
                    .is_some_and(|c| c.histories.iter().any(|h| !h.items.is_empty()))
            })
            .expect("夹具里该有带 changelog 的工单");
        let doc = render(with_log);
        assert!(doc.contains("## History"), "历史一节缺失：{doc}");
        assert!(doc.contains(" changed "), "变更行没写成字段级：{doc}");
        assert!(doc.contains(" → "), "缺少 from → to：{doc}");
    }

    /// 空评论不该留下一个只有标题的空节（与 GitHub 那边同一个口径）。
    #[test]
    fn an_empty_comment_leaves_no_stub() {
        let issue: Issue = serde_json::from_value(serde_json::json!({
            "key": "X-1",
            "fields": {
                "summary": "t", "description": null, "labels": [],
                "created": "2026-01-01T00:00:00.000+0000",
                "updated": "2026-01-01T00:00:00.000+0000",
                "comment": {"comments": [
                    {"author": {"displayName": "a"},
                     "created": "2026-01-02T00:00:00.000+0000", "body": "   "}
                ]}
            }
        }))
        .unwrap();
        let out = render(&issue);
        assert!(!out.contains("### a"), "空评论不该留下小节：{out}");
    }
}
