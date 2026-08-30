//! GitHub 工单来源：一张工单 = 一篇文档，正文里带**它的状态变更史**。
//!
//! ## 为什么不是只抓当前状态
//!
//! 工单最有价值的部分不是"它现在是 closed"，而是"它 8 月 18 日开出、8 月 20 日
//! 关掉、中间被指派给谁、标签怎么变的"。只抓当前状态，这条时间线要靠一次次同步
//! 慢慢攒出来——第一次同步只能看见此刻，之前发生的全丢了。
//!
//! 与我们给维基百科历史快照做的是同一个判断：**别取现在，取变化**。区别是
//! GitHub 直接把变化给了你，不用像维基那样从修订列表里采样。
//!
//! ## 评论走仓库级，事件走逐个——这不是不一致，是两个端点能力不同
//!
//! 第一版想让三样都走仓库级端点、一次分页取全，避免 200 张工单 401 次请求
//! （未认证时 GitHub 每小时只给 60 次）。**拿真实数据一跑就发现事件那一路是错的**：
//!
//! - `issues/comments` 支持 `since`，增量窗口内的评论一次取全。**好用**。
//! - `issues/events` **不支持 `since`**，只能从最新往回翻。而 GitHub 的模型里
//!   PR 也产生 issue 事件——实测本仓库工单事件埋在第 5 页，换一个 PR 活跃的仓库
//!   就会被推到翻页上限之外。**于是"状态变更史"悄悄变成空的，而它正是这个来源
//!   存在的理由。**
//!
//! 所以事件改成逐工单取 `GET /repos/{repo}/issues/{n}/events`。N+1 的代价是真的，
//! 但 N 只是**本轮要写入的工单数**：首次同步等于工单总数，之后有 `since` 兜着，
//! 通常是个位数。用一次准确换一次省事，这里该换。
//!
//! 三次拉取因此是：
//!
//! - `GET /repos/{repo}/issues?state=all&since=` —— 工单本体（分页）
//! - `GET /repos/{repo}/issues/comments?since=`  —— 全仓库评论（分页），按号归拢
//! - `GET /repos/{repo}/issues/{n}/events`       —— 每张工单一次
//!
//! ## 关于 PR
//!
//! GitHub 的数据模型里 PR 也是 issue，`/issues` 端点会把它们一起返回，靠
//! `pull_request` 字段区分。默认排除：问"工单系统"要的是工单。但留了开关——
//! 有些仓库（包括本仓库）的决策记录实际写在 PR 描述里。

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;

/// 一次同步最多取多少页（每页 100）。GitHub 的分页没有天然终点，
/// 一个活跃仓库能翻很久；这里封顶，超出的等下一次 `since` 增量取。
const MAX_PAGES: u32 = 10;
const PER_PAGE: u32 = 100;

#[derive(Debug, Deserialize)]
pub struct Issue {
    pub number: i64,
    pub title: String,
    pub state: String,
    pub body: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub assignees: Vec<Actor>,
    pub user: Option<Actor>,
    /// 存在即说明这条其实是 PR（GitHub 用同一张表存两者）
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Actor {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct Comment {
    /// 评论挂在哪张工单上：只有 URL 里有号，得从 `.../issues/18` 末段解析
    pub issue_url: String,
    pub user: Option<Actor>,
    pub created_at: DateTime<Utc>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub event: String,
    pub created_at: DateTime<Utc>,
    pub actor: Option<Actor>,
    pub label: Option<Label>,
    pub assignee: Option<Actor>,
}

/// 评论的 `issue_url` 末段就是工单号。
///
/// 解析失败返回 None 而不是 panic：GitHub 换了 URL 形状时，代价该是
/// "这条评论没归到工单上"，不是整次同步炸掉。
fn issue_number_from_url(url: &str) -> Option<i64> {
    url.rsplit('/').next()?.parse().ok()
}

/// 把一张工单连同它的评论与事件排成一篇文档。
///
/// **纯函数，不联网**——取回与组织分开，于是组织这一半测得动。
/// 三次分页的拼装逻辑（谁归谁、按什么排序）恰恰是最容易出错的部分。
pub fn render(issue: &Issue, comments: &[&Comment], events: &[&Event]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# #{} {}\n\n", issue.number, issue.title));

    // 抬头写成带日期的陈述句，而不是键值对：抽取器读的是句子。
    // "opened by X on 2026-08-18" 能抽出带 valid_from 的事实，
    // "created_at: 2026-08-18" 则要它自己去猜这是什么意思
    if let Some(u) = &issue.user {
        out.push_str(&format!(
            "Opened by {} on {}.\n",
            u.login,
            issue.created_at.format("%Y-%m-%d")
        ));
    }
    out.push_str(&format!("Currently {}.\n", issue.state));
    if let Some(c) = issue.closed_at {
        out.push_str(&format!("Closed on {}.\n", c.format("%Y-%m-%d")));
    }
    if !issue.labels.is_empty() {
        let names: Vec<&str> = issue.labels.iter().map(|l| l.name.as_str()).collect();
        out.push_str(&format!("Labelled {}.\n", names.join(", ")));
    }
    if !issue.assignees.is_empty() {
        let names: Vec<&str> = issue.assignees.iter().map(|a| a.login.as_str()).collect();
        out.push_str(&format!("Assigned to {}.\n", names.join(", ")));
    }

    if let Some(b) = issue
        .body
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
    {
        out.push_str("\n## Description\n\n");
        out.push_str(b);
        out.push('\n');
    }

    // **状态变更史是这个来源存在的理由。** 每一行都带日期，
    // 于是账本拿到的是"何时变成什么"，而不是一个静止的当前值
    if !events.is_empty() {
        out.push_str("\n## History\n\n");
        for e in events {
            let who = e.actor.as_ref().map(|a| a.login.as_str()).unwrap_or("?");
            let detail = match (e.event.as_str(), &e.label, &e.assignee) {
                ("labeled" | "unlabeled", Some(l), _) => format!(" ({})", l.name),
                ("assigned" | "unassigned", _, Some(a)) => format!(" ({})", a.login),
                _ => String::new(),
            };
            out.push_str(&format!(
                "- {} — {} by {}{}\n",
                e.created_at.format("%Y-%m-%d"),
                e.event,
                who,
                detail
            ));
        }
    }

    if !comments.is_empty() {
        out.push_str("\n## Comments\n\n");
        for c in comments {
            let who = c.user.as_ref().map(|a| a.login.as_str()).unwrap_or("?");
            let body = c.body.as_deref().unwrap_or("").trim();
            if body.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "### {} on {}\n\n{}\n\n",
                who,
                c.created_at.format("%Y-%m-%d"),
                body
            ));
        }
    }
    out
}

/// 把全仓库的评论按工单号归拢，各自按时间升序。
///
/// 评论是**全仓库**取回来的，里面混着不在本次工单集合里的（增量窗口不同步）。
/// 归不到工单上的直接丢——它们下次会跟着自己的工单一起回来。
pub fn group_comments<'a>(
    issues: &'a [Issue],
    comments: &'a [Comment],
) -> Vec<(&'a Issue, Vec<&'a Comment>)> {
    let mut by_issue: HashMap<i64, Vec<&Comment>> = HashMap::new();
    for c in comments {
        if let Some(n) = issue_number_from_url(&c.issue_url) {
            by_issue.entry(n).or_default().push(c);
        }
    }
    issues
        .iter()
        .map(|issue| {
            let mut cs = by_issue.remove(&issue.number).unwrap_or_default();
            cs.sort_by_key(|c| c.created_at);
            (issue, cs)
        })
        .collect()
}

/// 事件按时间升序。逐工单端点返回的**看起来**是升序，但顺序不是契约，
/// 而这里排错了的后果是历史倒着讲。
pub fn sort_events(mut events: Vec<Event>) -> Vec<Event> {
    events.sort_by_key(|e| e.created_at);
    events
}

/// 分页取一个端点，直到空页或触顶。
pub async fn fetch_all<T: for<'de> Deserialize<'de>>(
    http: &reqwest::Client,
    base: &str,
    query: &[(&str, String)],
    auth: Option<&str>,
) -> anyhow::Result<Vec<T>> {
    let mut out = Vec::new();
    for page in 1..=MAX_PAGES {
        // 自己拼 query：这套 reqwest 特性组合里没有 RequestBuilder::query，
        // 而 sync_custom 本来也是这么拼的
        let mut url = reqwest::Url::parse(base)?;
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in query {
                q.append_pair(k, v);
            }
            q.append_pair("per_page", &PER_PAGE.to_string());
            q.append_pair("page", &page.to_string());
        }
        let mut req = http.get(url);
        if let Some(a) = auth {
            req = req.header(reqwest::header::AUTHORIZATION, a);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            // 限流说清楚是限流：未认证时每小时 60 次，一个中等仓库一次同步就能吃光。
            // 报成通用 HTTP 错会让人去查网络，而正确的动作是配一个令牌
            let remaining = resp
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("?");
            if status == reqwest::StatusCode::FORBIDDEN && remaining == "0" {
                anyhow::bail!(
                    "GitHub 限流：本小时配额已用尽。未认证时每小时 60 次，配一个令牌可提到 5000"
                );
            }
            anyhow::bail!("HTTP {status} from GitHub");
        }
        let batch: Vec<T> = resp.json().await?;
        let n = batch.len();
        out.extend(batch);
        if n < PER_PAGE as usize {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(json: serde_json::Value) -> Issue {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_comment_url_yields_its_issue_number() {
        assert_eq!(
            issue_number_from_url("https://api.github.com/repos/a/b/issues/18"),
            Some(18)
        );
        // 形状变了就归不上，但不该炸
        assert_eq!(issue_number_from_url("https://example.com/"), None);
        assert_eq!(issue_number_from_url("nonsense"), None);
    }

    /// **状态变更史必须进正文。** 这是这个来源与"抓一个网页"的全部区别：
    /// 少了它，一张工单在账本里只是一个静止的当前值。
    #[test]
    fn the_history_lands_in_the_document_with_dates() {
        let i = issue(serde_json::json!({
            "number": 18, "title": "Deleting a conversation removes all turns",
            "state": "closed", "body": "Steps to reproduce…",
            "created_at": "2026-08-18T16:18:27Z", "updated_at": "2026-08-20T22:31:47Z",
            "closed_at": "2026-08-20T22:31:47Z",
            "labels": [{"name": "bug"}], "assignees": [{"login": "WaylandYang"}],
            "user": {"login": "Danmushu"}
        }));
        let e1: Event = serde_json::from_value(serde_json::json!({
            "event": "labeled", "created_at": "2026-08-19T01:00:00Z",
            "actor": {"login": "WaylandYang"}, "label": {"name": "bug"}
        }))
        .unwrap();
        let e2: Event = serde_json::from_value(serde_json::json!({
            "event": "closed", "created_at": "2026-08-20T22:31:47Z",
            "actor": {"login": "WaylandYang"}
        }))
        .unwrap();
        let out = render(&i, &[], &[&e1, &e2]);

        assert!(out.contains("Opened by Danmushu on 2026-08-18."), "{out}");
        assert!(out.contains("Closed on 2026-08-20."), "{out}");
        assert!(out.contains("Labelled bug."), "{out}");
        assert!(out.contains("Assigned to WaylandYang."), "{out}");
        // 事件行带日期与执行者，labeled 还要带上是哪个标签
        assert!(
            out.contains("- 2026-08-19 — labeled by WaylandYang (bug)"),
            "{out}"
        );
        assert!(
            out.contains("- 2026-08-20 — closed by WaylandYang"),
            "{out}"
        );
    }

    /// 评论是**全仓库**取的，归拢必须按工单号，且按时间升序。
    /// 归错了的后果很隐蔽：另一张工单的讨论出现在这张的正文里。
    #[test]
    fn repo_wide_comments_land_on_the_right_issue() {
        let issues = vec![
            issue(serde_json::json!({
                "number": 1, "title": "one", "state": "open", "body": null,
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-02T00:00:00Z",
                "closed_at": null, "user": {"login": "a"}
            })),
            issue(serde_json::json!({
                "number": 2, "title": "two", "state": "open", "body": null,
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-02T00:00:00Z",
                "closed_at": null, "user": {"login": "b"}
            })),
        ];
        let comments: Vec<Comment> = serde_json::from_value(serde_json::json!([
            {"issue_url": "https://api.github.com/repos/a/b/issues/2",
             "user": {"login": "x"}, "created_at": "2026-01-05T00:00:00Z", "body": "第二条"},
            {"issue_url": "https://api.github.com/repos/a/b/issues/2",
             "user": {"login": "y"}, "created_at": "2026-01-03T00:00:00Z", "body": "第一条"},
            // 不在本次工单集合里：该被丢掉，而不是挂到别人身上
            {"issue_url": "https://api.github.com/repos/a/b/issues/99",
             "user": {"login": "z"}, "created_at": "2026-01-04T00:00:00Z", "body": "别人的"}
        ]))
        .unwrap();
        let grouped = group_comments(&issues, &comments);
        assert_eq!(grouped.len(), 2);

        let (one, one_comments) = &grouped[0];
        assert_eq!(one.number, 1);
        assert!(one_comments.is_empty(), "1 号没有评论");

        let (two, two_comments) = &grouped[1];
        assert_eq!(two.number, 2);
        assert_eq!(two_comments.len(), 2, "99 号那条不该混进来");
        // 升序：先"第一条"（01-03）后"第二条"（01-05）
        assert_eq!(two_comments[0].body.as_deref(), Some("第一条"));
    }

    /// 逐工单端点返回的事件里**没有 issue 字段**（上下文已经在 URL 里），
    /// 所以那个字段必须是可选的——第一版按仓库级响应写成必填，换端点后会解不出来。
    #[test]
    fn per_issue_events_parse_without_an_issue_field() {
        let evs: Vec<Event> = serde_json::from_value(serde_json::json!([
            {"event": "closed", "created_at": "2026-01-06T00:00:00Z", "actor": {"login": "x"}},
            {"event": "labeled", "created_at": "2026-01-02T00:00:00Z",
             "actor": {"login": "y"}, "label": {"name": "bug"}}
        ]))
        .unwrap();
        let sorted = sort_events(evs);
        // 端点的顺序不是契约，排序自己做
        assert_eq!(sorted[0].event, "labeled");
        assert_eq!(sorted[1].event, "closed");
    }

    /// PR 在 GitHub 的模型里也是 issue，靠这个字段认出来。
    #[test]
    fn a_pull_request_is_recognisable_among_the_issues() {
        let pr = issue(serde_json::json!({
            "number": 3, "title": "a pr", "state": "open", "body": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "closed_at": null, "user": {"login": "a"},
            "pull_request": {"url": "https://api.github.com/repos/a/b/pulls/3"}
        }));
        assert!(pr.pull_request.is_some());
        let plain = issue(serde_json::json!({
            "number": 4, "title": "an issue", "state": "open", "body": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "closed_at": null, "user": {"login": "a"}
        }));
        assert!(plain.pull_request.is_none());
    }

    /// **拿真实响应钉住字段形状。**
    ///
    /// 手写的 JSON 只能证明"我以为的形状"解得出来。GitHub 一条 issue 有上百个字段，
    /// 我们只声明了十个；哪个字段其实叫别的名、哪个在某些情况下是 null，
    /// 只有真数据说得清。夹具取自 deeplethe/utopia，字段裁到我们声明的那些
    /// （裁剪本身也顺带证明了未声明的字段不会让 serde 失败）。
    ///
    /// 尤其钉住一件事：**逐工单事件端点不返回 `issue` 字段**。第一版按仓库级
    /// 响应写成必填，换端点后会整片解不出来。
    #[test]
    fn the_real_github_shapes_still_parse() {
        #[derive(serde::Deserialize)]
        struct Fixture {
            issues: Vec<Issue>,
            comments: Vec<Comment>,
            events_by_issue: std::collections::HashMap<String, Vec<Event>>,
        }
        let raw = include_str!("../tests/fixtures/github_issues.json");
        let f: Fixture = serde_json::from_str(raw).expect("真实响应该解得出来");

        assert!(!f.issues.is_empty(), "夹具是空的，这条测试就什么都没验");
        // 全是 PR 的话，PR 过滤那条路就没被这份夹具覆盖到
        assert!(
            f.issues.iter().all(|i| i.pull_request.is_none()),
            "夹具应当只含真工单"
        );

        let grouped = group_comments(&f.issues, &f.comments);
        assert_eq!(grouped.len(), f.issues.len());

        // 每张工单都排得出一篇非空文档，且抬头那句带真实日期
        for (issue, cs) in &grouped {
            let events = sort_events(
                f.events_by_issue
                    .get(&issue.number.to_string())
                    .cloned()
                    .unwrap_or_default(),
            );
            let es: Vec<&Event> = events.iter().collect();
            let doc = render(issue, cs, &es);
            assert!(
                doc.contains(&format!("# #{} ", issue.number)),
                "#{} 的抬头不对：{doc}",
                issue.number
            );
            assert!(
                doc.contains(&issue.created_at.format("%Y-%m-%d").to_string()),
                "#{} 正文里没有创建日期",
                issue.number
            );
        }

        // 这份夹具里每张工单都被关掉过，所以历史一节必须出现——
        // 它是这个来源存在的理由，空了就等于退回"抓一个网页"
        let (first, cs) = &grouped[0];
        let events = sort_events(
            f.events_by_issue
                .get(&first.number.to_string())
                .cloned()
                .unwrap_or_default(),
        );
        assert!(!events.is_empty(), "夹具里 #{} 没有事件", first.number);
        let es: Vec<&Event> = events.iter().collect();
        let doc = render(first, cs, &es);
        assert!(doc.contains("## History"), "历史一节缺失：{doc}");
        assert!(doc.contains("— closed by"), "关闭事件没写进历史：{doc}");
    }

    /// 空评论不该在正文里留下一个只有标题的空节。
    #[test]
    fn an_empty_comment_leaves_no_stub() {
        let i = issue(serde_json::json!({
            "number": 1, "title": "t", "state": "open", "body": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "closed_at": null, "user": {"login": "a"}
        }));
        let c: Comment = serde_json::from_value(serde_json::json!({
            "issue_url": "https://api.github.com/repos/a/b/issues/1",
            "user": {"login": "x"}, "created_at": "2026-01-02T00:00:00Z", "body": "   "
        }))
        .unwrap();
        let out = render(&i, &[&c], &[]);
        assert!(!out.contains("### x"), "空评论不该留下小节：{out}");
    }
}
