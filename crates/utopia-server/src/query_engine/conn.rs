//! 连接串解析。一个输入框、四种 scheme；这里把 URL 拆成各引擎要的字段。
//!
//! 写法沿用 `postgres://user:pass@host/db` 的形状：凭据在 userinfo 里，HTTP 族的
//! 令牌放 password 位（`databricks://:TOKEN@…`），路径是「目录 / 库 / schema」，
//! 引擎特有的开关走 query。`ssl=false` 让 HTTP 族走明文——给本地代理与测试用，
//! 线上的三家都只认 https。

use percent_encoding::percent_decode_str;
use url::Url;

fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn query(u: &Url, key: &str) -> Option<String> {
    u.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())
}

fn ssl_off(u: &Url) -> bool {
    matches!(
        query(u, "ssl").as_deref(),
        Some("false") | Some("0") | Some("off") | Some("no")
    )
}

fn segments(u: &Url) -> Vec<String> {
    u.path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).map(decode).collect())
        .unwrap_or_default()
}

/// 令牌：password 位优先；没有 password 时 username 位也算（`databricks://TOKEN@host`
/// 少打一个冒号是最常见的手滑）；最后看 `?token=`
fn token_of(u: &Url) -> Option<String> {
    u.password()
        .map(decode)
        .filter(|s| !s.is_empty())
        .or_else(|| Some(decode(u.username())).filter(|s| !s.is_empty()))
        .or_else(|| query(u, "token"))
}

fn base_of(u: &Url, https: bool, default_port: u16) -> anyhow::Result<String> {
    let host = u
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("{}://: a host is required", u.scheme()))?;
    let port = u.port().unwrap_or(default_port);
    let scheme = if https { "https" } else { "http" };
    // 默认端口不写进 URL：reqwest 照样能连，日志里也干净
    let explicit = match (https, port) {
        (true, 443) | (false, 80) => String::new(),
        _ => format!(":{port}"),
    };
    Ok(format!("{scheme}://{host}{explicit}"))
}

/// `trino://user[:password]@host[:port]/[catalog[/schema]][?ssl=true|false]`
///
/// 明文 http 是 Trino 的默认（8080）；带密码、`ssl=true`、或端口 443 / 8443 时走 https——
/// Trino 自己也拒绝在明文上收密码。`presto://` 是同一个协议的旧名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrinoConn {
    pub base: String,
    pub user: String,
    pub password: Option<String>,
    pub catalog: Option<String>,
    pub schema: Option<String>,
}

impl TrinoConn {
    pub fn parse(conn: &str) -> anyhow::Result<Self> {
        let u = Url::parse(conn.trim())?;
        let user = decode(u.username());
        if user.is_empty() {
            anyhow::bail!("trino://: a user is required (it becomes X-Trino-User), e.g. trino://alice@host:8080/hive/default");
        }
        let password = u.password().map(decode).filter(|s| !s.is_empty());
        let https = !ssl_off(&u)
            && (password.is_some()
                || query(&u, "ssl").as_deref() == Some("true")
                || matches!(u.port(), Some(443) | Some(8443)));
        let base = base_of(&u, https, if https { 443 } else { 8080 })?;
        let segs = segments(&u);
        Ok(Self {
            base,
            user,
            password,
            catalog: segs.first().cloned(),
            schema: segs.get(1).cloned(),
        })
    }
}

/// `databricks://:TOKEN@workspace-host/sql/1.0/warehouses/WAREHOUSE_ID[?catalog=main&schema=default]`
///
/// 路径就是 JDBC 里的 httpPath，从控制台复制过来不用改；`?warehouse=ID` 也认。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabricksConn {
    pub base: String,
    pub token: String,
    pub warehouse_id: String,
    pub catalog: Option<String>,
    pub schema: Option<String>,
}

impl DatabricksConn {
    pub fn parse(conn: &str) -> anyhow::Result<Self> {
        let u = Url::parse(conn.trim())?;
        let base = base_of(&u, !ssl_off(&u), if ssl_off(&u) { 80 } else { 443 })?;
        let token = token_of(&u).ok_or_else(|| {
            anyhow::anyhow!("databricks://: a personal access token is required, e.g. databricks://:TOKEN@host/sql/1.0/warehouses/ID")
        })?;
        let segs = segments(&u);
        let from_path = segs
            .iter()
            .position(|s| s == "warehouses")
            .and_then(|i| segs.get(i + 1).cloned());
        let warehouse_id = query(&u, "warehouse").or(from_path).ok_or_else(|| {
            anyhow::anyhow!("databricks://: a SQL warehouse is required — the /sql/1.0/warehouses/ID path or ?warehouse=ID")
        })?;
        Ok(Self {
            base,
            token,
            warehouse_id,
            catalog: query(&u, "catalog"),
            schema: query(&u, "schema"),
        })
    }
}

/// `snowflake://:TOKEN@account.snowflakecomputing.com/[DATABASE[/SCHEMA]][?warehouse=WH&role=R&token_type=pat|oauth]`
///
/// SQL API 不收密码，只收令牌：默认当作 programmatic access token，`token_type=oauth`
/// 换成 OAuth 令牌。密钥对 JWT 要本地签名，这一版不做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnowflakeConn {
    pub base: String,
    pub token: String,
    /// `X-Snowflake-Authorization-Token-Type` 的值
    pub token_type: &'static str,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub warehouse: Option<String>,
    pub role: Option<String>,
}

impl SnowflakeConn {
    pub fn parse(conn: &str) -> anyhow::Result<Self> {
        let u = Url::parse(conn.trim())?;
        let base = base_of(&u, !ssl_off(&u), if ssl_off(&u) { 80 } else { 443 })?;
        let token = token_of(&u).ok_or_else(|| {
            anyhow::anyhow!("snowflake://: a programmatic access token or OAuth token is required, e.g. snowflake://:TOKEN@account.snowflakecomputing.com/DB/SCHEMA?warehouse=WH")
        })?;
        let token_type = match query(&u, "token_type")
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("pat") | Some("programmatic_access_token") => "PROGRAMMATIC_ACCESS_TOKEN",
            Some("oauth") => "OAUTH",
            Some(other) => {
                anyhow::bail!("snowflake://: unknown token_type '{other}' (pat or oauth)")
            }
        };
        let segs = segments(&u);
        Ok(Self {
            base,
            token,
            token_type,
            database: segs.first().cloned(),
            schema: segs.get(1).cloned(),
            warehouse: query(&u, "warehouse"),
            role: query(&u, "role"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{DatabricksConn, SnowflakeConn, TrinoConn};

    #[test]
    fn trino_defaults_to_plain_http_and_upgrades_when_it_must() {
        let c = TrinoConn::parse("trino://alice@lake.internal:8080/iceberg/sales").unwrap();
        assert_eq!(c.base, "http://lake.internal:8080");
        assert_eq!(c.user, "alice");
        assert_eq!(c.catalog.as_deref(), Some("iceberg"));
        assert_eq!(c.schema.as_deref(), Some("sales"));

        let c = TrinoConn::parse("trino://alice:s%40cret@lake.internal/hive").unwrap();
        assert_eq!(c.base, "https://lake.internal");
        assert_eq!(c.password.as_deref(), Some("s@cret"));

        let c = TrinoConn::parse("trino://alice@lake.internal:8443/hive").unwrap();
        assert_eq!(c.base, "https://lake.internal:8443");

        let c = TrinoConn::parse("presto://bob@127.0.0.1:9000?ssl=false").unwrap();
        assert_eq!(c.base, "http://127.0.0.1:9000");
        assert_eq!(c.catalog, None);

        assert!(TrinoConn::parse("trino://lake.internal/hive").is_err());
    }

    #[test]
    fn databricks_reads_the_http_path_from_the_console() {
        let c = DatabricksConn::parse(
            "databricks://:dapi123@dbc-abc.cloud.databricks.com/sql/1.0/warehouses/9f2a?catalog=main&schema=sales",
        )
        .unwrap();
        assert_eq!(c.base, "https://dbc-abc.cloud.databricks.com");
        assert_eq!(c.token, "dapi123");
        assert_eq!(c.warehouse_id, "9f2a");
        assert_eq!(c.catalog.as_deref(), Some("main"));
        assert_eq!(c.schema.as_deref(), Some("sales"));

        let c = DatabricksConn::parse("databricks://dapi123@host?warehouse=w1").unwrap();
        assert_eq!(c.token, "dapi123");
        assert_eq!(c.warehouse_id, "w1");

        assert!(DatabricksConn::parse("databricks://host/sql/1.0/warehouses/w1").is_err());
        assert!(DatabricksConn::parse("databricks://:t@host").is_err());
    }

    #[test]
    fn snowflake_takes_a_token_and_the_session_knobs() {
        let c = SnowflakeConn::parse(
            "snowflake://:tok@xy12345.eu-central-1.snowflakecomputing.com/ANALYTICS/PUBLIC?warehouse=WH&role=ANALYST",
        )
        .unwrap();
        assert_eq!(
            c.base,
            "https://xy12345.eu-central-1.snowflakecomputing.com"
        );
        assert_eq!(c.token, "tok");
        assert_eq!(c.token_type, "PROGRAMMATIC_ACCESS_TOKEN");
        assert_eq!(c.database.as_deref(), Some("ANALYTICS"));
        assert_eq!(c.schema.as_deref(), Some("PUBLIC"));
        assert_eq!(c.warehouse.as_deref(), Some("WH"));
        assert_eq!(c.role.as_deref(), Some("ANALYST"));

        let c =
            SnowflakeConn::parse("snowflake://:tok@acct.snowflakecomputing.com?token_type=oauth")
                .unwrap();
        assert_eq!(c.token_type, "OAUTH");
        assert!(SnowflakeConn::parse(
            "snowflake://:tok@acct.snowflakecomputing.com?token_type=jwt"
        )
        .is_err());
        assert!(SnowflakeConn::parse("snowflake://acct.snowflakecomputing.com/DB").is_err());
    }
}
