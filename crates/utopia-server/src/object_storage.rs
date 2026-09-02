//! 对象存储来源：S3 与一切说同一套协议的东西（MinIO、Ceph RGW、阿里 OSS 的
//! S3 兼容端点、Cloudflare R2…）。
//!
//! 按 [0013](../../docs/decisions/0013-a-source-should-hand-over-its-history.md)
//! 的四条判据看，它跟工单系统不是一类：
//!
//! | 判据 | 对象存储 |
//! |---|---|
//! | 真实时间戳 | `LastModified`，但它是**写入时刻**不是文档自身的时间 |
//! | 会不会自我推翻 | 同 key 覆写就是一次改口，但要靠两次同步之间的差异看出来 |
//! | 稳定身份 | `bucket/key`，最干净的一条 |
//! | 企业知识住不住在那儿 | **最强的一条**——文档堆、归档、数据湖落地区都在这儿 |
//!
//! 所以它的价值在第四条：**够得着**。第二条只有部分——工单系统能一次交出
//! 整段变更史，而对象存储要么开了版本控制（本版不读，见下），要么只能靠
//! 一次次同步慢慢攒。这跟 `url` / `rss` 是同一档，不比它们低。
//!
//! **版本历史本版不做。** `ListObjectVersions` 能一次拿到 0013 想要的那段历史，
//! 但多数桶没开版本控制，开了的桶里也大多是写一次不再动的对象——把每个版本
//! 都摄进来，代价与收益不成比例。等有人真的需要（一份季度更新的制度文件，
//! 想看它改了什么）再按开关加。
//!
//! **为什么是 `object_store` 而不是 `aws-sdk-s3` / `rust-s3`**：实测传递依赖
//! `+7` 对 `+43` / `+42`——它复用了树里已有的 `reqwest`、`ring`、`quick-xml`。
//! 而且同一套 API 还覆盖 Azure Blob 与 GCS，路线图上「更多数据源」与
//! 「数据湖仓」两条都在这条线上，换后端只是换一个 builder。

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use futures_util::StreamExt as _;
use object_store::aws::AmazonS3Builder;
// `get` 住在 `ObjectStoreExt` 上，不在 `ObjectStore` 本体——后者只有
// `get_opts`。少这一行的报错是「&dyn ObjectStore 没有 get 方法」，
// 而 trait 名字里看不出来。
use object_store::{path::Path as StorePath, ObjectStore, ObjectStoreExt as _};

/// 一次同步最多摄入多少个对象。
///
/// **不是性能考虑，是「别把一个桶整个吸进来」。** 桶里放上百万个对象是常态，
/// 而摄入是不可逆的：每个对象要抽取、要向量化、要进图。配错一个前缀就能
/// 烧掉一整天的额度，而且清理起来比配置麻烦得多。
///
/// 到顶时报进同步统计，让人看得见「还有」，而不是悄悄截断。
const MAX_OBJECTS_PER_SYNC: usize = 2_000;

/// 单个对象的大小上限。跟上传路径同一个数量级——超过这个尺寸的多半不是
/// 文档而是数据文件（备份、镜像、视频），抽取器拿它没有用。
const MAX_OBJECT_BYTES: u64 = 32 * 1024 * 1024;

/// 一个待摄入的对象。
pub struct RemoteObject {
    /// `s3://bucket/key`——`ingest_item` 的 external_key 约定是 URI 形态
    pub external_key: String,
    /// 落进文档的文件名，取 key 的最后一段
    pub filename: String,
    pub bytes: Vec<u8>,
    pub last_modified: Option<DateTime<Utc>>,
}

/// 从来源配置建一个客户端。
///
/// **`endpoint` 在则走自建**（MinIO、Ceph、R2），此时强制 path-style：
/// 虚拟主机式寻址要求 `bucket.host` 这样的域名解析得开，而自建部署通常
/// 只有一个裸 IP 或一个主机名。不强制的话症状是「桶不存在」，
/// 而桶明明就在那儿。
///
/// 允许 http 也是为它：内网 MinIO 常常不挂 TLS。**这是配置说了算的降级**，
/// 不是默认——公有云那条路（不给 endpoint）仍然只走 https。
pub fn client(config: &serde_json::Value) -> anyhow::Result<Box<dyn ObjectStore>> {
    let s = |k: &str| {
        config[k]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let bucket = s("bucket")
        .ok_or_else(|| anyhow::anyhow!("object storage source is missing config.bucket"))?;

    let mut b = AmazonS3Builder::new().with_bucket_name(&bucket);
    if let Some(region) = s("region") {
        b = b.with_region(region);
    }
    if let (Some(key), Some(secret)) = (s("access_key_id"), s("secret_access_key")) {
        b = b.with_access_key_id(key).with_secret_access_key(secret);
    }
    if let Some(endpoint) = s("endpoint") {
        let insecure = endpoint.starts_with("http://");
        b = b
            .with_endpoint(endpoint)
            .with_virtual_hosted_style_request(false)
            .with_allow_http(insecure);
    }
    Ok(Box::new(b.build().context("object storage client")?))
}

/// 列出前缀下的对象并逐个取回。
///
/// **零字节的一律跳过，判据是大小不是名字。** 直觉的写法是看 key 以不以 `/`
/// 结尾——控制台和 `aws s3 sync` 就是这样造「文件夹」的。但 `object_store`
/// 的 `Path` 会**把尾斜杠规范化掉**，`docs/` 到手里就成了 `docs`，那个判断
/// 永远不成立；接着它去 GET `docs`，而桶里真实的 key 是 `docs/`，
/// 于是 404 把整次同步带走。实测在 MinIO 上就是这样炸的。
///
/// 按大小判还更宽：零字节的对象无论叫什么都不是文档，`ingest_item` 拿到
/// 空字节也只会返回 `Unchanged`。
///
/// 格式不在这里判。`utopia_ingest` 的抽取按扩展名加 mime 分派，认不出的
/// 一律按文本解码——在这里再维护一张白名单，就是同一件事写两遍，
/// 而两份清单迟早分叉。
pub async fn fetch(
    store: &dyn ObjectStore,
    bucket: &str,
    prefix: Option<&str>,
) -> anyhow::Result<(Vec<RemoteObject>, bool)> {
    let p = prefix.map(StorePath::from);
    let mut listing = store.list(p.as_ref());
    let mut out = Vec::new();
    let mut truncated = false;
    let mut unreadable = 0usize;

    while let Some(meta) = listing.next().await {
        let meta = meta.context("listing objects")?;
        let key = meta.location.as_ref().to_string();

        if meta.size == 0 {
            continue;
        }
        if meta.size > MAX_OBJECT_BYTES {
            tracing::info!(%key, size = meta.size, "对象太大，跳过");
            continue;
        }
        if out.len() >= MAX_OBJECTS_PER_SYNC {
            truncated = true;
            break;
        }

        // **一个对象取不回来不该带走整次同步。** 列表与取回之间隔着时间，
        // 中途被删、被换权限都是正常的；桶越大越常见。记下来在收尾时一并报，
        // 而不是让第 900 个对象的一次 404 把前 899 个的成果一起丢掉。
        let got = async {
            let r = store.get(&meta.location).await?;
            r.bytes().await
        }
        .await;
        let bytes = match got {
            Ok(b) => b,
            Err(e) => {
                unreadable += 1;
                tracing::warn!(%key, error = %e, "对象取不回来，跳过");
                continue;
            }
        };

        out.push(RemoteObject {
            external_key: format!("s3://{bucket}/{key}"),
            filename: key.rsplit('/').next().unwrap_or(&key).to_string(),
            bytes: bytes.to_vec(),
            last_modified: Some(meta.last_modified),
        });
    }
    if unreadable > 0 {
        tracing::warn!(unreadable, bucket, "有对象没能取回");
    }
    Ok((out, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 自建端点必须走 path-style，否则请求会打到 `bucket.host` 上，
    /// 而自建部署一般没有那条 DNS 记录。症状是「桶不存在」而桶就在那儿。
    #[test]
    fn a_self_hosted_endpoint_builds() {
        let cfg = serde_json::json!({
            "bucket": "docs",
            "endpoint": "http://127.0.0.1:9000",
            "region": "us-east-1",
            "access_key_id": "minioadmin",
            "secret_access_key": "minioadmin",
        });
        assert!(client(&cfg).is_ok());
    }

    /// 不给 bucket 要报得出是缺哪一项——照 jira / github 两个来源的措辞。
    #[test]
    fn a_missing_bucket_says_so() {
        let e = client(&serde_json::json!({ "region": "us-east-1" })).unwrap_err();
        assert!(e.to_string().contains("config.bucket"), "{e}");
    }

    /// 公有云那条路：不给 endpoint 就不该被强制成 path-style，
    /// 也不该允许明文 http。
    #[test]
    fn a_cloud_bucket_needs_no_endpoint() {
        let cfg = serde_json::json!({
            "bucket": "docs",
            "region": "ap-southeast-1",
            "access_key_id": "AKIA...",
            "secret_access_key": "...",
        });
        assert!(client(&cfg).is_ok());
    }

    /// 真连一个 S3 兼容端点，只测**读**这一侧：列出来、取回内容、身份对不对。
    ///
    /// **没有 `UTOPIA_S3_TEST_ENDPOINT` 时跳过而不是失败**——跟连库的测试
    /// 同一个约定（见 CONTRIBUTING）。上面那三条只证明 builder 拼得出来，
    /// 而这条要守的是**线上协议真的说得通**：path-style 有没有生效、
    /// 明文 http 放没放行、`s3://` 身份拼得对不对、`LastModified` 回不回来。
    /// 没有一条是构造 client 时能发现的。
    ///
    /// **数据由外部布置，测试不写只读。** 第一版让测试自己 `put`，结果被
    /// `object_store` 的 `Path` 咬了一口：它会把 `docs/` 规范化成 `docs`，
    /// 于是「目录占位」写成了一个名叫 `docs` 的普通对象，而 MinIO 不允许
    /// 对象 `docs` 与前缀 `docs/` 并存——整个前缀被顶掉，列表回来是空的。
    /// 真实的占位对象只能由别的客户端造，那就让它由别的客户端造。
    ///
    /// 跑法（先起 MinIO，再用 curl 布数据；curl 自带 SigV4）：
    /// ```text
    /// MINIO_ROOT_USER=u MINIO_ROOT_PASSWORD=p123456 \
    ///   minio server ./data --address 127.0.0.1:19000 &
    /// S3="curl -s --aws-sigv4 aws:amz:us-east-1:s3 -u u:p123456"
    /// B=http://127.0.0.1:19000/utopia-conn-test
    /// $S3 -X PUT $B                                  # 建桶
    /// $S3 -X PUT --data-raw hello    "$B/docs/a.txt"
    /// $S3 -X PUT --data-raw '# world' "$B/docs/b.md"
    /// $S3 -X PUT --data-raw ''        "$B/docs/"     # 目录占位
    /// UTOPIA_S3_TEST_ENDPOINT=http://127.0.0.1:19000 \
    ///   UTOPIA_S3_TEST_KEY=u UTOPIA_S3_TEST_SECRET=p123456 \
    ///   cargo test -p utopia-server object_storage
    /// ```
    #[tokio::test]
    async fn it_reads_from_a_real_s3_endpoint() -> anyhow::Result<()> {
        let Ok(endpoint) = std::env::var("UTOPIA_S3_TEST_ENDPOINT") else {
            eprintln!("跳过：未设 UTOPIA_S3_TEST_ENDPOINT");
            return Ok(());
        };
        let bucket = "utopia-conn-test";
        let cfg = serde_json::json!({
            "bucket": bucket,
            "endpoint": endpoint,
            "region": "us-east-1",
            "access_key_id": std::env::var("UTOPIA_S3_TEST_KEY").unwrap_or("minioadmin".into()),
            "secret_access_key": std::env::var("UTOPIA_S3_TEST_SECRET").unwrap_or("minioadmin".into()),
        });
        let store = client(&cfg)?;

        let (objs, truncated) = fetch(store.as_ref(), bucket, Some("docs")).await?;
        assert!(!truncated, "三个对象不该触发上限");

        let keys: Vec<&str> = objs.iter().map(|o| o.external_key.as_str()).collect();
        for want in [
            "s3://utopia-conn-test/docs/a.txt",
            "s3://utopia-conn-test/docs/b.md",
        ] {
            assert!(keys.contains(&want), "少了 {want}：{keys:?}");
        }

        // **这条守的是一个真炸过的 bug。** 桶里有一个 `docs/` 目录占位对象，
        // 而 `object_store` 把尾斜杠规范化掉了，于是它以 `docs` 的身份进到
        // 循环里，GET 回来 404，整次同步失败。三条构造 client 的单元测试
        // 一条都发现不了——只有真连一个装过「文件夹」的桶才看得见。
        assert_eq!(objs.len(), 2, "目录占位对象没被滤掉：{keys:?}");

        let a = objs.iter().find(|o| o.filename == "a.txt").expect("a.txt");
        assert_eq!(a.bytes, b"hello", "取回的内容不对");
        assert!(
            a.last_modified.is_some(),
            "LastModified 是 doc_time 的唯一来源"
        );
        Ok(())
    }
}
