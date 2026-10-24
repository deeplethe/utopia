//! Every ticket in the GitHub and Jira fixtures renders exactly the text it rendered before the
//! two connectors shared one renderer (0013 decision 7). A changed byte re-versions every ticket
//! on its next sync, so this compares whole documents, not fragments.
//!
//! Regenerate only on purpose: `UTOPIA_WRITE_TICKET_GOLDEN=1 cargo test -p utopia-server ticket_golden`.

use std::collections::BTreeMap;

const GOLDEN: &str = "tests/fixtures/ticket_render_golden.json";

fn rendered() -> BTreeMap<String, String> {
    use crate::{github_issues as gh, jira_issues as jira};
    #[derive(serde::Deserialize)]
    struct Fixture {
        issues: Vec<gh::Issue>,
        comments: Vec<gh::Comment>,
        events_by_issue: std::collections::HashMap<String, Vec<gh::Event>>,
    }
    let mut out = BTreeMap::new();
    // The fixtures never label a ticket, assign a GitHub one, leave a comment empty or a Jira comment
    // undated, so one variant of each first ticket adds those branches
    let mut raw: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/github_issues.json")).unwrap();
    let mut extra = raw["issues"][0].clone();
    let n = extra["number"].as_i64().unwrap() + 1000;
    extra["number"] = n.into();
    extra["labels"] = serde_json::json!([{ "name": "bug" }, { "name": "docs" }]);
    extra["assignees"] = serde_json::json!([{ "login": "ann" }, { "login": "bo" }]);
    let url = raw["comments"][0]["issue_url"]
        .as_str()
        .unwrap()
        .rsplit_once('/')
        .unwrap()
        .0
        .to_string();
    let at = extra["created_at"].clone();
    raw["comments"].as_array_mut().unwrap().extend([
        serde_json::json!({ "body": "  ", "created_at": at, "issue_url": format!("{url}/{n}"), "user": { "login": "ann" } }),
        serde_json::json!({ "body": "Looks right.", "created_at": at, "issue_url": format!("{url}/{n}"), "user": null }),
    ]);
    raw["events_by_issue"][n.to_string()] = serde_json::json!([
        { "event": "labeled", "created_at": at, "actor": { "login": "ann" }, "label": { "name": "bug" } },
        { "event": "assigned", "created_at": at, "actor": null, "assignee": { "login": "bo" } },
    ]);
    raw["issues"].as_array_mut().unwrap().push(extra);
    let f: Fixture = serde_json::from_value(raw).unwrap();
    for issue in &f.issues {
        // the same calls the sync makes (`ingest_sources`)
        let cs = gh::comments_for_issue(issue, &f.comments);
        let events = gh::sort_events(
            f.events_by_issue
                .get(&issue.number.to_string())
                .cloned()
                .unwrap_or_default(),
        );
        let es: Vec<&gh::Event> = events.iter().collect();
        out.insert(
            format!("github #{}", issue.number),
            gh::render(issue, &cs, &es),
        );
    }
    let mut raw: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/jira_issues.json")).unwrap();
    let mut extra = raw["issues"][0].clone();
    extra["key"] = format!("{}-VARIANT", extra["key"].as_str().unwrap()).into();
    extra["fields"]["labels"] = serde_json::json!(["backend", "flaky"]);
    extra["fields"]["comment"]["comments"] = serde_json::json!([
        { "author": { "displayName": "Ann" }, "body": "Undated.", "created": null },
        { "author": null, "body": " ", "created": null },
    ]);
    raw["issues"].as_array_mut().unwrap().push(extra);
    let page: jira::SearchPage = serde_json::from_value(raw).unwrap();
    for issue in &page.issues {
        out.insert(format!("jira {}", issue.key), jira::render(issue));
    }
    out
}

#[test]
fn ticket_golden_every_fixture_ticket_renders_its_recorded_text() {
    let now = rendered();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::var("UTOPIA_WRITE_TICKET_GOLDEN").is_ok() {
        std::fs::write(&path, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
    }
    let golden: BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        now.keys().collect::<Vec<_>>(),
        golden.keys().collect::<Vec<_>>()
    );
    for (key, text) in &now {
        assert_eq!(text, &golden[key], "{key} renders differently");
    }
}
