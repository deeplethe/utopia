//! 限额就地调整（#1302）：管理员调高，已经在等的任务当场放行；调低要等在飞的调用还回来才生效。

use super::ModelGates;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tower::ServiceExt;
use uuid::Uuid;

const SOON: Duration = Duration::from_millis(100);

/// 调高不靠下一个来取许可的任务：所有 worker 都等着时，管理员存下新限额的那一下就得把它们放进来
#[tokio::test]
async fn an_admin_raising_a_limit_admits_a_task_already_waiting() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, admin) = (Uuid::now_v7(), Uuid::now_v7());
    let base = format!("http://gate-{}", Uuid::now_v7());
    let dir = tempfile::tempdir()?;
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.path().to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = crate::state::AppState::new(
        pool.clone(),
        &cfg,
        Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?),
        "test-only".into(),
    );
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES('{org}','model-gate-test');
         INSERT INTO users(id,org_id,email,password_hash,display_name,is_admin)
              VALUES('{admin}','{org}','{admin}@example.test','unused','Admin',TRUE);"
    ))
    .execute(&pool)
    .await?;
    utopia_store::model_limits::set(&pool, &base, "m", Some(1)).await?;
    let result = async {
        let _in_flight = super::acquire(&state, &base, "m").await;
        let (s, b) = (state.clone(), base.clone());
        let mut waiting = tokio::spawn(async move { super::acquire(&s, &b, "m").await.is_some() });
        let parked = timeout(SOON, &mut waiting).await.is_err();

        // 这个接口每次都写 open_registration，原样送回去，部署设置那一行不变
        let open = utopia_store::access::open_registration(&pool).await?;
        let body = json!({
            "open_registration": open,
            "model_limit": {"base_url": base, "model": "m", "max_concurrent": 2},
        });
        let request = Request::builder()
            .method("PUT")
            .uri("/api/v1/admin/deployment")
            .header(
                "authorization",
                format!("Bearer {}", crate::auth::issue_token(&state, admin)?),
            )
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))?;
        let status = crate::api::router(state.clone(), &Default::default())
            .oneshot(request)
            .await?
            .status();
        let admitted = timeout(SOON, &mut waiting).await.is_ok();
        anyhow::ensure!(
            status == StatusCode::OK && parked && admitted,
            "status: {status}, parked: {parked}, admitted: {admitted}"
        );
        anyhow::Ok(())
    }
    .await;
    utopia_store::model_limits::set(&pool, &base, "m", None).await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}

#[tokio::test]
async fn lowering_the_limit_takes_effect_as_in_flight_calls_return() {
    let gates = Arc::new(ModelGates::default());
    let first = gates.admit("u", "m", 2).await.unwrap();
    let second = gates.admit("u", "m", 2).await.unwrap();
    let g = gates.clone();
    let mut waiting = tokio::spawn(async move { g.admit("u", "m", 1).await });

    // 两个在飞降到一个：还回来的第一张被收走，第二个调用还没完就仍满额
    drop(first);
    assert!(timeout(SOON, &mut waiting).await.is_err());
    drop(second);
    assert!(timeout(SOON, &mut waiting).await.is_ok());
}
