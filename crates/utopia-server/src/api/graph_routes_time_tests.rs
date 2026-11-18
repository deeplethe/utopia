//! 改区间的路由收发的是存的边界（#994）：`end_after_period` 是唯一一个缺省即保留的字段。
//!
//! 实体详情读得到标记；不带它的请求，终点（时刻与精度）没动、起点还在就留着，改了终点或清掉
//! 起点就丢掉（时刻照旧）；显式的值照收，只改标记也算改了终点；显式 true 得是状态属性、有起点、
//! 终点有日期；倒置按截到精度之后的两端判。每一步的结果收进一张表一起比，dev 上哪几步不同一次
//! 看全。没有 `UTOPIA_DATABASE_URL` 时跳过。
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const MARCH: &str = "2024-03-01T00:00:00Z";
const FEB: &str = "2024-02-01T00:00:00Z";
const END: &str = "2025-01-01T00:00:00Z";
const LATER: &str = "2026-01-01T00:00:00Z";

async fn send(
    app: &Router,
    token: &str,
    method: &str,
    uri: String,
    body: &Value,
) -> anyhow::Result<(StatusCode, Value)> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

/// 一个请求体：起点日精度、终点年精度，空串是没有；标记给了才带
fn span(from: &str, to: &str, mark: Option<bool>) -> Value {
    let from = Some(from).filter(|t| !t.is_empty());
    let to = Some(to).filter(|t| !t.is_empty());
    let mut body = json!({
        "valid_from": from, "valid_from_precision": from.map(|_| "day"),
        "valid_to": to, "valid_to_precision": to.map(|_| "year"),
    });
    if let Some(mark) = mark {
        body["end_after_period"] = json!(mark);
    }
    body
}

#[tokio::test]
async fn a_correction_keeps_the_mark_unless_the_request_changes_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let [org, ws, kb, editor, works_for, lin, meridian, marked, loose] =
        std::array::from_fn(|_| Uuid::now_v7());
    // Only locally generated UUIDs and the constants above are interpolated into fixture SQL.
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','fact-time-route');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','fact-time-route');
         INSERT INTO users(id,org_id,email,display_name,password_hash)
              VALUES ('{editor}','{org}','{editor}@time.test','editor','unused');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','times');
         INSERT INTO kb_members(kb_id,user_id,role) VALUES ('{kb}','{editor}','editor');
         INSERT INTO relation_types(id,kb_id,key,label,temporal)
              VALUES ('{works_for}','{kb}','works_for','works for','state');
         INSERT INTO entities(id,kb_id,canonical_name)
              VALUES ('{lin}','{kb}','Lin Zhao'), ('{meridian}','{kb}','Meridian Systems');
         -- 「left in 2024」关上的一行，记着说的是 2024；同样的区间、没有谓词的一行
         INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,layer,confidence,
                           valid_from,valid_from_precision,valid_to,valid_to_precision,
                           end_after_period) VALUES
             ('{marked}','{kb}','{lin}','{works_for}','{meridian}','typed',0.9,
              '{MARCH}','day','{END}','year',TRUE),
             ('{loose}','{kb}','{lin}',NULL,'{meridian}','typed',0.9,
              '{MARCH}','day','{END}','year',FALSE);"
    ))
    .execute(&pool)
    .await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let config = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(&dir.path().join("search"))?);
        let state = crate::state::AppState::new(pool.clone(), &config, search, "test-only".into());
        let token = crate::auth::issue_token(&state, editor)?;
        let app = crate::api::router(state, &config);

        let detail = format!("/api/v1/kbs/{kb}/entities/{lin}");
        let (_, detail) = send(&app, &token, "GET", detail, &Value::Null).await?;
        let mut read = detail["facts"].as_array().into_iter().flatten();
        let read = read.find(|x| x["id"] == json!(marked));
        let mut seen = vec![("entity detail", json!(read.map(|x| &x["end_after_period"])))];
        let mut wanted = vec![("entity detail", json!(true))];
        // 每一步：改哪一行（None 是上一步写下的那行）、请求体、要的结果——写下的那一行（标记、
        // 人改过哪一端、终点），或拒绝的代码。不带标记的：只改起点留着，改了终点的时刻或精度、
        // 清掉起点都丢掉；显式的照收，只改标记把终点那一端记成人改的；显式 true 缺起点、缺终点、
        // 不是状态的都不收
        let bad = json!([422, "bad_end_after_period"]);
        let steps = [
            ("start only", None, span(FEB, END, None), json!([true, "start", END])),
            ("mark false", None, span(FEB, END, Some(false)), json!([false, "both", END])),
            ("mark true", None, span(FEB, END, Some(true)), json!([true, "both", END])),
            ("new end", None, span(FEB, LATER, None), json!([false, "both", LATER])),
            ("mark true again", None, span(FEB, END, Some(true)), json!([true, "both", END])),
            (
                "precision only",
                None,
                json!({"valid_from": FEB, "valid_from_precision": "day",
                       "valid_to": END, "valid_to_precision": "month"}),
                json!([false, "both", END]),
            ),
            ("marked once more", None, span(FEB, END, Some(true)), json!([true, "both", END])),
            ("start cleared", None, span("", END, None), json!([false, "both", END])),
            ("true, no start", None, span("", END, Some(true)), bad.clone()),
            ("true, no end", None, span(FEB, "", Some(true)), bad.clone()),
            ("true, no state", Some(loose), span(MARCH, END, Some(true)), bad),
            // 小时精度的 08:30 是 08:00，早于分钟精度的起点 08:10
            (
                "inverted once truncated",
                Some(loose),
                json!({"valid_from": "2026-06-01T08:10:00Z", "valid_from_precision": "minute",
                       "valid_to": "2026-06-01T08:30:00Z", "valid_to_precision": "hour"}),
                json!([422, "interval_inverted"]),
            ),
        ];
        let mut fact = marked;
        for (label, target, body, want) in steps {
            let uri = format!("/api/v1/kbs/{kb}/facts/{}", target.unwrap_or(fact));
            let (status, reply) = send(&app, &token, "PATCH", uri, &body).await?;
            let got = match reply["fact_id"].as_str() {
                Some(id) => {
                    fact = id.parse()?;
                    let row: (bool, Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
                        "SELECT end_after_period, corrected_ends, valid_to FROM facts WHERE id = $1",
                    )
                    .bind(fact)
                    .fetch_one(&pool)
                    .await?;
                    json!(row)
                }
                None => json!([status.as_u16(), reply["code"]]),
            };
            seen.push((label, got));
            wanted.push((label, want));
        }
        assert_eq!(seen, wanted);
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}

#[tokio::test]
async fn review_closes_refuse_inverted_intervals_without_settling_the_item() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let [org, ws, kb, editor, property, lin, meridian, orion, old, new, conflict, violation] =
        std::array::from_fn(|_| Uuid::now_v7());
    // Only locally generated UUIDs and fixed timestamps enter this fixture SQL.
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','review-close-time');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','review-close-time');
         INSERT INTO users(id,org_id,email,display_name,password_hash)
              VALUES ('{editor}','{org}','{editor}@time.test','editor','unused');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','times');
         INSERT INTO kb_members(kb_id,user_id,role) VALUES ('{kb}','{editor}','editor');
         INSERT INTO relation_types(id,kb_id,key,label,temporal)
              VALUES ('{property}','{kb}','works_for','works for','state');
         INSERT INTO entities(id,kb_id,canonical_name) VALUES
              ('{lin}','{kb}','Lin Zhao'), ('{meridian}','{kb}','Meridian'), ('{orion}','{kb}','Orion');
         INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,confidence,
                           valid_from,valid_from_precision) VALUES
              ('{old}','{kb}','{lin}','{property}','{meridian}',0.9,'{MARCH}','day'),
              ('{new}','{kb}','{lin}','{property}','{orion}',0.9,NULL,NULL);
         INSERT INTO fact_conflicts(id,kb_id,old_fact_id,new_fact_id,reason)
              VALUES ('{conflict}','{kb}','{old}','{new}','no_time');
         INSERT INTO axiom_violations(id,kb_id,kind,left_fact,right_fact)
              VALUES ('{violation}','{kb}','derived_contradiction','{old}','{new}');"
    ))
    .execute(&pool)
    .await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let config = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(&dir.path().join("search"))?);
        let state = crate::state::AppState::new(pool.clone(), &config, search, "test-only".into());
        let token = crate::auth::issue_token(&state, editor)?;
        let app = crate::api::router(state, &config);
        let prefix = format!("/api/v1/kbs/{kb}");
        // The first date is later as written, but earlier at year precision.
        // The existing equal-state refusal must reach all three cards as well.
        for (at, precision, code) in [
            ("2024-10-01T00:00:00Z", "year", "interval_inverted"),
            (MARCH, "month", "empty_state_span"),
        ] {
            for (path, body) in [
                (
                    format!("facts/{old}/close"),
                    json!({"valid_to": at, "valid_to_precision": precision}),
                ),
                (
                    format!("conflicts/{conflict}"),
                    json!({"action": "close", "close_at": at, "close_at_precision": precision}),
                ),
                (
                    format!("review/violations/{violation}"),
                    json!({"resolution": "fact_closed", "close_at": at, "close_at_precision": precision}),
                ),
            ] {
                let (status, reply) = send(&app, &token, "POST", format!("{prefix}/{path}"), &body).await?;
                assert_eq!((status, reply["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some(code)), "{path}: {reply}");
            }
        }
        // A conflict with no explicit date closes at the new row's start; it must
        // get the same store guard, even though no date came through the route.
        sqlx::query("UPDATE facts SET valid_from = '2024-01-01', valid_from_precision = 'year' WHERE id = $1")
            .bind(new).execute(&pool).await?;
        let (status, reply) = send(&app, &token, "POST", format!("{prefix}/conflicts/{conflict}"), &json!({"action": "close"})).await?;
        assert_eq!((status, reply["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("interval_inverted")));
        let unchanged: bool = sqlx::query_scalar(
            "SELECT (SELECT count(*) = 2 FROM facts WHERE kb_id = $1)
                AND EXISTS (SELECT 1 FROM facts WHERE id = $2 AND invalidated_at IS NULL AND valid_to IS NULL)
                AND EXISTS (SELECT 1 FROM fact_conflicts WHERE id = $3 AND status = 'open' AND resolution IS NULL)
                AND EXISTS (SELECT 1 FROM axiom_violations WHERE id = $4 AND status = 'open' AND resolution IS NULL)",
        ).bind(kb).bind(old).bind(conflict).bind(violation).fetch_one(&pool).await?;
        assert!(unchanged, "refused closes write no replacement and leave both queues open");

        // A later date can still close the refused row, with the endpoint stored
        // at its supplied precision, without translating it to a period's end.
        let (status, _) = send(&app, &token, "POST", format!("{prefix}/facts/{old}/close"),
            &json!({"valid_to": "2024-09-19T00:00:00Z", "valid_to_precision": "month"})).await?;
        assert_eq!(status, StatusCode::OK);
        let closed: (DateTime<Utc>, String, bool) = sqlx::query_as(
            "SELECT valid_to, valid_to_precision, end_after_period FROM facts WHERE supersedes = $1",
        ).bind(old).fetch_one(&pool).await?;
        assert_eq!(closed, ("2024-09-01T00:00:00Z".parse()?, "month".into(), false));
        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
