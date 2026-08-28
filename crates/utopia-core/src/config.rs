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
    pub bind_addr: String,
    pub jwt_secret: String,
    /// 前端构建产物目录；存在时由服务端托管 SPA（history fallback）。
    pub web_dist: String,
    /// 数据目录：原始文件（files/）与 Tantivy 索引（index/）。
    pub data_dir: String,
    /// 数据库连接池上限。缺省 32，与 worker 并发的缺省对齐——池子小于并发时
    /// 症状是请求变慢而不是任何一处说"池子不够"，所以它必须可调。
    pub db_max_connections: Option<u32>,
    /// 是否开放注册。false 时仅首个用户（引导部署）可注册，其余需管理员开放。
    pub open_registration: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            database_url: "postgres://utopia:utopia@localhost:5432/utopia".into(),
            bind_addr: "0.0.0.0:8080".into(),
            jwt_secret: "dev-secret-change-me".into(),
            web_dist: "web/dist".into(),
            data_dir: "data".into(),
            db_max_connections: None,
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
