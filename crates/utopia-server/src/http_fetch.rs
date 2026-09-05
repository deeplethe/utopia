//! 一条受管的抓取路径（#330）。
//!
//! 从前每个来源各建各的 `reqwest::Client`——六处，每处只设了超时和 UA，没有
//! 一处问过「这个请求要连到哪里去」。于是 `http://169.254.169.254/latest/meta-data/`
//! 会被当成一篇网页抓回来、切块、索引；重定向默认跟十跳，公网主机可以把
//! 第二跳指进内网；响应体一次 `bytes()` 读进内存，没有上限。
//!
//! **谁选的这个地址，决定该有多严。**
//!
//! - [`Reach::Operator`]：地址是这个库的 editor 自己填的（url / webdav / jira /
//!   custom 来源）。内网是**正当目标**——自部署的内部 wiki 正是这个产品的用途，
//!   `sync_custom` 至今为回环地址专门关掉了代理。
//! - [`Reach::Content`]：地址来自抓回来的内容（feed 里的文章链接）。这时
//!   「一个人选的」已经退化成「一个 feed 说的」，只准公网。
//!
//! 两种都拿到的东西：解析后**逐个地址校验**、`resolve_to_addrs` 把地址钉死
//! （校验完再解析一次就能被改掉，那是 DNS rebinding 的整个手法）、重定向手动
//! 逐跳跟并复验、https 不许降级成 http、响应体有字节上限、整趟有总时限。
//!
//! **重定向不许升级可达范围**：operator 填的是公网主机时，第二跳就不能指进内网——
//! 他选的是那台机器，不是那台机器的转发能力。
//!
//! 形状与实现要点来自 #326（@J-i-K）为 RSS 全文抓取写的那一版；这里把它抽成
//! 所有来源共用的一条路。

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::Url;

/// 目的地是谁选的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// 库里的 editor 自己填的主机：内网也放行
    Operator,
    /// 抓回来的内容里给出的地址：只准公网
    Content,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_redirects: usize,
    pub connect_timeout: Duration,
    pub overall_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
            max_redirects: 5,
            connect_timeout: Duration::from_secs(10),
            overall_timeout: Duration::from_secs(30),
        }
    }
}

impl Limits {
    pub fn with_overall(mut self, overall: Duration) -> Self {
        self.overall_timeout = overall;
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("only http and https are fetched, got {0:?}")]
    UnsupportedScheme(String),
    #[error("destination {0} is not a public address")]
    BlockedAddress(IpAddr),
    #[error("could not resolve {0}")]
    Dns(String),
    #[error("redirect to {0} is not allowed from here")]
    BlockedRedirect(String),
    #[error("too many redirects")]
    RedirectLimit,
    #[error("response is larger than {0} bytes")]
    TooLarge(usize),
    #[error("HTTP {0}")]
    Status(u16),
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
}

#[derive(Debug)]
pub struct Fetched {
    pub final_url: Url,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// 一个只连得到这个主机、且地址已按 `reach` 校验过的 client。
///
/// 给那些自己要发好几个请求的来源用（GitHub / Jira / 自定义端点 / WebDAV 都要
/// 带认证头、要翻页、要发 PROPFIND）——它们保留自己的请求逻辑，只是换一个
/// 连得到的地方受管的 client。
pub async fn client_for(
    url: &Url,
    reach: Reach,
    limits: Limits,
) -> Result<reqwest::Client, FetchError> {
    build_client(url, reach, limits, true).await
}

/// `follow`：`get` 自己逐跳跟并复验，所以它要一个**不跟**的 client；
/// 那些自己发请求的来源（GitHub / Jira / WebDAV）保持库默认的跟随行为。
async fn build_client(
    url: &Url,
    reach: Reach,
    limits: Limits,
    follow: bool,
) -> Result<reqwest::Client, FetchError> {
    let host = url
        .host_str()
        .ok_or_else(|| FetchError::InvalidUrl(url.to_string()))?;
    let addrs = resolve(host, url, reach).await?;
    let private = addrs.iter().any(|a| !is_public_ip(a.ip()));
    let mut builder = reqwest::Client::builder()
        .connect_timeout(limits.connect_timeout)
        .timeout(limits.overall_timeout)
        .user_agent(crate::ingest_sources::UA)
        .resolve_to_addrs(host, &addrs);
    if !follow {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }
    // 内网/回环不走系统代理：代理对这些地址只会 502（`sync_custom` 一直是这么做的）
    if private {
        builder = builder.no_proxy();
    }
    Ok(builder.build()?)
}

/// GET 一个 URL，自己跟重定向。
pub async fn get(raw_url: &str, reach: Reach, limits: Limits) -> Result<Fetched, FetchError> {
    let mut url = parse(raw_url)?;
    let deadline = Instant::now() + limits.overall_timeout;

    for hop in 0..=limits.max_redirects {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(FetchError::Status(408));
        }
        // **自带的重定向要关掉**：跟哪一跳由上面那个循环决定，逐跳复验；
        // 交给 reqwest 跟就等于没人看它跟去了哪里
        let client = build_client(&url, reach, limits.with_overall(remaining), false).await?;
        let response = client
            .get(url.clone())
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await?;

        if response.status().is_redirection() {
            if hop == limits.max_redirects {
                return Err(FetchError::RedirectLimit);
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| FetchError::BlockedRedirect("(no location)".into()))?;
            let next = url
                .join(location)
                .map_err(|_| FetchError::BlockedRedirect(location.to_string()))?;
            check_hop(&url, &next, reach).await?;
            url = next;
            continue;
        }
        if !response.status().is_success() {
            return Err(FetchError::Status(response.status().as_u16()));
        }
        return read_bounded(response, url, limits.max_bytes).await;
    }
    Err(FetchError::RedirectLimit)
}

/// 一跳是否走得过去。
///
/// 除了 `reach` 本身的规则，还有一条**不许升级**：这一跳原本落在公网上，
/// 下一跳就不能落进内网。operator 选的是那台主机，不是那台主机的转发能力。
async fn check_hop(from: &Url, to: &Url, reach: Reach) -> Result<(), FetchError> {
    if from.scheme() == "https" && to.scheme() == "http" {
        return Err(FetchError::BlockedRedirect(to.to_string()));
    }
    let host = to
        .host_str()
        .ok_or_else(|| FetchError::InvalidUrl(to.to_string()))?;
    let addrs = resolve(host, to, reach).await?;
    let from_host = from.host_str().unwrap_or_default();
    let from_public = resolve(from_host, from, reach)
        .await
        .map(|a| a.iter().all(|s| is_public_ip(s.ip())))
        .unwrap_or(true);
    if from_public && addrs.iter().any(|a| !is_public_ip(a.ip())) {
        return Err(FetchError::BlockedRedirect(to.to_string()));
    }
    Ok(())
}

fn parse(raw: &str) -> Result<Url, FetchError> {
    let url = Url::parse(raw.trim()).map_err(|e| FetchError::InvalidUrl(e.to_string()))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(FetchError::UnsupportedScheme(other.to_string())),
    }
}

/// 解析主机并按 `reach` 校验每一个地址。
///
/// **一次解析，之后钉住**：校验完再让 client 自己解析一遍，中间那一瞬就是
/// DNS rebinding 的下手处。端口留 0，hyper 的连接器会用 URL 上的端口。
async fn resolve(host: &str, url: &Url, reach: Reach) -> Result<Vec<SocketAddr>, FetchError> {
    let port = url
        .port_or_known_default()
        .ok_or_else(|| FetchError::InvalidUrl(url.to_string()))?;
    let resolved: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| FetchError::Dns(host.to_string()))?
            .collect()
    };
    if resolved.is_empty() {
        return Err(FetchError::Dns(host.to_string()));
    }
    if reach == Reach::Content {
        // **一个不合格就整体拒绝**，不是挑出公网的那些来连：一个主机同时解析出
        // 公网与内网地址，本身就是要把请求引进内网的手法
        if let Some(bad) = resolved.iter().find(|a| !is_public_ip(a.ip())) {
            return Err(FetchError::BlockedAddress(bad.ip()));
        }
    }
    let mut seen = HashSet::new();
    Ok(resolved
        .into_iter()
        .filter(|a| seen.insert(a.ip()))
        .map(|a| SocketAddr::new(a.ip(), 0))
        .collect())
}

async fn read_bounded(
    response: reqwest::Response,
    final_url: Url,
    max_bytes: usize,
) -> Result<Fetched, FetchError> {
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "application/octet-stream".into());
    // Content-Length 说自己超了就别开始读——但它是对方说的，不能只信它
    if response
        .content_length()
        .is_some_and(|n| n as usize > max_bytes)
    {
        return Err(FetchError::TooLarge(max_bytes));
    }
    let mut bytes: Vec<u8> = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() + chunk.len() > max_bytes {
            return Err(FetchError::TooLarge(max_bytes));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Fetched {
        final_url,
        mime,
        bytes,
    })
}

/// 这个地址在公网上吗。
///
/// 按 RFC 6890 的特殊用途地址表拦，**只拦表里的**：`192.0.0.0/16` 整段当保留
/// 是个常见的过度拦截（保留的只有 `192.0.0.0/24` 与 `192.0.2.0/24`），而多拦
/// 一段就是一个用户抓不到的正当站点。
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_loopback()          // 127.0.0.0/8
                || v4.is_private()      // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()   // 169.254/16
                || v4.is_broadcast()
                || v4.is_documentation() // 192.0.2/24, 198.51.100/24, 203.0.113/24
                || v4.is_unspecified()  // 0.0.0.0/8
                || a == 0
                || (a == 100 && (64..=127).contains(&b))   // 100.64/10 CGNAT
                || (a == 192 && b == 0 && c == 0)          // 192.0.0/24 IETF 协议分配
                || (a == 192 && b == 88 && c == 99)        // 6to4 中继任播
                || (a == 198 && (b == 18 || b == 19))      // 198.18/15 基准测试
                || a >= 224) // 组播与保留
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let seg = v6.segments();
            // NAT64（64:ff9b::/96 与 64:ff9b:1::/48）裹着一个 v4 地址，按里面那个判
            if seg[0] == 0x0064 && seg[1] == 0xff9b {
                let v4 = std::net::Ipv4Addr::new(
                    (seg[6] >> 8) as u8,
                    seg[6] as u8,
                    (seg[7] >> 8) as u8,
                    seg[7] as u8,
                );
                return is_public_ip(IpAddr::V4(v4));
            }
            // 6to4（2002::/16）同理，v4 在第 2、3 段
            if seg[0] == 0x2002 {
                let v4 = std::net::Ipv4Addr::new(
                    (seg[1] >> 8) as u8,
                    seg[1] as u8,
                    (seg[2] >> 8) as u8,
                    seg[2] as u8,
                );
                return is_public_ip(IpAddr::V4(v4));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || (seg[0] & 0xff00) == 0xff00      // ff00::/8 组播
                || (seg[0] & 0xfe00) == 0xfc00      // fc00::/7 唯一本地
                || (seg[0] & 0xffc0) == 0xfe80      // fe80::/10 链路本地
                || (seg[0] == 0x2001 && seg[1] == 0x0db8)) // 2001:db8::/32 文档用
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_addresses_a_fetch_must_refuse() {
        for blocked in [
            "127.0.0.1",
            "0.0.0.0",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.5",
            "169.254.169.254", // 云上的元数据服务，这条路最常被人走
            "100.64.0.1",
            "192.0.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1", // v4 映射回环
            "64:ff9b::7f00:1",  // NAT64 裹着回环
            "2002:7f00:1::",    // 6to4 裹着回环
        ] {
            assert!(!is_public_ip(ip(blocked)), "{blocked} 该被拦下");
        }
    }

    #[test]
    fn the_addresses_a_fetch_must_still_allow() {
        for allowed in [
            "1.1.1.1",
            "8.8.8.8",
            "192.0.1.1",   // 192.0.0.0/16 里**不**保留的那部分
            "192.88.98.1", // 6to4 中继只占 192.88.99.0/24
            "2606:4700::1111",
            "2002:0808:0808::", // 6to4 裹着 8.8.8.8
        ] {
            assert!(is_public_ip(ip(allowed)), "{allowed} 不该被拦");
        }
    }

    /// 真的起一个本机服务打一遍：回环是**私有地址**，所以这套断言正好卡在
    /// 两种 reach 的分界上——内容给的地址够不着它，操作者填的够得着。
    #[tokio::test]
    async fn a_loopback_host_is_reachable_only_when_a_person_typed_it() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/page"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw("<html><body>hello</body></html>", "text/html"),
            )
            .mount(&server)
            .await;
        let url = format!("{}/page", server.uri());

        // feed 里的一个链接指向 127.0.0.1 —— 正是这条路要拦的东西
        let refused = get(&url, Reach::Content, Limits::default()).await;
        assert!(
            matches!(refused, Err(FetchError::BlockedAddress(_))),
            "内容给出的地址不该够得到回环，得到的是 {refused:?}"
        );

        // 同一个地址，来源是这个库的人自己填的：内部服务本来就是正当目标
        let page = get(&url, Reach::Operator, Limits::default())
            .await
            .expect("操作者填的内网地址该抓得到");
        assert_eq!(page.mime, "text/html");
        assert!(String::from_utf8_lossy(&page.bytes).contains("hello"));
    }

    #[tokio::test]
    async fn a_body_larger_than_the_cap_is_refused() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("x".repeat(4096)))
            .mount(&server)
            .await;
        // 从前这里是一句 `resp.bytes()`：对面回多少就读多少
        let limits = Limits {
            max_bytes: 1024,
            ..Limits::default()
        };
        let out = get(&server.uri(), Reach::Operator, limits).await;
        assert!(matches!(out, Err(FetchError::TooLarge(1024))), "{out:?}");
    }

    #[tokio::test]
    async fn a_redirect_chain_ends_instead_of_looping() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(302).insert_header("location", "/next"))
            .mount(&server)
            .await;
        let limits = Limits {
            max_redirects: 2,
            ..Limits::default()
        };
        let out = get(&server.uri(), Reach::Operator, limits).await;
        assert!(matches!(out, Err(FetchError::RedirectLimit)), "{out:?}");
    }

    #[test]
    fn only_http_and_https_are_fetched() {
        assert!(parse("file:///etc/passwd").is_err());
        assert!(parse("gopher://example.com/").is_err());
        assert!(parse("https://example.com/a").is_ok());
    }
}
