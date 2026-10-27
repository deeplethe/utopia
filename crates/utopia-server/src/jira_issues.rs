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
use std::collections::BTreeMap;
use std::time::Duration;

use crate::ticket::stamp;

/// 达到列表预算后仍读完当前时间组，下次从完整组之后继续。
const MAX_PAGES: u32 = 10;
const PAGE_SIZE: u32 = 50;
const MAX_RATE_LIMIT_RETRIES: u32 = 3;

/// 想要的字段。**必须显式列**：不列 `comment` 就不返回评论，
/// 而默认返回全部字段会把响应撑到几百 KB 一条。
const FIELDS: &str = "summary,status,issuetype,priority,created,updated,resolutiondate,\
                      labels,assignee,reporter,description,comment";

#[derive(Debug, Deserialize)]
pub struct SearchPage {
    #[serde(default)]
    pub issues: Vec<Issue>,
    // total 用来识别提前结束的分页，缺失时不能把空页当作完整结果。
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
    // 抬头写成带日期的陈述句，不是键值对：抽取器读的是句子，
    // "Reported by X on 2026-08-24T11:11:52Z" 能抽出带 valid_from 的事实。
    // Jira 给的毫秒不写，抽取器也只认到秒
    let mut sentences = Vec::new();
    if let (Some(r), Some(c)) = (&f.reporter, &f.created) {
        sentences.push(format!(
            "Reported by {} on {}.",
            who(&Some(User {
                display_name: r.display_name.clone()
            })),
            stamp(c.0)
        ));
    }
    if let Some(t) = name(&f.issuetype) {
        sentences.push(format!("Type {t}."));
    }
    if let Some(s) = name(&f.status) {
        sentences.push(format!("Currently {s}."));
    }
    if let Some(p) = name(&f.priority) {
        sentences.push(format!("Priority {p}."));
    }
    if let Some(a) = &f.assignee {
        sentences.push(format!(
            "Assigned to {}.",
            a.display_name.as_deref().unwrap_or("?")
        ));
    }
    if let Some(r) = &f.resolutiondate {
        sentences.push(format!("Resolved on {}.", stamp(r.0)));
    }
    if !f.labels.is_empty() {
        sentences.push(format!("Labelled {}.", f.labels.join(", ")));
    }

    // **字段级变更史。** Jira 比 GitHub 多给的正是这个：不只是"发生了什么事件"，
    // 而是"哪个字段从什么变成了什么"——from/to 本身就是一次认知变更的两端
    let mut history = Vec::new();
    for h in issue.changelog.iter().flat_map(|c| c.histories.iter()) {
        let Some(at) = h.created else { continue };
        for item in &h.items {
            let Some(field) = item.field.as_deref() else {
                continue;
            };
            let from = item.from_string.as_deref().unwrap_or("(empty)");
            let to = item.to_string.as_deref().unwrap_or("(empty)");
            history.push((
                at.0,
                format!("{} changed {field}: {from} → {to}", who(&h.author)),
            ));
        }
    }

    let comments = f
        .comment
        .iter()
        .flat_map(|c| c.comments.iter())
        .map(|c| crate::ticket::Comment {
            author: who(&c.author).to_string(),
            at: c.created.map(|t| t.0),
            body: c.body.clone().unwrap_or_default(),
        })
        .collect();
    crate::ticket::render(&crate::ticket::Ticket {
        heading: format!("{} {}", issue.key, f.summary.as_deref().unwrap_or("")),
        sentences,
        description: f.description.clone(),
        history,
        comments,
    })
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
    q.push_str(" ORDER BY updated ASC, key ASC");
    q
}

pub struct IssueWindow {
    pub issues: Vec<Issue>,
    pub checkpoint: Option<DateTime<Utc>>,
}

/// 复读固定的已选前缀，检测更新导致的 offset 位移；变化时不确认本轮 checkpoint。
pub async fn fetch_window(
    http: &reqwest::Client,
    base_url: &str,
    project: &str,
    since: Option<DateTime<Utc>>,
    item_limit: usize,
    auth: Option<&str>,
) -> anyhow::Result<IssueWindow> {
    let query = jql(project, since);
    let (window, positions) =
        fetch_window_pass(http, base_url, &query, since, item_limit, auth, None).await?;
    if let Some(checkpoint) = window
        .checkpoint
        .filter(|checkpoint| Some(*checkpoint) != since)
    {
        let (verified, verified_positions) = fetch_window_pass(
            http,
            base_url,
            &query,
            since,
            item_limit,
            auth,
            Some(checkpoint),
        )
        .await?;
        anyhow::ensure!(
            verified.checkpoint == Some(checkpoint) && positions == verified_positions,
            "Jira issues changed during pagination; retry the previous checkpoint"
        );
    }
    Ok(window)
}

async fn fetch_window_pass(
    http: &reqwest::Client,
    base_url: &str,
    query: &str,
    since: Option<DateTime<Utc>>,
    item_limit: usize,
    auth: Option<&str>,
    through: Option<DateTime<Utc>>,
) -> anyhow::Result<(IssueWindow, BTreeMap<String, DateTime<Utc>>)> {
    let mut window = IssueWindow {
        issues: Vec::new(),
        checkpoint: since,
    };
    let mut positions = BTreeMap::new();
    let mut previous_time = None;
    let mut stop_after = through;
    let mut listing_pages = 0;
    let mut start_at = 0;
    loop {
        let mut url = reqwest::Url::parse(&format!(
            "{}/rest/api/2/search",
            base_url.trim_end_matches('/')
        ))?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("jql", query);
            q.append_pair("expand", "changelog");
            q.append_pair("fields", FIELDS);
            q.append_pair("maxResults", &PAGE_SIZE.to_string());
            q.append_pair("startAt", &start_at.to_string());
        }
        let page_data = fetch_page(http, url, auth).await?;
        let count = page_data.issues.len();
        anyhow::ensure!(
            page_data.total >= 0 && (start_at + count) as i64 <= page_data.total,
            "Jira returned an inconsistent page total; retry the previous checkpoint"
        );
        anyhow::ensure!(
            count != 0 || start_at as i64 >= page_data.total,
            "Jira returned an empty page before its total; retry the previous checkpoint"
        );
        let mut page_counted = false;
        for issue in page_data.issues {
            let updated = issue
                .fields
                .updated
                .ok_or_else(|| {
                    anyhow::anyhow!("Jira issue {} is missing its update time", issue.key)
                })?
                .0;
            anyhow::ensure!(
                previous_time.is_none_or(|previous| updated >= previous),
                "Jira issues are not ordered by update time; retry the previous checkpoint"
            );
            previous_time = Some(updated);
            if stop_after.is_some_and(|boundary| updated > boundary) {
                return Ok((window, positions));
            }
            // 分钟取整返回的旧记录也可能改变 offset，复读时不能只比较新记录。
            anyhow::ensure!(
                positions.insert(issue.key.clone(), updated).is_none(),
                "Jira issue repeated during pagination; retry the previous checkpoint"
            );
            if since.is_some_and(|checkpoint| updated <= checkpoint) {
                continue;
            }
            if !page_counted {
                listing_pages += 1;
                page_counted = true;
            }
            window.checkpoint = Some(updated);
            window.issues.push(issue);
            if through.is_none()
                && (window.issues.len() >= item_limit || listing_pages >= MAX_PAGES)
            {
                stop_after = Some(updated);
            }
        }
        start_at += count;
        if count == 0 || start_at as i64 >= page_data.total {
            return Ok((window, positions));
        }
    }
}

async fn fetch_page(
    http: &reqwest::Client,
    url: reqwest::Url,
    auth: Option<&str>,
) -> anyhow::Result<SearchPage> {
    let mut retries = 0;
    loop {
        let mut req = http
            .get(url.clone())
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(auth) = auth {
            req = req.header(reqwest::header::AUTHORIZATION, auth);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp.json().await?);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS && retries < MAX_RATE_LIMIT_RETRIES {
            if let Some(delay) = rate_limit_delay(resp.headers(), Utc::now()) {
                // 保留已读取的组和当前页，不能把限流变成从头扫描的循环。
                let delay = delay.max(Duration::from_secs(1 << retries));
                retries += 1;
                tracing::info!(
                    retries,
                    wait_seconds = delay.as_secs(),
                    "Jira rate limited; waiting before retry"
                );
                drop(resp);
                tokio::time::sleep(delay).await;
                continue;
            }
        }
        // Jira 把 JQL 语法错误也报成 400，正文里才是原因。
        let detail = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "HTTP {status} from Jira: {}",
            detail.chars().take(200).collect::<String>()
        );
    }
}

fn rate_limit_delay(headers: &reqwest::header::HeaderMap, now: DateTime<Utc>) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    let delay = value
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .or_else(|| {
            DateTime::parse_from_rfc2822(value)
                .ok()
                .map(|at| at.signed_duration_since(now).to_std().unwrap_or_default())
        })?;
    // shortcut: 长等待走普通失败重试；严格遵循跨任务的 Retry-After 需要持久化调度时间。
    (delay <= Duration::from_secs(30)).then_some(delay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, Ordering};
    use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

    fn listing_row(number: i64, updated: &str) -> Value {
        json!({"key": format!("X-{number}"), "fields": {"summary": "Issue", "updated": updated}})
    }

    fn requested_offset(request: &Request) -> usize {
        request
            .url
            .query_pairs()
            .find(|(key, _)| key == "startAt")
            .unwrap()
            .1
            .parse()
            .unwrap()
    }

    #[test]
    fn rate_limit_headers_supply_a_bounded_wait_without_guessing() {
        let now: DateTime<Utc> = "2026-01-02T00:00:00Z".parse().unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(rate_limit_delay(&headers, now), None);
        for (value, expected) in [
            ("2", Some(2)),
            ("0", Some(0)),
            ("30", Some(30)),
            ("31", None),
            ("-1", None),
            ("broken", None),
            ("86401", None),
        ] {
            headers.insert(reqwest::header::RETRY_AFTER, value.parse().unwrap());
            assert_eq!(
                rate_limit_delay(&headers, now),
                expected.map(Duration::from_secs)
            );
        }
        for (seconds, expected) in [(3, Some(3)), (30, Some(30)), (31, None)] {
            headers.insert(
                reqwest::header::RETRY_AFTER,
                (now + chrono::Duration::seconds(seconds))
                    .to_rfc2822()
                    .parse()
                    .unwrap(),
            );
            assert_eq!(
                rate_limit_delay(&headers, now),
                expected.map(Duration::from_secs)
            );
        }
    }

    #[tokio::test]
    async fn a_long_rate_limit_wait_fails_without_holding_the_sync_slot() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "31"))
            .mount(&server)
            .await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = reqwest::Url::parse(&server.uri()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), fetch_page(&http, url, None))
            .await
            .expect("a long Retry-After must return an error without waiting");
        assert!(result.unwrap_err().to_string().contains("HTTP 429"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_rate_limited_page_resumes_without_rereading_the_previous_page() {
        let server = MockServer::start().await;
        let limited = AtomicBool::new(false);
        Mock::given(method("GET"))
            .respond_with(move |request: &Request| {
                let offset = requested_offset(request);
                if offset == 50 && !limited.swap(true, Ordering::SeqCst) {
                    return ResponseTemplate::new(429).insert_header("Retry-After", "0");
                }
                let rows: Vec<_> = (1..=51)
                    .skip(offset)
                    .take(50)
                    .map(|number| listing_row(number, "2026-01-02T00:00:00Z"))
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({"issues": rows, "total": 51}))
            })
            .mount(&server)
            .await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let started = std::time::Instant::now();
        let window = fetch_window(&http, &server.uri(), "X", None, 200, Some("Bearer fixture"))
            .await
            .unwrap();
        assert_eq!(window.issues.len(), 51);
        assert!(started.elapsed() >= Duration::from_secs(1));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.iter().map(requested_offset).collect::<Vec<_>>(),
            [0, 50, 50, 0, 50]
        );
        assert!(requests
            .iter()
            .all(
                |request| request.headers.get("authorization").unwrap() == "Bearer fixture"
                    && request.headers.get("accept").unwrap() == "application/json"
            ));
    }

    #[tokio::test]
    async fn an_empty_window_retains_the_previous_checkpoint_without_verification() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"issues": [], "total": 0})),
            )
            .mount(&server)
            .await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let since = Some("2026-01-02T00:00:00Z".parse().unwrap());
        let window = fetch_window(&http, &server.uri(), "X", since, 200, None)
            .await
            .unwrap();
        assert!(window.issues.is_empty());
        assert_eq!(window.checkpoint, since);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_large_timestamp_group_finishes_and_the_next_run_passes_its_old_pages() {
        // Jira may return fewer items per page than the requested maxResults.
        for (group_size, page_size) in [(501, 50), (101, 10)] {
            let server = MockServer::start().await;
            let group_time = "2026-01-02T12:34:56Z";
            let later_time = "2026-01-02T12:34:57Z";
            let mut rows: Vec<_> = (1..=group_size)
                .map(|number| listing_row(number, group_time))
                .collect();
            rows.push(listing_row(group_size + 1, later_time));
            Mock::given(method("GET"))
                .respond_with(move |request: &Request| {
                    let offset = requested_offset(request);
                    let page: Vec<_> = rows.iter().skip(offset).take(page_size).cloned().collect();
                    ResponseTemplate::new(200).set_body_json(
                        json!({"issues": page, "total": rows.len(), "maxResults": page_size}),
                    )
                })
                .mount(&server)
                .await;
            let http = reqwest::Client::builder().no_proxy().build().unwrap();
            let first = fetch_window(&http, &server.uri(), "X", None, 200, None)
                .await
                .unwrap();
            assert_eq!(first.issues.len(), group_size as usize);
            assert_eq!(
                first
                    .issues
                    .iter()
                    .map(|issue| issue.key.clone())
                    .collect::<Vec<_>>(),
                (1..=group_size)
                    .map(|number| format!("X-{number}"))
                    .collect::<Vec<_>>()
            );
            assert_eq!(first.checkpoint, Some(group_time.parse().unwrap()));
            let next = fetch_window(&http, &server.uri(), "X", first.checkpoint, 200, None)
                .await
                .unwrap();
            assert_eq!(next.issues.len(), 1);
            assert_eq!(next.issues[0].key, format!("X-{}", group_size + 1));
            assert_eq!(next.checkpoint, Some(later_time.parse().unwrap()));
            let requests = server.received_requests().await.unwrap();
            assert!(requests.iter().any(|request| {
                requested_offset(request) == 10 * page_size
                    && request.url.query_pairs().any(|(key, value)| {
                        key == "jql"
                    && value
                        == "project = X AND updated >= \"2026-01-02 12:34\" ORDER BY updated ASC, key ASC"
                    })
            }));
        }
    }

    #[tokio::test]
    async fn a_moved_issue_rejects_the_window_and_a_stable_retry_recovers_every_issue() {
        let server = MockServer::start().await;
        let moved = AtomicBool::new(false);
        Mock::given(method("GET"))
            .respond_with(move |request: &Request| {
                let offset = requested_offset(request);
                if offset == 50 {
                    moved.store(true, Ordering::SeqCst);
                }
                let mut rows: Vec<_> = (1..=201)
                    .map(|number| listing_row(number, "2026-01-02T00:00:00Z"))
                    .collect();
                if moved.load(Ordering::SeqCst) {
                    rows.remove(0);
                    rows.push(listing_row(1, "2026-01-03T00:00:00Z"));
                }
                let page: Vec<_> = rows.into_iter().skip(offset).take(50).collect();
                ResponseTemplate::new(200).set_body_json(json!({"issues": page, "total": 201}))
            })
            .mount(&server)
            .await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let result = fetch_window(&http, &server.uri(), "X", None, 200, None).await;
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("changed during pagination"));

        let recovered = fetch_window(&http, &server.uri(), "X", None, 200, None)
            .await
            .unwrap();
        let mut keys: Vec<_> = recovered.issues.iter().map(|issue| &issue.key).collect();
        keys.sort();
        let mut expected_keys: Vec<_> = (2..=201).map(|number| format!("X-{number}")).collect();
        expected_keys.sort();
        assert_eq!(keys, expected_keys.iter().collect::<Vec<_>>());
        assert_eq!(
            recovered.checkpoint,
            Some("2026-01-02T00:00:00Z".parse().unwrap())
        );

        let next = fetch_window(&http, &server.uri(), "X", recovered.checkpoint, 200, None)
            .await
            .unwrap();
        assert_eq!(next.issues.len(), 1);
        assert_eq!(next.issues[0].key, "X-1");
        assert_eq!(
            next.checkpoint,
            Some("2026-01-03T00:00:00Z".parse().unwrap())
        );
    }

    #[tokio::test]
    async fn an_issue_without_an_update_time_cannot_advance_a_checkpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issues": [{"key": "X-1", "fields": {"summary": "Missing time"}}], "total": 1,
            })))
            .mount(&server)
            .await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let result = fetch_window(&http, &server.uri(), "X", None, 200, None).await;
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("missing its update time"));
    }

    #[tokio::test]
    async fn an_empty_page_cannot_finish_a_group_when_jira_reports_unread_items() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(move |request: &Request| {
                let rows: Vec<_> = if requested_offset(request) == 0 {
                    (1..=50)
                        .map(|number| listing_row(number, "2026-01-02T00:00:00Z"))
                        .collect()
                } else {
                    Vec::new()
                };
                ResponseTemplate::new(200).set_body_json(json!({"issues": rows, "total": 51}))
            })
            .mount(&server)
            .await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let result = fetch_window(&http, &server.uri(), "X", None, 200, None).await;
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("empty page before its total"));
        assert!(serde_json::from_value::<SearchPage>(json!({"issues": []})).is_err());
        for response in [
            json!({"issues": [], "total": -1}),
            json!({"issues": [listing_row(1, "2026-01-02T00:00:00Z")], "total": 0}),
        ] {
            server.reset().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .mount(&server)
                .await;
            let result = fetch_window(&http, &server.uri(), "X", None, 200, None).await;
            assert!(result
                .err()
                .unwrap()
                .to_string()
                .contains("inconsistent page total"));
        }
    }

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
        assert_eq!(
            jql("KAFKA", Some(t)),
            "project = KAFKA AND updated >= \"2026-08-30 12:34\" ORDER BY updated ASC, key ASC"
        );
        assert_eq!(
            jql("KAFKA", None),
            "project = KAFKA ORDER BY updated ASC, key ASC"
        );
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

    /// 正文里的时刻写到秒、带 `Z`，毫秒不写；抽取器读回来是 second 精度（#691）
    #[test]
    fn a_time_in_the_document_is_written_to_the_second() {
        let issue: Issue = serde_json::from_value(serde_json::json!({
            "key": "X-2",
            "fields": {
                "summary": "t", "description": null, "labels": [],
                "reporter": {"displayName": "Mickael"},
                "created": "2026-08-24T23:59:59.944+0000",
                "updated": "2026-08-25T00:00:00.000+0000",
                "resolutiondate": "2026-08-25T08:30:00.123+0800"
            }
        }))
        .unwrap();
        let out = render(&issue);
        assert!(
            out.contains("Reported by Mickael on 2026-08-24T23:59:59Z."),
            "{out}"
        );
        assert!(out.contains("Resolved on 2026-08-25T00:30:00Z."), "{out}");
        let (_, precision) = utopia_extract::parse_time("2026-08-24T23:59:59Z").unwrap();
        assert_eq!(precision, "second");
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
