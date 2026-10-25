//! Google Drive 来源：一个人用自己的 Google 账号授权，按他选的文件夹同步（0074）。
//!
//! 按 [0013](../../docs/decisions/0013-a-source-should-hand-over-its-history.md) 的四条判据：
//!
//! | 判据 | Google Drive |
//! |---|---|
//! | 真实时间戳 | `modifiedTime`，文件内容最后一次改动的时刻 |
//! | 会不会自我推翻 | 方案、纪要、合同的新版本覆盖旧版本 |
//! | 稳定身份 | 文件 id，改名、挪目录都不变 |
//! | 企业知识住不住在那儿 | 大多数团队的文档就住在这儿 |
//!
//! 与 Notion 一样**只交出现状**：修订历史在 API 里有，但 Google 文档的旧修订只能
//! 导出成当时的全文，抽样哪几个是另一个决定，这一版不做。
//!
//! ## 授权是一个人的，不是部署的
//!
//! Notion、对象存储是管理员贴一把密钥；这里是**这个来源的编辑者**点「连接 Google
//! Drive」，在 Google 那边同意只读授权，我们存下刷新令牌（封印，`SOURCE_SECRET_KEYS`）。
//! OAuth 客户端是部署级的（`UTOPIA_GOOGLE_DRIVE_CLIENT_ID` 等），没配就没有这个按钮。
//! 授权顺带要 `openid email`，只为在界面上说清「这是以谁的身份在读」。
//!
//! ## 几个容易踩的
//!
//! **没有递归查询。** `q` 只认 `'{id}' in parents`，一层一层走；两个选中的文件夹
//! 互相嵌套时同一个文件会出现两次，按 id 去重。
//!
//! **共享云端硬盘里的东西默认看不见。** 每个请求都得带 `supportsAllDrives=true`，
//! 列表还得带 `includeItemsFromAllDrives=true`，少了不报错，只是结果是空的。
//!
//! **Google 自己的格式要导出。** Docs/Sheets/Slides 没有字节，`alt=media` 回 403；
//! 走 `export` 换成 DOCX/XLSX/PPTX，交给已有的解析器（标题层级、表格、多张工作表都在）。
//! 导出上限 10 MB，超了是 403 `exportSizeLimitExceeded`，按读不出来的那一份处理。
//!
//! **导出的字节每次都不同**（压缩包里带时间戳），所以「变没变」不能靠内容哈希，
//! 靠 Drive 给的标记：二进制用 `md5Checksum`，Google 格式用 `modifiedTime`。
//!
//! 错误文案是英文：它会原样落到来源的同步状态里给用户看。

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use anyhow::Context as _;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde_json::Value;
use uuid::Uuid;

pub const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const API_ROOT: &str = "https://www.googleapis.com/drive/v3";

/// 只读整个 Drive。`drive.file` 不够：它只给应用自己建的、或经 Picker 逐个选中的
/// 文件，选一个文件夹并不连带它里面的东西
pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
pub const SCOPES: &str = "openid email https://www.googleapis.com/auth/drive.readonly";

const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const SHORTCUT_MIME: &str = "application/vnd.google-apps.shortcut";
/// 一个文件要用到的元数据，列表与快捷方式的目标都取这些
const FILE_FIELDS: &str = "id,name,mimeType,modifiedTime,md5Checksum,size";

/// 一次遍历最多列多少个文件。列表只是元数据，标记没变的文件不下载，所以这里放得宽；
/// 它只防一棵大得离谱的树把内存撑爆。到了这个数，这一轮说出来，不报成功
const MAX_FILES_LISTED: usize = 50_000;
/// 一次同步最多下载（或导出）多少个文件，与 WebDAV、对象存储同一个量级：摄入不可逆。
/// 上限卡在下载上而不是列表上：列表每轮都是全的，没变的不花钱，到了上限的这一轮
/// 说出来，下一轮从还没下的接着下，不会每次都停在同一个地方（#1212 那类缺口）
pub const MAX_DOWNLOADS_PER_SYNC: usize = 2_000;
/// 单个文件的上限，同对象存储
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// 选中文件夹往下走几层。再深的留着不走，比一轮同步卡在一棵树上好
const MAX_DEPTH: usize = 16;
/// 文件夹浏览器一次最多回多少个子文件夹
const MAX_FOLDERS_LISTED: usize = 1_000;
/// 429 / 5xx 最多再试几回
const MAX_RETRIES: u32 = 3;
const JSON_LIMIT: usize = 4 * 1024 * 1024;

/// 部署级的 OAuth 客户端。三项都得有，缺一项就当没配
#[derive(Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect: String,
}

pub fn oauth_config() -> Option<OAuthConfig> {
    let get = |key: &str| {
        std::env::var(key)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    Some(OAuthConfig {
        client_id: get("UTOPIA_GOOGLE_DRIVE_CLIENT_ID")?,
        client_secret: get("UTOPIA_GOOGLE_DRIVE_CLIENT_SECRET")?,
        redirect: get("UTOPIA_GOOGLE_DRIVE_REDIRECT_URI")?,
    })
}

/// 文件、文件夹、共享盘的 id 只有这些字符。它们要拼进 `q` 的单引号里，
/// 不在这张表上的一律不收——比转义可靠
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 来源配置里选中的文件夹：`folders: [{id, name}]`。名字只给界面看，同步只认 id
pub fn selected_folders(config: &Value) -> Vec<String> {
    config["folders"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| f["id"].as_str().or_else(|| f.as_str()))
                .map(str::trim)
                .filter(|id| valid_id(id))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// 谁连上的这个来源：回调时记下的 Utopia 用户。没有 = 还没连，或是这条规则之前连的
pub fn connected_by(config: &Value) -> Option<Uuid> {
    config["connected_by"].as_str()?.parse().ok()
}

/// 授权码换来的东西
#[derive(Debug)]
pub struct Grant {
    pub refresh_token: String,
    /// 授权的那个 Google 账号，ID token 里核验过的邮箱；只用来在界面上显示
    pub account_email: Option<String>,
}

fn http() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?)
}

/// 授权码 + PKCE verifier 换令牌。要的是刷新令牌：访问令牌一小时就过期，
/// 同步是在没人看着的时候跑的
pub async fn exchange_code(cfg: &OAuthConfig, code: &str, verifier: &str) -> anyhow::Result<Grant> {
    exchange_code_at(&http()?, TOKEN_URL, cfg, code, verifier).await
}

async fn exchange_code_at(
    http: &reqwest::Client,
    token_url: &str,
    cfg: &OAuthConfig,
    code: &str,
    verifier: &str,
) -> anyhow::Result<Grant> {
    let resp = http
        .post(token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("client_id", &cfg.client_id),
            ("client_secret", &cfg.client_secret),
            ("redirect_uri", &cfg.redirect),
        ])
        .send()
        .await
        .context("google token")?;
    let v = token_response(resp).await?;
    // 同意页上 Google 允许人把某一项权限的勾去掉；去掉了 Drive 的那一项，
    // 令牌照发，只是什么都读不到。在这里说清楚，比让第一次同步报 403 好
    let scopes = v["scope"].as_str().unwrap_or("");
    anyhow::ensure!(
        scopes.split_whitespace().any(|s| s == DRIVE_SCOPE),
        "Google did not grant read access to Drive; connect again and allow it"
    );
    // 只有第一次同意（或 `prompt=consent`）才发刷新令牌；`begin` 每次都带 consent
    let refresh_token = v["refresh_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .context("Google returned no refresh token; connect again")?
        .to_string();
    let account_email = match v["id_token"].as_str() {
        Some(t) => id_token_email(t, &cfg.client_id)?,
        None => None,
    };
    Ok(Grant {
        refresh_token,
        account_email,
    })
}

/// 刷新令牌换一个访问令牌
async fn access_token_at(
    http: &reqwest::Client,
    token_url: &str,
    cfg: &OAuthConfig,
    refresh_token: &str,
) -> anyhow::Result<String> {
    let resp = http
        .post(token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &cfg.client_id),
            ("client_secret", &cfg.client_secret),
        ])
        .send()
        .await
        .context("google token")?;
    let v = token_response(resp).await?;
    Ok(v["access_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .context("google token: response has no access_token")?
        .to_string())
}

/// 令牌端点的响应。`invalid_grant` 单独说：那是人撤了授权、改了密码，或者
/// 刷新令牌半年没用过期了——重连能好，重试不能
async fn token_response(resp: reqwest::Response) -> anyhow::Result<Value> {
    let status = resp.status();
    let v: Value = resp
        .json()
        .await
        .context("google token: reading the response")?;
    if status.is_success() {
        return Ok(v);
    }
    if v["error"] == "invalid_grant" {
        anyhow::bail!(
            "Google Drive access was revoked or has expired; reconnect the source in its settings"
        );
    }
    anyhow::bail!(
        "google token returned {status}: {}",
        v["error_description"]
            .as_str()
            .or(v["error"].as_str())
            .unwrap_or("no message")
    )
}

/// ID token 里的邮箱。
///
/// **不验签名**：这枚令牌是我们自己拿授权码、经 TLS 直接从 Google 的令牌端点换回来的，
/// OIDC Core 3.1.3.7 允许这时以 TLS 代替签名校验。issuer 与 audience 照查——查的是
/// 「这是 Google 签给本部署的」。邮箱只拿来显示，不认身份、不开账号；
/// 没验证过的邮箱不显示，免得界面上写着一个那人并不拥有的地址
fn id_token_email(id_token: &str, client_id: &str) -> anyhow::Result<Option<String>> {
    let payload = id_token
        .split('.')
        .nth(1)
        .context("google id_token is not a JWT")?;
    let claims: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .context("google id_token payload is not base64url")?,
    )
    .context("google id_token payload is not JSON")?;
    anyhow::ensure!(
        matches!(
            claims["iss"].as_str(),
            Some("https://accounts.google.com" | "accounts.google.com")
        ),
        "google id_token has an unexpected issuer"
    );
    anyhow::ensure!(
        claims["aud"].as_str() == Some(client_id),
        "google id_token was issued to a different client"
    );
    anyhow::ensure!(
        claims["exp"]
            .as_i64()
            .is_some_and(|exp| exp > Utc::now().timestamp()),
        "google id_token has expired"
    );
    Ok(match claims["email_verified"].as_bool() {
        Some(true) => claims["email"].as_str().map(String::from),
        _ => None,
    })
}

/// 文件夹浏览器里的一项
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FolderEntry {
    pub id: String,
    pub name: String,
}

/// 选中的文件夹里的一个文件
#[derive(Debug, Clone)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub modified: Option<DateTime<Utc>>,
    md5: Option<String>,
    modified_raw: Option<String>,
}

impl DriveFile {
    /// `gdrive://{file_id}`——文件 id 是它最稳的身份
    pub fn external_key(&self) -> String {
        format!("gdrive://{}", self.id)
    }

    /// 「变没变」的标记，见模块注释
    pub fn marker(&self) -> String {
        self.md5
            .clone()
            .or_else(|| self.modified_raw.clone())
            .unwrap_or_default()
    }
}

/// 一轮遍历的结果
#[derive(Debug, Default)]
pub struct Listing {
    pub files: Vec<DriveFile>,
    /// 到了文件数上限，或有文件夹深过 `MAX_DEPTH`
    pub truncated: bool,
    /// 选中却打不开的文件夹（删了、或不再共享给这个人）。有它时不对账：
    /// 打不开不等于里面的东西都没了
    pub unreachable: Vec<String>,
    /// 跳过的文件，每个一句原因：表单、超大的
    pub skipped: usize,
    /// 指向这个人打不开的文件的快捷方式（没共享给他、删了），按快捷方式的名字
    pub broken_shortcuts: Vec<String>,
}

/// 读一个文件拿到的字节
pub struct Content {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub mime: &'static str,
}

/// Google 自己的格式导出成什么。不在这张表上的 Google 格式（表单、绘图、
/// 快捷方式、站点……）没有能读的正文，跳过
fn export_as(mime: &str) -> Option<(&'static str, &'static str)> {
    Some(match mime {
        "application/vnd.google-apps.document" => (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "docx",
        ),
        "application/vnd.google-apps.spreadsheet" => (
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "xlsx",
        ),
        "application/vnd.google-apps.presentation" => (
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "pptx",
        ),
        _ => return None,
    })
}

/// 拿着一个访问令牌的 Drive 客户端
pub struct Drive {
    http: reqwest::Client,
    token: std::sync::Mutex<String>,
    /// 访问令牌一小时就过期，一轮同步可以更长：遇到 401 凭刷新令牌换一枚新的，再试一次
    renewal: Option<Renewal>,
    // 私有地址入口让 HTTP 测试走真实的遍历逻辑，不增加用户配置
    api_root: String,
}

struct Renewal {
    cfg: OAuthConfig,
    refresh_token: String,
    token_url: String,
}

impl Drive {
    /// 用刷新令牌换访问令牌，得到一个能用的客户端
    pub async fn connect(cfg: &OAuthConfig, refresh_token: &str) -> anyhow::Result<Self> {
        let http = http()?;
        let token = access_token_at(&http, TOKEN_URL, cfg, refresh_token).await?;
        Ok(Self {
            http,
            token: std::sync::Mutex::new(token),
            renewal: Some(Renewal {
                cfg: cfg.clone(),
                refresh_token: refresh_token.into(),
                token_url: TOKEN_URL.into(),
            }),
            api_root: API_ROOT.into(),
        })
    }

    /// 发一次 GET。429 与 5xx 退避重试；401 换一枚访问令牌再试一次（只一次：换来的也被拒，
    /// 就是授权本身出了问题）；其余非 2xx 带上 Google 自己的 message 报错，
    /// 状态码与 `reason` 留在错误里给调用方分辨（404 = 打不开）
    async fn get(
        &self,
        what: &str,
        url: &str,
        query: &[(&str, &str)],
    ) -> anyhow::Result<reqwest::Response> {
        let url = reqwest::Url::parse_with_params(url, query)
            .with_context(|| format!("google drive {what}: building the URL"))?;
        let mut attempt = 0u32;
        let mut renewed = false;
        loop {
            let token = self.token.lock().expect("token lock").clone();
            let resp = self
                .http
                .get(url.clone())
                .bearer_auth(&token)
                .send()
                .await
                .with_context(|| format!("google drive {what}"))?;
            let status = resp.status();
            if let (StatusCode::UNAUTHORIZED, false, Some(r)) = (status, renewed, &self.renewal) {
                renewed = true;
                let fresh =
                    access_token_at(&self.http, &r.token_url, &r.cfg, &r.refresh_token).await?;
                *self.token.lock().expect("token lock") = fresh;
                continue;
            }
            if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
                && attempt < MAX_RETRIES
            {
                attempt += 1;
                let wait = Duration::from_millis(500 * 2u64.pow(attempt));
                tracing::info!(what, attempt, %status, "google drive busy, waiting before the next attempt");
                tokio::time::sleep(wait).await;
                continue;
            }
            if status.is_success() {
                return Ok(resp);
            }
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            let reason = v["error"]["errors"][0]["reason"].as_str().unwrap_or("");
            return Err(DriveError {
                status,
                message: format!(
                    "google drive {what} returned {status}{}: {}",
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!(" ({reason})")
                    },
                    v["error"]["message"].as_str().unwrap_or("no message")
                ),
            }
            .into());
        }
    }

    async fn get_json(
        &self,
        what: &str,
        url: &str,
        query: &[(&str, &str)],
    ) -> anyhow::Result<Value> {
        let resp = self.get(what, url, query).await?;
        let bytes = read_capped(resp, JSON_LIMIT as u64)
            .await
            .with_context(|| format!("google drive {what}"))?;
        serde_json::from_slice(&bytes).with_context(|| format!("google drive {what}: not JSON"))
    }

    /// 按 `q` 列文件，翻完所有页或到 `limit` 为止。第二项：是不是因为到了上限才停的
    async fn list(
        &self,
        q: &str,
        fields: &str,
        limit: usize,
    ) -> anyhow::Result<(Vec<Value>, bool)> {
        let url = format!("{}/files", self.api_root);
        let fields = format!("nextPageToken,files({fields})");
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![
                ("q", q),
                ("fields", fields.as_str()),
                ("pageSize", "1000"),
                ("orderBy", "name"),
                ("supportsAllDrives", "true"),
                ("includeItemsFromAllDrives", "true"),
            ];
            if let Some(t) = &page_token {
                query.push(("pageToken", t));
            }
            let v = self.get_json("files.list", &url, &query).await?;
            for f in v["files"].as_array().into_iter().flatten() {
                if out.len() >= limit {
                    return Ok((out, true));
                }
                out.push(f.clone());
            }
            page_token = v["nextPageToken"].as_str().map(String::from);
            if page_token.is_none() {
                return Ok((out, false));
            }
        }
    }

    /// 文件夹浏览器的第一层：我的云端硬盘、与我共享的、每个看得见的共享云端硬盘。
    /// 前两项的名字由界面按 id 换成它的语言
    pub async fn roots(&self) -> anyhow::Result<Vec<FolderEntry>> {
        let mut out = vec![
            FolderEntry {
                id: "root".into(),
                name: "My Drive".into(),
            },
            FolderEntry {
                id: SHARED_WITH_ME.into(),
                name: "Shared with me".into(),
            },
        ];
        let url = format!("{}/drives", self.api_root);
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![
                ("pageSize", "100"),
                ("fields", "nextPageToken,drives(id,name)"),
            ];
            if let Some(t) = &page_token {
                query.push(("pageToken", t));
            }
            let v = self.get_json("drives.list", &url, &query).await?;
            for d in v["drives"].as_array().into_iter().flatten() {
                if let (Some(id), Some(name)) = (d["id"].as_str(), d["name"].as_str()) {
                    out.push(FolderEntry {
                        id: id.into(),
                        name: name.into(),
                    });
                }
            }
            page_token = v["nextPageToken"].as_str().map(String::from);
            if page_token.is_none() || out.len() >= MAX_FOLDERS_LISTED {
                return Ok(out);
            }
        }
    }

    /// 一个文件夹（或共享盘、或「与我共享」）下面的子文件夹
    pub async fn child_folders(&self, parent: &str) -> anyhow::Result<Vec<FolderEntry>> {
        let q = if parent == SHARED_WITH_ME {
            format!("sharedWithMe = true and mimeType = '{FOLDER_MIME}' and trashed = false")
        } else {
            anyhow::ensure!(valid_id(parent), "invalid folder id");
            format!("'{parent}' in parents and mimeType = '{FOLDER_MIME}' and trashed = false")
        };
        let (items, _) = self.list(&q, "id,name", MAX_FOLDERS_LISTED).await?;
        Ok(items
            .iter()
            .filter_map(|f| {
                Some(FolderEntry {
                    id: f["id"].as_str()?.into(),
                    name: f["name"].as_str()?.into(),
                })
            })
            .collect())
    }

    /// 走遍选中的文件夹，收集文件。
    ///
    /// 广度优先，访问过的文件夹不再进（两个选中的文件夹互相嵌套时），文件按 id 去重
    pub async fn walk(&self, folders: &[String]) -> anyhow::Result<Listing> {
        let mut listing = Listing::default();
        let mut visited: HashSet<String> = HashSet::new();
        let mut seen_files: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();

        for id in folders {
            match self.folder_is_open(id).await {
                Ok(true) => queue.push_back((id.clone(), 0)),
                // 进了回收站：里面的东西对这个人已经不算数了，不进去，让对账把它们标出来
                Ok(false) => {}
                Err(e) if is_not_found(&e) => listing.unreachable.push(id.clone()),
                Err(e) => return Err(e),
            }
        }

        while let Some((folder, depth)) = queue.pop_front() {
            if !visited.insert(folder.clone()) {
                continue;
            }
            let q = format!("'{folder}' in parents and trashed = false");
            let room = MAX_FILES_LISTED.saturating_sub(listing.files.len()) + MAX_FOLDERS_LISTED;
            let fields = format!("{FILE_FIELDS},shortcutDetails(targetId,targetMimeType)");
            let (items, cut) = self.list(&q, &fields, room).await?;
            if cut {
                listing.truncated = true;
            }
            for item in items {
                // 别人组织的会议，纪要与转写住在他的 Drive 里，这边的文件夹里只有快捷方式：
                // 换成它指向的那一个，之后与普通文件一样
                let f = if item["mimeType"] == SHORTCUT_MIME {
                    match self.shortcut_target(&item).await {
                        Ok(Some(target)) => target,
                        Ok(None) => {
                            listing.skipped += 1;
                            continue;
                        }
                        Err(e) if is_inaccessible(&e) => {
                            let name = item["name"].as_str().unwrap_or("a shortcut");
                            listing.broken_shortcuts.push(name.to_string());
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                } else {
                    item
                };
                let (Some(id), Some(name), Some(mime)) =
                    (f["id"].as_str(), f["name"].as_str(), f["mimeType"].as_str())
                else {
                    continue;
                };
                // Drive 回来的 id 之后要拼进查询和地址：与选中的文件夹同一个字符集，不合的不收
                if !valid_id(id) {
                    listing.skipped += 1;
                    continue;
                }
                if mime == FOLDER_MIME {
                    if depth + 1 > MAX_DEPTH {
                        listing.truncated = true;
                    } else {
                        queue.push_back((id.to_string(), depth + 1));
                    }
                    continue;
                }
                let google_format = mime.starts_with("application/vnd.google-apps.");
                if google_format && export_as(mime).is_none() {
                    listing.skipped += 1;
                    continue;
                }
                // Drive 的 size 是字符串里的整数；Google 格式没有 size
                let size = f["size"].as_str().and_then(|s| s.parse::<u64>().ok());
                if !google_format && size.is_none_or(|n| n == 0 || n > MAX_FILE_BYTES) {
                    listing.skipped += 1;
                    continue;
                }
                if !seen_files.insert(id.to_string()) {
                    continue;
                }
                if listing.files.len() >= MAX_FILES_LISTED {
                    listing.truncated = true;
                    return Ok(listing);
                }
                let modified_raw = f["modifiedTime"].as_str().map(String::from);
                listing.files.push(DriveFile {
                    id: id.into(),
                    name: name.into(),
                    mime: mime.into(),
                    modified: modified_raw
                        .as_deref()
                        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc)),
                    md5: f["md5Checksum"].as_str().map(String::from),
                    modified_raw,
                });
            }
        }
        Ok(listing)
    }

    /// 快捷方式指向的那一个。指向文件夹的照文件夹走；指向文件的取目标自己的元数据，
    /// 变更标记、大小、导出格式都按目标算，文件也按目标的 id 认（直接选到与经快捷方式
    /// 到达的是同一份）。目标在回收站里 → `None`；打不开 → 403/404 的错误
    async fn shortcut_target(&self, shortcut: &Value) -> anyhow::Result<Option<Value>> {
        let details = &shortcut["shortcutDetails"];
        let Some(target) = details["targetId"].as_str().filter(|id| valid_id(id)) else {
            return Ok(None);
        };
        if details["targetMimeType"] == FOLDER_MIME {
            return Ok(Some(serde_json::json!({
                "id": target,
                "name": shortcut["name"],
                "mimeType": FOLDER_MIME,
            })));
        }
        let url = format!("{}/files/{target}", self.api_root);
        let fields = format!("{FILE_FIELDS},trashed");
        let v = self
            .get_json(
                "files.get",
                &url,
                &[("fields", fields.as_str()), ("supportsAllDrives", "true")],
            )
            .await?;
        Ok((v["trashed"].as_bool() != Some(true)).then_some(v))
    }

    /// 选中的这个还在不在：在且没进回收站 → true；进了回收站 → false；
    /// 打不开 → 404 的错误。`root` 是「我的云端硬盘」，共享盘的 id 也能当文件夹用
    async fn folder_is_open(&self, id: &str) -> anyhow::Result<bool> {
        let url = format!("{}/files/{id}", self.api_root);
        let v = self
            .get_json(
                "files.get",
                &url,
                &[
                    ("fields", "id,mimeType,trashed"),
                    ("supportsAllDrives", "true"),
                ],
            )
            .await?;
        anyhow::ensure!(
            v["mimeType"] == FOLDER_MIME,
            "google drive: selected item {id} is not a folder"
        );
        Ok(v["trashed"].as_bool() != Some(true))
    }

    /// 一个文件的字节：Google 格式导出，其余原样下载
    pub async fn content(&self, file: &DriveFile) -> anyhow::Result<Content> {
        anyhow::ensure!(
            valid_id(&file.id),
            "google drive: file id {:?} is not a Drive id",
            file.id
        );
        if let Some((mime, ext)) = export_as(&file.mime) {
            let url = format!("{}/files/{}/export", self.api_root, file.id);
            let resp = self.get("export", &url, &[("mimeType", mime)]).await?;
            let bytes = read_capped(resp, MAX_FILE_BYTES).await?;
            let filename = if file.name.to_lowercase().ends_with(&format!(".{ext}")) {
                file.name.clone()
            } else {
                format!("{}.{ext}", file.name)
            };
            return Ok(Content {
                bytes,
                filename,
                mime,
            });
        }
        let url = format!("{}/files/{}", self.api_root, file.id);
        let resp = self
            .get(
                "download",
                &url,
                &[("alt", "media"), ("supportsAllDrives", "true")],
            )
            .await?;
        // 不猜 mime，理由同对象存储：解析先看魔数、再看扩展名
        Ok(Content {
            bytes: read_capped(resp, MAX_FILE_BYTES).await?,
            filename: file.name.clone(),
            mime: "application/octet-stream",
        })
    }
}

/// 文件夹浏览器里「与我共享」的占位 id。真的 id 里没有冒号，撞不上
pub const SHARED_WITH_ME: &str = "shared:with-me";

/// Drive 的一个非 2xx 响应
#[derive(Debug)]
struct DriveError {
    status: StatusCode,
    message: String,
}

impl std::fmt::Display for DriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DriveError {}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<DriveError>()
        .is_some_and(|d| d.status == StatusCode::NOT_FOUND)
}

/// 打不开：Drive 对没共享给这个人的文件有时回 404，有时回 403
fn is_inaccessible(e: &anyhow::Error) -> bool {
    e.downcast_ref::<DriveError>()
        .is_some_and(|d| d.status == StatusCode::NOT_FOUND || d.status == StatusCode::FORBIDDEN)
}

/// 读一个限了体积的响应体。先看 Content-Length，再边读边数
async fn read_capped(mut resp: reqwest::Response, limit: u64) -> anyhow::Result<Vec<u8>> {
    if resp.content_length().is_some_and(|n| n > limit) {
        anyhow::bail!("response is larger than {limit} bytes");
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() as u64 > limit {
            anyhow::bail!("response is larger than {limit} bytes");
        }
    }
    Ok(body)
}

/// 上一轮摄入时每个文件的标记
pub async fn markers(
    pool: &sqlx::PgPool,
    source_id: Uuid,
) -> anyhow::Result<HashMap<String, String>> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT file_id, marker FROM google_drive_files WHERE source_id = $1")
            .bind(source_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

pub async fn remember(
    pool: &sqlx::PgPool,
    source_id: Uuid,
    file_id: &str,
    marker: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO google_drive_files (source_id, file_id, marker) VALUES ($1, $2, $3)
         ON CONFLICT (source_id, file_id) DO UPDATE SET marker = EXCLUDED.marker",
    )
    .bind(source_id)
    .bind(file_id)
    .bind(marker)
    .execute(pool)
    .await?;
    Ok(())
}

/// 这一轮没出现的文件忘掉标记：它再出现时得重新读一遍，不能凭旧标记跳过。
/// 只在遍历完整的轮次调用，与对账同一个条件
pub async fn forget_others(
    pool: &sqlx::PgPool,
    source_id: Uuid,
    seen: &[String],
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM google_drive_files WHERE source_id = $1 AND NOT (file_id = ANY($2))")
        .bind(source_id)
        .bind(seen)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
#[path = "google_drive_tests.rs"]
mod tests;
