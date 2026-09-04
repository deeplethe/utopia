use figment::{
    providers::{Env, Serialized},
    Figment,
};
use serde::{Deserialize, Serialize};

/// 全局配置。来源优先级：环境变量（前缀 `UTOPIA_`）> 默认值。
/// `.env` 文件由二进制入口通过 dotenvy 预加载进环境变量。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub database_url: String,
    /// 跑迁移用的连接串。迁移要建表建触发器，运行时不需要那些权限——分开之后
    /// 应用可以用一个只读写业务表、对台账只增不改的受限角色连库。
    /// 不设则回落到 `database_url`：既有部署无需改动即可照常升级。
    pub migration_url: Option<String>,
    pub bind_addr: String,
    /// JWT 签名密钥。留空则首次启动时自动生成并存进 deployment_settings——
    /// 要求部署者手填一个随机串，现实中的结果是默认值原样上生产。
    /// 显式给出时优先于库里那条：密钥轮换与多实例显式对齐走这条路。
    pub jwt_secret: Option<String>,
    /// 凭据封印钥匙（32 字节，64 位十六进制或 base64）。留空则用数据目录下的
    /// `secret.key`，首次启动生成。**钥匙不进库**：库泄漏不等于凭据泄漏，是静态加密
    /// 的全部意义；备份数据目录时把它一起带走，没有它库里的凭据读不出来。
    pub secret_key: Option<String>,
    /// 前端构建产物目录；存在时由服务端托管 SPA（history fallback）。
    pub web_dist: String,
    /// 数据目录：原始文件（files/）与 Tantivy 索引（index/）。
    pub data_dir: String,
    /// 数据库连接池上限。缺省 32，与 worker 并发的缺省对齐——池子小于并发时
    /// 症状是请求变慢而不是任何一处说"池子不够"，所以它必须可调。
    pub db_max_connections: Option<u32>,
    /// 强制给会话 cookie 打 Secure。缺省 false：由请求的 X-Forwarded-Proto 判定，
    /// 走 TLS 才打。只有代理不发那个头时才需要在这里强制打开。
    pub cookie_secure: bool,
    /// 是否开放注册。false 时仅首个用户（引导部署）可注册，其余需管理员开放。
    pub open_registration: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            database_url: "postgres://utopia:utopia@localhost:1517/utopia".into(),
            migration_url: None,
            bind_addr: "0.0.0.0:1516".into(),
            jwt_secret: None,
            secret_key: None,
            web_dist: "web/dist".into(),
            data_dir: "data".into(),
            db_max_connections: None,
            cookie_secure: false,
            open_registration: true,
        }
    }
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        let cfg = Figment::from(Serialized::defaults(AppConfig::default()))
            .merge(Env::prefixed("UTOPIA_"))
            .extract()?;
        Ok(cfg)
    }
}

impl AppConfig {
    /// 迁移连接串：未单独配置时用运行时那一个。
    pub fn migration_url(&self) -> &str {
        self.migration_url.as_deref().unwrap_or(&self.database_url)
    }
}
