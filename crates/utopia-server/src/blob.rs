//! BlobStore：原始文件字节的存取接缝。
//!
//! 内容寻址——key 就是文件内容的 sha256，接口里没有"路径"概念：本地是分桶前的
//! 平铺目录、对象存储是 object key，任何 KV 都能实现。幂等、去重、不可变
//! （内容变了指纹就变，旧版永不覆盖——"版本回放有料"的物质基础）全部由
//! "内容即地址"免费获得。
//!
//! 现阶段唯一实现是本地磁盘（data/files/{sha256}）。将来接对象存储/网盘
//! （P5 连接器、多实例部署共享存储）只需新增实现，摄入/上传/解析/回放的
//! 调用方一行不改。配置入口 UTOPIA_BLOB_BACKEND 预留，当前仅接受 "local"。

use std::path::PathBuf;

#[async_trait::async_trait]
pub trait BlobStore: Send + Sync {
    /// 幂等写入：同指纹已存在即跳过。
    async fn put(&self, sha256: &str, bytes: &[u8]) -> anyhow::Result<()>;
    async fn get(&self, sha256: &str) -> anyhow::Result<Vec<u8>>;
    #[allow(dead_code)] // 接口完整性：回放/GC 路径的将来消费者
    async fn exists(&self, sha256: &str) -> anyhow::Result<bool>;
    /// 真删（#268 下半）：只在库里确认没人再引用这份指纹之后调用。幂等：不存在也算成功
    async fn delete(&self, sha256: &str) -> anyhow::Result<()>;
}

/// 本地磁盘实现：`{dir}/{sha256}` 平铺存放（与历史行为逐字节一致）。
pub struct LocalBlobStore {
    dir: PathBuf,
}

impl LocalBlobStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

#[async_trait::async_trait]
impl BlobStore for LocalBlobStore {
    async fn put(&self, sha256: &str, bytes: &[u8]) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let path = self.dir.join(sha256);
        if !path.exists() {
            tokio::fs::write(&path, bytes).await?;
        }
        Ok(())
    }

    async fn get(&self, sha256: &str) -> anyhow::Result<Vec<u8>> {
        Ok(tokio::fs::read(self.dir.join(sha256)).await?)
    }

    async fn exists(&self, sha256: &str) -> anyhow::Result<bool> {
        Ok(self.dir.join(sha256).exists())
    }

    async fn delete(&self, sha256: &str) -> anyhow::Result<()> {
        match tokio::fs::remove_file(self.dir.join(sha256)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
