//! 请求来源捕获：把客户端 IP 与 User-Agent 放进 task-local，供审计写入时读取。
//!
//! 走 task-local 而非逐层传参，是因为 `audit::record` 有二十多个调用点，
//! 它们分散在各个业务 handler 里；为了两个与业务无关的字段改遍所有签名，
//! 只会让每个调用点都记得住这件与它无关的事。

use axum::extract::ConnectInfo;
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use std::net::SocketAddr;
use utopia_store::audit::{ClientContext, CLIENT};

/// 反向代理转发的原始客户端地址。取 X-Forwarded-For 的第一段（最靠近客户端的
/// 那一跳），其次 X-Real-IP，都没有则用实际的 TCP 对端地址。
///
/// 这两个头是客户端可伪造的，直连部署时不该轻信；但直连时它们本来就不存在，
/// 落回 TCP 对端地址即为真值。部署在代理之后时，代理应当覆写而非追加它们。
fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<String> {
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff
            .split(',')
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(first.to_string());
        }
    }
    if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        let real = real.trim();
        if !real.is_empty() {
            return Some(real.to_string());
        }
    }
    peer.map(|a| a.ip().to_string())
}

/// User-Agent 截断到 256 字节：它由客户端完全控制，不该让一条请求头把台账撑爆。
fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut end = s.len().min(256);
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            s[..end].to_string()
        })
}

pub async fn capture(req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0);
    let ctx = ClientContext {
        ip: client_ip(req.headers(), peer),
        user_agent: user_agent(req.headers()),
    };
    CLIENT.scope(ctx, next.run(req)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn xff_wins_and_takes_the_client_hop() {
        let h = headers(&[("x-forwarded-for", "203.0.113.7, 10.0.0.1, 10.0.0.2")]);
        let peer = Some("10.0.0.9:5000".parse().unwrap());
        assert_eq!(client_ip(&h, peer).as_deref(), Some("203.0.113.7"));
    }

    #[test]
    fn falls_back_to_real_ip_then_peer() {
        let h = headers(&[("x-real-ip", "198.51.100.4")]);
        assert_eq!(client_ip(&h, None).as_deref(), Some("198.51.100.4"));

        let peer = Some("192.0.2.5:443".parse().unwrap());
        assert_eq!(
            client_ip(&HeaderMap::new(), peer).as_deref(),
            Some("192.0.2.5")
        );
        assert_eq!(client_ip(&HeaderMap::new(), None), None);
    }

    #[test]
    fn blank_headers_do_not_shadow_the_peer() {
        let h = headers(&[("x-forwarded-for", "  "), ("x-real-ip", "")]);
        let peer = Some("192.0.2.5:443".parse().unwrap());
        assert_eq!(client_ip(&h, peer).as_deref(), Some("192.0.2.5"));
    }

    #[test]
    fn user_agent_is_capped_at_256_bytes() {
        let long = "Mozilla/5.0 ".repeat(50); // 600 字节，远超上限
        let h = headers(&[("user-agent", long.as_str())]);
        let got = user_agent(&h).unwrap();
        assert_eq!(got.len(), 256);
        assert!(long.starts_with(&got), "截断后仍是原串的前缀");
    }

    /// HeaderValue::to_str 只接受可见 ASCII，非 ASCII 的 UA 读不出来——
    /// 记空好过记一段乱码。这也是上面按字节截断安全的原因：能读出来的一定是 ASCII。
    #[test]
    fn non_ascii_user_agent_is_dropped_rather_than_mangled() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::USER_AGENT,
            axum::http::HeaderValue::from_bytes("浏览器".as_bytes()).unwrap(),
        );
        assert_eq!(user_agent(&h), None);
    }
}
