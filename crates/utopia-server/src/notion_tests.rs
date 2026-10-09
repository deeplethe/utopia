use super::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use wiremock::{matchers::method, matchers::path, Mock, MockServer, Request, ResponseTemplate};

fn paced(server: &MockServer) -> Paced {
    // 本地 mock 不走开发机的系统代理。
    let mut http = Paced::new(reqwest::Client::builder().no_proxy().build().unwrap());
    http.api_root = server.uri();
    http
}

fn block(id: &str, kind: &str, text: &str, has_children: bool) -> Value {
    json!({
        "id": id, "type": kind, "has_children": has_children,
        kind: {"rich_text": [{"plain_text": text}]}
    })
}

async fn children(
    server: &MockServer,
    parent: &str,
    cursor: Option<&str>,
    blocks: Vec<Value>,
    next_cursor: Option<&str>,
) {
    let cursor = cursor.map(str::to_owned);
    Mock::given(method("GET"))
        .and(path(format!("/blocks/{parent}/children")))
        .and(move |request: &Request| {
            let actual = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "start_cursor");
            actual.map(|(_, value)| value.into_owned()) == cursor
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": blocks, "has_more": next_cursor.is_some(), "next_cursor": next_cursor
        })))
        .expect(1)
        .mount(server)
        .await;
}

async fn request_paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| {
            assert!(request
                .url
                .query_pairs()
                .any(|(key, value)| key == "page_size" && value == "100"));
            request.url.path().to_owned()
        })
        .collect()
}

#[tokio::test]
async fn toggle_body_is_read_before_the_next_sibling() {
    let server = MockServer::start().await;
    children(
        &server,
        "page",
        None,
        vec![
            block("toggle", "toggle", "Quarterly policy", true),
            block("after", "paragraph", "After toggle", false),
        ],
        None,
    )
    .await;
    children(
        &server,
        "toggle",
        None,
        vec![block(
            "paragraph",
            "paragraph",
            "Revenue was 42 million.",
            false,
        )],
        None,
    )
    .await;
    assert_eq!(
        page_text(&mut paced(&server), "page").await.unwrap(),
        "Quarterly policy\nRevenue was 42 million.\nAfter toggle\n"
    );
    assert_eq!(
        request_paths(&server).await,
        ["/blocks/page/children", "/blocks/toggle/children"]
    );
}

#[tokio::test]
async fn columns_without_rich_text_keep_nested_body_in_reading_order() {
    let server = MockServer::start().await;
    children(
        &server,
        "page",
        None,
        vec![
            json!({"id":"columns", "type":"column_list", "has_children":true, "column_list":{}}),
            block("after", "paragraph", "After columns", false),
        ],
        None,
    )
    .await;
    children(
        &server,
        "columns",
        None,
        vec![
            json!({"id":"left", "type":"column", "has_children":true, "column":{}}),
            json!({"id":"right", "type":"column", "has_children":true, "column":{}}),
        ],
        None,
    )
    .await;
    children(
        &server,
        "left",
        None,
        vec![
            block("nested", "toggle", "Left toggle", true),
            block("left-after", "paragraph", "Left after", false),
        ],
        None,
    )
    .await;
    children(
        &server,
        "nested",
        None,
        vec![block("body", "paragraph", "Deep body", false)],
        None,
    )
    .await;
    children(
        &server,
        "right",
        None,
        vec![block("right-body", "paragraph", "Right body", false)],
        None,
    )
    .await;
    assert_eq!(
        page_text(&mut paced(&server), "page").await.unwrap(),
        "Left toggle\nDeep body\nLeft after\nRight body\nAfter columns\n"
    );
    assert_eq!(
        request_paths(&server).await,
        [
            "/blocks/page/children",
            "/blocks/columns/children",
            "/blocks/left/children",
            "/blocks/nested/children",
            "/blocks/right/children"
        ]
    );
}

#[tokio::test]
async fn pagination_cursors_belong_to_each_parent() {
    let server = MockServer::start().await;
    children(
        &server,
        "page",
        None,
        vec![
            block("a", "toggle", "A", true),
            block("b", "toggle", "B", true),
        ],
        Some("root-next"),
    )
    .await;
    children(
        &server,
        "a",
        None,
        vec![block("a1", "paragraph", "A1", false)],
        Some("a-next"),
    )
    .await;
    children(
        &server,
        "a",
        Some("a-next"),
        vec![block("a2", "paragraph", "A2", false)],
        None,
    )
    .await;
    children(
        &server,
        "b",
        None,
        vec![block("b1", "paragraph", "B1", false)],
        Some("b-next"),
    )
    .await;
    children(
        &server,
        "b",
        Some("b-next"),
        vec![block("b2", "paragraph", "B2", false)],
        None,
    )
    .await;
    children(
        &server,
        "page",
        Some("root-next"),
        vec![block("end", "paragraph", "End", false)],
        None,
    )
    .await;
    assert_eq!(
        page_text(&mut paced(&server), "page").await.unwrap(),
        "A\nA1\nA2\nB\nB1\nB2\nEnd\n"
    );
    assert_eq!(
        request_paths(&server).await,
        [
            "/blocks/page/children",
            "/blocks/a/children",
            "/blocks/a/children",
            "/blocks/b/children",
            "/blocks/b/children",
            "/blocks/page/children"
        ]
    );
}

#[tokio::test]
async fn block_budget_is_shared_and_stops_before_any_further_request() {
    let server = MockServer::start().await;
    children(
        &server,
        "page",
        None,
        vec![
            json!({"id":"container", "type":"column", "has_children":true, "column":{}}),
            block("sibling", "toggle", "Must not appear", true),
        ],
        Some("root-next"),
    )
    .await;
    // 无文字容器也占一块；子层第五页的第 99 块耗尽整页预算。
    for page in 0..5 {
        let cursor = format!("child-{page}");
        let next = format!("child-{}", page + 1);
        let blocks = (0..100)
            .map(|index| {
                let id = format!("item-{}", page * 100 + index);
                block(&id, "paragraph", &id, page == 4 && index == 98)
            })
            .collect();
        children(
            &server,
            "container",
            if page == 0 { None } else { Some(&cursor) },
            blocks,
            Some(&next),
        )
        .await;
    }
    let text = page_text(&mut paced(&server), "page").await.unwrap();
    assert_eq!(text.lines().count(), MAX_BLOCKS_PER_PAGE - 1);
    assert!(text.ends_with("item-498\n"));
    assert!(!text.contains("Must not appear"));
    assert_eq!(request_paths(&server).await.len(), 6);
}

#[tokio::test]
async fn exact_top_level_budget_does_not_fetch_another_page() {
    let server = MockServer::start().await;
    for page in 0..5 {
        let cursor = format!("root-{page}");
        let next = format!("root-{}", page + 1);
        let blocks = (0..100)
            .map(|index| block(&format!("{page}-{index}"), "paragraph", "Body", false))
            .collect();
        children(
            &server,
            "page",
            if page == 0 { None } else { Some(&cursor) },
            blocks,
            Some(&next),
        )
        .await;
    }
    assert_eq!(
        page_text(&mut paced(&server), "page")
            .await
            .unwrap()
            .lines()
            .count(),
        MAX_BLOCKS_PER_PAGE
    );
    assert_eq!(request_paths(&server).await.len(), 5);
}

#[tokio::test]
async fn independent_pages_and_databases_are_not_inlined() {
    let server = MockServer::start().await;
    children(&server, "page", None, vec![
        json!({"id":"child", "type":"child_page", "has_children":true, "child_page":{"title":"Separate page"}}),
        json!({"id":"database", "type":"child_database", "has_children":true, "child_database":{"title":"Separate database"}}),
        block("body", "paragraph", "Parent body", false),
    ], None).await;
    assert_eq!(
        page_text(&mut paced(&server), "page").await.unwrap(),
        "- Separate page\nParent body\n"
    );
    assert_eq!(request_paths(&server).await, ["/blocks/page/children"]);
}

#[tokio::test]
async fn nested_requests_share_pacing_and_honor_retry_after() {
    let server = MockServer::start().await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    Mock::given(method("GET")).respond_with(move |request: &Request| {
        let mut calls = recorded.lock().unwrap();
        calls.push((request.url.path().to_owned(), Instant::now()));
        match calls.len() {
            1 => ResponseTemplate::new(200).set_body_json(json!({"results":[block("child", "toggle", "Parent", true)], "has_more":false})),
            2 => ResponseTemplate::new(429).insert_header("Retry-After", "0.5").set_body_json(json!({"message":"slow down"})),
            _ => ResponseTemplate::new(200).set_body_json(json!({"results":[block("body", "paragraph", "Child", false)], "has_more":false})),
        }
    }).expect(3).mount(&server).await;
    assert_eq!(
        page_text(&mut paced(&server), "page").await.unwrap(),
        "Parent\nChild\n"
    );
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "/blocks/page/children",
            "/blocks/child/children",
            "/blocks/child/children"
        ]
    );
    assert!(calls[1].1.duration_since(calls[0].1) >= MIN_INTERVAL);
    assert!(calls[2].1.duration_since(calls[1].1) >= Duration::from_millis(500));
}

#[tokio::test]
async fn a_page_whose_body_cannot_be_read_is_left_out_and_the_others_still_come_back() {
    let server = MockServer::start().await;
    let page = |id: &str, title: &str| {
        json!({"id": id, "properties": {"Name": {"type": "title", "title": [{"plain_text": title}]}},
            "last_edited_time": "2026-10-01T12:00:00Z"})
    };
    Mock::given(method("POST")).and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [page("page", "Quarterly policy"), page("other", "Travel policy")], "has_more": false
        }))).expect(1).mount(&server).await;
    children(
        &server,
        "page",
        None,
        vec![block("child", "toggle", "Partial body", true)],
        None,
    )
    .await;
    Mock::given(path("/blocks/child/children"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({"message":"unavailable"})))
        .expect(1)
        .mount(&server)
        .await;
    children(
        &server,
        "other",
        None,
        vec![block(
            "p",
            "paragraph",
            "Economy class under six hours",
            false,
        )],
        None,
    )
    .await;

    let (pages, truncated, unread) = fetch_pages(&mut paced(&server), None).await.unwrap();
    // 读坏的那一页不带着半截正文或只有标题回来，另一页不受它连累
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].external_key, "notion://other");
    assert!(pages[0].text.contains("Economy class under six hours"));
    assert!(!truncated);
    assert_eq!(
        unread,
        vec!["notion page page: notion blocks returned 503 Service Unavailable: unavailable"]
    );
}

#[tokio::test]
async fn a_nested_table_keeps_its_header_cells_and_paginated_rows() {
    let server = MockServer::start().await;
    children(
        &server,
        "page",
        None,
        vec![block("toggle", "toggle", "Quarterly policy", true)],
        None,
    )
    .await;
    children(
        &server,
        "toggle",
        None,
        vec![
            json!({"id":"table", "type":"table", "has_children":true,
                "table":{"table_width":3, "has_column_header":true, "has_row_header":true}}),
            block("after", "paragraph", "After table", false),
        ],
        None,
    )
    .await;
    children(
        &server,
        "table",
        None,
        vec![
            json!({"type":"table_row", "table_row":{"cells":[
                [{"plain_text":"Metric"}], [{"plain_text":"Value"}], [{"plain_text":"Notes"}]
            ]}}),
            json!({"type":"table_row", "table_row":{"cells":[
                [{"plain_text":"Revenue"}], [], [{"plain_text":"Pending"}]
            ]}}),
        ],
        Some("table-next"),
    )
    .await;
    children(
        &server,
        "table",
        Some("table-next"),
        vec![json!({"type":"table_row", "table_row":{"cells":[
            [{"plain_text":"Pipe | slash \\"}],
            [{"plain_text":"42"}, {"plain_text":" million"}],
            [{"plain_text":"First\r\nSecond\nThird\rLast"}]
        ]}})],
        None,
    )
    .await;
    let text = page_text(&mut paced(&server), "page").await.unwrap();
    assert_eq!(
        text,
        "Quarterly policy\n\n| Metric | Value | Notes |\n| --- | --- | --- |\n| Revenue |  | Pending |\n| Pipe \\| slash \\\\ | 42 million | First<br>Second<br>Third<br>Last |\n\nAfter table\n"
    );
    assert_eq!(
        request_paths(&server).await,
        [
            "/blocks/page/children",
            "/blocks/toggle/children",
            "/blocks/table/children",
            "/blocks/table/children"
        ]
    );
    let chunks = utopia_ingest::chunk_text(&text);
    assert!(chunks
        .iter()
        .any(|chunk| chunk.text.contains("| --- | --- | --- |")));
}

#[tokio::test]
async fn a_table_without_column_headers_keeps_the_first_row_as_data() {
    let server = MockServer::start().await;
    children(
        &server,
        "page",
        None,
        vec![
            block("before", "paragraph", "Before table", false),
            json!({"id":"table", "type":"table", "has_children":true,
                "table":{"table_width":2, "has_column_header":false}}),
            block("after", "paragraph", "After table", false),
        ],
        None,
    )
    .await;
    children(
        &server,
        "table",
        None,
        vec![
            json!({"type":"table_row", "table_row":{"cells":[
                [{"plain_text":"Revenue"}], [{"plain_text":"42 million"}]
            ]}}),
            json!({"type":"table_row", "table_row":{"cells":[[], []]}}),
        ],
        None,
    )
    .await;
    assert_eq!(
        page_text(&mut paced(&server), "page").await.unwrap(),
        "Before table\n\n|  |  |\n| --- | --- |\n| Revenue | 42 million |\n|  |  |\n\nAfter table\n"
    );
}
