//! Google Drive 的遍历、导出与授权。响应的形状照 Drive v3 与 Google 令牌端点的文档写，
//! **没有对一个真实的 Drive 跑过**（0013 决定 4 要求说出来）：真连的那条在最下面，
//! 给了令牌才跑。
use super::*;
use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn drive(server: &MockServer) -> Drive {
    Drive {
        // 本地 mock 不走开发机的系统代理
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        token: "ya29.test".into(),
        api_root: server.uri(),
    }
}

fn cfg() -> OAuthConfig {
    OAuthConfig {
        client_id: "client-1.apps.googleusercontent.com".into(),
        client_secret: "secret".into(),
        redirect: "https://utopia.example/api/v1/google-drive/callback".into(),
    }
}

fn folder_meta(id: &str, trashed: bool) -> serde_json::Value {
    json!({"id": id, "mimeType": FOLDER_MIME, "trashed": trashed})
}

async fn mount_folder(server: &MockServer, id: &str, trashed: bool) {
    Mock::given(method("GET"))
        .and(path(format!("/files/{id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(folder_meta(id, trashed)))
        .mount(server)
        .await;
}

async fn mount_children(server: &MockServer, parent: &str, files: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(query_param(
            "q",
            format!("'{parent}' in parents and trashed = false"),
        ))
        .and(query_param("supportsAllDrives", "true"))
        .and(query_param("includeItemsFromAllDrives", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "files": files })))
        .mount(server)
        .await;
}

fn id_token(claims: serde_json::Value) -> String {
    format!(
        "{}.{}.sig",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    )
}

/// 子文件夹往下走；Google 表单这类没有正文的格式与超大文件跳过；
/// 两个选中的文件夹互相嵌套时同一个文件只收一次
#[tokio::test]
async fn a_walk_descends_skips_what_cannot_be_read_and_counts_each_file_once() -> anyhow::Result<()>
{
    let server = MockServer::start().await;
    mount_folder(&server, "top", false).await;
    mount_folder(&server, "sub", false).await;
    mount_children(
        &server,
        "top",
        json!([
            {"id": "sub", "name": "Sub", "mimeType": FOLDER_MIME},
            {"id": "doc1", "name": "Plan", "mimeType": "application/vnd.google-apps.document",
             "modifiedTime": "2026-10-01T09:30:00.000Z"},
            {"id": "form1", "name": "Survey", "mimeType": "application/vnd.google-apps.form"},
            {"id": "big", "name": "video.mp4", "mimeType": "video/mp4", "size": "999999999"}
        ]),
    )
    .await;
    mount_children(
        &server,
        "sub",
        json!([
            {"id": "pdf1", "name": "contract.pdf", "mimeType": "application/pdf",
             "size": "1024", "md5Checksum": "abc", "modifiedTime": "2026-09-01T00:00:00Z"}
        ]),
    )
    .await;

    // `sub` 也被单独选中：它的文件不该出现两次
    let listing = drive(&server)
        .walk(&["top".to_string(), "sub".to_string()])
        .await?;
    let ids: Vec<&str> = listing.files.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["doc1", "pdf1"]);
    assert_eq!(listing.skipped, 2, "表单与超大文件");
    assert!(!listing.truncated);
    assert!(listing.unreachable.is_empty());

    let doc = &listing.files[0];
    assert_eq!(doc.external_key(), "gdrive://doc1");
    assert_eq!(
        doc.marker(),
        "2026-10-01T09:30:00.000Z",
        "Google 格式没有 md5，用修改时刻"
    );
    assert_eq!(
        doc.modified.map(|d| d.to_rfc3339()),
        Some("2026-10-01T09:30:00+00:00".into())
    );
    assert_eq!(listing.files[1].marker(), "abc", "二进制文件用 md5");
    Ok(())
}

/// 选中的文件夹打不开（删了、或不再共享）不是错误，但要点名——同步据此不对账
#[tokio::test]
async fn an_unreachable_folder_is_named_not_fatal() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files/gone"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": 404, "message": "File not found: gone.",
                      "errors": [{"reason": "notFound"}]}
        })))
        .mount(&server)
        .await;
    mount_folder(&server, "here", false).await;
    mount_children(&server, "here", json!([])).await;

    let listing = drive(&server)
        .walk(&["gone".to_string(), "here".to_string()])
        .await?;
    assert_eq!(listing.unreachable, ["gone"]);
    Ok(())
}

/// 进了回收站的文件夹不进去，也不算打不开：里面的东西对这个人已经不算数了
#[tokio::test]
async fn a_trashed_folder_is_not_walked() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    mount_folder(&server, "binned", true).await;
    let listing = drive(&server).walk(&["binned".to_string()]).await?;
    assert!(listing.files.is_empty());
    assert!(listing.unreachable.is_empty());
    // 没有任何 files.list 请求
    let lists = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/files")
        .count();
    assert_eq!(lists, 0);
    Ok(())
}

/// 列表翻页：`nextPageToken` 一直跟到没有为止
#[tokio::test]
async fn a_listing_follows_page_tokens() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    mount_folder(&server, "top", false).await;
    let file = |id: &str| {
        json!({"id": id, "name": format!("{id}.txt"), "mimeType": "text/plain",
               "size": "3", "md5Checksum": id})
    };
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(query_param("pageToken", "p2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"files": [file("b")]})))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"files": [file("a")], "nextPageToken": "p2"})),
        )
        .with_priority(2)
        .mount(&server)
        .await;

    let listing = drive(&server).walk(&["top".to_string()]).await?;
    let ids: Vec<&str> = listing.files.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
    Ok(())
}

/// Google 文档走 export 换成 DOCX，文件名补扩展名；别的原样下载，不猜 mime
#[tokio::test]
async fn google_formats_are_exported_and_files_are_downloaded() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files/doc1/export"))
        .and(query_param(
            "mimeType",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PK-docx".to_vec()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files/pdf1"))
        .and(query_param("alt", "media"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"%PDF-1.7".to_vec()))
        .mount(&server)
        .await;

    let d = drive(&server);
    let doc = DriveFile {
        id: "doc1".into(),
        name: "Plan".into(),
        mime: "application/vnd.google-apps.document".into(),
        modified: None,
        md5: None,
        modified_raw: None,
    };
    let c = d.content(&doc).await?;
    assert_eq!(c.bytes, b"PK-docx");
    assert_eq!(c.filename, "Plan.docx");

    let pdf = DriveFile {
        id: "pdf1".into(),
        name: "contract.pdf".into(),
        mime: "application/pdf".into(),
        modified: None,
        md5: Some("x".into()),
        modified_raw: None,
    };
    let c = d.content(&pdf).await?;
    assert_eq!(c.bytes, b"%PDF-1.7");
    assert_eq!(c.filename, "contract.pdf");
    assert_eq!(c.mime, "application/octet-stream");
    Ok(())
}

/// 导出超过 10 MB 时 Google 回 403 带 `exportSizeLimitExceeded`；错误里要留着这个原因，
/// 同步把它报给人看
#[tokio::test]
async fn an_export_refusal_keeps_googles_reason() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files/huge/export"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {"code": 403, "message": "This file is too large to be exported.",
                      "errors": [{"reason": "exportSizeLimitExceeded"}]}
        })))
        .mount(&server)
        .await;
    let f = DriveFile {
        id: "huge".into(),
        name: "Budget".into(),
        mime: "application/vnd.google-apps.spreadsheet".into(),
        modified: None,
        md5: None,
        modified_raw: None,
    };
    let Err(e) = drive(&server).content(&f).await else {
        panic!("a 403 must not read as content");
    };
    let msg = format!("{e:#}");
    assert!(msg.contains("exportSizeLimitExceeded"), "{msg}");
    assert!(msg.contains("too large"), "{msg}");
}

/// 文件夹浏览器：「与我共享」用 sharedWithMe 查询，别的按父文件夹查，只要文件夹
#[tokio::test]
async fn the_folder_browser_lists_only_folders() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(query_param(
            "q",
            format!("sharedWithMe = true and mimeType = '{FOLDER_MIME}' and trashed = false"),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "files": [{"id": "s1", "name": "Client X"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(query_param(
            "q",
            format!("'root' in parents and mimeType = '{FOLDER_MIME}' and trashed = false"),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "files": [{"id": "f1", "name": "Projects"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/drives"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "drives": [{"id": "0AAdrive", "name": "Team"}]
        })))
        .mount(&server)
        .await;

    let d = drive(&server);
    assert_eq!(
        d.child_folders(SHARED_WITH_ME).await?,
        [FolderEntry {
            id: "s1".into(),
            name: "Client X".into()
        }]
    );
    assert_eq!(
        d.child_folders("root").await?,
        [FolderEntry {
            id: "f1".into(),
            name: "Projects".into()
        }]
    );
    let roots: Vec<String> = d.roots().await?.into_iter().map(|r| r.id).collect();
    assert_eq!(roots, ["root", SHARED_WITH_ME, "0AAdrive"]);
    // 拼进 q 的 id 不收引号
    assert!(d.child_folders("x' or '1'='1").await.is_err());
    Ok(())
}

/// 换令牌：要刷新令牌、要 Drive 那一项权限；邮箱从 ID token 里拿
#[tokio::test]
async fn a_code_exchange_needs_a_refresh_token_and_the_drive_scope() -> anyhow::Result<()> {
    let token_ok = json!({
        "access_token": "ya29.a", "refresh_token": "1//r", "expires_in": 3599,
        "scope": format!("openid {DRIVE_SCOPE} https://www.googleapis.com/auth/userinfo.email"),
        "id_token": id_token(json!({
            "iss": "https://accounts.google.com", "aud": cfg().client_id,
            "sub": "1", "email": "bruno@example.com", "email_verified": true
        }))
    });
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("code=good"))
        .and(body_string_contains("code_verifier=v"))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_ok.clone()))
        .mount(&server)
        .await;
    let mut no_drive = token_ok.clone();
    no_drive["scope"] = json!("openid email");
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("code=nodrive"))
        .respond_with(ResponseTemplate::new(200).set_body_json(no_drive))
        .mount(&server)
        .await;
    let mut no_refresh = token_ok.clone();
    no_refresh.as_object_mut().unwrap().remove("refresh_token");
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("code=norefresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(no_refresh))
        .mount(&server)
        .await;

    let http = reqwest::Client::builder().no_proxy().build()?;
    let url = format!("{}/token", server.uri());
    let grant = exchange_code_at(&http, &url, &cfg(), "good", "v").await?;
    assert_eq!(grant.refresh_token, "1//r");
    assert_eq!(grant.account_email.as_deref(), Some("bruno@example.com"));

    let e = exchange_code_at(&http, &url, &cfg(), "nodrive", "v")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("read access to Drive"), "{e}");
    let e = exchange_code_at(&http, &url, &cfg(), "norefresh", "v")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no refresh token"), "{e}");
    Ok(())
}

/// 撤了授权或刷新令牌过期：说「去重连」，而不是一句 400
#[tokio::test]
async fn a_revoked_grant_says_reconnect() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant", "error_description": "Token has been expired or revoked."
        })))
        .mount(&server)
        .await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let e = access_token_at(&http, &format!("{}/token", server.uri()), &cfg(), "1//old")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("reconnect"), "{e}");
}

/// ID token：签给别的 client 的、别的 issuer 的不收；没验证过的邮箱不显示
#[test]
fn an_id_token_must_be_googles_and_ours() {
    let ours = cfg().client_id;
    let ok = id_token(json!({"iss": "accounts.google.com", "aud": ours,
                              "email": "a@b.c", "email_verified": true}));
    assert_eq!(
        id_token_email(&ok, &ours).unwrap().as_deref(),
        Some("a@b.c")
    );

    let unverified = id_token(json!({"iss": "https://accounts.google.com", "aud": ours,
                                      "email": "a@b.c", "email_verified": false}));
    assert_eq!(id_token_email(&unverified, &ours).unwrap(), None);

    let other_client = id_token(
        json!({"iss": "https://accounts.google.com", "aud": "someone-else",
                                        "email": "a@b.c", "email_verified": true}),
    );
    assert!(id_token_email(&other_client, &ours).is_err());

    let other_issuer = id_token(json!({"iss": "https://evil.example", "aud": ours,
                                        "email": "a@b.c", "email_verified": true}));
    assert!(id_token_email(&other_issuer, &ours).is_err());
}

/// 配置里的文件夹：对象或字符串都认，id 不合字符表的丢掉
#[test]
fn selected_folders_keep_only_valid_ids() {
    let cfg = json!({"folders": [
        {"id": "1AbC-_x", "name": "Clients"}, "root", {"id": "bad'id"}, {"name": "no id"}
    ]});
    assert_eq!(selected_folders(&cfg), ["1AbC-_x", "root"]);
    assert!(selected_folders(&json!({})).is_empty());
}

/// 真连一个 Google Drive。给了令牌才跑，CI 上永远跳过。
///
/// ```text
/// # 在 Google Cloud 建一个 OAuth 客户端（Web 应用），在 Utopia 里连接一个 google_drive 来源，
/// # 然后用那个客户端和一枚刷新令牌：
/// UTOPIA_GOOGLE_DRIVE_CLIENT_ID=… UTOPIA_GOOGLE_DRIVE_CLIENT_SECRET=… \
/// UTOPIA_GOOGLE_DRIVE_REDIRECT_URI=… UTOPIA_GOOGLE_DRIVE_TEST_REFRESH_TOKEN=1//… \
/// UTOPIA_GOOGLE_DRIVE_TEST_FOLDER=<folder id> cargo test -p utopia-server google_drive
/// ```
#[tokio::test]
async fn it_reads_from_a_real_drive() -> anyhow::Result<()> {
    let (Some(cfg), Ok(refresh), Ok(folder)) = (
        oauth_config(),
        std::env::var("UTOPIA_GOOGLE_DRIVE_TEST_REFRESH_TOKEN"),
        std::env::var("UTOPIA_GOOGLE_DRIVE_TEST_FOLDER"),
    ) else {
        eprintln!("跳过：未设 UTOPIA_GOOGLE_DRIVE_TEST_REFRESH_TOKEN / _FOLDER");
        return Ok(());
    };
    let d = Drive::connect(&cfg, &refresh).await?;
    assert!(!d.roots().await?.is_empty());
    let listing = d.walk(&[folder]).await?;
    assert!(
        !listing.files.is_empty(),
        "那个文件夹里一个能读的文件都没有"
    );
    let first = &listing.files[0];
    assert!(first.modified.is_some(), "modifiedTime 是 doc_time 的来源");
    let c = d.content(first).await?;
    assert!(!c.bytes.is_empty());
    Ok(())
}
