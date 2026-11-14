//! GitLab 工单来源（gitlab.com，#1327）：一张工单或一个合并请求 = 一篇文档，正文里带它的
//! 状态变更史。排版是三个工单来源共用的 [`crate::ticket`]（0013 决定 7），这里只管把 GitLab
//! 的数据装进去、以及怎么取。同步在 `ingest_sources`。
//!
//! 取法落在 GitHub 那个形状里：列表按更新时刻增量筛选，评论与事件逐工单读到末页。
//!
//! - `GET /projects/:id/issues?state=all&order_by=updated_at&sort=asc&updated_after=`
//! - `GET /projects/:id/merge_requests?…`（同样的参数，`include_merge_requests` 打开才读）
//! - 每张工单：`notes`、`resource_state_events`、`resource_label_events`、
//!   `resource_milestone_events`
//!
//! 与 GitHub 不同的三处：
//!
//! - **工单与合并请求是两张列表，却只有一个 checkpoint。** 所以 checkpoint 只能走到两张
//!   列表都读完的那个时刻，见 [`fetch_window`]。
//! - **变更史取 resource events，不取系统备注。** GitLab 把状态、标签、里程碑的变化记成
//!   结构化事件，同时又写一条给人看的系统备注；两样都读，每次变化就在历史里出现两遍。
//! - **备注与事件没有令牌就是 401**，公开项目也一样（2026-10-22 与 2026-11-14 在
//!   gitlab.com 上核对过）。所以这个来源要令牌，没有就直说，不去发那个注定失败的请求。
//!
//! 自建 GitLab 没有验证过。

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::ticket::stamp;

/// 列表预算可在完整时间组边界延长；单张工单的详情不受此预算限制。与 GitHub 同一口径（0072）
const MAX_LISTING_PAGES: u32 = 10;
/// GitLab 每页至多 100 条
const PER_PAGE: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Issue,
    MergeRequest,
}

impl Kind {
    /// API 路径里的那一段
    pub fn path(self) -> &'static str {
        match self {
            Self::Issue => "issues",
            Self::MergeRequest => "merge_requests",
        }
    }

    /// GitLab 自己的写法：`#12` 是工单，`!7` 是合并请求。两者的 iid 各自从 1 数起，
    /// 所以抬头和逻辑身份都得带上它
    pub fn sigil(self) -> char {
        match self {
            Self::Issue => '#',
            Self::MergeRequest => '!',
        }
    }
}

/// 列表里的一行。工单与合并请求读的是同一组字段，合并请求多一个 `merged_at`
#[derive(Debug, Deserialize)]
pub struct Item {
    pub iid: i64,
    pub title: String,
    pub state: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<User>,
    pub author: Option<User>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub struct Note {
    /// GitLab 自己写的那种（「closed」「added ~bug label」）；它们说的事 resource events 已经说了
    #[serde(default)]
    pub system: bool,
    pub author: Option<User>,
    pub created_at: DateTime<Utc>,
    pub body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StateEvent {
    pub user: Option<User>,
    pub created_at: DateTime<Utc>,
    /// `closed` / `reopened` / `merged`
    pub state: String,
}

#[derive(Debug, Deserialize)]
pub struct Named {
    #[serde(alias = "title")]
    pub name: String,
}

/// 标签事件与里程碑事件同一个形状：谁、何时、`add` 还是 `remove`、哪一个。
/// 标签或里程碑事后被删掉时那一项是 null
#[derive(Debug, Deserialize)]
pub struct ChangeEvent {
    pub user: Option<User>,
    pub created_at: DateTime<Utc>,
    pub action: String,
    #[serde(alias = "label", alias = "milestone")]
    pub what: Option<Named>,
}

/// 一张工单的四样详情，字段名就是端点名
#[derive(Debug, Default, Deserialize)]
pub struct Details {
    pub notes: Vec<Note>,
    pub resource_state_events: Vec<StateEvent>,
    pub resource_label_events: Vec<ChangeEvent>,
    pub resource_milestone_events: Vec<ChangeEvent>,
}

/// 把一张工单连同它的详情排成一篇文档。纯函数，不联网。
///
/// 句子的顺序跟 GitHub 一样（开出、状态、关闭、标签、指派），合并请求在「Closed on」旁边
/// 多一句「Merged on」。事件的说法也用 GitHub 的那几个词（labeled / unlabeled /
/// milestoned / demilestoned），同一件事在两个来源里读起来是同一句话
pub fn render(kind: Kind, item: &Item, details: &Details) -> String {
    fn who(u: &Option<User>) -> &str {
        u.as_ref().map(|u| u.username.as_str()).unwrap_or("?")
    }
    let mut sentences = Vec::new();
    if let Some(u) = &item.author {
        sentences.push(format!(
            "Opened by {} on {}.",
            u.username,
            stamp(item.created_at)
        ));
    }
    sentences.push(format!("Currently {}.", item.state));
    if let Some(t) = item.closed_at {
        sentences.push(format!("Closed on {}.", stamp(t)));
    }
    if let Some(t) = item.merged_at {
        sentences.push(format!("Merged on {}.", stamp(t)));
    }
    if !item.labels.is_empty() {
        sentences.push(format!("Labelled {}.", item.labels.join(", ")));
    }
    if !item.assignees.is_empty() {
        let names: Vec<&str> = item.assignees.iter().map(|a| a.username.as_str()).collect();
        sentences.push(format!("Assigned to {}.", names.join(", ")));
    }

    let change = |e: &ChangeEvent, add: &str, remove: &str| {
        let verb = match e.action.as_str() {
            "add" => add,
            "remove" => remove,
            other => other,
        };
        let detail = e
            .what
            .as_ref()
            .map(|w| format!(" ({})", w.name))
            .unwrap_or_default();
        (e.created_at, format!("{verb} by {}{detail}", who(&e.user)))
    };
    let history = details
        .resource_state_events
        .iter()
        .map(|e| (e.created_at, format!("{} by {}", e.state, who(&e.user))))
        .chain(
            details
                .resource_label_events
                .iter()
                .map(|e| change(e, "labeled", "unlabeled")),
        )
        .chain(
            details
                .resource_milestone_events
                .iter()
                .map(|e| change(e, "milestoned", "demilestoned")),
        )
        .collect();

    // 端点给的顺序不是契约，自己排
    let mut notes: Vec<&Note> = details.notes.iter().filter(|n| !n.system).collect();
    notes.sort_by_key(|n| n.created_at);
    let comments = notes
        .into_iter()
        .map(|n| crate::ticket::Comment {
            author: who(&n.author).to_owned(),
            at: Some(n.created_at),
            body: n.body.clone().unwrap_or_default(),
        })
        .collect();

    crate::ticket::render(&crate::ticket::Ticket {
        heading: format!("{}{} {}", kind.sigil(), item.iid, item.title),
        sentences,
        description: item.description.clone(),
        history,
        comments,
    })
}

/// `…/projects/:id` 后面接上几段。项目路径里的 `/` 已经在 `project` 那一段里编成 `%2F`
fn at(project: &reqwest::Url, segments: &[&str]) -> anyhow::Result<reqwest::Url> {
    let mut url = project.clone();
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("API base cannot take a path"))?
        .extend(segments);
    Ok(url)
}

/// 读到末页。详情不能用截断的备注或事件覆盖已有的完整正文（0072）
async fn fetch_all<T: for<'de> Deserialize<'de>>(
    http: &reqwest::Client,
    base: &reqwest::Url,
    auth: Option<&str>,
) -> anyhow::Result<Vec<T>> {
    let mut out = Vec::new();
    let mut page = 1;
    loop {
        let mut url = base.clone();
        url.query_pairs_mut()
            .append_pair("per_page", &PER_PAGE.to_string())
            .append_pair("page", &page.to_string());
        let batch: Vec<T> =
            crate::ingest_sources::gitlab_checked(http, reqwest::Method::GET, url, auth)
                .await?
                .json()
                .await?;
        let n = batch.len();
        out.extend(batch);
        if n < PER_PAGE as usize {
            return Ok(out);
        }
        page += 1;
    }
}

pub async fn fetch_details(
    http: &reqwest::Client,
    project: &reqwest::Url,
    kind: Kind,
    iid: i64,
    auth: Option<&str>,
) -> anyhow::Result<Details> {
    let iid = iid.to_string();
    let of = |what: &str| at(project, &[kind.path(), &iid, what]);
    Ok(Details {
        notes: fetch_all(http, &of("notes")?, auth).await?,
        resource_state_events: fetch_all(http, &of("resource_state_events")?, auth).await?,
        resource_label_events: fetch_all(http, &of("resource_label_events")?, auth).await?,
        resource_milestone_events: fetch_all(http, &of("resource_milestone_events")?, auth).await?,
    })
}

pub struct Window {
    pub tickets: Vec<(Kind, Item)>,
    pub checkpoint: Option<DateTime<Utc>>,
}

struct Pass {
    items: Vec<Item>,
    /// 这一遍列到的每一行（含跳过的旧行），复读时拿来核对有没有挪动
    positions: BTreeMap<i64, DateTime<Utc>>,
    checkpoint: Option<DateTime<Utc>>,
    /// 停在预算上，后面还有没读的时间组
    more: bool,
}

/// 选出本轮要写的工单，以及读完之后 checkpoint 该落在哪。
///
/// 每张列表各自按 0072 读：升序、跳过 checkpoint 及之前的、预算用完也把当前时间组读完。
/// **两张列表共用一个 checkpoint**，所以它只能落在两张都读完的时刻：有列表停在预算上，
/// 就取其中最早的那个停点，另一张列表里更晚的工单本轮不写，留给下一轮；都读到了头，
/// 才取最晚的那一个。不这样，停在预算上的那张列表里没读到的工单会被另一张的时刻越过去，
/// 之后的增量再也读不到它。
///
/// 然后每张列表复读到这个边界，核对分页期间有没有工单挪了位置（0072 的 2026-10-23 修订）
pub async fn fetch_window(
    http: &reqwest::Client,
    project: &reqwest::Url,
    kinds: &[Kind],
    since: Option<DateTime<Utc>>,
    item_limit: usize,
    auth: Option<&str>,
) -> anyhow::Result<Window> {
    // 预算是整轮的，两张列表各分一半；不分的话打开合并请求就是一轮读两倍的工单
    let item_limit = item_limit.div_ceil(kinds.len().max(1));
    let mut passes = Vec::new();
    for kind in kinds {
        let list = at(project, &[kind.path()])?;
        passes.push(fetch_pass(http, &list, since, item_limit, auth, None).await?);
    }
    let boundary = passes
        .iter()
        .filter(|p| p.more)
        .map(|p| p.checkpoint)
        .min()
        .unwrap_or_else(|| passes.iter().map(|p| p.checkpoint).max().flatten());
    let within = |t: DateTime<Utc>| boundary.is_some_and(|b| t <= b);

    let mut tickets = Vec::new();
    for (kind, pass) in kinds.iter().zip(passes) {
        if boundary != since {
            let list = at(project, &[kind.path()])?;
            let verified = fetch_pass(http, &list, since, item_limit, auth, boundary).await?;
            anyhow::ensure!(
                pass.positions
                    .iter()
                    .filter(|(_, t)| within(**t))
                    .eq(verified.positions.iter()),
                "GitLab {} changed during pagination; retry the previous checkpoint",
                kind.path()
            );
        }
        tickets.extend(
            pass.items
                .into_iter()
                .filter(|item| within(item.updated_at))
                .map(|item| (*kind, item)),
        );
    }
    Ok(Window {
        tickets,
        checkpoint: boundary,
    })
}

/// 一张列表读一遍。`through` 给了就是复读：不看预算，读到那个时刻为止
async fn fetch_pass(
    http: &reqwest::Client,
    list: &reqwest::Url,
    since: Option<DateTime<Utc>>,
    item_limit: usize,
    auth: Option<&str>,
    through: Option<DateTime<Utc>>,
) -> anyhow::Result<Pass> {
    let mut pass = Pass {
        items: Vec::new(),
        positions: BTreeMap::new(),
        checkpoint: since,
        more: false,
    };
    let mut previous_time = None;
    let mut stop_after = through;
    let mut listing_pages = 0;
    let mut page = 1;
    loop {
        let mut url = list.clone();
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("state", "all")
                .append_pair("order_by", "updated_at")
                .append_pair("sort", "asc");
            if let Some(since) = since {
                q.append_pair("updated_after", &since.to_rfc3339());
            }
            q.append_pair("per_page", &PER_PAGE.to_string())
                .append_pair("page", &page.to_string());
        }
        let batch: Vec<Item> =
            crate::ingest_sources::gitlab_checked(http, reqwest::Method::GET, url, auth)
                .await?
                .json()
                .await?;
        let count = batch.len();
        let mut page_counted = false;
        for item in batch {
            let updated = item.updated_at;
            anyhow::ensure!(
                previous_time.is_none_or(|previous| updated >= previous),
                "GitLab tickets are not ordered by update time; retry the previous checkpoint"
            );
            previous_time = Some(updated);
            if stop_after.is_some_and(|boundary| updated > boundary) {
                pass.more = true;
                return Ok(pass);
            }
            anyhow::ensure!(
                pass.positions.insert(item.iid, updated).is_none(),
                "GitLab ticket repeated during pagination; retry the previous checkpoint"
            );
            // `updated_after` 含边界（2026-11-14 在 gitlab.com 上核对过），读过的会再回来一次
            if since.is_some_and(|checkpoint| updated <= checkpoint) {
                continue;
            }
            if !page_counted {
                listing_pages += 1;
                page_counted = true;
            }
            pass.checkpoint = Some(updated);
            pass.items.push(item);
            if through.is_none()
                && (pass.items.len() >= item_limit || listing_pages >= MAX_LISTING_PAGES)
            {
                stop_after = Some(updated);
            }
        }
        if count < PER_PAGE as usize {
            return Ok(pass);
        }
        page += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

    #[derive(Deserialize)]
    struct Fixture {
        issues: Vec<Item>,
        merge_requests: Vec<Item>,
        details: std::collections::HashMap<String, Details>,
    }

    fn fixture() -> Fixture {
        serde_json::from_str(include_str!("../tests/fixtures/gitlab_issues.json")).unwrap()
    }

    #[test]
    fn a_recorded_issue_tells_its_history_from_events_and_its_comments_from_people() {
        let f = fixture();
        let issue = f.issues.iter().find(|i| i.iid == 1).unwrap();
        let text = render(Kind::Issue, issue, &f.details["issues/1"]);
        assert!(text.starts_with(
            "# #1 A test\n\nOpened by quintinraptorbotes on 2026-10-22T13:50:23Z.\nCurrently opened.\nLabelled bug.\n"
        ));
        // 毫秒截到秒，事件按时刻排，三种事件混在一条时间线上
        let history: Vec<&str> = text
            .split("## History\n\n")
            .nth(1)
            .unwrap()
            .split("\n\n")
            .next()
            .unwrap()
            .lines()
            .collect();
        assert_eq!(
            history,
            [
                "- 2026-10-22T13:51:28Z — closed by quintinraptorbotes",
                "- 2026-10-22T13:51:31Z — reopened by quintinraptorbotes",
                "- 2026-11-14T21:36:03Z — labeled by quintinraptorbotes (bug)",
                "- 2026-11-14T21:36:03Z — labeled by quintinraptorbotes (docs)",
                "- 2026-11-14T21:36:05Z — unlabeled by quintinraptorbotes (docs)",
                "- 2026-11-14T21:36:05Z — milestoned by quintinraptorbotes (v1)",
                "- 2026-11-14T21:36:06Z — closed by quintinraptorbotes",
                "- 2026-11-14T21:36:07Z — reopened by quintinraptorbotes",
            ]
        );
        assert!(text.contains(
            "### quintinraptorbotes on 2026-11-14T21:36:07Z\n\nStill happens on the nightly build, reopening.\n"
        ));
    }

    #[test]
    fn a_merged_request_says_when_it_merged_and_leaves_system_notes_out() {
        let f = fixture();
        let merged = f.merge_requests.iter().find(|m| m.iid == 1).unwrap();
        let details = &f.details["merge_requests/1"];
        assert!(
            details.notes.iter().any(|n| n.system),
            "the recording should hold a system note, or this test proves nothing"
        );
        let text = render(Kind::MergeRequest, merged, details);
        assert!(text.starts_with("# !1 Add the merged fixture file\n"));
        assert!(text.contains("Currently merged.\nMerged on 2026-11-14T21:36:15Z.\n"));
        assert!(text.contains("— merged by quintinraptorbotes\n"));
        assert!(text.contains("Looks right. Merging."));
        assert!(!text.contains("mentioned in commit"));

        let closed = f.merge_requests.iter().find(|m| m.iid == 2).unwrap();
        let text = render(Kind::MergeRequest, closed, &f.details["merge_requests/2"]);
        assert!(text.contains("Currently closed.\nClosed on 2026-11-14T21:36:19Z.\n"));
        assert!(!text.contains("Merged on"));
    }

    fn row(iid: i64, updated: &str) -> Value {
        json!({
            "iid": iid, "title": format!("Ticket {iid}"), "state": "opened", "description": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": updated, "closed_at": null,
        })
    }

    /// 两张列表各给一组行，按 `page` 切页
    async fn server(issues: Vec<Value>, merge_requests: Vec<Value>) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(move |request: &Request| {
                let rows = if request.url.path().ends_with("/merge_requests") {
                    &merge_requests
                } else {
                    &issues
                };
                let page: usize = request
                    .url
                    .query_pairs()
                    .find(|(key, _)| key == "page")
                    .unwrap()
                    .1
                    .parse()
                    .unwrap();
                let rows: Vec<_> = rows.iter().skip((page - 1) * 100).take(100).collect();
                ResponseTemplate::new(200).set_body_json(rows)
            })
            .mount(&server)
            .await;
        server
    }

    async fn window(
        server: &MockServer,
        since: Option<&str>,
        item_limit: usize,
    ) -> anyhow::Result<Window> {
        let http = reqwest::Client::builder().no_proxy().build()?;
        let project = reqwest::Url::parse(&format!("{}/projects/g%2Fp", server.uri()))?;
        fetch_window(
            &http,
            &project,
            &[Kind::Issue, Kind::MergeRequest],
            since.map(|s| s.parse().unwrap()),
            item_limit,
            None,
        )
        .await
    }

    fn listed(window: &Window) -> Vec<String> {
        window
            .tickets
            .iter()
            .map(|(kind, item)| format!("{}{}", kind.sigil(), item.iid))
            .collect()
    }

    #[tokio::test]
    async fn a_list_that_stops_at_its_budget_holds_the_checkpoint_back_for_both_lists(
    ) -> anyhow::Result<()> {
        // 整轮预算四条，每张列表两条。工单在 01:00 那一组用完预算，02:00 的留给下一轮。
        // 合并请求只有两条，读到了头，但 03:00 那条在工单的停点之后
        let server = server(
            vec![
                row(1, "2026-01-02T01:00:00Z"),
                row(2, "2026-01-02T01:00:00Z"),
                row(3, "2026-01-02T02:00:00Z"),
            ],
            vec![
                row(1, "2026-01-02T00:30:00Z"),
                row(2, "2026-01-02T03:00:00Z"),
            ],
        )
        .await;
        let first = window(&server, None, 4).await?;
        assert_eq!(listed(&first), ["#1", "#2", "!1"]);
        assert_eq!(
            first.checkpoint,
            Some("2026-01-02T01:00:00Z".parse().unwrap())
        );

        // 下一轮从那个停点起：读过的三条回来了也不再写，剩下的两条都读到，checkpoint 走到最晚的
        let second = window(&server, Some("2026-01-02T01:00:00Z"), 200).await?;
        assert_eq!(listed(&second), ["#3", "!2"]);
        assert_eq!(
            second.checkpoint,
            Some("2026-01-02T03:00:00Z".parse().unwrap())
        );

        // 没有新东西：checkpoint 留在原地，不走到墙上的钟
        let third = window(&server, Some("2026-01-02T03:00:00Z"), 200).await?;
        assert!(third.tickets.is_empty());
        assert_eq!(third.checkpoint, second.checkpoint);
        Ok(())
    }

    #[tokio::test]
    async fn a_ticket_that_moves_between_the_two_readings_fails_the_window() -> anyhow::Result<()> {
        // 第一遍读到 #1 在 01:00；复读时它已经被改到 01:00 之后，边界以内少了一行
        let server = MockServer::start().await;
        let calls = std::sync::atomic::AtomicUsize::new(0);
        Mock::given(method("GET"))
            .respond_with(move |request: &Request| {
                if request.url.path().ends_with("/merge_requests") {
                    return ResponseTemplate::new(200).set_body_json(json!([]));
                }
                let moved = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0;
                let first = if moved {
                    row(1, "2026-01-02T05:00:00Z")
                } else {
                    row(1, "2026-01-02T01:00:00Z")
                };
                let mut rows = vec![row(2, "2026-01-02T01:00:00Z"), first];
                rows.sort_by_key(|r| r["updated_at"].as_str().unwrap().to_owned());
                ResponseTemplate::new(200).set_body_json(rows)
            })
            .mount(&server)
            .await;
        let error = window(&server, None, 200).await.err().unwrap().to_string();
        assert!(error.contains("changed during pagination"), "{error}");
        Ok(())
    }
}
