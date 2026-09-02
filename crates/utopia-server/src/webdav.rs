//! WebDAV 来源：Nextcloud、ownCloud、坚果云、群晖、Apache mod_dav、
//! rclone serve webdav，以及一切说这套协议的网盘。
//!
//! 按 [0013](../../docs/decisions/0013-a-source-should-hand-over-its-history.md)
//! 的四条判据，它跟对象存储是同一档：有 `getlastmodified`、身份是路径、
//! 企业里确实有一堆文档住在网盘上；而「会不会自我推翻」只能靠两次同步的
//! 差异看出来——WebDAV 的版本控制扩展（RFC 3253）几乎没有服务端实现。
//!
//! **不引 WebDAV 客户端库。** 这套协议要的东西很窄：一个 `PROPFIND` 拿列表、
//! 一个 `GET` 取内容，响应是 XML。`reqwest` 与 `quick-xml` 都已经在树里，
//! 加起来零新增依赖；而现有的 dav 客户端 crate 要么裹着自己的 HTTP 栈，
//! 要么把整套 RFC 4918（锁、属性写、版本）都拖进来，而那些我们一条都不用。

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;

/// 一次同步最多摄入多少个文件。理由与对象存储那边相同：网盘也能装下
/// 十万个文件，而摄入不可逆。
const MAX_FILES_PER_SYNC: usize = 2_000;

/// 单个文件的大小上限。
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// 目录递归的层数上限。**不是怕深，是怕环**：有些服务端会把符号链接
/// 或共享目录暴露成可以自己指向自己的路径，`Depth: infinity` 在那种目录上
/// 永远回不来。逐层走并且封顶，比信任服务端安全。
const MAX_DEPTH: usize = 8;

/// 一个远端文件。
pub struct RemoteFile {
    /// `webdav://host/path`——`ingest_item` 的 external_key 约定是 URI 形态
    pub external_key: String,
    pub filename: String,
    pub bytes: Vec<u8>,
    pub last_modified: Option<DateTime<Utc>>,
}

/// `PROPFIND` 回来的一条。
#[derive(Debug, PartialEq)]
struct Entry {
    /// 服务端给的 href，已解码
    href: String,
    is_dir: bool,
    len: u64,
    modified: Option<DateTime<Utc>>,
}

/// 解析 `multistatus` 响应。
///
/// **只认本地名，不认前缀。** 服务端可能用 `D:`、`d:`、`ns0:`，也可能干脆
/// 不加前缀——RFC 允许任意前缀绑到 `DAV:` 命名空间上。按 `d:response` 这样
/// 硬匹配的解析器，换一台服务端就一条都读不出来，而症状是「同步成功，
/// 零个文件」。
///
/// **目录判据是 `<collection/>` 存在，不是 href 以 `/` 结尾。** 后者是约定
/// 不是规范，而 rclone 与 Nextcloud 在这一点上就不一致。
fn parse_multistatus(xml: &str) -> anyhow::Result<Vec<Entry>> {
    let mut r = Reader::from_str(xml);
    r.config_mut().trim_text(true);

    let mut out = Vec::new();
    let mut cur: Option<Entry> = None;
    let mut field = String::new();
    let mut buf = Vec::new();

    loop {
        match r.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().into_inner());
                match name.as_str() {
                    "response" => {
                        cur = Some(Entry {
                            href: String::new(),
                            is_dir: false,
                            len: 0,
                            modified: None,
                        });
                    }
                    "collection" => {
                        if let Some(c) = cur.as_mut() {
                            c.is_dir = true;
                        }
                    }
                    other => field = other.to_string(),
                }
            }
            // `<collection/>` 通常是自闭合标签，走 Empty 而不是 Start——
            // 少了这一条，所有目录都会被当成文件去 GET
            Ok(Event::Empty(e)) => {
                if local_name(e.name().into_inner()) == "collection" {
                    if let Some(c) = cur.as_mut() {
                        c.is_dir = true;
                    }
                }
            }
            Ok(Event::Text(t)) => {
                let Some(c) = cur.as_mut() else { continue };
                let v = t
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .to_string();
                match field.as_str() {
                    "href" => c.href = percent_decode(&v),
                    "getcontentlength" => c.len = v.trim().parse().unwrap_or(0),
                    "getlastmodified" => {
                        // RFC 1123，`Wed, 02 Sep 2026 15:04:05 GMT`
                        c.modified = DateTime::parse_from_rfc2822(v.trim())
                            .ok()
                            .map(|d| d.with_timezone(&Utc));
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                if local_name(e.name().into_inner()) == "response" {
                    if let Some(c) = cur.take() {
                        out.push(c);
                    }
                }
                field.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("PROPFIND 响应解析失败: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// 取标签的本地名，丢掉命名空间前缀并转小写。
///
/// 规范说元素名区分大小写，但实测有服务端写 `getLastModified`。
/// 前缀也自己切：`D:`、`d:`、`ns0:` 都合法，RFC 允许任意前缀绑到 `DAV:`。
fn local_name(raw: &str) -> String {
    raw.rsplit(':').next().unwrap_or(raw).to_ascii_lowercase()
}

/// href 里的百分号转义。
///
/// 自己解而不是引 `percent-encoding`：这里只需要解码，而解码是十来行。
/// 非法序列原样留下——href 是拿来拼 URL 的，猜错了不如不动。
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 逐层走目录，取回所有文件。
///
/// **不用 `Depth: infinity`。** 规范允许服务端拒绝它（`403 Propfind-Finite-Depth`），
/// 而 Nextcloud 与 Apache mod_dav 默认就是拒绝的；能接受它的服务端在大目录上
/// 又会一次性吐出几十兆 XML。逐层加封顶，两头的问题都没有。
pub async fn fetch(
    http: &reqwest::Client,
    base: &str,
    root: &str,
    auth: Option<(&str, &str)>,
) -> anyhow::Result<(Vec<RemoteFile>, bool)> {
    let host = reqwest::Url::parse(base)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "webdav".into());
    let base = base.trim_end_matches('/').to_string();

    let mut out = Vec::new();
    let mut truncated = false;
    let mut queue = vec![(normalize(root), 0usize)];
    let mut unreadable = 0usize;

    while let Some((dir, depth)) = queue.pop() {
        if depth > MAX_DEPTH {
            tracing::warn!(%dir, "目录太深，不再往下");
            continue;
        }
        let url = format!("{base}{dir}");
        let mut req = http
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .header("Depth", "1")
            .header("Content-Type", "application/xml");
        if let Some((u, p)) = auth {
            req = req.basic_auth(u, Some(p));
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("PROPFIND {url}"))?;
        if !resp.status().is_success() {
            anyhow::bail!("PROPFIND {url} 回了 {}", resp.status());
        }
        let xml = resp.text().await.context("读取 PROPFIND 响应")?;

        for e in parse_multistatus(&xml)? {
            let path = normalize(&strip_base(&e.href, &base));
            // 服务端会把被查询的目录自己也列进来，跳过它否则会无限打转
            if path == dir {
                continue;
            }
            if e.is_dir {
                queue.push((path, depth + 1));
                continue;
            }
            if e.len == 0 || e.len > MAX_FILE_BYTES {
                continue;
            }
            if out.len() >= MAX_FILES_PER_SYNC {
                truncated = true;
                break;
            }

            // 一个文件取不回来不该带走整次同步——与对象存储那边同一个判断
            let mut g = http.get(format!("{base}{path}"));
            if let Some((u, p)) = auth {
                g = g.basic_auth(u, Some(p));
            }
            let got = async {
                let r = g.send().await?;
                if !r.status().is_success() {
                    anyhow::bail!("{}", r.status());
                }
                Ok::<_, anyhow::Error>(r.bytes().await?)
            }
            .await;
            let bytes = match got {
                Ok(b) => b,
                Err(err) => {
                    unreadable += 1;
                    tracing::warn!(%path, error = %err, "文件取不回来，跳过");
                    continue;
                }
            };

            out.push(RemoteFile {
                external_key: format!("webdav://{host}{path}"),
                filename: path.rsplit('/').next().unwrap_or(&path).to_string(),
                bytes: bytes.to_vec(),
                last_modified: e.modified,
            });
        }
        if truncated {
            break;
        }
    }
    if unreadable > 0 {
        tracing::warn!(unreadable, "有文件没能取回");
    }
    Ok((out, truncated))
}

/// 路径统一成 `/a/b` 的形状：前有斜杠、后无斜杠（根除外）。
fn normalize(p: &str) -> String {
    let t = p.trim();
    let t = t.strip_suffix('/').unwrap_or(t);
    if t.is_empty() {
        "/".into()
    } else if t.starts_with('/') {
        t.into()
    } else {
        format!("/{t}")
    }
}

/// href 可能是绝对 URL 也可能只是路径——两种都合法，服务端各写各的。
fn strip_base(href: &str, base: &str) -> String {
    if let Some(rest) = href.strip_prefix(base) {
        return rest.to_string();
    }
    // 绝对但换了主机名（反代改写过）：退回取它的 path
    if href.starts_with("http://") || href.starts_with("https://") {
        if let Ok(u) = reqwest::Url::parse(href) {
            return u.path().to_string();
        }
    }
    href.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **命名空间前缀是任意的。** 同一份响应换个前缀必须解出同样的东西，
    /// 否则换一台服务端就「同步成功，零个文件」——最难查的那种失败。
    #[test]
    fn any_namespace_prefix_parses_the_same() {
        let with_d = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:"><D:response>
  <D:href>/docs/a.txt</D:href>
  <D:propstat><D:prop>
    <D:getcontentlength>5</D:getcontentlength>
    <D:getlastmodified>Wed, 02 Sep 2026 15:04:05 GMT</D:getlastmodified>
    <D:resourcetype/>
  </D:prop></D:propstat>
</D:response></D:multistatus>"#;
        // 前缀换掉，命名空间声明本身不受影响（`xmlns:D="DAV:"` 里没有 `D:` 这个子串）
        let with_ns0 = with_d.replace("D:", "ns0:");
        let a = parse_multistatus(with_d).unwrap();
        let b = parse_multistatus(&with_ns0).unwrap();
        assert_eq!(a, b, "换个前缀就解不出来了");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].href, "/docs/a.txt");
        assert_eq!(a[0].len, 5);
        assert!(!a[0].is_dir);
        assert!(
            a[0].modified.is_some(),
            "getlastmodified 是 doc_time 的来源"
        );
    }

    /// **目录判据是 `<collection/>`，而它通常是自闭合标签。**
    /// 只处理 `Event::Start` 的解析器会把每个目录都当成文件去 GET。
    #[test]
    fn a_self_closing_collection_is_still_a_directory() {
        let xml = r#"<multistatus xmlns="DAV:"><response>
  <href>/docs/</href>
  <propstat><prop><resourcetype><collection/></resourcetype></prop></propstat>
</response></multistatus>"#;
        let e = parse_multistatus(xml).unwrap();
        assert!(e[0].is_dir, "自闭合的 collection 没被认出来");
    }

    /// href 是百分号转义过的，中文路径直接拼 URL 会 404。
    #[test]
    fn a_percent_encoded_href_is_decoded() {
        assert_eq!(
            percent_decode("/docs/%E4%B8%AD%E6%96%87.txt"),
            "/docs/中文.txt"
        );
        // 非法序列原样留着，不猜
        assert_eq!(percent_decode("/a%ZZb"), "/a%ZZb");
    }

    /// 路径统一：前有斜杠、后无斜杠。两端不一致会让同一个目录被走两遍。
    #[test]
    fn paths_normalise_to_one_shape() {
        for (raw, want) in [
            ("docs", "/docs"),
            ("/docs/", "/docs"),
            ("/", "/"),
            ("", "/"),
        ] {
            assert_eq!(normalize(raw), want, "{raw}");
        }
    }

    /// 真连一个 WebDAV 服务端。没有 `UTOPIA_DAV_TEST_URL` 就跳过。
    ///
    /// 用 rclone 起一个最省事，它和 Nextcloud 在 href 形态上不一样，
    /// 正好能验解析不挑服务端：
    /// ```text
    /// mkdir -p /tmp/dav/docs && echo hello > /tmp/dav/docs/a.txt
    /// rclone serve webdav /tmp/dav --addr 127.0.0.1:18081 --user u --pass p
    /// UTOPIA_DAV_TEST_URL=http://127.0.0.1:18081 UTOPIA_DAV_TEST_USER=u     ///   UTOPIA_DAV_TEST_PASS=p cargo test -p utopia-server webdav
    /// ```
    #[tokio::test]
    async fn it_reads_from_a_real_webdav_server() -> anyhow::Result<()> {
        let Ok(base) = std::env::var("UTOPIA_DAV_TEST_URL") else {
            eprintln!("跳过：未设 UTOPIA_DAV_TEST_URL");
            return Ok(());
        };
        let user = std::env::var("UTOPIA_DAV_TEST_USER").unwrap_or_default();
        let pass = std::env::var("UTOPIA_DAV_TEST_PASS").unwrap_or_default();
        let auth = (!user.is_empty()).then_some((user.as_str(), pass.as_str()));

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let (files, truncated) = fetch(&http, &base, "/docs", auth).await?;
        assert!(!truncated);

        let a = files.iter().find(|f| f.filename == "a.txt").expect("a.txt");
        assert_eq!(a.bytes, b"hello from webdav");
        assert!(
            a.external_key.starts_with("webdav://"),
            "{}",
            a.external_key
        );
        assert!(a.last_modified.is_some());
        // 目录本身不该混进来
        assert!(files.iter().all(|f| !f.filename.is_empty()));
        Ok(())
    }
}
