//! 连接流程的几道关卡与「谁连上的谁来选」（0074 决定 3、5），连真库跑。
//! 测试环境没配 OAuth 客户端：过了所有关卡的回调停在「没配置」，不会去敲 Google
use super::*;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;
use utopia_core::models::User;

struct Fixture {
    pool: sqlx::PgPool,
    state: AppState,
    kb: Uuid,
    source: Uuid,
    connector: User,
    colleague: User,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let id = Uuid::now_v7;
        let (org, ws, kb, connector, colleague) = (id(), id(), id(), id(), id());
        // 插进去的只有本地生成的 UUID
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','google-drive-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','google-drive-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
                 ('{connector}','{org}','{connector}@drive.test','Connector','unused'),
                 ('{colleague}','{org}','{colleague}@drive.test','Colleague','unused');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','drive');
             INSERT INTO kb_members(kb_id,user_id,role) VALUES
                 ('{kb}','{connector}','editor'), ('{kb}','{colleague}','editor');"
        ))
        .execute(&pool)
        .await?;
        let source = utopia_store::sources::create(
            &pool,
            kb,
            "google_drive",
            "Drive",
            &json!({}),
            None,
            None,
            None,
        )
        .await?
        .id;
        utopia_store::sources::patch_config(
            &pool,
            source,
            &json!({
                "refresh_token": "1//grant",
                "connected_by": connector,
                "folders": [{"id": "chosen", "name": "Chosen"}],
            }),
            &[],
        )
        .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
        let user = |id| utopia_store::accounts::find_user_by_id(&pool, id);
        let (connector, colleague) = (user(connector).await?, user(colleague).await?);
        Ok(Some(Self {
            state,
            kb,
            source,
            connector: connector.expect("connector exists"),
            colleague: colleague.expect("colleague exists"),
            pool,
            _dir: dir,
        }))
    }

    async fn flow(&self) -> anyhow::Result<String> {
        let (flow_state, _) = record_flow(&self.pool, self.connector.id, self.source)
            .await?
            .expect("room for a flow");
        Ok(flow_state)
    }

    async fn callback(&self, session: Option<&User>, flow_state: &str) -> &'static str {
        let q = Callback {
            code: Some("code".into()),
            state: Some(flow_state.into()),
            error: None,
        };
        match finish(&self.state, session.cloned(), q).await {
            Ok(()) => "connected",
            Err((code, _)) => code,
        }
    }

    async fn demote_connector(&self) -> anyhow::Result<()> {
        sqlx::query("UPDATE kb_members SET role = 'viewer' WHERE kb_id = $1 AND user_id = $2")
            .bind(self.kb)
            .bind(self.connector.id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn config(&self) -> anyhow::Result<Value> {
        Ok(utopia_store::sources::get(&self.pool, self.source)
            .await?
            .config)
    }

    async fn sync_http(&self) -> anyhow::Result<(StatusCode, Value)> {
        self.post_http(
            &format!("/api/v1/kbs/{}/sources/{}/sync", self.kb, self.source),
            json!({}),
        )
        .await
    }

    async fn post_http(&self, path: &str, body: Value) -> anyhow::Result<(StatusCode, Value)> {
        let token = crate::auth::issue_token(&self.state, self.connector.id)?;
        let response = super::super::router(self.state.clone(), &Default::default())
            .oneshot(
                Request::post(path)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))?,
            )
            .await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    async fn sync_state(&self) -> anyhow::Result<(String, i64)> {
        Ok(sqlx::query_as(
            "SELECT last_sync_status,
                    (SELECT count(*) FROM jobs WHERE kind = 'sync_source'
                       AND payload->>'source_id' = sources.id::text)
               FROM sources WHERE id = $1",
        )
        .bind(self.source)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn choose_folder(&self, id: &str) -> ApiResult<Json<Value>> {
        select(
            State(self.state.clone()),
            AuthUser(self.connector.clone()),
            Path((self.kb, self.source)),
            Json(SelectBody {
                folders: vec![FolderEntry {
                    id: id.into(),
                    name: id.into(),
                }],
            }),
        )
        .await
    }

    async fn reject_sync_jobs(&self, reject: bool) -> anyhow::Result<()> {
        // Install once: dropping a table trigger between tests can deadlock parallel fixtures.
        static INSTALLED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        INSTALLED
            .get_or_try_init(|| async {
                sqlx::raw_sql(
                    "SELECT pg_advisory_xact_lock(hashtext('test_reject_source_sync'));
                     CREATE OR REPLACE FUNCTION test_reject_source_sync() RETURNS trigger
                     LANGUAGE plpgsql AS $$ BEGIN
                       IF NEW.kind = 'sync_source' AND EXISTS(
                         SELECT 1 FROM sources WHERE id::text = NEW.payload->>'source_id'
                           AND name = 'reject-source-sync-test')
                       THEN RAISE EXCEPTION 'test source sync insert failure'; END IF;
                       RETURN NEW;
                     END $$;
                     DO $$ BEGIN
                       IF NOT EXISTS(SELECT 1 FROM pg_trigger WHERE tgname = 'test_reject_source_sync'
                                        AND tgrelid = 'jobs'::regclass) THEN
                         CREATE TRIGGER test_reject_source_sync BEFORE INSERT ON jobs
                           FOR EACH ROW EXECUTE FUNCTION test_reject_source_sync();
                       END IF;
                     END $$;",
                )
                .execute(&self.pool)
                .await
                .map(drop)
            })
            .await?;
        sqlx::query("UPDATE sources SET name = $2 WHERE id = $1")
            .bind(self.source)
            .bind(if reject {
                "reject-source-sync-test"
            } else {
                "Drive"
            })
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn source_sync_enqueue_creation_failure_keeps_source_schedulable() -> anyhow::Result<()> {
    let Some(mut fx) = Fixture::new().await? else {
        return Ok(());
    };
    fx.reject_sync_jobs(false).await?;
    let config = json!({"url": "https://example.com/source-sync-test"});
    let (status, body) = fx
        .post_http(
            &format!("/api/v1/kbs/{}/sources", fx.kb),
            json!({"kind": "url", "name": "reject-source-sync-test", "config": config}),
        )
        .await?;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    fx.source = sqlx::query_scalar(
        "SELECT id FROM sources WHERE kb_id = $1 AND kind = 'url'
           AND name = 'reject-source-sync-test'",
    )
    .bind(fx.kb)
    .fetch_one(&fx.pool)
    .await?;
    assert_eq!(fx.config().await?, config);
    assert_eq!(fx.sync_state().await?, ("never".into(), 0));
    fx.reject_sync_jobs(false).await?;
    let (status, body) = fx.sync_http().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["queued"], true);
    assert_eq!(fx.sync_state().await?, ("queued".into(), 1));
    Ok(())
}

#[tokio::test]
async fn source_sync_enqueue_manual_failure_rolls_back_and_can_retry() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    fx.reject_sync_jobs(true).await?;
    let (status, body) = fx.sync_http().await?;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(fx.sync_state().await?, ("never".into(), 0));

    fx.reject_sync_jobs(false).await?;
    let (status, body) = fx.sync_http().await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["queued"], true);
    assert_eq!(fx.sync_state().await?, ("queued".into(), 1));
    let (payload, max_attempts): (Value, i32) = sqlx::query_as(
        "SELECT payload, max_attempts FROM jobs WHERE kind = 'sync_source'
           AND payload->>'source_id' = $1",
    )
    .bind(fx.source.to_string())
    .fetch_one(&fx.pool)
    .await?;
    assert_eq!(payload, json!({"source_id": fx.source}));
    assert_eq!(max_attempts, 3);

    for pending in ["queued", "running"] {
        sqlx::query("UPDATE sources SET last_sync_status = $2 WHERE id = $1")
            .bind(fx.source)
            .bind(pending)
            .execute(&fx.pool)
            .await?;
        let (status, body) = fx.sync_http().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["queued"], false);
        assert_eq!(fx.sync_state().await?, (pending.into(), 1));
    }
    Ok(())
}

#[tokio::test]
async fn source_sync_enqueue_concurrent_manual_requests_enqueue_once() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    // AppState and the router do not start a worker, so the first task stays queued.
    let (first, second) = tokio::join!(fx.sync_http(), fx.sync_http());
    let mut queued = Vec::new();
    for result in [first, second] {
        let (status, body) = result?;
        assert_eq!(status, StatusCode::OK, "{body}");
        queued.push(body["queued"].as_bool().expect("queued is a boolean"));
    }
    queued.sort_unstable();
    assert_eq!(queued, [false, true]);
    assert_eq!(fx.sync_state().await?, ("queued".into(), 1));
    Ok(())
}

#[tokio::test]
async fn source_sync_enqueue_drive_selection_keeps_follow_up_when_pending() -> anyhow::Result<()> {
    for pending in ["queued", "running"] {
        let Some(fx) = Fixture::new().await? else {
            return Ok(());
        };
        let (status, body) = fx.sync_http().await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        sqlx::query("UPDATE sources SET last_sync_status = $2 WHERE id = $1")
            .bind(fx.source)
            .bind(pending)
            .execute(&fx.pool)
            .await?;
        sqlx::query("UPDATE jobs SET status = $2 WHERE payload->>'source_id' = $1")
            .bind(fx.source.to_string())
            .bind(pending)
            .execute(&fx.pool)
            .await?;
        let _ = fx
            .choose_folder("next-folder")
            .await
            .map_err(|ApiErr(error)| error)?;
        assert_eq!(fx.sync_state().await?, (pending.into(), 2));
        let latest_status: String = sqlx::query_scalar(
            "SELECT status FROM jobs WHERE kind = 'sync_source'
               AND payload->>'source_id' = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind(fx.source.to_string())
        .fetch_one(&fx.pool)
        .await?;
        assert_eq!(latest_status, "queued");
        assert_eq!(fx.config().await?["folders"][0]["id"], "next-folder");
    }
    Ok(())
}

#[tokio::test]
async fn source_sync_enqueue_drive_failure_keeps_config_but_rolls_back_queue() -> anyhow::Result<()>
{
    for initial in ["never", "running"] {
        let Some(fx) = Fixture::new().await? else {
            return Ok(());
        };
        let existing_jobs = if initial == "running" {
            let (status, body) = fx.sync_http().await?;
            assert_eq!(status, StatusCode::OK, "{body}");
            sqlx::query("UPDATE sources SET last_sync_status = 'running' WHERE id = $1")
                .bind(fx.source)
                .execute(&fx.pool)
                .await?;
            sqlx::query("UPDATE jobs SET status = 'running' WHERE payload->>'source_id' = $1")
                .bind(fx.source.to_string())
                .execute(&fx.pool)
                .await?;
            1
        } else {
            0
        };
        fx.reject_sync_jobs(true).await?;
        assert!(matches!(
            fx.choose_folder("saved-folder").await,
            Err(ApiErr(AppError::Db(_)))
        ));
        assert_eq!(fx.config().await?["folders"][0]["id"], "saved-folder");
        assert_eq!(fx.sync_state().await?, (initial.into(), existing_jobs));
        fx.reject_sync_jobs(false).await?;
        let _ = fx
            .choose_folder("saved-folder")
            .await
            .map_err(|ApiErr(error)| error)?;
        let expected_status = if initial == "running" {
            "running"
        } else {
            "queued"
        };
        assert_eq!(
            fx.sync_state().await?,
            (expected_status.into(), existing_jobs + 1)
        );
    }
    Ok(())
}

fn denied_code<T>(result: ApiResult<T>) -> Option<&'static str> {
    match result {
        Err(ApiErr(AppError::CodedForbidden { code, .. } | AppError::Invalid { code, .. })) => {
            Some(code)
        }
        _ => None,
    }
}

#[tokio::test]
async fn a_state_used_a_second_time_is_expired() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let flow_state = fx.flow().await?;
    assert_eq!(
        fx.callback(Some(&fx.connector), &flow_state).await,
        "unavailable"
    );
    assert_eq!(
        fx.callback(Some(&fx.connector), &flow_state).await,
        "expired"
    );
    Ok(())
}

#[tokio::test]
async fn a_callback_in_another_session_is_refused() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let flow_state = fx.flow().await?;
    assert_eq!(
        fx.callback(Some(&fx.colleague), &flow_state).await,
        "session"
    );
    let flow_state = fx.flow().await?;
    assert_eq!(fx.callback(None, &flow_state).await, "session");
    Ok(())
}

#[tokio::test]
async fn a_callback_after_the_person_stopped_being_an_editor_is_refused() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let flow_state = fx.flow().await?;
    fx.demote_connector().await?;
    assert_eq!(
        fx.callback(Some(&fx.connector), &flow_state).await,
        "session"
    );
    Ok(())
}

#[tokio::test]
async fn a_callback_after_ten_minutes_is_expired() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let flow_state = fx.flow().await?;
    sqlx::query(
        "UPDATE google_drive_flows SET expires_at = now() - interval '1 second' WHERE state = $1",
    )
    .bind(&flow_state)
    .execute(&fx.pool)
    .await?;
    assert_eq!(
        fx.callback(Some(&fx.connector), &flow_state).await,
        "expired"
    );
    Ok(())
}

#[tokio::test]
async fn a_person_has_at_most_five_connections_in_progress() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    for _ in 0..5 {
        fx.flow().await?;
    }
    let sixth = record_flow(&fx.pool, fx.connector.id, fx.source).await?;
    let colleagues_first = record_flow(&fx.pool, fx.colleague.id, fx.source).await?;
    assert_eq!((sixth.is_some(), colleagues_first.is_some()), (false, true));
    Ok(())
}

#[tokio::test]
async fn a_colleague_cannot_browse_or_choose_the_connected_persons_folders() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let path = || Path((fx.kb, fx.source));
    let browsed = folders(
        State(fx.state.clone()),
        AuthUser(fx.colleague.clone()),
        path(),
        Query(FoldersQuery { parent: None }),
    )
    .await;
    let chosen = select(
        State(fx.state.clone()),
        AuthUser(fx.colleague.clone()),
        path(),
        Json(SelectBody {
            folders: vec![FolderEntry {
                id: "theirs".into(),
                name: "Theirs".into(),
            }],
        }),
    )
    .await;
    assert_eq!(
        (denied_code(browsed), denied_code(chosen)),
        (
            Some("google_drive_not_connector"),
            Some("google_drive_not_connector")
        )
    );
    assert_eq!(
        fx.config().await?["folders"],
        json!([{"id": "chosen", "name": "Chosen"}])
    );
    Ok(())
}

#[tokio::test]
async fn the_source_settings_cannot_rewrite_a_drive_connection() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let before = fx.config().await?;
    let _ = super::super::sources_routes::update(
        State(fx.state.clone()),
        AuthUser(fx.colleague.clone()),
        Path((fx.kb, fx.source)),
        Json(super::super::sources_routes::UpdateBody {
            name: None,
            config: Some(json!({
                "connected_by": fx.colleague.id,
                "folders": [{"id": "theirs", "name": "Theirs"}],
            })),
            icon: None,
            schedule: None,
        }),
    )
    .await
    .map_err(|ApiErr(e)| e)?;
    assert_eq!(fx.config().await?, before);
    Ok(())
}

#[tokio::test]
async fn a_sync_stops_once_the_connected_person_is_no_longer_an_editor() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let reason = |e: anyhow::Error| format!("{e:#}");
    let while_editor = crate::ingest_sources::sync_source(&fx.state, fx.source)
        .await
        .map_err(reason);
    fx.demote_connector().await?;
    let after = crate::ingest_sources::sync_source(&fx.state, fx.source)
        .await
        .map_err(reason);
    let says_reconnect = |r: &Result<(), String>| {
        r.as_ref()
            .is_err_and(|e| e.contains("no longer an editor") && e.contains("reconnect"))
    };
    assert_eq!(
        (says_reconnect(&while_editor), says_reconnect(&after)),
        (false, true)
    );
    Ok(())
}

#[tokio::test]
async fn a_colleague_cannot_reconnect_the_connected_persons_drive() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let started = connect(
        State(fx.state.clone()),
        AuthUser(fx.colleague.clone()),
        Path((fx.kb, fx.source)),
    )
    .await;
    assert_eq!(
        denied_code(started),
        Some("google_drive_connected_by_someone_else")
    );
    Ok(())
}

#[tokio::test]
async fn a_callback_after_someone_else_connected_is_refused() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let (flow_state, _) = record_flow(&fx.pool, fx.colleague.id, fx.source)
        .await?
        .expect("room for a flow");
    assert_eq!(fx.callback(Some(&fx.colleague), &flow_state).await, "taken");
    Ok(())
}

#[tokio::test]
async fn a_grant_is_kept_while_another_source_uses_the_same_account() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let account = format!("{}@drive.test", Uuid::now_v7());
    let other = utopia_store::sources::create(
        &fx.pool,
        fx.kb,
        "google_drive",
        "Other Drive",
        &json!({}),
        None,
        None,
        None,
    )
    .await?
    .id;
    // 同一个邮箱，大小写不同：Google 账号不分大小写
    for (id, email) in [
        (fx.source, account.clone()),
        (other, account.to_uppercase()),
    ] {
        let grant = json!({ "refresh_token": "1//grant", "account_email": email });
        utopia_store::sources::patch_config(&fx.pool, id, &grant, &[]).await?;
    }
    let while_other_connected = grant_still_used(&fx.pool, fx.source, &account).await?;
    utopia_store::sources::patch_config(&fx.pool, other, &json!({}), CONNECTION_KEYS).await?;
    let after_other_disconnected = grant_still_used(&fx.pool, fx.source, &account).await?;
    assert_eq!(
        (while_other_connected, after_other_disconnected),
        (true, false)
    );
    Ok(())
}

#[tokio::test]
async fn a_drive_source_cannot_be_created_without_an_oauth_client() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let created = super::super::sources_routes::create(
        State(fx.state.clone()),
        crate::auth::SessionOrToken {
            user: fx.connector.clone(),
            pat: None,
        },
        Path(fx.kb),
        Json(super::super::sources_routes::CreateBody {
            kind: "google_drive".into(),
            name: "Drive".into(),
            config: json!({}),
            icon: None,
            sync_interval_minutes: None,
            sync_cron: None,
        }),
    )
    .await;
    assert_eq!(denied_code(created), Some("google_drive_not_configured"));
    Ok(())
}

#[test]
fn a_reconnect_keeps_the_grant_only_for_the_same_account() {
    assert_eq!(
        [
            same_account(Some("Bruno@re-cinq.com"), Some("bruno@re-cinq.com")),
            same_account(Some("bruno@re-cinq.com"), Some("ana@re-cinq.com")),
            same_account(None, Some("bruno@re-cinq.com")),
            same_account(None, None),
        ],
        [true, false, false, false]
    );
}

#[test]
fn a_reconnect_revokes_the_previous_grant_only_when_both_accounts_are_known_and_differ() {
    assert_eq!(
        [
            revokes_previous(Some("bruno@re-cinq.com"), Some("ana@re-cinq.com")),
            revokes_previous(Some("bruno@re-cinq.com"), Some("Bruno@re-cinq.com")),
            revokes_previous(Some("bruno@re-cinq.com"), None),
            revokes_previous(None, Some("ana@re-cinq.com")),
        ],
        [true, false, false, false]
    );
}
