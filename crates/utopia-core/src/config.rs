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
            .merge(blank_is_unset(Env::prefixed("UTOPIA_")))
            .extract()?;
        Ok(cfg)
    }
}

/// 值为空的环境变量按**没设**处理（#343）。
///
/// 容器编排里 `UTOPIA_X: ${UTOPIA_X:-}` 这种写法，在变量没设时传进容器的是空串，
/// 不是「不传」。Figment 照单全收，于是 `Option<String>` 拿到 `Some("")`、`String`
/// 字段被空串盖掉默认值、`bool` 与数字字段连反序列化都过不去。
///
/// 判断放在读环境变量这一层，而不是各个取值点：空串对这里的每一个字段都不是合法值，
/// 逐个字段加守卫只会漏掉下一个加进来的字段——`jwt_secret` 和 `secret_key` 各自守着，
/// `migration_url` 就是漏掉的那个，症状是空串交给 sqlx 报 "relative URL without a
/// base"，照 README 快速开始起的部署第一步就退出。同一件事 `init-app-role.sh` 里
/// 用 `[ -z ]` 判过了，Rust 这侧缺的就是它。
fn blank_is_unset(env: Env) -> Env {
    // iter() 给的是剥掉前缀、转成小写之后的键；filter 拿到的是剥掉前缀、
    // 还没转小写的那个，所以这里按不分大小写比。
    let blank: Vec<String> = env
        .iter()
        .filter(|(_, value)| value.trim().is_empty())
        .map(|(key, _)| key.to_string())
        .collect();
    env.filter(move |key| {
        !blank
            .iter()
            .any(|blank_key| key.as_str().eq_ignore_ascii_case(blank_key))
    })
}

impl AppConfig {
    /// 迁移连接串：未单独配置时用运行时那一个。
    pub fn migration_url(&self) -> &str {
        self.migration_url.as_deref().unwrap_or(&self.database_url)
    }
}

#[cfg(test)]
// Jail 的闭包必须返回 figment::Result，那个 Err 变体的大小不由我们定
#[allow(clippy::result_large_err)]
mod tests {
    use super::AppConfig;
    use figment::Jail;

    /// #343：`docker-compose.yml` 里 `UTOPIA_MIGRATION_URL: ${UTOPIA_MIGRATION_URL:-}`
    /// 在变量没设时传进容器的是**空串**而不是「不传」。空串盖掉回落之后
    /// `migration_url()` 返回 ""，sqlx 报 "relative URL without a base"，
    /// 照 README 快速开始起的部署第一步就退出。
    #[test]
    fn a_blank_migration_url_falls_back_to_the_database_url() {
        Jail::expect_with(|jail| {
            jail.set_env("UTOPIA_DATABASE_URL", "postgres://u:p@db:5432/utopia");
            jail.set_env("UTOPIA_MIGRATION_URL", "");
            let cfg = AppConfig::load().unwrap();
            assert_eq!(cfg.migration_url, None, "空串要当作没设");
            assert_eq!(cfg.migration_url(), cfg.database_url);
            Ok(())
        });
    }

    /// 只有空白也算空：`UTOPIA_MIGRATION_URL=" "` 同样不是连接串。
    #[test]
    fn whitespace_counts_as_blank() {
        Jail::expect_with(|jail| {
            jail.set_env("UTOPIA_DATABASE_URL", "postgres://u:p@db:5432/utopia");
            jail.set_env("UTOPIA_MIGRATION_URL", "   ");
            let cfg = AppConfig::load().unwrap();
            assert_eq!(cfg.migration_url, None);
            Ok(())
        });
    }

    /// 真给了值就照旧生效——这一条是拦着上面那个过滤越界的。
    #[test]
    fn a_migration_url_that_is_set_still_wins() {
        Jail::expect_with(|jail| {
            jail.set_env("UTOPIA_DATABASE_URL", "postgres://app:p@db:5432/utopia");
            jail.set_env("UTOPIA_MIGRATION_URL", "postgres://owner:p@db:5432/utopia");
            let cfg = AppConfig::load().unwrap();
            assert_eq!(
                cfg.migration_url.as_deref(),
                Some("postgres://owner:p@db:5432/utopia")
            );
            assert_ne!(cfg.migration_url(), cfg.database_url);
            Ok(())
        });
    }

    /// 空串不止坑 `Option<String>`：`String` 字段没有回落可言，空串直接盖掉默认值。
    #[test]
    fn a_blank_string_field_keeps_its_default() {
        Jail::expect_with(|jail| {
            jail.set_env("UTOPIA_DATA_DIR", "");
            jail.set_env("UTOPIA_WEB_DIST", "");
            jail.set_env("UTOPIA_BIND_ADDR", "");
            let cfg = AppConfig::load().unwrap();
            assert_eq!(cfg.data_dir, "data");
            assert_eq!(cfg.web_dist, "web/dist");
            assert_eq!(cfg.bind_addr, "0.0.0.0:1516");
            Ok(())
        });
    }

    /// 非字符串字段更早一步：空串连反序列化都过不去，`load()` 直接报错，
    /// 而报的是 figment 的类型错误，看不出是哪个环境变量传空了。
    #[test]
    fn a_blank_typed_field_does_not_break_loading() {
        Jail::expect_with(|jail| {
            jail.set_env("UTOPIA_DB_MAX_CONNECTIONS", "");
            jail.set_env("UTOPIA_COOKIE_SECURE", "");
            let cfg = AppConfig::load().unwrap();
            assert_eq!(cfg.db_max_connections, None);
            assert!(!cfg.cookie_secure);
            Ok(())
        });
    }

    /// 同上的反面：给了值仍然解析。
    #[test]
    fn a_typed_field_that_is_set_still_parses() {
        Jail::expect_with(|jail| {
            jail.set_env("UTOPIA_DB_MAX_CONNECTIONS", "8");
            jail.set_env("UTOPIA_COOKIE_SECURE", "true");
            let cfg = AppConfig::load().unwrap();
            assert_eq!(cfg.db_max_connections, Some(8));
            assert!(cfg.cookie_secure);
            Ok(())
        });
    }
}
